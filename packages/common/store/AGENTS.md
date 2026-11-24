# Working in `packages/common/store` (for agents)

`packages/common/store` (`affine-store`) holds the client storage and sync protocol core for AFFiNE.
It owns the macro-free SQLite storage engine, the root document and metadata table writer, the narrow `WorkspaceDocDirectory` port, the sans-I/O `SpaceSync` state machine, and the source-scoped blob REST codecs.

## Boundaries and invariants

- Keep `#![forbid(unsafe_code)]` at the crate root.
- Do not add GUI, windowing, or platform crate dependencies (`ui`, `wgpu`, `objc2`, `web-sys`).
- Do not add `package.json` to this directory.
- Do not enable `sqlx` `macros`. Do not depend on `packages/frontend/native/schema` across workspaces.
- Freeze the SQLite migration sequence at version 5 (`workspace_inventory`). Keep all 5 migration strings byte-for-byte identical to `packages/frontend/native/schema/src/lib.rs`.
- Never scan a workspace directory for `*.db` files to discover local workspaces. Open only explicit allowlisted workspace identifiers.
- Never call system wall clocks (`SystemTime::now`, `Instant::now`, `Utc::now`). Pass explicit timestamps or a `Clock` implementation.
- Keep every `.rs` file at or under 800 non-test lines. Do not use `super::super::` outside `mod tests`.
- Keep both native and `wasm32-unknown-unknown` builds clean.

## Verification

Run commands inside `packages/common/store` with root NAPI flags cleared:

```sh
CARGO_ENCODED_RUSTFLAGS="" cargo fmt --all -- --check
CARGO_ENCODED_RUSTFLAGS="" cargo clippy --locked --all-targets --all-features -- -D warnings
CARGO_ENCODED_RUSTFLAGS="" cargo check --locked --target wasm32-unknown-unknown
CARGO_ENCODED_RUSTFLAGS="" cargo test --locked
```
