//! Host-facing payload shapes for every Core operation.
//!
//! The dispatch table in `core.rs` validates each payload inline, so an empty
//! call answers with a precise `field must be ...` error. That precision is
//! real, but it is only reachable by making a call that fails. This module
//! states the same facts up front so a host can build a request before trying
//! it, and so the capability endpoint can answer "what does this take?"
//!
//! How the table was derived, because it matters more than the table:
//!
//! 1. Call every operation with `{}` and read the reported missing field.
//! 2. Fill that field and call again; repeat until the call succeeds or fails
//!    for a reason other than a missing field.
//! 3. Merge with a scan of the handlers for `required_str` / `field_u64` /
//!    `p.get(...)` so a field that is only reached later is not lost.
//!
//! The result is checked by `tests/capability_schema.rs`: every operation in
//! `OPERATIONS` must have exactly one entry here, and vice versa. That test
//! is what keeps the two lists from drifting apart.
//!
//! A deliberate gap: `required` lists only fields whose absence is refused.
//! Fields that must appear *together*, or that are mutually exclusive, are
//! described in `notes` rather than encoded, because a flat field list cannot
//! express "one of these two" without being wrong in the other direction.

/// One payload field and the wire type the handler enforces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadField {
    pub name: &'static str,
    pub kind: FieldKind,
}

/// The type a handler enforces, not the type it later interprets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    String,
    Integer,
    Boolean,
    Array,
    Object,
    Any,
}

impl FieldKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldKind::String => "string",
            FieldKind::Integer => "integer",
            FieldKind::Boolean => "boolean",
            FieldKind::Array => "array",
            FieldKind::Object => "object",
            FieldKind::Any => "any",
        }
    }
}

const fn s(name: &'static str) -> PayloadField {
    PayloadField { name, kind: FieldKind::String }
}
const fn n(name: &'static str) -> PayloadField {
    PayloadField { name, kind: FieldKind::Integer }
}
const fn b(name: &'static str) -> PayloadField {
    PayloadField { name, kind: FieldKind::Boolean }
}
const fn a(name: &'static str) -> PayloadField {
    PayloadField { name, kind: FieldKind::Array }
}
const fn o(name: &'static str) -> PayloadField {
    PayloadField { name, kind: FieldKind::Object }
}
const fn x(name: &'static str) -> PayloadField {
    PayloadField { name, kind: FieldKind::Any }
}

/// The payload shape of one operation.
pub struct OperationPayload {
    pub operation: &'static str,
    pub required: &'static [PayloadField],
    pub optional: &'static [PayloadField],
    /// Rules the flat field list cannot express: pairing, exclusivity, and
    /// the reason a bare call is refused when the field list looks empty.
    pub notes: &'static str,
}

/// Every operation's payload shape, in dispatch-table order.
pub const OPERATION_PAYLOADS: &[OperationPayload] = &[
    OperationPayload {
        operation: "harness.call", required: &[s("scope_id"),s("name"),o("arguments")],
        optional: &[s("grant_id"),s("event_id"),n("expected_revision")],
        notes: "Native lifecycle tools; mutations additionally require a trusted host grant and durable event identity.",
    },
    OperationPayload {
        operation: "harness.get", required: &[s("scope_id"),s("object_id")], optional: &[o("adoption_probe")],
        notes: "Read the actor-scoped SIC lifecycle record and revision; optional adoption_probe verifies actual selected-cut collection P without a grant, write, or lifecycle mutation.",
    },
    OperationPayload {
        operation: "harness.observe", required: &[s("scope_id"),s("grant_id"),s("event_id"),o("observation")], optional: &[s("object_id"),n("expected_revision")],
        notes: "Registration creates a stable activity; updates require object identity and expected revision. Exact round grants are required for goal continuations.",
    },
    OperationPayload {
        operation: "native.begin", required: &[s("format"), o("request")], optional: &[a("editable_unit_ids"),a("editable_anchors"),a("protected_cb_bindings"),a("volatile_source_bindings"),a("host_message_origins"),a("host_opaque_origins"),o("adopted_i"),o("provenance"),a("provenances"),s("idempotency_key"),s("native_step_id"),s("wire_sha256"),o("tool_wait")],
        notes: "Host-only source. Use editable_unit_ids or editable_anchors, not both; an existing chain requires an explicit scope. protected_cb_bindings associates every selected prior CB with read-only unit_ids or native_anchors. A chain-bound DSH or Pi SDK tool_wait binds assistant_message_id, assistant_event_seq and ordered call_ids to formed editable text. For pi-messages, bind the durable Session entry ID through host_message_origins. The open candidate is staged until atomic publication; the host owns the original tool results. Unbound candidates cannot be attached implicitly; model admission belongs to Harness.",
    },
    OperationPayload {
        operation: "native.get", required: &[s("source_id")], optional: &[],
        notes: "Read actor/chain-owned current native request, anchors, version, publication and receipt; source/CB revocations apply.",
    },
    OperationPayload {
        operation: "native.commit", required: &[s("source_id"),s("expected_source_digest"),n("expected_version"),a("edits")], optional: &[o("native_tool"),o("delivery"),s("idempotency_key")],
        notes: "Host-only atomic native text transaction. Identical payload retries are idempotent; a reused idempotency_key with different input conflicts. Other stale versions and protected anchors are rejected. No model called.",
    },
    OperationPayload {
        operation: "native.history", required: &[s("source_id")], optional: &[n("after_version"),n("after_i_revision"),n("limit")],
        notes: "Read source, ordered immutable candidate versions, CB publication and independent I projection receipts. Candidate and I cursors are independent. Actor/chain, current Board binding and dependency revocation apply; each page limit is 1–100.",
    },
    OperationPayload {
        operation:"native.preview",required:&[s("source_id"),s("expected_source_digest"),n("expected_version"),a("ops"),o("pvi")],optional:&[],
        notes:"Read-only host control plane: actual editing engine returns native candidate edits and material within the persisted scope. No model tool or version/CB mutation.",
    },
    OperationPayload {
        operation:"native.refresh_i",required:&[s("source_id"),n("expected_version"),x("cut_id")],optional:&[s("idempotency_key")],
        notes:"Host program refreshes only its registered readonly I text leaf after publication; immutable independent projection receipt, no candidate-version or CB mutation. Cut and Board ACL are rechecked.",
    },
    OperationPayload {
        operation: "native.publish", required: &[s("source_id"),s("expected_source_digest"),n("expected_version"),n("expected_cut"),s("idempotency_key")], optional: &[a("ops"),o("pvi"),o("replace_unpublished_source")],
        notes: "Chain-bound candidate to existing WB/edit/CB/cut. All text operations and P append into authorized native leaves; REORDER stays within a pure sibling-text editable segment. V becomes an actual pending source for a subsequent host native append and binding. Operations must reproduce the candidate. Published candidates are immutable and same-transaction retries replay.",
    },
    OperationPayload {
        operation: "native.abandon", required: &[s("native_step_id"),a("sources")], optional: &[],
        notes: "Release the exact unpublished, unassigned pending candidates for a native step, restoring their original V sources. Source and candidate versions remain in the archive.",
    },
    OperationPayload {
        operation: "chain.create",
        required: &[],
        optional: &[],
        notes: "No payload. A chain is created empty with the caller as creator.",
    },
    OperationPayload {
        operation: "chain.list",
        required: &[],
        optional: &[],
        notes: "No payload outside a chain context; lists route rows.",
    },
    OperationPayload {
        operation: "chain.get",
        required: &[],
        optional: &[],
        notes: "No payload. Returns the full snapshot; this is the most expensive read.",
    },
    OperationPayload {
        operation: "chain.checkpoint",
        required: &[],
        optional: &[x("cut_id")],
        notes: "A published cut is required; the pass cannot checkpoint an empty chain.",
    },
    OperationPayload {
        operation: "socket.create",
        required: &[s("checkpoint_id")],
        optional: &[x("acl")],
        notes: "checkpoint_id must name an existing checkpoint.",
    },
    OperationPayload {
        operation: "socket.acl.update",
        required: &[s("socket_id"), o("acl")],
        optional: &[],
        notes: "socket_id and acl required.",
    },
    OperationPayload {
        operation: "socket.read",
        required: &[s("socket_id")],
        optional: &[n("offset"), n("limit")],
        notes: "socket_id required; offset and limit page the frozen checkpoint.",
    },
    OperationPayload {
        operation: "socket.spawn",
        required: &[s("socket_id"), s("request_id"), s("message_id"), s("text")],
        optional: &[],
        notes: "socket_id, request_id, message_id and text are all required.",
    },
    OperationPayload {
        operation: "socket.message",
        required: &[s("socket_id"), s("request_id"), s("message_id"), s("text")],
        optional: &[x("task_id"), x("phase_id")],
        notes: "socket_id, request_id, message_id and text are all required.",
    },
    OperationPayload {
        operation: "socket.signal",
        required: &[s("socket_id"), s("signal_id"), s("action")],
        optional: &[],
        notes: "socket_id and signal_id required; action selects the signal operation.",
    },
    OperationPayload {
        operation: "socket.inbox",
        required: &[],
        optional: &[],
        notes: "No payload. Reads this route's inbox.",
    },
    OperationPayload {
        operation: "signal.wait",
        required: &[],
        optional: &[s("signal_id"), n("after_sequence")],
        notes: "Provide exactly one of signal_id or after_sequence; both or neither is invalid.",
    },
    OperationPayload {
        operation: "message.wait",
        required: &[n("after_sequence")],
        optional: &[],
        notes: "after_sequence must be the caller's last seen sequence, not a guess.",
    },
    OperationPayload {
        operation: "peer.tasks",
        required: &[n("offset"), n("limit")],
        optional: &[],
        notes: "offset and limit page host-plane task rows.",
    },
    OperationPayload {
        operation: "peer.task.claim",
        required: &[s("task_id"), s("claim_id")],
        optional: &[],
        notes: "task_id and claim_id are required.",
    },
    OperationPayload {
        operation: "peer.task.prepare",
        required: &[],
        optional: &[o("policy")],
        notes: "A bounded scheduler policy is required.",
    },
    OperationPayload {
        operation: "peer.task.reply",
        required: &[s("task_id"), s("reply_id"), s("expected_attempt_id"), s("socket_id")],
        optional: &[],
        notes: "task_id, reply_id, expected_attempt_id and socket_id are all required.",
    },
    OperationPayload {
        operation: "peer.task.advance",
        required: &[s("task_id"), s("claim_id"), s("event_id"), s("state"), n("expected_revision")],
        optional: &[],
        notes: "state must be running, completed or failed.",
    },
    OperationPayload {
        operation: "peer.task.unknown",
        required: &[s("task_id"), s("claim_id"), s("event_id"), n("expected_revision")],
        optional: &[s("reason")],
        notes: "reason is optional; the outcome is recorded as unknown, never as success.",
    },
    OperationPayload {
        operation: "board.create",
        required: &[],
        optional: &[],
        notes: "",
    },
    OperationPayload {
        operation: "board.bind",
        required: &[],
        optional: &[],
        notes: "",
    },
    OperationPayload {
        operation: "board.unbind",
        required: &[],
        optional: &[],
        notes: "",
    },
    OperationPayload {
        operation: "board.get",
        required: &[],
        optional: &[],
        notes: "board_id or a bound board is required; binding is what makes the short form work.",
    },
    OperationPayload {
        operation: "board.acl.update",
        required: &[s("idempotency_key"), n("expected_revision")],
        optional: &[x("acl")],
        notes: "board_id or bound board; acl is required in practice and is validated separately.",
    },
    OperationPayload {
        operation: "board.entry.text",
        required: &[s("idempotency_key"), s("text"), n("expected_revision")],
        optional: &[],
        notes: "board_id or bound board; text is required and expected_revision guards the CAS.",
    },
    OperationPayload {
        operation: "board.entry.data",
        required: &[s("idempotency_key"), n("expected_revision")],
        optional: &[x("content")],
        notes: "board_id or bound board; content is required and expected_revision guards the CAS.",
    },
    OperationPayload {
        operation: "board.task.create",
        required: &[s("idempotency_key"), s("title"), n("expected_revision")],
        optional: &[x("detail")],
        notes: "board_id or bound board; title required; detail optional.",
    },
    OperationPayload {
        operation: "board.task.claim",
        required: &[s("task_id"), s("idempotency_key"), n("expected_revision")],
        optional: &[],
        notes: "board_id or bound board; idempotency_key makes the claim replay-safe.",
    },
    OperationPayload {
        operation: "board.task.release",
        required: &[s("task_id"), s("idempotency_key"), n("expected_revision")],
        optional: &[],
        notes: "board_id or bound board; release without idempotency_key cannot be replayed.",
    },
    OperationPayload {
        operation: "board.task.progress",
        required: &[s("task_id"), s("idempotency_key"), n("expected_revision")],
        optional: &[x("status")],
        notes: "board_id or bound board; status is the new progress label.",
    },
    OperationPayload {
        operation: "board.task.transition",
        required: &[s("task_id"), s("idempotency_key"), n("expected_revision")],
        optional: &[x("status")],
        notes: "board_id or bound board; status is the terminal transition.",
    },
    OperationPayload {
        operation: "board.history",
        required: &[],
        optional: &[x("after_revision"), x("limit")],
        notes: "board_id or bound board; after_revision and limit page the history.",
    },
    OperationPayload {
        operation: "tools.configure",
        required: &[o("acl"), a("tools")],
        optional: &[],
        notes: "acl must be an object with discover/expand/execute grants; tools must be an array.",
    },
    OperationPayload {
        operation: "tools.sync",
        required: &[],
        optional: &[x("acl")],
        notes: "acl required; tools array required; unlike configure, a sync needs an existing toolset.",
    },
    OperationPayload {
        operation: "tools.update_acl",
        required: &[],
        optional: &[x("acl")],
        notes: "acl required; only the ACL is replaced, the catalog is left as it was.",
    },
    OperationPayload {
        operation: "tools.register",
        required: &[o("descriptor")],
        optional: &[],
        notes: "descriptor required; the descriptor carries name, description and schema.",
    },
    OperationPayload {
        operation: "tools.status",
        required: &[],
        optional: &[],
        notes: "No payload; fails with not_found when no toolset is configured.",
    },
    OperationPayload {
        operation: "tools.tasks",
        required: &[],
        optional: &[],
        notes: "No payload.",
    },
    OperationPayload {
        operation: "tool.claim",
        required: &[s("task_id"), s("claim_id")],
        optional: &[],
        notes: "task_id and claim_id are required.",
    },
    OperationPayload {
        operation: "tool.complete",
        required: &[s("task_id")],
        optional: &[x("succeeded"), x("delivery")],
        notes: "task_id required; succeeded and delivery describe the outcome.",
    },
    OperationPayload {
        operation: "tool.observe",
        required: &[s("task_id"), s("claim_id"), s("event_id"), s("source"), s("outcome")],
        optional: &[],
        notes: "task_id, claim_id, event_id, source and outcome are all required.",
    },
    OperationPayload {
        operation: "source.ingest",
        required: &[o("request")],
        optional: &[s("format"), s("idempotency_key")],
        notes: "request must be a Chat request object, not a message array; only openai-chat is supported.",
    },
    OperationPayload {
        operation: "source.read",
        required: &[s("source_id")],
        optional: &[b("group")],
        notes: "source_id required; group optionally narrows to one group.",
    },
    OperationPayload {
        operation: "source.text",
        required: &[s("source_id"), s("unit_id")],
        optional: &[],
        notes: "source_id and unit_id are required; read source.read first to get unit IDs.",
    },
    OperationPayload {
        operation: "wb.form",
        required: &[a("source_ids")],
        optional: &[s("reason"), b("include_earlier_pending")],
        notes: "source_ids must list the earliest consecutive pending sources.",
    },
    OperationPayload {operation:"edit.host.prepare",required:&[s("source_id")],optional:&[a("assignments")],notes:"Plan a durable host Editor on an existing WB without model profiles or Core Provider execution."},
    OperationPayload {operation:"edit.host.get",required:&[s("job_id")],optional:&[s("editor")],notes:"Read source snapshot, assigned units, claim metadata and current freshness without consuming execution permission."},
    OperationPayload {operation:"edit.host.claim",required:&[s("job_id"),s("editor"),s("claim_id")],optional:&[],notes:"Only the first claim response grants execute=true; retries expose state and cannot dispatch another Editor."},
    OperationPayload {operation:"edit.host.submit",required:&[s("job_id"),s("editor"),s("claim_id")],optional:&[a("ops"),o("pvi")],notes:"Validate and persist the claimed original Harness Editor delivery; exact replay is idempotent, stale cut or epochs conflict."},
    OperationPayload {operation:"edit.host.fail",required:&[s("job_id"),s("editor"),s("claim_id"),s("reason")],optional:&[],notes:"Record an unknown external outcome; scheduler never automatically reissues this attempt."},
    OperationPayload {operation:"edit.host.reopen",required:&[s("job_id"),s("editor"),s("claim_id"),b("confirm"),s("reason")],optional:&[],notes:"Explicitly reopen a current failed single-Editor job without a candidate. Preserves the failed attempt and requires a different claim."},
    OperationPayload {
        operation: "edit.prepare",
        required: &[s("source_id")],
        optional: &[],
        notes: "source_id must name a pending WB source.",
    },
    OperationPayload {
        operation: "edit.program",
        required: &[s("source_id")],
        optional: &[],
        notes: "source_id must name a pending WB source.",
    },
    OperationPayload {
        operation: "edit.submit",
        required: &[s("job_id"), s("editor"), a("ops"), o("pvi")],
        optional: &[],
        notes: "job_id and editor are required.",
    },
    OperationPayload {
        operation: "edit.run",
        required: &[s("job_id")],
        optional: &[],
        notes: "",
    },
    OperationPayload {
        operation: "edit.validate",
        required: &[s("job_id")],
        optional: &[],
        notes: "",
    },
    OperationPayload {
        operation: "edit.candidate.get",
        required: &[s("job_id")],
        optional: &[],
        notes: "",
    },
    OperationPayload {
        operation: "edit.candidate.update",
        required: &[s("job_id"), n("expected_revision"), s("editor"), a("ops"), o("pvi")],
        optional: &[],
        notes: "job_id and expected_revision are required; the revision guards the CAS.",
    },
    OperationPayload {
        operation: "worker.run",
        required: &[],
        optional: &[],
        notes: "Accepts no payload: only the optional retry_after_attempt, which a host should not send.",
    },
    OperationPayload {
        operation: "runtime.abandon",
        required: &[s("kind")],
        optional: &[],
        notes: "kind must be worker or editor.",
    },
    OperationPayload {
        operation: "runtime.discard_unpublished",
        required: &[s("reason"), b("confirm")],
        optional: &[],
        notes: "confirm must be true; the operation refuses a bare call on purpose.",
    },
    OperationPayload {
        operation: "runtime.schedule",
        required: &[],
        optional: &[],
        notes: "A bounded scheduler policy is required; see SchedulerPolicy in scheduler.rs. At a safe planning boundary it adopts already-published higher shared representations within that policy; new publications adopt atomically.",
    },
    OperationPayload {
        operation: "runtime.budget",
        required: &[s("wb_source_id")],
        optional: &[],
        notes: "Provide either budget (to grant) or permit_id with cost (to consume).",
    },
    OperationPayload {
        operation: "runtime.wait",
        required: &[s("action")],
        optional: &[],
        notes: "action must name a supported wait action; an unknown action is rejected.",
    },
    OperationPayload {
        operation: "experiment.pack",
        required: &[],
        optional: &[],
        notes: "No payload. Produces the reproducible package.",
    },
    OperationPayload {
        operation: "experiment.verify",
        required: &[o("pack")],
        optional: &[],
        notes: "pack must be the object returned by experiment.pack.",
    },
    OperationPayload {
        operation: "accounting.get",
        required: &[],
        optional: &[],
        notes: "No payload. Reports host-measured resources and the stats projection.",
    },
    OperationPayload {
        operation: "legacy.status",
        required: &[],
        optional: &[],
        notes: "No payload.",
    },
    OperationPayload {
        operation: "legacy.queue",
        required: &[],
        optional: &[],
        notes: "No payload.",
    },
    OperationPayload {
        operation: "legacy.activation.audit",
        required: &[],
        optional: &[],
        notes: "Requires a verified upgrade manifest for the target Agent.",
    },
    OperationPayload {
        operation: "legacy.activation.apply",
        required: &[n("expected_board_revision"), s("idempotency_key")],
        optional: &[],
        notes: "expected_board_revision and idempotency_key required; also needs a verified manifest.",
    },
    OperationPayload {
        operation: "legacy.activation.start",
        required: &[s("idempotency_key")],
        optional: &[],
        notes: "idempotency_key required; also needs a verified manifest.",
    },
    OperationPayload {
        operation: "cut.publish",
        required: &[s("job_id"), n("expected_cut"), s("idempotency_key")],
        optional: &[],
        notes: "job_id and expected_cut are required; idempotency_key makes the publish replay-safe. Publishing in the active public context updates the common Worker/Editor representation in the same cut, replacing covered lower CBs while retaining uncovered tail and archives.",
    },
    OperationPayload {
        operation: "cut.get",
        required: &[],
        optional: &[],
        notes: "No payload. Returns the current cut view and pending sources.",
    },
    OperationPayload {
        operation: "memory.page",
        required: &[s("kind"), s("object_id")],
        optional: &[],
        notes: "kind must be source, cb, ops or wb; object_id must name an object on this chain.",
    },
    OperationPayload {
        operation: "memory.search",
        required: &[s("query")],
        optional: &[],
        notes: "query is required; limit and offset page the results.",
    },
    OperationPayload {
        operation: "memory.replay",
        required: &[s("cb_id")],
        optional: &[n("step"), n("offset"), n("limit")],
        notes: "cb_id must name a CB on this chain; step is the operation prefix length.",
    },
    OperationPayload {
        operation: "hierarchy.get",
        required: &[],
        optional: &[s("context_id")],
        notes: "No payload; context_id optional.",
    },
    OperationPayload {
        operation: "hierarchy.form",
        required: &[n("level")],
        optional: &[a("cb_ids"),a("volatile_source_ids"),s("context_id")],
        notes: "level must be >= 2; a higher WB cannot be formed at level 1.",
    },
    OperationPayload {
        operation: "hierarchy.timeline",
        required: &[],
        optional: &[s("context_id"), n("level"), s("kind"), n("offset"), n("limit")],
        notes: "context_id, level and kind select the timeline to page.",
    },
    OperationPayload {
        operation: "hierarchy.expand",
        required: &[],
        optional: &[s("cb_id"), s("wb_id")],
        notes: "Specify exactly one of cb_id or wb_id; both together is invalid.",
    },
    OperationPayload {
        operation: "hierarchy.select",
        required: &[n("level")],
        optional: &[s("context_id"),a("roles")],
        notes: "level must name a level that already has a published CB. A public Worker or Editor selection updates both common roles; per-Editor private background remains separate.",
    },
    OperationPayload {
        operation: "hierarchy.context.create",
        required: &[s("context_id")],
        optional: &[],
        notes: "context_id must be new.",
    },
    OperationPayload {
        operation: "hierarchy.context.ingest",
        required: &[s("context_id"), o("request")],
        optional: &[s("format"), s("idempotency_key")],
        notes: "context_id plus a Chat request object; only openai-chat is supported.",
    },
    OperationPayload {
        operation: "memory.archive_search",
        required: &[s("query")],
        optional: &[n("limit"), o("cursor")],
        notes: "Search original published source units in the Worker context; continue with the returned cursor.",
    },
    OperationPayload {
        operation: "memory.archive_expand",
        required: &[],
        optional: &[s("cut_id"), s("cb_id"), s("wb_id"), n("offset"), n("limit")],
        notes: "Specify one cut_id, cb_id or wb_id. Page the Worker directory or a published WB and CB pair.",
    },
    OperationPayload {
        operation: "memory.archive_ops",
        required: &[s("kind"), s("object_id")],
        optional: &[n("offset"), n("limit")],
        notes: "Use kind ops to page the ordered operation record of a published Worker CB.",
    },
    OperationPayload {
        operation: "memory.source_text_page",
        required: &[s("source_id"), s("unit_id")],
        optional: &[n("offset"), n("limit")],
        notes: "Read exact text from a published source in the Worker context.",
    },
    OperationPayload {
        operation: "decision.get",
        required: &[],
        optional: &[],
        notes: "No payload.",
    },
    OperationPayload {
        operation: "decision.update",
        required: &[n("expected_revision"), a("events")],
        optional: &[],
        notes: "expected_revision guards the four-view decision CAS. Available only in a decision-tree build.",
    },
    OperationPayload {
        operation: "config.get",
        required: &[],
        optional: &[],
        notes: "No payload.",
    },
    OperationPayload {
        operation: "config.update",
        required: &[],
        optional: &[s("task_goal"), a("task_constraints"), s("editor_prompt"), s("schedule_policy"), n("interleave_budget")],
        notes: "Any subset of configuration keys; unknown keys are refused.",
    },
    OperationPayload {
        operation: "security.revoke",
        required: &[s("source_id")],
        optional: &[],
        notes: "",
    },
    OperationPayload {
        operation: "security.classify",
        required: &[s("source_id")],
        optional: &[x("idempotency_key")],
        notes: "source_id required; policy selects the classification.",
    },
    OperationPayload {
        operation: "signal.spawn",
        required: &[s("child_route_id")],
        optional: &[x("limits"), x("initial_policy")],
        notes: "Set origin_route_id plus at least one prefix_cb_ids to fork; omit both to spawn.",
    },
    OperationPayload {
        operation: "signal.pause",
        required: &[s("target_route_id")],
        optional: &[x("mode"), x("wake_reason")],
        notes: "mode selects now or after-output.",
    },
    OperationPayload {
        operation: "signal.stop",
        required: &[s("target_route_id")],
        optional: &[x("mode")],
        notes: "Stop records control_epoch; it does not terminate a child process.",
    },
    OperationPayload {
        operation: "signal.sleep",
        required: &[],
        optional: &[x("timeout_s"), x("reason")],
        notes: "sleep is a pause, never a self-stop.",
    },
    OperationPayload {
        operation: "signal.source_policy",
        required: &[s("target_route_id")],
        optional: &[],
        notes: "A terminal Agent accepts no sources.",
    },
    OperationPayload {
        operation: "signal.admin_grant",
        required: &[s("target_route_id"), s("holder_route_id")],
        optional: &[],
        notes: "holder_route_id plus at least one right; a grant with no rights is invalid.",
    },
    OperationPayload {
        operation: "signal.admin_revoke",
        required: &[s("target_route_id"), s("holder_route_id")],
        optional: &[],
        notes: "Self rights cannot be revoked.",
    },
    OperationPayload {
        operation: "signal.source.register",
        required: &[s("source_id"), s("kind")],
        optional: &[x("owner")],
        notes: "kind must be a known source kind.",
    },
    OperationPayload {
        operation: "signal.arrive",
        required: &[s("source_id"), s("target_route_id")],
        optional: &[x("payload")],
        notes: "The source must be registered before admission.",
    },
    OperationPayload {
        operation: "signal.drain",
        required: &[],
        optional: &[x("limit")],
        notes: "limit caps how many arrivals are drained.",
    },
    OperationPayload {
        operation: "signal.status",
        required: &[],
        optional: &[],
        notes: "No payload; route_id selects the route.",
    },
    OperationPayload {
        operation: "signal.broadcast",
        required: &[],
        optional: &[x("log_limit")],
        notes: "log_limit caps the broadcast log.",
    },
];

/// Look up one operation's payload shape.
pub fn operation_payload(operation: &str) -> Option<&'static OperationPayload> {
    OPERATION_PAYLOADS
        .iter()
        .find(|row| row.operation == operation)
}

/// Render one payload shape as JSON for the capability response.
pub fn payload_json(row: &OperationPayload) -> serde_json::Value {
    use serde_json::json;
    let describe = |fields: &[PayloadField]| {
        fields
            .iter()
            .map(|field| json!({ "name": field.name, "kind": field.kind.as_str() }))
            .collect::<Vec<_>>()
    };
    json!({
        "required": describe(row.required),
        "optional": describe(row.optional),
        "notes": row.notes,
    })
}
