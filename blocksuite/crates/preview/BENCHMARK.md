# Preview Kernel Benchmark and Engine Selection

This document records empirical measurements for `blocksuite-preview` on Linux x86_64 (Intel Xeon Platinum 8581C, Rust 1.98.1, `--release` with `opt-level = "s"`, `lto = "thin"`, `strip = true`).

## 1. LaTeX Math Engine Selection

We evaluated two pure-Rust candidates against a 48-formula `0.27.5` KaTeX corpus (subscripts, fractions, roots, matrices, sums, integrals, limits, Greek symbols, math alphabets, `\text{CJK}`, `aligned`, `cases`, `array`, `\left...\right`, accents, logic operators, and boxed color expressions) plus four AFFiNE edge cases.

### Summary Comparison

| Metric | RaTeX (`0.1.16`, `embed-fonts`) | `mitex 0.2.4` + `InMemoryTypstWorld` (`0.15.1`) |
| --- | --- | --- |
| 48-formula corpus pass rate | **48 / 48 (100.0%)** | 26 / 48 (54.2%) without external `@preview/mitex` package |
| Baseline metric fidelity (`ascent_pt` / `descent_pt`) | Exact KaTeX box height and depth | Outer page frame reports `descent_pt = 0.00` on inline fractions |
| Release batch throughput (3 runs) | 1,920 renders in `155 / 141 / 138 ms` (`~0.07 ms` per formula) | 52 renders in `20 / 22 / 14 ms` (`0.27` to `0.42 ms` per formula) |
| Standalone Worker Wasm (`raw`) | **3,592,190 B (3.43 MiB)** | 23,994,025 B (22.88 MiB, `typst` feature) |
| Standalone Worker Wasm (`gzip -9`) | **1,178,106 B (1,150.5 KiB)** | 7,882,611 B (7,697.9 KiB, `typst` feature) |

### Baseline Metric Samples (`16 pt` base size)

| Sample | RaTeX (`width / ascent / descent pt`) | `mitex` + Typst (`width / ascent / descent pt`) |
| --- | --- | --- |
| `E = mc^2` | `61.44 / 13.03 / 0.00` | `62.65 / 10.93 / 0.00` |
| `x_{i,j}^{n+1} + y_k^{(2)}` | `74.06 / 16.72 / 6.61` | `76.87 / 10.93 / 0.00` |
| `\frac{a+b}{c+d}` | `23.28 / 14.08 / 6.45` | `24.52 / 10.93 / 0.00` |

### Selection Decision and Unsupported Macro Ledger

RaTeX wins on corpus coverage, baseline precision, latency, and Wasm bundle size. We ship RaTeX with `embed_glyphs: true` as the production `math` backend and keep `InMemoryTypstWorld` under the separate `typst` feature for native Typst code blocks.

| Edge Case | RaTeX | `mitex` + Typst | Notes |
| --- | --- | --- | --- |
| `\def\arraystretch{1.5}` + `\hdashline` array | Pass | Fail | Matches AFFiNE `inline.spec.ts` fixture. |
| `\vcenter{\hbox{...}}` | Pass | Fail | Matches AFFiNE `inline.spec.ts` highlight fixture. |
| `\ce{2H2 + O2 -> 2H2O}` (`mhchem`) | Pass | Fail | Supported natively by `ratex-parser/mhchem`. |
| Dynamic `\newcommand{\R}{\mathbb{R}}` | Unsupported | Pass | Documented macro limit. Callers show an inline error badge. |

## 2. Mermaid 2x RGBA Footprint and Cache Calibration

All nine diagram families ran through `merman 0.11.0` with `SvgPipeline::resvg_safe()` (zero `<foreignObject>` elements) and explicit `PreviewDate` injection, then rasterized at `2.0x` scale via `SvgRasterizer`.

| Diagram Family | 2x Pixel Dimensions | RGBA8 Bytes | Size (KiB) |
| --- | --- | --- | --- |
| `flowchart` | `720 x 836` | 2,407,680 | 2,351.2 |
| `flowchart_cjk` | `1104 x 122` | 538,752 | 526.1 |
| `sequence` (CJK) | `900 x 562` | 2,023,200 | 1,975.8 |
| `class` | `384 x 622` | 955,392 | 933.0 |
| `state` | `304 x 874` | 1,062,784 | 1,037.9 |
| `er` | `270 x 922` | 995,760 | 972.4 |
| `gantt` (CJK) | `1600 x 296` | 1,894,400 | 1,850.0 |
| `pie` (CJK) | `1160 x 900` | 4,176,000 | 4,078.1 |
| `gitgraph` | `541 x 329` | 711,956 | 695.3 |

The nine-family mean is 1,602 KiB (1.56 MiB) and the peak is 4,078 KiB (3.98 MiB). Setting `CacheLimits::MOBILE_DEFAULT` to `max_entries = 64` and `max_bytes = 32 MiB` holds about 20 full-size 2x Mermaid diagrams or 64 smaller formula tiles while `PinnedPixelCache` protects all visible on-screen entries from eviction.

## 3. Standalone WebAssembly Worker Sizes

Each preview capability builds independently for `wasm32-unknown-unknown --release` with `--no-default-features --features <feature>,wasm-worker` so the main Web application pays zero upfront preview bundle cost.

| Worker Feature | Raw `.wasm` Bytes | Raw Size | `gzip -9` Bytes | `gzip -9` Size |
| --- | --- | --- | --- | --- |
| `math,wasm-worker` (RaTeX + outlines) | 3,592,190 | 3.43 MiB | 1,178,106 | 1,150.5 KiB |
| `pdf,wasm-worker` (`hayro 0.9.0`) | 5,514,543 | 5.26 MiB | 1,808,003 | 1,765.6 KiB |
| `mermaid,wasm-worker` (`merman 0.11.0`) | 16,843,820 | 16.06 MiB | 5,167,811 | 5,046.7 KiB |
| `typst,wasm-worker` (`typst 0.15.1`) | 23,994,025 | 22.88 MiB | 7,882,611 | 7,697.9 KiB |
