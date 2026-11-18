# Rust v2 deployment and recovery

This procedure applies to the authenticated local Core and the opt-in transparent DSH SDK integration. It does not install the legacy Chat gateway. The accepted release manifest and readiness report define the tested source revision, DSH source version, model route, platform and outstanding limitations. Do not infer another platform's support from the Windows ARM64 package.

## Build and package

Use a clean checkout, the committed Cargo.lock and the selected build/evidence directories. `build_private_release.ps1` builds only `sic-core-rust`, remaps private build paths, strips symbols and verifies the resulting executable. It requires the compiler and pinned dependencies already available locally. Pass an explicit target directory outside the source checkout. Build again after source changes; a prior privacy report does not certify a different binary.

```powershell
./scripts/build_private_release.ps1 -TargetDirectory $BuildDirectory -PythonExecutable $PythonExecutable
python ./scripts/package_rust_v2.py --binary "$BuildDirectory/release/sic-core-rust.exe" --output $NewPackageDirectory --runtime $ToolchainRuntimeDirectory --strip $LlvmStripExecutable
```

The package contains the executable, required ARM64 runtime and licenses, transparent plugin/source patches, backup tool, manifest and checksums. It contains no Provider settings, credentials or user database. Verify checksums before launch. The legacy `package_windows_arm64.ps1` describes the old v1 demo and is not this release path.

## Local service

Keep the data directory and its access restricted to the service account. The SQLite database, WAL and shared-memory files form live state: never copy just a live `.db` as a backup. Use a persistent local filesystem supporting SQLite WAL and atomic hard links. Keep adequate free space for the database, projections, Session logs and an independent backup. A filesystem failure must stop dispatch until durable acknowledgement can succeed.

Initialize a new database with `sic-core-rust init DB`. Serve with `sic-core-rust serve DB 127.0.0.1:PORT ACTOR TOKEN_ENV`. `TOKEN_ENV` is the name of a host-supplied environment variable containing the Core bearer. Use an independently generated value; do not put its contents on the command line or in a profile checked into source control. The transparent path needs no Core model configuration or model grant: the original Harness owns all Provider/model decisions. The CLI binds loopback only. Access from other machines requires a separately authenticated host boundary; this release does not expose a public listener.

`/health` and `/api/v2/capabilities` require authentication. Health means the process answers, not that the Provider, disk, host projection or native Session flush is healthy. Readiness must also exercise one authorized context transaction and its durable host acknowledgement. Core control RPCs have a 30-second deadline, including response bodies, and preserve the caller's cancellation reason. This deadline does not constrain Provider/model execution. An uncertain mutation is not automatically retried; inspect or replay its original identity. A second service for the same database must fail its exclusive lock. Each Session/trace source has one active Harness writer; stop the old Harness before restoring or moving its projection directory.

## DSH composition

Use the versioned `dsh-plugin/INSTALL.md`. Apply all required native SDK, Goal and jobs source patches together; add compaction when present. The raw-wire patch is optional for SDK mode. Rebuild the selected Harness and check its capability handshakes before installing the plugin. Missing required native methods are errors, not silent passthrough. Keep the original Provider adapters, tool schema/names, executors and original permission checks.

The default Cordis entry accepts JSON/YAML with a host credential environment reference and a durable binding-store path. Use its explicit bind-session command to bind an actual native Session to an existing owned chain or create one. The command persists creation intent and refuses blind recreation after an uncertain response. Host composition supplies the authenticated Core actor/credential, exact Session-to-chain/scope binding, durable projection root and the actual Session flush listener. These are trusted host dependencies; a model or a user message cannot mint them. Use the durable binding store or an equivalent authenticated existing host store. Do not substitute a transient Map that disappears on restart for deployment bindings. Preserve these bindings with the corresponding native Session records. Use the same original Provider/model/cache scope for Worker and Editor. Changing model identity is a Harness decision; it changes physical cache eligibility.

Install before a selected Agent starts. On installation unload, its frozen Agents retain dispatch/tool rejection guards and are cancelled through the original host hook; recreate them under a reviewed composition before further use. Unselected Agents continue their native behavior. Validate a new Session, tool-result continuation, higher CB publication and cold reopen through the actual original adapter. Verify that archived result text has authenticated source attribution; a tool envelope retained for call linkage may be empty after archiving. Inspect the final selected cut rather than assuming a low-level publication was eventually used. Provider retries may repeat a Worker request; SIC editing/publication and external native jobs must not repeat as a consequence.

Compression authority belongs to the selected Session. Sessions without SIC retain native automatic, manual and positional-region compaction. Sessions started with SIC use its continual editing, recursive publication and automatic common adoption; native pressure, overflow and manual compression entries must reach that same authority. A positional region without a trusted complete-CB mapping is an explicit local-interface compatibility boundary, not missing SIC core compression. It must preserve the archive and must not silently run global compression. Importing an existing ordinary Session into SIC is a separate explicit history migration, never an implicit effect of a region request or ordinary binding.

Validate each advertised native API through its actual Adapter, including tool continuation, publication, final Worker/Editor prefix adoption and recovery. An endpoint probe does not certify the full path. For multimodal Sessions, also inspect media bytes/references and their original message/tool associations after editing, recursive replacement and recovery. Report opaque preservation, editable text, actual model media understanding and external Provider API support separately; a local protocol fixture cannot establish an external Provider's capabilities.

## Backup and restore

`python scripts/backup_rust_v2.py SOURCE NEW_DESTINATION` uses SQLite backup, including committed WAL, and verifies integrity plus every immutable object's digest. The destination must not exist; partial snapshots are never published under its final name. The source is opened readonly. The tool backs up Core SQLite only.

For a complete deployment restore point, quiesce the Harness (stop accepting turns and drain/cancel its active executors), stop its writer, then snapshot Core and copy the corresponding native Session files, projection/context/trace directory and trusted Session binding state to the same backup set. Record source commit, executable hash, manifest and all file hashes. Provider credentials remain in the existing credential store. Do not replay an uncertain side effect because a receipt was lost.

Restore into a new directory, verify hashes and permissions, and start an isolated Core/Harness with the matching bindings. Check its selected cut, source/CB replay, Session acknowledgements and unknown jobs before selecting it for normal traffic. Lost process handles become unknown; the actual host must explicitly reconcile them. Retain the previous complete backup and stop its writers before switching. Never restore onto a running database or edit an immutable CB to repair it.

## Operational checks

Monitor failed durable acknowledgements, disk space, request latency, unknown claims/jobs, repeated rejection reasons and Editor progress. The trace is append-only JSONL with fsync at semantic boundaries; incomplete final lines are not published as complete events. Legacy malformed observational files remain on disk and surface diagnostics without granting authority or invalidating the independently committed projection. Complete malformed trace records remain errors. Automatic deletion of archives/projections is not implemented; size and retention belong to deployment operations.

Test abrupt Core/Harness termination, service reopen, online WAL backup and restore, simultaneous Sessions, cancellation and plugin unload in the chosen environment. Review genuine model task evidence, not just JSON shape or presence of a marker that also appeared in user input. Keep request/response metadata and usage redacted. Cache telemetry is Provider-reported evidence; equal logical prefixes alone do not prove a physical cache hit. A bounded soak sample is not a capacity SLA.

Keep the readiness report with the release. It must distinguish supported behavior, explicit unsupported requests, genuine model failures, host/environment requirements and checks that were not performed. Updating the source or Harness invalidates any assumptions about those untested combinations.
