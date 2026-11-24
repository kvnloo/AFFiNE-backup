# Working in `packages/frontend/` (for agents)

`packages/frontend/` holds the client Rust Cargo workspace (`Cargo.toml` and `Cargo.lock`) alongside legacy TypeScript frontend packages.
It uses `resolver = "3"`, edition 2024, and its own `rust-toolchain.toml` (`1.98.1` with `wasm32-unknown-unknown`) and `rustfmt.toml`.

## Workspace boundaries

- `packages/frontend/Cargo.toml` lists every client member crate explicitly (`../ui/ui`, `../../blocksuite/crates/doc`, and `../../blocksuite/crates/playground`).
  Never use directory wildcards that can scan asset directories.

- `packages/frontend/Cargo.toml` excludes `native` and `mobile-native`.
  Those legacy crates remain members of the root `Cargo.toml` workspace.

- Legacy directories (`native/`, `mobile-native/`, and `apps/`) carry a `1.97.1` `rust-toolchain.toml` file that matches the repository root byte-for-byte.
  `native/` and `mobile-native/` also carry a `rustfmt.toml` file that matches the repository root byte-for-byte.

- Client trees (`packages/frontend/`, `packages/ui/`, and `blocksuite/crates/`) share byte-identical `1.98.1` `rust-toolchain.toml` and client `rustfmt.toml` files.

- `unexpected_cfgs = "deny"` applies across the whole workspace.
  Remove dead upstream feature gates instead of allowing them.

- See `packages/ui/AGENTS.md` for GPU UI, platform matrix, and token extraction rules.
  See `blocksuite/crates/AGENTS.md` for CRDT document, playground editor, edgeless, and oracle rules.

## Static invariants and verification scripts

- `scripts/check-invariants.sh` discovers client workspace members from `cargo metadata --locked --no-deps`.
  It checks domain isolation in `packages/ui/ui`, GUI isolation in `blocksuite/crates/doc`, and `#![forbid(unsafe_code)]`.
  It checks wall-clock isolation, zero tester branches, module structure, and the 800-line non-test limit on non-vendored crates.
  It also checks the `packages/ui/ui` line ceiling (`95000`), toolchain alignment, workspace roots, YAML validity, and `CARGO_ENCODED_RUSTFLAGS=""` isolation.

- `scripts/web-smoke.mjs` serves `blocksuite/crates/playground/web/` in headless Chromium with SwiftShader WebGL2.
  It checks Wasm startup, glyph ink pixels, cross-block IME composition, and link-field paste.
  It also verifies Chromium surface baselines (`blocksuite/crates/oracle/surface-baselines.json`) and Edgeless transitions.
  Use `--query` to pass a URL query string and `--min-title-ink` to set the minimum ink threshold before capture.

## Performance guardrail

- `perf/` holds `schema.json`, `accepted.json`, `compare-n3.mjs`, and `compare-n3.test.mjs`.
  `compare-n3.mjs` compares current commit metrics against the rolling `N-3` baseline on `darksky/native-playground`.

- Deterministic byte sizes (`artifacts_bytes.*`) and work counters (non-`_ms` `benchmarks.*`) fail CI when they increase by more than 10%.

- Shared-runner wall-clock timings (`benchmarks.*_ms` and `ci_wall_ms.*`) report `ADVISORY_SLOWDOWN` without failing CI.

- Record intentional increases in `perf/accepted.json` with `{ metric, accepted_value, reason }`.
  Omit or set `null` for unmeasured metrics so new metrics start in `BOOTSTRAP` mode.

## Build and verification commands

Always clear inherited root rustflags before you run Cargo in `packages/frontend/`:

```sh
CARGO_ENCODED_RUSTFLAGS="" cargo fmt --all -- --check
CARGO_ENCODED_RUSTFLAGS="" cargo clippy --locked --workspace --all-targets --features harness -- -D warnings
CARGO_ENCODED_RUSTFLAGS="" cargo check --locked -p ui -p playground --target wasm32-unknown-unknown
CARGO_ENCODED_RUSTFLAGS="" cargo check --locked -p blocksuite-playground-doc --no-default-features --target wasm32-unknown-unknown
CARGO_ENCODED_RUSTFLAGS="" cargo test --locked --workspace
CARGO_ENCODED_RUSTFLAGS="" cargo test --locked --workspace --features harness
CARGO_ENCODED_RUSTFLAGS="" cargo run --locked --bin bs-harness --features harness -- render --theme dark --mode page --out frame.png --snapshot frame.json
CARGO_ENCODED_RUSTFLAGS="" cargo run --locked --bin bs-harness --features harness -- probes
CARGO_ENCODED_RUSTFLAGS="" cargo run --locked --bin bs-harness --features harness -- bench --json perf-metrics.json
CARGO_ENCODED_RUSTFLAGS="" cargo run --locked --bin bs-harness --features harness -- render-surface-sample --theme light --out surface-light.png --snapshot surface-light.json
CARGO_ENCODED_RUSTFLAGS="" cargo run --locked --bin bs-harness --features harness -- render-editor artifacts/editor
CARGO_ENCODED_RUSTFLAGS="" cargo run --locked --bin bs-harness --features harness -- render --state slash_menu_open --out slash.png --snapshot slash.json
CARGO_ENCODED_RUSTFLAGS="" cargo run --locked --bin bs-harness --features harness -- audit-states
CARGO_ENCODED_RUSTFLAGS="" cargo run --locked --bin bs-harness --features harness -- fps-check
CARGO_ENCODED_RUSTFLAGS="" cargo run --locked --bin bs-harness --features harness -- record-demo artifacts/demo
./scripts/check-invariants.sh
node --test perf/compare-n3.test.mjs
node ../ui/tokens/extract-tokens.mjs --check
rustc --edition 2024 --test -D warnings ../ui/tokens/tokens.rs -o /tmp/tokens_test && /tmp/tokens_test
rustc --edition 2024 --test -D warnings ../ui/tokens/icons.rs -o /tmp/icons_test && /tmp/icons_test
rm -f /tmp/tokens_test /tmp/icons_test
```
