# SIC v0.1.4

Rust Core packages target Windows x86-64, Windows ARM64, Linux x86-64 and Linux ARM64. Each archive records its source revision, target and per-file SHA-256 hashes. Build and verify an isolated installation before opening an existing Core database. [Deployment and recovery](docs/rust-v2-production-operations.md) · [Platform build and packaging](docs/portability-and-hosts.md)

## Native host integrations

| Host | Supported entry | Manual |
| --- | --- | --- |
| DSH | Patched native DSH 0.2.1-rc.2 integration | [DSH installation](rust-core/dsh-plugin/transparent/INSTALL.md) |
| Pi 1.5.2 | Explicit fresh session with native tools, tool-wait editing, recursive adoption, visual projection and archive lookup | [Pi native compaction](docs/pi-native-compaction.md) |
| OpenCode | Explicit fresh single-agent session, with Chat Completions, Responses and Messages routing | [OpenCode native compaction](docs/opencode-native-compaction.md) |
| Hermes v0.23.2 | Pinned source integration with a dedicated native SessionDB and custom Chat Completions route | [Hermes native compaction](docs/hermes-native-compaction.md) |
| Claude Code 2.1.328 | Dedicated text session via the native Messages gateway and pre-compact hook | [Claude Code native compaction](docs/claude-code-native-compaction.md) |
| Codex | Dedicated native App Server text session with an explicitly configured Responses route | [Codex native compaction](docs/codex-native-compaction.md) |

Cursor remains an upstream integration item. Protocol, model, and migration support follow each manual's scope.

## Visual context in Pi

Pi can use originals, separate crops, a combined mosaic or a text description. The visual Editor returns region selections and the host records the resulting immutable lineage. The Worker can look up the preceding representation through its native `sic_image_source` tool. An image can be traced from description to mosaic, crop, and original. Keep the native session and Core archive together for recovery.

Real-model validation covered selecting two bounded regions and reading both from the I-view mosaic after restarting Pi. Protocol matrices and fault recovery used native host processes with local provider fixtures. The real-model sample does not estimate general benchmark accuracy, cost, or visual token savings.
