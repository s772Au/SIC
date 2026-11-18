# Dependency-only SQLite security patch

Upstream SQLx SQLite driver 0.8.6, preserved under its MIT OR Apache-2.0 licenses. See SIC-UPSTREAM.json for the crates.io archive SHA-256, original git revision and original published-file hashes. Cargo.toml.orig is retained byte-for-byte from that release.

The only changed upstream file is the active normalized Cargo.toml: libsqlite3-sys 0.30.1 is replaced with exactly 0.39.1, which bundles upstream SQLite 3.54.1. Driver Rust sources, SQLite C sources and package version are not edited. The workspace crates.io patch selects this local driver; it does not alter the global Cargo registry.

RUSTSEC-2026-0459 marks libsqlite3-sys >=0.39.0 patched. SQLx releases available at this change exclude the patched dependency. Remove this local patch when an upstream release accepts a patched SQLite dependency and passes the same transactional/recovery checks.

The application also disables SQLx default features and enables only runtime-tokio/sqlite, removing unused macros/MySQL/RSA dependencies. No advisory is ignored.

Sources: https://rustsec.org/advisories/RUSTSEC-2026-0459.html and https://github.com/launchbadge/sqlx/tree/v0.8.6/sqlx-sqlite .
