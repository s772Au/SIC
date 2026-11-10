//! Pure, serializable Socket / Signal / Message state for independent Agent routes.
//!
//! The caller owns persistence transactions, current deployment/revocation checks,
//! scheduler integration, and actual process or tool execution. This module never
//! starts or kills an OS process and never treats model text as authority.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn err(message: &str) -> String {
    message.to_owned()
}
fn nonempty(value: &str, name: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(err(&format!("{name} must be nonempty")))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteStatus {
    Running,
    Waiting,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub actor_id: String,
    pub route_id: String,
}

impl Principal {
    pub fn new(actor_id: impl Into<String>, route_id: impl Into<String>) -> Result<Self, String> {
        let value = Self {
            actor_id: actor_id.into(),
            route_id: route_id.into(),
        };
        nonempty(&value.actor_id, "actor identity")?;
        nonempty(&value.route_id, "Agent route identity")?;
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    #[serde(rename = "P")]
    Persistent,
    #[serde(rename = "V")]
    Volatile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SocketAcl {
    pub message: Vec<String>,
    pub signal: Vec<String>,
    pub spawn: Vec<String>,
    pub read: Vec<String>,
}

impl SocketAcl {
    pub fn empty() -> Self {
        Self {
            message: Vec::new(),
            signal: Vec::new(),
            spawn: Vec::new(),
            read: Vec::new(),
        }
    }
    fn validate(&self) -> Result<(), String> {
        for rows in [&self.message, &self.signal, &self.spawn, &self.read] {
            if rows.iter().any(|row| row.trim().is_empty()) {
                return Err(err("Socket ACL identities must be nonempty"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    checkpoint_id: String,
    owner_route_id: String,
    cut_id: String,
    cb_id: String,
    cut_sequence: u64,
    snapshot: Value,
}

impl Checkpoint {
    pub fn id(&self) -> &str {
        &self.checkpoint_id
    }
    pub fn owner_route_id(&self) -> &str {
        &self.owner_route_id
    }
    pub fn cut_id(&self) -> &str {
        &self.cut_id
    }
    pub fn cb_id(&self) -> &str {
        &self.cb_id
    }
    pub fn cut_sequence(&self) -> u64 {
        self.cut_sequence
    }
    pub fn snapshot(&self) -> &Value {
        &self.snapshot
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Socket {
    socket_id: String,
    owner_route_id: String,
    checkpoint_id: String,
    acl: SocketAcl,
}

impl Socket {
    pub fn id(&self) -> &str {
        &self.socket_id
    }
    pub fn owner_route_id(&self) -> &str {
        &self.owner_route_id
    }
    pub fn checkpoint_id(&self) -> &str {
        &self.checkpoint_id
    }
    pub fn acl(&self) -> &SocketAcl {
        &self.acl
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub message_id: String,
    pub request_id: String,
    pub sender_route_id: String,
    pub recipient_route_id: String,
    pub socket_id: String,
    pub checkpoint_id: String,
    pub text: String,
    pub lifecycle: Lifecycle,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub phase_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxEntry {
    pub sequence: u64,
    pub message: Message,
    pub receive_receipt_id: String,
    pub consumed_by_cut: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageReceipt {
    pub receipt_id: String,
    pub sequence: u64,
    pub message_id: String,
    pub request_id: String,
    pub sender_route_id: String,
    pub recipient_route_id: String,
    pub socket_id: String,
    pub lifecycle: Lifecycle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryOutcome {
    pub receipt: MessageReceipt,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalAction {
    Pause,
    Resume,
    Wake,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalReceipt {
    pub sequence: u64,
    pub signal_id: String,
    pub sender_actor_id: String,
    pub sender_route_id: String,
    pub target_route_id: String,
    pub socket_id: String,
    pub action: SignalAction,
    pub previous_status: RouteStatus,
    pub resulting_status: RouteStatus,
    pub transitioned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalOutcome {
    pub receipt: SignalReceipt,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedMessage {
    pub sequence: u64,
    pub message: Message,
    pub receive_receipt_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationPreview {
    route_id: String,
    cut_id: String,
    eligible: bool,
    messages: Vec<PublishedMessage>,
    digest: String,
}

impl PublicationPreview {
    pub fn route_id(&self) -> &str {
        &self.route_id
    }
    pub fn cut_id(&self) -> &str {
        &self.cut_id
    }
    pub fn eligible(&self) -> bool {
        self.eligible
    }
    pub fn messages(&self) -> &[PublishedMessage] {
        &self.messages
    }
    pub fn persistent_in_cb(&self) -> Vec<&PublishedMessage> {
        self.messages
            .iter()
            .filter(|entry| entry.message.lifecycle == Lifecycle::Persistent)
            .collect()
    }
    pub fn volatile_next_wb(&self) -> Vec<&PublishedMessage> {
        self.messages
            .iter()
            .filter(|entry| entry.message.lifecycle == Lifecycle::Volatile)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationReceipt {
    pub receipt_id: String,
    pub sequence: u64,
    pub route_id: String,
    pub cut_id: String,
    pub cb_id: String,
    pub preview_digest: String,
    pub message_ids: Vec<String>,
    pub persistent_in_cb: Vec<PublishedMessage>,
    pub volatile_next_wb: Vec<PublishedMessage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmationOutcome {
    pub receipt: PublicationReceipt,
    pub replayed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRoute {
    route_id: String,
    owner_actor_id: String,
    owners: Vec<String>,
    creators: Vec<String>,
    origin_checkpoint: Option<String>,
    initial_snapshot: Value,
    status: RouteStatus,
    next_sequence: u64,
    inbox: Vec<InboxEntry>,
    message_receipts: BTreeMap<String, MessageReceipt>,
    signal_receipts: BTreeMap<String, SignalReceipt>,
    publication_receipts: BTreeMap<String, PublicationReceipt>,
}

impl AgentRoute {
    pub fn id(&self) -> &str {
        &self.route_id
    }
    pub fn owner_actor_id(&self) -> &str {
        &self.owner_actor_id
    }
    pub fn owners(&self) -> &[String] {
        &self.owners
    }
    pub fn creators(&self) -> &[String] {
        &self.creators
    }
    pub fn origin_checkpoint(&self) -> Option<&str> {
        self.origin_checkpoint.as_deref()
    }
    pub fn initial_snapshot(&self) -> &Value {
        &self.initial_snapshot
    }
    pub fn status(&self) -> &RouteStatus {
        &self.status
    }
    pub fn inbox(&self) -> &[InboxEntry] {
        &self.inbox
    }
    pub fn message_receipts(&self) -> &BTreeMap<String, MessageReceipt> {
        &self.message_receipts
    }
    pub fn signal_receipts(&self) -> &BTreeMap<String, SignalReceipt> {
        &self.signal_receipts
    }
    pub fn publication_receipts(&self) -> &BTreeMap<String, PublicationReceipt> {
        &self.publication_receipts
    }
    fn allocate_sequence(&mut self) -> u64 {
        let value = self.next_sequence;
        self.next_sequence += 1;
        value
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IpcState {
    routes: BTreeMap<String, AgentRoute>,
    checkpoints: BTreeMap<String, Checkpoint>,
    sockets: BTreeMap<String, Socket>,
}

impl IpcState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn route(&self, route_id: &str) -> Option<&AgentRoute> {
        self.routes.get(route_id)
    }
    pub fn checkpoint(&self, checkpoint_id: &str) -> Option<&Checkpoint> {
        self.checkpoints.get(checkpoint_id)
    }
    pub fn socket(&self, socket_id: &str) -> Option<&Socket> {
        self.sockets.get(socket_id)
    }

    fn verify_principal(&self, principal: &Principal) -> Result<&AgentRoute, String> {
        let route = self
            .routes
            .get(&principal.route_id)
            .ok_or_else(|| err("caller Agent route not found"))?;
        if !route.owners.contains(&principal.actor_id) {
            return Err(err("caller is not authorized on this Agent route"));
        }
        Ok(route)
    }

    /// Create an independent Agent identity. Equal snapshots never merge routes.
    pub fn create_route(
        &mut self,
        route_id: impl Into<String>,
        owner_actor_id: impl Into<String>,
        creators: Vec<String>,
        principal: &Principal,
    ) -> Result<(), String> {
        let route_id = route_id.into();
        let owner_actor_id = owner_actor_id.into();
        nonempty(&route_id, "Agent route identity")?;
        nonempty(&owner_actor_id, "route owner")?;
        if self.routes.contains_key(&route_id) {
            return Err(err("Agent route identity already exists"));
        }
        if principal.actor_id != owner_actor_id {
            return Err(err("route creation requires explicit owner authority"));
        }
        if creators.iter().any(|creator| creator.trim().is_empty()) {
            return Err(err("creator identities must be nonempty"));
        }
        self.routes.insert(
            route_id.clone(),
            AgentRoute {
                route_id,
                owner_actor_id: owner_actor_id.clone(),
                owners: vec![owner_actor_id],
                creators,
                origin_checkpoint: None,
                initial_snapshot: Value::Null,
                status: RouteStatus::Running,
                next_sequence: 1,
                inbox: Vec::new(),
                message_receipts: BTreeMap::new(),
                signal_receipts: BTreeMap::new(),
                publication_receipts: BTreeMap::new(),
            },
        );
        Ok(())
    }

    /// Record a host-validated cut as an immutable fork point.
    /// The caller persists this state in the same transaction as the CB/cut.
    pub fn record_checkpoint(
        &mut self,
        principal: &Principal,
        checkpoint_id: impl Into<String>,
        cut_id: impl Into<String>,
        cb_id: impl Into<String>,
        cut_sequence: u64,
        snapshot: Value,
    ) -> Result<(), String> {
        let checkpoint_id = checkpoint_id.into();
        let cut_id = cut_id.into();
        let cb_id = cb_id.into();
        nonempty(&checkpoint_id, "checkpoint identity")?;
        nonempty(&cut_id, "cut identity")?;
        nonempty(&cb_id, "published CB identity")?;
        let route = self.verify_principal(principal)?;
        if route.status == RouteStatus::Stopped {
            return Err(err("stopped Agent cannot publish a checkpoint"));
        }
        if self.checkpoints.contains_key(&checkpoint_id) {
            return Err(err("checkpoint identity already exists"));
        }
        self.checkpoints.insert(
            checkpoint_id.clone(),
            Checkpoint {
                checkpoint_id,
                owner_route_id: principal.route_id.clone(),
                cut_id,
                cb_id,
                cut_sequence,
                snapshot,
            },
        );
        Ok(())
    }

    /// Configure a receivable Socket at one checkpoint owned by its target route.
    pub fn create_socket(
        &mut self,
        principal: &Principal,
        socket_id: impl Into<String>,
        checkpoint_id: impl Into<String>,
        acl: SocketAcl,
    ) -> Result<(), String> {
        let socket_id = socket_id.into();
        let checkpoint_id = checkpoint_id.into();
        nonempty(&socket_id, "Socket identity")?;
        self.verify_principal(principal)?;
        let checkpoint = self
            .checkpoints
            .get(&checkpoint_id)
            .ok_or_else(|| err("Socket checkpoint not found"))?;
        if checkpoint.owner_route_id != principal.route_id {
            return Err(err("Socket checkpoint belongs to another Agent route"));
        }
        if self.sockets.contains_key(&socket_id) {
            return Err(err("Socket identity already exists"));
        }
        acl.validate()?;
        self.sockets.insert(
            socket_id.clone(),
            Socket {
                socket_id,
                owner_route_id: principal.route_id.clone(),
                checkpoint_id,
                acl,
            },
        );
        Ok(())
    }

    fn allowed(acl: &[String], principal: &Principal) -> bool {
        acl.iter()
            .any(|id| id == &principal.actor_id || id == &principal.route_id)
    }

    /// Only the Socket owner may replace grants; checkpoint contents remain fixed.
    pub fn update_socket_acl(
        &mut self,
        principal: &Principal,
        socket_id: &str,
        acl: SocketAcl,
    ) -> Result<SocketAcl, String> {
        self.verify_principal(principal)?;
        acl.validate()?;
        let socket = self
            .sockets
            .get_mut(socket_id)
            .ok_or_else(|| err("Socket not found"))?;
        if socket.owner_route_id != principal.route_id {
            return Err(err("only the Socket owner can change its ACL"));
        }
        Ok(std::mem::replace(&mut socket.acl, acl))
    }

    /// Direct historical reads use read ACL only. This operation creates no route.
    pub fn read_checkpoint(&self, principal: &Principal, socket_id: &str) -> Result<Value, String> {
        self.verify_principal(principal)?;
        let socket = self
            .sockets
            .get(socket_id)
            .ok_or_else(|| err("Socket not found"))?;
        if !Self::allowed(&socket.acl.read, principal) {
            return Err(err("historical checkpoint read denied"));
        }
        Ok(self
            .checkpoints
            .get(&socket.checkpoint_id)
            .ok_or_else(|| err("Socket checkpoint missing"))?
            .snapshot
            .clone())
    }

    /// Fork a new sibling route from the Socket's frozen checkpoint.
    /// `spawn_acl` authorizes restoring that snapshot; `read_acl` remains an
    /// independent permission for non-fork direct reads, per host contract.
    pub fn spawn_peer(
        &mut self,
        principal: &Principal,
        socket_id: &str,
        new_route_id: impl Into<String>,
        request_id: impl Into<String>,
        message_id: impl Into<String>,
        question: impl Into<String>,
        lifecycle: Lifecycle,
    ) -> Result<String, String> {
        let new_route_id = new_route_id.into();
        let request_id = request_id.into();
        let message_id = message_id.into();
        let question = question.into();
        let socket = self
            .sockets
            .get(socket_id)
            .ok_or_else(|| err("Socket not found"))?
            .clone();
        if !Self::allowed(&socket.acl.spawn, principal) {
            return Err(err("historical spawn permission denied"));
        }
        nonempty(&new_route_id, "new Agent route identity")?;
        nonempty(&request_id, "request identity")?;
        nonempty(&message_id, "message identity")?;
        nonempty(&question, "peer task")?;
        self.verify_principal(principal)?;
        if self.routes.contains_key(&new_route_id) {
            return Err(err("Agent route identity already exists"));
        }
        let checkpoint = self
            .checkpoints
            .get(&socket.checkpoint_id)
            .ok_or_else(|| err("Socket checkpoint missing"))?
            .clone();
        let mut peer = AgentRoute {
            route_id: new_route_id.clone(),
            owner_actor_id: principal.actor_id.clone(),
            owners: vec![principal.actor_id.clone()],
            creators: Vec::new(),
            origin_checkpoint: Some(checkpoint.checkpoint_id.clone()),
            initial_snapshot: checkpoint.snapshot,
            status: RouteStatus::Running,
            next_sequence: 1,
            inbox: Vec::new(),
            message_receipts: BTreeMap::new(),
            signal_receipts: BTreeMap::new(),
            publication_receipts: BTreeMap::new(),
        };
        let initial = Message {
            message_id,
            request_id,
            sender_route_id: principal.route_id.clone(),
            recipient_route_id: new_route_id.clone(),
            socket_id: socket.socket_id,
            checkpoint_id: checkpoint.checkpoint_id,
            text: question,
            lifecycle,
            task_id: None,
            phase_id: None,
        };
        Self::receive_into(&mut peer, initial)?;
        self.routes.insert(new_route_id.clone(), peer);
        Ok(new_route_id)
    }

    /// Deliver to the Socket owner's live mailbox; it never changes route state.
    pub fn deliver_message(
        &mut self,
        principal: &Principal,
        socket_id: &str,
        request_id: impl Into<String>,
        message_id: impl Into<String>,
        text: impl Into<String>,
        lifecycle: Lifecycle,
        task_id: Option<String>,
        phase_id: Option<String>,
    ) -> Result<DeliveryOutcome, String> {
        let request_id = request_id.into();
        let message_id = message_id.into();
        let text = text.into();
        let socket = self
            .sockets
            .get(socket_id)
            .ok_or_else(|| err("Socket not found"))?
            .clone();
        if !Self::allowed(&socket.acl.message, principal) {
            return Err(err("Socket message permission denied"));
        }
        self.verify_principal(principal)?;
        nonempty(&request_id, "request identity")?;
        nonempty(&message_id, "message identity")?;
        let message = Message {
            message_id,
            request_id,
            sender_route_id: principal.route_id.clone(),
            recipient_route_id: socket.owner_route_id.clone(),
            socket_id: socket.socket_id,
            checkpoint_id: socket.checkpoint_id,
            text,
            lifecycle,
            task_id,
            phase_id,
        };
        let route = self
            .routes
            .get_mut(&socket.owner_route_id)
            .ok_or_else(|| err("Socket owner route not found"))?;
        Self::receive_into(route, message)
    }

    fn receive_into(route: &mut AgentRoute, message: Message) -> Result<DeliveryOutcome, String> {
        if let Some(existing) = route.message_receipts.get(&message.message_id) {
            let prior = route
                .inbox
                .iter()
                .find(|entry| entry.message.message_id == message.message_id)
                .ok_or_else(|| err("message receipt has no inbox record"))?;
            if prior.message != message {
                return Err(err("message id reused with different content or sender"));
            }
            return Ok(DeliveryOutcome {
                receipt: existing.clone(),
                replayed: true,
            });
        }
        let sequence = route.allocate_sequence();
        let receipt_id = format!("message:{}:{}", route.route_id, message.message_id);
        let receipt = MessageReceipt {
            receipt_id: receipt_id.clone(),
            sequence,
            message_id: message.message_id.clone(),
            request_id: message.request_id.clone(),
            sender_route_id: message.sender_route_id.clone(),
            recipient_route_id: route.route_id.clone(),
            socket_id: message.socket_id.clone(),
            lifecycle: message.lifecycle.clone(),
        };
        route.inbox.push(InboxEntry {
            sequence,
            message,
            receive_receipt_id: receipt_id,
            consumed_by_cut: None,
        });
        route
            .message_receipts
            .insert(receipt.message_id.clone(), receipt.clone());
        Ok(DeliveryOutcome {
            receipt,
            replayed: false,
        })
    }

    /// Snapshot the currently unconsumed ordered mailbox without changing it.
    /// `visible_in_worker` is a host-computed condition for this published view.
    pub fn preview_publication(
        &self,
        route_id: &str,
        cut_id: impl Into<String>,
        visible_in_worker: bool,
    ) -> Result<PublicationPreview, String> {
        let cut_id = cut_id.into();
        nonempty(&cut_id, "cut identity")?;
        let route = self
            .routes
            .get(route_id)
            .ok_or_else(|| err("Agent route not found"))?;
        if route.status == RouteStatus::Stopped {
            return Err(err("stopped Agent cannot publish a cut"));
        }
        if route.publication_receipts.contains_key(&cut_id) {
            return Err(err(
                "cut already has a communication receipt; retry with its original preview",
            ));
        }
        let messages = if visible_in_worker {
            route
                .inbox
                .iter()
                .filter(|entry| entry.consumed_by_cut.is_none())
                .map(|entry| PublishedMessage {
                    sequence: entry.sequence,
                    message: entry.message.clone(),
                    receive_receipt_id: entry.receive_receipt_id.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut preview = PublicationPreview {
            route_id: route_id.to_owned(),
            cut_id,
            eligible: visible_in_worker,
            messages,
            digest: String::new(),
        };
        preview.digest = Self::preview_digest(&preview)?;
        Ok(preview)
    }

    fn preview_digest(preview: &PublicationPreview) -> Result<String, String> {
        let body = serde_json::to_vec(&(
            &preview.route_id,
            &preview.cut_id,
            preview.eligible,
            &preview.messages,
        ))
        .map_err(|_| err("cannot encode publication preview"))?;
        Ok(format!("{:x}", Sha256::digest(body)))
    }

    /// Confirm message consumption only after a successful CB publication.
    /// Call within the caller's DB transaction with the actual published IDs.
    pub fn confirm_publication(
        &mut self,
        preview: &PublicationPreview,
        cb_id: impl Into<String>,
    ) -> Result<ConfirmationOutcome, String> {
        let cb_id = cb_id.into();
        nonempty(&cb_id, "published CB identity")?;
        if Self::preview_digest(preview)? != preview.digest {
            return Err(err("publication preview was modified"));
        }
        if !preview.eligible {
            return Err(err(
                "communication was not visible in the published Worker view",
            ));
        }
        let route = self
            .routes
            .get_mut(&preview.route_id)
            .ok_or_else(|| err("Agent route not found"))?;
        if route.status == RouteStatus::Stopped {
            return Err(err("stopped Agent cannot consume messages"));
        }
        if let Some(existing) = route.publication_receipts.get(&preview.cut_id) {
            if existing.cb_id == cb_id && existing.preview_digest == preview.digest {
                return Ok(ConfirmationOutcome {
                    receipt: existing.clone(),
                    replayed: true,
                });
            }
            return Err(err(
                "cut communication receipt conflicts with prior confirmation",
            ));
        }
        for published in &preview.messages {
            let current = route
                .inbox
                .iter()
                .find(|entry| entry.message.message_id == published.message.message_id)
                .ok_or_else(|| err("preview message no longer exists in inbox"))?;
            if current.sequence != published.sequence
                || current.message != published.message
                || current.consumed_by_cut.is_some()
            {
                return Err(err("preview is stale or message already consumed"));
            }
        }
        let persistent_in_cb = preview
            .messages
            .iter()
            .filter(|entry| entry.message.lifecycle == Lifecycle::Persistent)
            .cloned()
            .collect::<Vec<_>>();
        let volatile_next_wb = preview
            .messages
            .iter()
            .filter(|entry| entry.message.lifecycle == Lifecycle::Volatile)
            .cloned()
            .collect::<Vec<_>>();
        let receipt_id = format!("publication:{}:{}", preview.route_id, preview.cut_id);
        let message_ids = preview
            .messages
            .iter()
            .map(|entry| entry.message.message_id.clone())
            .collect();
        for entry in &mut route.inbox {
            if preview
                .messages
                .iter()
                .any(|item| item.message.message_id == entry.message.message_id)
            {
                entry.consumed_by_cut = Some(preview.cut_id.clone());
            }
        }
        let sequence = route.allocate_sequence();
        let receipt = PublicationReceipt {
            receipt_id,
            sequence,
            route_id: preview.route_id.clone(),
            cut_id: preview.cut_id.clone(),
            cb_id,
            preview_digest: preview.digest.clone(),
            message_ids,
            persistent_in_cb,
            volatile_next_wb,
        };
        route
            .publication_receipts
            .insert(preview.cut_id.clone(), receipt.clone());
        Ok(ConfirmationOutcome {
            receipt,
            replayed: false,
        })
    }

    /// Apply a host-authorized Signal. The result is durable data; the caller
    /// must separately make its scheduler/process obey the resulting status.
    pub fn signal(
        &mut self,
        principal: &Principal,
        socket_id: &str,
        signal_id: impl Into<String>,
        action: SignalAction,
    ) -> Result<SignalOutcome, String> {
        let signal_id = signal_id.into();
        nonempty(&signal_id, "Signal identity")?;
        let socket = self
            .sockets
            .get(socket_id)
            .ok_or_else(|| err("Socket not found"))?
            .clone();
        if !Self::allowed(&socket.acl.signal, principal) {
            return Err(err("Socket Signal permission denied"));
        }
        self.verify_principal(principal)?;
        let route = self
            .routes
            .get_mut(&socket.owner_route_id)
            .ok_or_else(|| err("Socket owner route not found"))?;
        if let Some(existing) = route.signal_receipts.get(&signal_id) {
            if existing.sender_actor_id == principal.actor_id
                && existing.sender_route_id == principal.route_id
                && existing.socket_id == socket.socket_id
                && existing.action == action
            {
                return Ok(SignalOutcome {
                    receipt: existing.clone(),
                    replayed: true,
                });
            }
            return Err(err(
                "Signal identity reused with different authority or content",
            ));
        }
        let previous_status = route.status.clone();
        let resulting_status = match (&route.status, &action) {
            (RouteStatus::Stopped, _) => RouteStatus::Stopped,
            (_, SignalAction::Stop) => RouteStatus::Stopped,
            (RouteStatus::Waiting, SignalAction::Resume | SignalAction::Wake) => {
                RouteStatus::Running
            }
            (RouteStatus::Running, SignalAction::Pause) => RouteStatus::Waiting,
            (status, _) => status.clone(),
        };
        let transitioned = resulting_status != previous_status;
        route.status = resulting_status.clone();
        let sequence = route.allocate_sequence();
        let receipt = SignalReceipt {
            sequence,
            signal_id: signal_id.clone(),
            sender_actor_id: principal.actor_id.clone(),
            sender_route_id: principal.route_id.clone(),
            target_route_id: route.route_id.clone(),
            socket_id: socket.socket_id,
            action,
            previous_status,
            resulting_status,
            transitioned,
        };
        route.signal_receipts.insert(signal_id, receipt.clone());
        Ok(SignalOutcome {
            receipt,
            replayed: false,
        })
    }
}
