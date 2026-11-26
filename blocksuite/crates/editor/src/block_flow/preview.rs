//! Narrow preview ports (`MathPreviewProvider`, `PdfPageProvider`) and pure-Rust
//! `blocksuite-preview` (`RaTeX` + `Hayro`) integration for `block_flow`.
//!
//! On all non-`wasm32` targets, `blocksuite-preview` (`math` + `pdf`) is compiled in directly.
//! On `wasm32`, hosts register worker-backed providers via `register_math_preview_provider` /
//! `register_pdf_page_provider` and bump [`bump_preview_revision`] whenever an async worker
//! result lands in cache so [`BlockFlow`] invalidates and repaints automatically.

use std::sync::{
    Arc, OnceLock, RwLock,
    atomic::{AtomicU64, Ordering},
};

#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;

#[cfg(not(target_arch = "wasm32"))]
use blocksuite_preview::{
    FontPool, SvgRasterizer,
    math::{MathDisplayMode, MathRenderOptions, render_latex_rgba},
    pdf::{HayroPdfRenderer, PdfPageRenderer, PdfRenderRequest},
    svg_raster::RgbaImageBuffer,
};
use ui::{Hsla, RenderImage};

#[cfg(not(target_arch = "wasm32"))]
use crate::theme::{bundled_font_bytes, bundled_script_font_bytes};

/// Rasterized LaTeX formula ready to paint as a GPUI [`RenderImage`].
#[derive(Clone)]
pub struct MathRenderSlot {
    pub image: Arc<RenderImage>,
    pub width_pt: f32,
    pub ascent_pt: f32,
    pub descent_pt: f32,
}

/// Three-state outcome of rendering a LaTeX math expression.
///
/// Distinguishes genuine syntax/parse errors (`Error` -> red `"Error equation"`) from async
/// Web Worker computations still in flight or an unconfigured worker (`Pending` -> clean
/// unframed fallback text, never `"Error equation"`).
#[derive(Clone)]
pub enum MathRenderResult {
    Ready(MathRenderSlot),
    Pending,
    Error,
}

/// Rasterized PDF page preview ready to paint inside an `affine:attachment` block.
#[derive(Clone)]
pub struct PdfPreviewPage {
    pub page_index: usize,
    pub page_count: usize,
    pub image: Arc<RenderImage>,
    pub width_pt: f32,
    pub height_pt: f32,
}

/// Narrow port allowing platform hosts (Web PDF worker, iOS PDFKit, Android PdfRenderer)
/// or tests to supply rasterized PDF pages.
pub trait PdfPageProvider: Send + Sync {
    fn page_count(&self, bytes: &[u8]) -> Option<usize>;
    fn render_pdf_page(
        &self,
        bytes: &[u8],
        page_index: usize,
        scale: f32,
    ) -> Option<PdfPreviewPage>;
}

/// Narrow port allowing platform hosts (Web RaTeX math worker) or tests to supply
/// rasterized LaTeX math bitmaps.
pub trait MathPreviewProvider: Send + Sync {
    fn render_math(
        &self,
        latex: &str,
        display: bool,
        font_size_pt: f32,
        color: Hsla,
        dpr: f32,
    ) -> MathRenderResult;
}

static PREVIEW_REVISION: AtomicU64 = AtomicU64::new(0);

/// Monotonic revision bumped whenever an asynchronous preview result (math or PDF) lands in
/// cache. Views include this in their layout cache key and `BlockFlow` fingerprints.
pub fn preview_revision() -> u64 {
    PREVIEW_REVISION.load(Ordering::Acquire)
}

/// Increment [`preview_revision`] after writing a new preview result into a provider cache.
pub fn bump_preview_revision() -> u64 {
    PREVIEW_REVISION.fetch_add(1, Ordering::AcqRel) + 1
}

fn pdf_provider_slot() -> &'static RwLock<Option<Arc<dyn PdfPageProvider>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn PdfPageProvider>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

fn math_provider_slot() -> &'static RwLock<Option<Arc<dyn MathPreviewProvider>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn MathPreviewProvider>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Register a host/worker [`PdfPageProvider`] for embedded PDF attachment previews.
pub fn register_pdf_page_provider(provider: Option<Arc<dyn PdfPageProvider>>) {
    if let Ok(mut guard) = pdf_provider_slot().write() {
        *guard = provider;
    }
    bump_preview_revision();
}

/// Whether PDF page rasterization is available in this runtime (built-in Hayro on native
/// targets or a host-registered [`PdfPageProvider`] on Web).
pub fn can_render_pdf_pages() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        true
    }
    #[cfg(target_arch = "wasm32")]
    {
        pdf_provider_slot().read().ok().is_some_and(|g| g.is_some())
    }
}

/// Register a host/worker [`MathPreviewProvider`] for rasterized LaTeX previews.
pub fn register_math_preview_provider(provider: Option<Arc<dyn MathPreviewProvider>>) {
    if let Ok(mut guard) = math_provider_slot().write() {
        *guard = provider;
    }
    bump_preview_revision();
}

/// Parsed PDF handle used to rasterize attachment preview pages on demand.
pub struct EmbeddedPdfDocument {
    #[cfg(not(target_arch = "wasm32"))]
    renderer: HayroPdfRenderer,
    #[cfg(target_arch = "wasm32")]
    bytes: Vec<u8>,
    #[cfg(target_arch = "wasm32")]
    page_count: usize,
}

impl EmbeddedPdfDocument {
    /// Total number of pages in this PDF without rasterizing any page.
    pub fn page_count(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.renderer.page_count()
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.page_count
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn shared_font_pool() -> &'static FontPool {
    static POOL: OnceLock<FontPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let mut pool = FontPool::new();
        for bytes in bundled_font_bytes()
            .into_iter()
            .chain(bundled_script_font_bytes())
        {
            let _ = pool.register_font_bytes(bytes.to_vec());
        }
        pool
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn shared_rasterizer() -> &'static SvgRasterizer {
    static RASTER: OnceLock<SvgRasterizer> = OnceLock::new();
    RASTER.get_or_init(|| SvgRasterizer::new(shared_font_pool()))
}

/// Convert raw RGBA8 pixels into a GPUI BGRA8 [`RenderImage`].
pub fn rgba_pixels_to_render_image(
    width_px: u32,
    height_px: u32,
    rgba: &[u8],
) -> Option<Arc<RenderImage>> {
    if width_px == 0
        || height_px == 0
        || rgba.len() != (width_px as usize) * (height_px as usize) * 4
    {
        return None;
    }
    let mut bgra = rgba.to_vec();
    for px in bgra.chunks_exact_mut(4) {
        ui::swap_rgba_pa_to_bgra(px);
    }
    let img = image::RgbaImage::from_raw(width_px, height_px, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(img)])))
}

/// Convert an RGBA8 [`RgbaImageBuffer`] into a GPUI BGRA8 [`RenderImage`].
#[cfg(not(target_arch = "wasm32"))]
pub fn rgba_buffer_to_render_image(buf: &RgbaImageBuffer) -> Option<Arc<RenderImage>> {
    rgba_pixels_to_render_image(buf.width_px, buf.height_px, &buf.pixels)
}

pub fn hsla_to_css_hex(color: Hsla) -> String {
    let rgba: ui::Rgba = color.into();
    let r = (rgba.r.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (rgba.g.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (rgba.b.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(not(target_arch = "wasm32"))]
type MathCacheKey = (String, bool, u32, String, u32);

/// Render and cache a LaTeX math expression through any registered [`MathPreviewProvider`]
/// or (on non-`wasm32` targets) through pure-Rust RaTeX outlines.
pub fn render_math_cached(
    latex: &str,
    display: bool,
    font_size_pt: f32,
    color: Hsla,
    dpr: f32,
) -> MathRenderResult {
    let trimmed = latex.trim();
    if trimmed.is_empty() {
        return MathRenderResult::Pending;
    }
    let norm_dpr = if dpr.is_finite() && dpr > 0.0 {
        dpr.clamp(1.0, 4.0)
    } else {
        2.0
    };
    if let Ok(guard) = math_provider_slot().read()
        && let Some(provider) = guard.as_ref()
    {
        return provider.render_math(trimmed, display, font_size_pt, color, norm_dpr);
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let color_css = hsla_to_css_hex(color);
        let key = (
            trimmed.to_string(),
            display,
            font_size_pt.to_bits(),
            color_css.clone(),
            norm_dpr.to_bits(),
        );
        static CACHE: OnceLock<RwLock<HashMap<MathCacheKey, MathRenderResult>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| RwLock::new(HashMap::new()));
        if let Ok(guard) = cache.read()
            && let Some(hit) = guard.get(&key)
        {
            return hit.clone();
        }

        let options = MathRenderOptions {
            mode: if display {
                MathDisplayMode::Display
            } else {
                MathDisplayMode::Inline
            },
            font_size_pt: f64::from(font_size_pt),
            padding_pt: if display { 4.0 } else { 1.5 },
            color_css,
        };
        let rendered = match render_latex_rgba(
            trimmed,
            &options,
            shared_font_pool(),
            shared_rasterizer(),
            norm_dpr,
        ) {
            Ok(out) => match rgba_buffer_to_render_image(&out.image) {
                Some(image) => MathRenderResult::Ready(MathRenderSlot {
                    image,
                    width_pt: out.width_pt as f32,
                    ascent_pt: out.ascent_pt as f32,
                    descent_pt: out.descent_pt as f32,
                }),
                None => MathRenderResult::Error,
            },
            Err(_) => MathRenderResult::Error,
        };

        if let Ok(mut guard) = cache.write() {
            if guard.len() > 256 {
                guard.clear();
            }
            guard.insert(key, rendered.clone());
        }
        rendered
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (display, font_size_pt, color, norm_dpr);
        MathRenderResult::Pending
    }
}

/// Render a single PDF page from an [`EmbeddedPdfDocument`] into a [`PdfPreviewPage`].
pub fn render_pdf_page(
    doc: &EmbeddedPdfDocument,
    page_index: usize,
    scale: f32,
) -> Option<PdfPreviewPage> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let page_count = doc.renderer.page_count();
        if page_count == 0 {
            return None;
        }
        let idx = page_index.min(page_count - 1);
        let buf = doc
            .renderer
            .render_page(PdfRenderRequest::for_page(idx, scale))
            .ok()?;
        let image = rgba_buffer_to_render_image(&buf)?;
        Some(PdfPreviewPage {
            page_index: idx,
            page_count,
            image,
            width_pt: buf.logical_width,
            height_pt: buf.logical_height,
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        if doc.page_count == 0 {
            return None;
        }
        let idx = page_index.min(doc.page_count - 1);
        if let Ok(guard) = pdf_provider_slot().read()
            && let Some(provider) = guard.as_ref()
        {
            return provider.render_pdf_page(&doc.bytes, idx, scale);
        }
        None
    }
}

/// Construct an [`EmbeddedPdfDocument`] from raw PDF bytes.
pub fn parse_pdf_renderer(bytes: &[u8]) -> Option<EmbeddedPdfDocument> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        HayroPdfRenderer::from_bytes(bytes.to_vec())
            .ok()
            .map(|renderer| EmbeddedPdfDocument { renderer })
    }
    #[cfg(target_arch = "wasm32")]
    {
        if !bytes.starts_with(b"%PDF-") {
            return None;
        }
        let page_count = pdf_provider_slot().read().ok().and_then(|guard| {
            guard
                .as_ref()
                .and_then(|provider| provider.page_count(bytes))
        })?;
        (page_count > 0).then(|| EmbeddedPdfDocument {
            bytes: bytes.to_vec(),
            page_count,
        })
    }
}

impl super::FlowPass<'_> {
    pub(super) fn latex_body(&mut self, latex: &str, width: f32) -> super::BlockBody {
        use super::{
            BlockBody, CARD_BLOCK_MARGIN, FlowKind, FlowRect, InlineAlign, InlineStyle, Prim,
        };
        use crate::theme::{FONT_CODE, FONT_SANS, TYPOGRAPHY};

        // Match BlockSuite `latex/src/styles.ts:5-40`: `padding: 10px 24px`, `border-radius: 4px`,
        // transparent at rest with zero permanent background or border (hover tint is drawn in
        // `overlays/paint_page`).
        let pad_x = 24.0;
        let pad_y = 10.0;
        let inner = (width - 2.0 * pad_x).max(1.0);
        let empty = latex.trim().is_empty();
        if empty {
            let style = InlineStyle::new(
                FONT_SANS,
                TYPOGRAPHY.font_xs_px,
                500.0,
                self.palette.text_secondary,
            );
            let layout = self.plain_inline("Equation", style, inner, 20.0, InlineAlign::Center);
            let height = (layout.height() + 2.0 * pad_y).max(40.0);
            let text_y = ((height - layout.height()) / 2.0).floor().max(pad_y);
            return BlockBody {
                kind: FlowKind::Latex,
                width,
                height,
                margin_top: CARD_BLOCK_MARGIN,
                margin_bottom: CARD_BLOCK_MARGIN,
                plain: "Equation".to_string(),
                prims: vec![Prim::Text {
                    x: pad_x,
                    y: text_y,
                    layout,
                    clip: Some(FlowRect::new(pad_x, pad_y, inner, height - 2.0 * pad_y)),
                }],
                text: None,
                prefix: None,
                code_scroll: None,
                pdf_controls: None,
                table_cells: Vec::new(),
            };
        }

        let dpr = self.window.scale_factor();
        match render_math_cached(latex, true, 18.0, self.palette.text_primary, dpr) {
            MathRenderResult::Ready(math) => {
                let natural_w = math.width_pt.max(1.0);
                let natural_h = (math.ascent_pt + math.descent_pt).max(18.0);
                let scale = if natural_w > inner {
                    inner / natural_w
                } else {
                    1.0
                };
                let img_w = (natural_w * scale).round().max(1.0);
                let img_h = (natural_h * scale).round().max(1.0);
                let height = (img_h + 2.0 * pad_y).max(40.0);
                let img_x = ((width - img_w) / 2.0).floor().max(pad_x);
                let img_y = ((height - img_h) / 2.0).floor().max(pad_y);
                BlockBody {
                    kind: FlowKind::Latex,
                    width,
                    height,
                    margin_top: CARD_BLOCK_MARGIN,
                    margin_bottom: CARD_BLOCK_MARGIN,
                    plain: latex.to_string(),
                    prims: vec![Prim::Image {
                        rect: FlowRect::new(img_x, img_y, img_w, img_h),
                        image: math.image,
                    }],
                    text: None,
                    prefix: None,
                    code_scroll: None,
                    pdf_controls: None,
                    table_cells: Vec::new(),
                }
            }
            MathRenderResult::Error => {
                let err_color = self
                    .input
                    .theme
                    .css_var("--affine-v2-text-highlight-fg-red")
                    .or_else(|| self.input.theme.css_var("--affine-error-color"))
                    .unwrap_or_else(|| ui::rgb(0xeb4335).into());
                let style = InlineStyle::new(FONT_SANS, TYPOGRAPHY.font_xs_px, 500.0, err_color);
                let layout =
                    self.plain_inline("Error equation", style, inner, 20.0, InlineAlign::Center);
                let height = (layout.height() + 2.0 * pad_y).max(40.0);
                let text_y = ((height - layout.height()) / 2.0).floor().max(pad_y);
                BlockBody {
                    kind: FlowKind::Latex,
                    width,
                    height,
                    margin_top: CARD_BLOCK_MARGIN,
                    margin_bottom: CARD_BLOCK_MARGIN,
                    plain: "Error equation".to_string(),
                    prims: vec![Prim::Text {
                        x: pad_x,
                        y: text_y,
                        layout,
                        clip: Some(FlowRect::new(pad_x, pad_y, inner, height - 2.0 * pad_y)),
                    }],
                    text: None,
                    prefix: None,
                    code_scroll: None,
                    pdf_controls: None,
                    table_cells: Vec::new(),
                }
            }
            MathRenderResult::Pending => {
                // Async worker calculation in progress: keep clean unframed source text with
                // zero box, zero border, and never "Error equation".
                let style = InlineStyle::new(
                    FONT_CODE,
                    TYPOGRAPHY.font_xs_px,
                    400.0,
                    self.palette.text_secondary,
                );
                let layout = self.plain_inline(latex, style, inner, 20.0, InlineAlign::Center);
                let height = (layout.height() + 2.0 * pad_y).max(40.0);
                let text_y = ((height - layout.height()) / 2.0).floor().max(pad_y);
                BlockBody {
                    kind: FlowKind::Latex,
                    width,
                    height,
                    margin_top: CARD_BLOCK_MARGIN,
                    margin_bottom: CARD_BLOCK_MARGIN,
                    plain: latex.to_string(),
                    prims: vec![Prim::Text {
                        x: pad_x,
                        y: text_y,
                        layout,
                        clip: Some(FlowRect::new(pad_x, pad_y, inner, height - 2.0 * pad_y)),
                    }],
                    text: None,
                    prefix: None,
                    code_scroll: None,
                    pdf_controls: None,
                    table_cells: Vec::new(),
                }
            }
        }
    }

    pub(super) fn unknown_body(
        &mut self,
        flavour: &str,
        block_id: &str,
        width: f32,
    ) -> super::BlockBody {
        use super::{
            BlockBody, CARD_BLOCK_MARGIN, FlowKind, FlowRect, InlineAlign, InlineStyle, Prim,
            inline::truncate_to_width,
        };
        use crate::theme::{FONT_CODE, FONT_SANS, TYPOGRAPHY};

        let pad = 12.0;
        let inner = (width - 2.0 * pad).max(1.0);
        let title_style = InlineStyle::new(
            FONT_SANS,
            TYPOGRAPHY.font_sm_px,
            500.0,
            self.palette.text_secondary,
        );
        let id_style = InlineStyle::new(
            FONT_CODE,
            TYPOGRAPHY.font_xs_px,
            400.0,
            self.palette.text_secondary,
        );
        let label = format!("Unsupported block: {flavour}");
        let title = self.plain_inline(&label, title_style, inner, 22.0, InlineAlign::Left);
        let id_text = truncate_to_width(self.window, block_id, &id_style, inner)
            .text
            .to_string();
        let id = self.plain_inline(&id_text, id_style, inner, 20.0, InlineAlign::Left);
        let height = pad + title.height() + id.height() + pad;
        let id_y = pad + title.height();
        BlockBody {
            kind: FlowKind::Unknown,
            width,
            height,
            margin_top: CARD_BLOCK_MARGIN,
            margin_bottom: CARD_BLOCK_MARGIN,
            plain: format!("[Unsupported block: {flavour}]"),
            prims: vec![
                Prim::Quad {
                    rect: FlowRect::new(0.0, 0.0, width, height),
                    fill: None,
                    border: Some(self.palette.divider),
                    radius: 8.0,
                    dashed: true,
                },
                Prim::Text {
                    x: pad,
                    y: pad,
                    layout: title,
                    clip: None,
                },
                Prim::Text {
                    x: pad,
                    y: id_y,
                    layout: id,
                    clip: None,
                },
            ],
            text: None,
            prefix: None,
            code_scroll: None,
            pdf_controls: None,
            table_cells: Vec::new(),
        }
    }
}
