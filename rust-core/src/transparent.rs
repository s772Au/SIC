//! Native text transactions for a Harness-owned model dispatch path.
//!
//! SIC owns text anchors, source identity and atomic versioned edits. The host
//! owns admission, route/model selection, credentials, transport and responses.
//! No worker profile, request projection or completion/SSE conversion is used.

use crate::protocol::{self, DecodedRequest, TextUnit};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, future::Future};

fn digest(value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[derive(Clone, Debug)]
pub struct TextEdit {
    pub unit_id: String,
    pub expected_text: String,
    pub replacement: String,
}

/// An in-memory transaction context. Persistent Core integration is separate;
/// callers must retain the receipt and context together if durability is needed.
pub struct NativeContext {
    decoded: DecodedRequest,
    version: u64,
    source_digest: String,
}

impl NativeContext {
    pub fn new(format: &str, native_request: &Value) -> Result<Self, String> {
        Ok(Self {
            decoded: protocol::decode_host_owned(format, native_request)?,
            version: 0,
            source_digest: digest(&json!({"format":format,"request":native_request}))?,
        })
    }

    pub fn units(&self) -> &[TextUnit] {
        &self.decoded.units
    }

    /// Rehydrate Core-owned state; only anchored text may differ from source.
    pub(crate) fn restore(
        format: &str,
        source: &Value,
        current: &Value,
        version: u64,
    ) -> Result<Self, String> {
        let mut context = Self::new(format, source)?;
        let edited = protocol::decode_host_owned(format, current)?;
        if context.decoded.units.len() != edited.units.len() {
            return Err("persisted native unit set changed".into());
        }
        for (original, edited) in context.decoded.units.iter_mut().zip(edited.units) {
            if original.unit_id != edited.unit_id || original.anchor != edited.anchor {
                return Err("persisted native anchors changed".into());
            }
            original.text = edited.text;
        }
        if context.request()? != *current {
            return Err("persisted native sidecar changed".into());
        }
        context.version = version;
        Ok(context)
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn source_digest(&self) -> &str {
        &self.source_digest
    }
    pub fn source_request(&self) -> &Value {
        self.decoded.sidecar()
    }
    pub fn request(&self) -> Result<Value, String> {
        protocol::encode(&self.decoded)
    }

    /// All edits succeed together; stale, duplicate or protected anchors fail
    /// without changing any text or version. The digest binds the native source,
    /// including its protocol, model and all opaque controls.
    pub fn commit(
        &mut self,
        expected_source: &str,
        expected_version: u64,
        edits: &[TextEdit],
    ) -> Result<Value, String> {
        if expected_source != self.source_digest || expected_version != self.version {
            return Err("native source or version conflict".into());
        }
        let next_version = self
            .version
            .checked_add(1)
            .ok_or("native version exhausted")?;
        let mut candidate = self.decoded.clone();
        let mut seen = HashSet::new();
        let mut changes = Vec::new();
        for edit in edits {
            if !seen.insert(edit.unit_id.as_str()) {
                return Err("duplicate native edit".into());
            }
            let unit = candidate
                .units
                .iter_mut()
                .find(|u| u.unit_id == edit.unit_id)
                .ok_or("unknown or protected native unit")?;
            if unit.text != edit.expected_text {
                return Err("native text conflict".into());
            }
            changes.push(json!({"unit_id":unit.unit_id,"anchor":unit.anchor,
                "before_sha256":digest(&json!(unit.text))?,
                "after_sha256":digest(&json!(edit.replacement))?}));
            unit.text.clone_from(&edit.replacement);
        }
        let output = protocol::encode(&candidate)?;
        let receipt = json!({"source_digest":self.source_digest,"format":candidate.format(),
            "base_version":self.version,"version":next_version,"changes":changes,
            "output_digest":digest(&output)?});
        self.decoded = candidate;
        self.version = next_version;
        Ok(receipt)
    }

    /// The host receives the same native protocol and owns the returned type.
    /// In particular, a streaming HTTP Response can pass through uninspected.
    /// Host admission must occur on this final edited request, before dispatch.
    pub async fn dispatch<F, Fut, T>(&self, host: F) -> Result<T, String>
    where
        F: FnOnce(Value) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        host(self.request()?).await
    }
}
