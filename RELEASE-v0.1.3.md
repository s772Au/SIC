# SIC v0.1.3

This release packages the authenticated Rust v2 Core and the opt-in transparent DSH SDK integration for Windows ARM64.

## Changes since v0.1.2

- Recursive CB publication automatically advances the common Worker/Editor cut at a safe boundary. In-flight requests retain their admitted snapshots, and uncovered tails stay in the common context.
- The DSH plugin edits native SDK context before the host's original Messages, Chat Completions, or Responses adapter. Model routing, provider request options, streaming, and native tool execution remain with the host.
- Selected sessions route native automatic and manual compaction to SIC. Ordinary sessions retain DSH compaction. Binding enrolls a fresh session. Existing-session history migration requires a separate flow.
- Optional image editing preserves lineage from original images through crops, mosaics, and descriptions. Workers can inspect progressively richer representations through `sic_image_source`. The native attachment service and Sharp are required for image editing.
- The revised Goal, Job, and peer ports coordinate host lifecycle state with durable Core state. Peer recovery covers process restart and a lost activation acknowledgement before attaching a child.

## Acceptance scope

The reviewed host baseline is DSH 0.2.1-rc.2 with the supplied source patches. All three native adapter formats passed local stream and tool-continuation tests. The real AutoDL run used the native Messages route with DeepSeek-V4.1-Flash-1017 on four synthetic tasks. The Editor covered all eight target regions. Full I with the mosaic and description answered four of four tasks. The isolated mosaic answered three of four. The isolated original, crop group, and description each answered four of four. A separate Worker run recovered an omitted code by moving from description to mosaic to crop to original. These results describe this sample and model route.

The release bundle includes the Core executable and ARM64 runtime, the transparent plugin and native source patches, backup tooling, and per-file SHA-256 checksums. See the [deployment and recovery guide](docs/rust-v2-production-operations.md) and [DSH installation instructions](rust-core/dsh-plugin/transparent/INSTALL.md).
