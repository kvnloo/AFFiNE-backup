# Working in `packages/ui/` (for agents)

`packages/ui/` holds the domain-neutral GPU UI library (`ui/`) and generated design tokens (`tokens/`).
This directory belongs to the client Cargo workspace rooted at `packages/frontend/Cargo.toml`.

## Domain isolation and crate boundaries

- `packages/ui/ui/` is a single vendored GPU UI crate with zero internal `proc-macro = true` crates and zero external UI-kit dependencies.
  See `packages/ui/ui/VENDOR.md` for provenance.

- `packages/ui/ui/` must contain zero BlockSuite or AFFiNE domain terms.
  `packages/frontend/scripts/check-invariants.sh` rejects `blocksuite` and `affine` in `packages/ui/ui/src/` and `packages/ui/ui/Cargo.toml`.

- `packages/ui/ui/` embeds zero fonts.
  Applications register fonts through `cx.text_system().add_fonts(...)`.

- Keep total lines in `packages/ui/ui/src/` at or below `UI_LOC_CEILING` (`95000`).

## Platform matrix

One `ui` crate compiles across three targets:

- **macOS (`target_os = "macos"`)**: AppKit, Metal, and CoreText.
  Includes windowless `MetalHeadlessRenderer` readback, zero-initialized mip-0 `MetalAtlas`, and offscreen `MacWindow` frame fallbacks.

- **Linux headless (`target_os = "linux"`)**: `wgpu` and `cosmic-text` via `WgpuHeadlessRenderer` on Mesa `lavapipe` for deterministic CI rasterization.

- **Web (`target_family = "wasm"`)**: Single-threaded `wasm32-unknown-unknown`, WebGPU/WebGL2, `CanvasFontFallback`, and block-scoped `ImeMirror`.
  Do not add `wasm_thread` or `http_client` dependencies.

## Determinism, single clock, and callsite tracing

- **No wall clock outside `packages/ui/ui/src/platform/`**: `Instant::now()`, `SystemTime::now()`, `Zoned::now()`, and `Timestamp::now()` are forbidden outside `packages/ui/ui/src/platform/`.
  Host thread-park deadlines read monotonic time in `packages/ui/ui/src/platform/clock.rs`.
  Animations and transitions read the scheduler clock (`cx.background_executor().now()`).

- **Motion is pure data**: Declare transitions once as `(from, to, curve, start_ms, duration_ms)` and evaluate them from scheduler time.
  Never write frame-by-frame recurrence loops or `sleep` waits.

- **Callsite tracing**: Under the `harness` feature, `InteractiveElement::snapshot(...)` records bounds, masks, interactivity, focus, and `file:line` origins into `Window::frame_snapshot()`.
  The snapshot schema and audit rules live in `tools/frame-audit` (a `serde`-only path dependency re-exported from `ui::harness`).
  Non-harness release builds compile to no-op stubs.

## GPU transforms and text rendering

- `Window::with_paint_transformation` composes 2D affine transforms (`TransformationMatrix`) on a stack.
  `Window::with_local_paint_mask` intersects pre-transform local clip rects (`ContentMask<Pixels>`).

- All GPU primitives (`Quad`, `Shadow`, `Underline`, `MonochromeSprite`, `SubpixelSprite`, `PolychromeSprite`, and `PathRasterizationVertex`) must stay 16-byte aligned for WebGL2 `RGBA32U` decoding.

- Metal (`shaders.metal`) and wgpu/WebGL2 (`shaders.wgsl`, `shaders_webgl.wgsl`, `shaders_subpixel.wgsl`) transform vertices on the GPU.
  They reserve bit 31 of `quad_id` (`0x80000000u`) for solid quads, scale SDF anti-aliasing by `transform_scale`, and clip against both masks.

- `Window::paint_glyph` and `Window::paint_glyph_in_run` quantize rasterization scale to quarter-octave buckets (`[0.25, 8.0]`).
  Cached `1.0x` layouts stay crisp across `0.1x..6.0x` pan and zoom.

- `BackgroundTag::DotGrid = 4` renders the Edgeless dot grid in one fullscreen `Quad` draw call.
  CoreText and Swash specialize upright variable fonts at minimum OpenType `slnt` for italic and oblique requests.

## Design tokens (`packages/ui/tokens/`)

- `extract-tokens.mjs` generates Rust-`Ord`-sorted `tokens.rs`, `icons.rs` (with inherited `<svg fill>` and stroke attributes), and `fonts.lock`.

- Do not edit `tokens.rs`, `icons.rs`, or `fonts.lock` by hand.
  Regenerate them with `node packages/ui/tokens/extract-tokens.mjs` and verify with `--check` and `rustc --edition 2024 --test -D warnings`.
