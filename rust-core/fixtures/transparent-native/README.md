# Native SIC text editing, host-owned dispatch

This fixture compiles and executes the actual `src/protocol.rs` and
`src/transparent.rs` modules. It opens only listeners on `127.0.0.1:0`, uses
synthetic request/response bodies and bypasses proxy environment configuration.
It does not open a Core database, load model profiles, resolve credentials or
contact any external provider.

Run from this worktree in PowerShell:

```powershell
$env:CARGO_TARGET_DIR='./03-backend-builds\transparent-native-20261117-target'
cargo test --manifest-path rust-core\fixtures\transparent-native\Cargo.toml --offline --locked
```

The public API is also exported by the real Rust library as
`sic_core_rust::transparent`. A host integration can use it directly:

```rust,ignore
let mut context = NativeContext::new(native_format, &incoming_request)?;
let source = context.source_digest().to_owned();
let version = context.version();
// SIC's authorized Editor/transaction coordinator supplies edits against
// context.units(). No request JSON may grant itself editing authority.
let receipt = context.commit(&source, version, &authorized_edits)?;
// Persist context + receipt using the Core's transaction boundary as needed.
let response = context.dispatch(|edited_native_request| async move {
    harness.admit_and_dispatch(original_route, edited_native_request).await
}).await?;
```

`dispatch` returns the host's response type unchanged. The host must admit the
final edited native input, keep its existing route/model configuration and
forward native streaming responses without converting or buffering them.
The module never calls `provider.rs`, counts tokens or rewrites model controls.
Unknown fields/blocks are retained as immutable sidecar data; this is text
extraction rather than provider-schema validation.

The Chat HTTP fixture preserves `n=2`, nondefault temperature/budget/seed,
reasoning, stream usage options, metadata and function schema. It proves that
the first native SSE bytes arrive before the provider can produce the final
bytes. It also preserves nonstreaming multi-choice output order. Separate
Responses and Anthropic HTTP routes preserve their native event bytes and
response status/headers. All request assertions compare complete JSON Values
after changing only the authorized text anchor.

Core operations now make this path callable through the existing authenticated
RPC boundary with durable transactions:

- `native.begin {format, request}` creates an actor-owned immutable source.
- `native.get {source_id}` returns current native request, source request,
  source digest, text units, durable version and last receipt.
- `native.commit {source_id, expected_source_digest, expected_version, edits}`
  applies `{unit_id, expected_text, replacement}` edits together. Optional
  `native_tool {name, arguments_sha256}` preserves host tool provenance.

Results use the standard Core envelope's `result` field. `begin` and `commit`
require host `allow_write`; all operations enforce authenticated source actor.
They require no model configuration and have `model=false`. The Harness must
perform scoped Editor authorization before calling host-only `native.commit`;
request JSON does not grant a model an editing role. Versions and receipts are
immutable Store objects; the `BEGIN IMMEDIATE` transaction serializes version
checks and successor insertion across independently opened Stores. Rehydration
proves that source/current differ only at supported text anchors. The source
request and all original model/options remain inspectable with `native.get`.

Run the real Core and capability tests with:

```powershell
cargo test -p sic-core-rust --test native_transactions --test capability_schema --offline --locked
```

Scope: existing Editor scheduling, DSH ingress, published-context projection
and shared-cache routing still need to call this API. The existing `dsh_bridge`
remains on its historical fixed-provider route
until that integration is changed. This fixture does not establish that the
running DSH service has adopted the new path.
