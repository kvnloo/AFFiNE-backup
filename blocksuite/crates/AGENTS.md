# Working in `blocksuite/crates/` (for agents)

`blocksuite/crates/` holds the pure BlockSuite CRDT document engine (`doc/`), the standalone playground application (`playground/`), and browser geometry/style oracle fixtures (`oracle/`).
These crates belong to the client Cargo workspace rooted at `packages/frontend/Cargo.toml`.

## Crate boundaries

- `doc/` (`blocksuite-playground-doc` and `bs-relay`): Pure CRDT document engine and binary room sync relay built on `y-octo 0.4.0`.
  It declares `#![forbid(unsafe_code)]` and has zero GUI or platform dependencies (`ui`, `wgpu`, `objc2`, and `web-sys` are forbidden in `doc/Cargo.toml`).
  It builds on `wasm32-unknown-unknown` with `--no-default-features`.
  It preserves 53-bit `clientId`, complete flavour schema defaults with zero JS writeback, top-level Awareness fields, and `now_ms`-driven undo clocks.
  It also handles single-winner `seed-claim` arbitration, awareness refcounting, and `?ws=` relay origin forwarding.
  `read/` (`PageDocSession::read_page_snapshot`) returns an ordered `PageSnapshot` tree and preserves unknown flavours as `Unknown`.

- `playground/`: Standalone application owning `PlaygroundView`, tracked repository fonts (`Inter` and four `IBM Plex Mono` faces), desktop entry (`native-playground`), WebAssembly entry (`wasm_main` with `web/index.html`), and headless verification CLI (`bs-harness`).
  `theme.rs` pulls generated `packages/ui/tokens/tokens.rs` and `packages/ui/tokens/icons.rs` via `include!`.

- `oracle/`: Browser-captured style and surface geometry fixtures (`style-oracle.json`, `surface-oracle.json`, `surface-baselines.json`, `surface-metrics-test.ttf`), `surface-baselines.mjs`, `surface-oracle.spec.mjs`, `extract-style-oracle.spec.ts`, and browser-mode `vitest.config.ts`.
  Keep `browser.enabled: true` in `oracle/vitest.config.ts` so `scripts/ci/vitest-node-projects.mjs` skips it.

## Determinism and zero tester branches

- Headless rendering (`bs-harness render`), desktop (`main.rs`), and Web (`wasm_main`) all render the same `PlaygroundView` through `Application::with_platform(...)` and `Window::draw()`.

- Never inspect `is_headless` or add inline `#[cfg(any(test, feature = "harness"))]` branches in shipping `playground/src/` modules.

- Stepped `PlaygroundView` transitions must produce byte-identical terminal `FrameSnapshot` JSON at `60`, `120`, `24`, and `7` fps (`bs-harness fps-check`).

## Page rendering, editing, and overlays

- `BlockFlow` lays out `PageSnapshot` in world coordinates and reuses unchanged block bodies.
  `PageViewport` maps visible blocks to screen coordinates.

- Block metrics and inline code boxes mirror BlockSuite CSS and match `blocksuite/crates/oracle/style-oracle.json` in both themes.
  Change a metric only together with the oracle.

- `RichParagraph` handles rich paragraph wrapping, painting, hit-testing, soft-wrap caret affinity, UTF-16 positions, and inline references.

- `EditorSession` owns selection, caret, IME preedit, compound structural edits, clipboard slices (BlockSuite JSON, HTML with LZ-String `data-blocksuite-snapshot`, and plain text), and undo/redo.

- `Overlays` (`overlays/`) owns the slash menu, formatting toolbar, link bar, drag handle, hover state, and block controls.
  Every overlay edit commits one undo step through `PageDocSession`.
  Popovers and chevrons animate over 150 ms using scheduler time.

- `RoomProtocol` implements the sans-I/O binary room protocol (`SyncStep1`, `SyncStep2`, `Update`, awareness, and blob retries on the scheduler clock).

- Benchmarks enforce `layout_single_keystroke_blocks_relayout == 1` and `edgeless_1k_pan_zoom_blocks_relayout == 0`.

## Edgeless and surface rendering

- `edgeless/` manages the spatial camera (`[0.1, 6.0]`), merged block/vector paint order, authoring tools, hit-testing, selection handles, and inline IME editing.

- `surface::layout_surface` routes straight, orthogonal, and curved connectors, computes RoughJS scribbled shapes, and builds freehand brush and highlighter outlines.

- `oracle/surface-oracle.spec.mjs` compares BlockSuite JS renderers against `oracle/surface-oracle.json`.
  `cargo test --locked -p blocksuite-playground-doc --example bs_doc_driver js_authored_surface` checks JS-authored Yjs surface documents for zero writeback.

## Rust module organization

- Split files by responsibility and keep every non-vendored `.rs` file at or below 800 non-test lines.

- Promote `<name>.rs` to `<name>/mod.rs` when you add submodules.
  Never keep `<name>.rs` beside `<name>/`.

- Never use `super::super::` outside `mod tests`.
  Inside `src/<mod>/<child>.rs`, use `super::` instead of `crate::<mod>::`.

- Keep every `.rs` file declared by a `mod <name>;` item.
  Put unit tests inside `#[cfg(test)] mod tests` and group related cases into table-driven loops.
