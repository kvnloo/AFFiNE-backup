//! Cached RaTeX vector math and Hayro PDF rasterization bridge for `block_flow`.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock},
};

use blocksuite_preview::{
    FontPool, SvgRasterizer,
    math::{MathDisplayMode, MathRenderOptions, render_latex_rgba},
    pdf::{HayroPdfRenderer, PdfPageRenderer, PdfRenderRequest},
    svg_raster::RgbaImageBuffer,
};
use ui::{Hsla, RenderImage};

use crate::theme::{bundled_font_bytes, bundled_script_font_bytes};

/// Rasterized LaTeX formula ready to paint as a GPUI [`RenderImage`].
#[derive(Clone)]
pub struct MathRenderSlot {
    pub image: Arc<RenderImage>,
    pub width_pt: f32,
    pub ascent_pt: f32,
    pub descent_pt: f32,
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

fn shared_rasterizer() -> &'static SvgRasterizer {
    static RASTER: OnceLock<SvgRasterizer> = OnceLock::new();
    RASTER.get_or_init(|| SvgRasterizer::new(shared_font_pool()))
}

/// Convert an RGBA8 [`RgbaImageBuffer`] into a GPUI BGRA8 [`RenderImage`].
pub fn rgba_buffer_to_render_image(buf: &RgbaImageBuffer) -> Option<Arc<RenderImage>> {
    if buf.width_px == 0 || buf.height_px == 0 {
        return None;
    }
    let mut bgra = buf.pixels.clone();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let img = image::RgbaImage::from_raw(buf.width_px, buf.height_px, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(img)])))
}

fn hsla_to_css_hex(color: Hsla) -> String {
    let rgba: ui::Rgba = color.into();
    let r = (rgba.r.clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (rgba.g.clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (rgba.b.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{r:02x}{g:02x}{b:02x}")
}

type MathCacheKey = (String, bool, u32, String);

/// Render and cache a LaTeX math expression through pure-Rust RaTeX outlines.
pub fn render_math_cached(
    latex: &str,
    display: bool,
    font_size_pt: f32,
    color: Hsla,
) -> Option<MathRenderSlot> {
    let trimmed = latex.trim();
    if trimmed.is_empty() {
        return None;
    }
    let color_css = hsla_to_css_hex(color);
    let key = (
        trimmed.to_string(),
        display,
        font_size_pt.to_bits(),
        color_css.clone(),
    );
    static CACHE: OnceLock<RwLock<HashMap<MathCacheKey, Option<MathRenderSlot>>>> = OnceLock::new();
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
    let rendered = render_latex_rgba(
        trimmed,
        &options,
        shared_font_pool(),
        shared_rasterizer(),
        2.0,
    )
    .ok()
    .and_then(|out| {
        let image = rgba_buffer_to_render_image(&out.image)?;
        Some(MathRenderSlot {
            image,
            width_pt: out.width_pt as f32,
            ascent_pt: out.ascent_pt as f32,
            descent_pt: out.descent_pt as f32,
        })
    });

    if let Ok(mut guard) = cache.write() {
        if guard.len() > 256 {
            guard.clear();
        }
        guard.insert(key, rendered.clone());
    }
    rendered
}

/// Render a single PDF page through any [`PdfPageRenderer`] into a [`PdfPreviewPage`].
pub fn render_pdf_page(
    renderer: &dyn PdfPageRenderer,
    page_index: usize,
    scale: f32,
) -> Option<PdfPreviewPage> {
    let page_count = renderer.page_count();
    if page_count == 0 {
        return None;
    }
    let idx = page_index.min(page_count - 1);
    let buf = renderer
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

/// Convenience helper constructing a pure-Rust [`HayroPdfRenderer`] from raw PDF bytes.
pub fn parse_pdf_renderer(bytes: &[u8]) -> Option<HayroPdfRenderer> {
    HayroPdfRenderer::from_bytes(bytes.to_vec()).ok()
}

impl super::FlowPass<'_> {
    pub(super) fn latex_body(&mut self, latex: &str, width: f32) -> super::BlockBody {
        use super::{
            BlockBody, CARD_BLOCK_MARGIN, FlowKind, FlowRect, InlineAlign, InlineStyle, Prim,
        };
        use crate::theme::{FONT_CODE, FONT_SANS, TYPOGRAPHY};

        let pad_x = 16.0;
        let pad_y = 14.0;
        let inner = (width - 2.0 * pad_x).max(1.0);
        let empty = latex.trim().is_empty();
        if !empty
            && let Some(math) = render_math_cached(latex, true, 18.0, self.palette.text_primary)
        {
            let natural_w = math.width_pt.max(1.0);
            let natural_h = (math.ascent_pt + math.descent_pt).max(18.0);
            let scale = if natural_w > inner {
                inner / natural_w
            } else {
                1.0
            };
            let img_w = (natural_w * scale).round().max(1.0);
            let img_h = (natural_h * scale).round().max(1.0);
            let height = (img_h + 2.0 * pad_y).max(56.0);
            let img_x = ((width - img_w) / 2.0).floor().max(pad_x);
            let img_y = ((height - img_h) / 2.0).floor().max(pad_y);
            return BlockBody {
                kind: FlowKind::Latex,
                width,
                height,
                margin_top: CARD_BLOCK_MARGIN,
                margin_bottom: CARD_BLOCK_MARGIN,
                plain: latex.to_string(),
                prims: vec![
                    Prim::Quad {
                        rect: FlowRect::new(0.0, 0.0, width, height),
                        fill: Some(self.palette.background_secondary),
                        border: Some(self.palette.border),
                        radius: 8.0,
                        dashed: false,
                    },
                    Prim::Image {
                        rect: FlowRect::new(img_x, img_y, img_w, img_h),
                        image: math.image,
                    },
                ],
                text: None,
                prefix: None,
                code_scroll: None,
                pdf_controls: None,
                table_cells: Vec::new(),
            };
        }
        let (display, style, line_height) = if empty {
            (
                "Add an equation",
                InlineStyle::new(
                    FONT_SANS,
                    TYPOGRAPHY.font_sm_px,
                    400.0,
                    self.palette.text_secondary,
                ),
                22.0,
            )
        } else {
            (
                latex,
                InlineStyle::new(
                    FONT_CODE,
                    TYPOGRAPHY.font_base_px,
                    400.0,
                    self.palette.text_primary,
                ),
                24.0,
            )
        };
        let layout = self.plain_inline(display, style, inner, line_height, InlineAlign::Center);
        let height = (layout.height() + 2.0 * pad_y).max(52.0);
        let text_y = ((height - layout.height()) / 2.0).floor().max(pad_y);
        BlockBody {
            kind: FlowKind::Latex,
            width,
            height,
            margin_top: CARD_BLOCK_MARGIN,
            margin_bottom: CARD_BLOCK_MARGIN,
            plain: if empty {
                "Add an equation".to_string()
            } else {
                latex.to_string()
            },
            prims: vec![
                Prim::Quad {
                    rect: FlowRect::new(0.0, 0.0, width, height),
                    fill: Some(self.palette.background_secondary),
                    border: Some(self.palette.border),
                    radius: 8.0,
                    dashed: false,
                },
                Prim::Text {
                    x: pad_x,
                    y: text_y,
                    layout,
                    clip: Some(FlowRect::new(pad_x, pad_y, inner, height - 2.0 * pad_y)),
                },
            ],
            text: None,
            prefix: None,
            code_scroll: None,
            pdf_controls: None,
            table_cells: Vec::new(),
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

    pub(super) fn surface_ref_body(
        &mut self,
        reference: &str,
        ref_flavour: &str,
        caption: &str,
        width: f32,
    ) -> super::BlockBody {
        use super::{
            BlockBody, CARD_BLOCK_MARGIN, FlowKind, FlowRect, InlineAlign, InlineStyle, Prim,
            inline::truncate_to_width,
        };
        use crate::theme::{FONT_CODE, FONT_SANS, TYPOGRAPHY};
        let card_h = 84.0;
        let pad = 14.0;
        let inner_w = (width - pad * 2.0 - 32.0).max(40.0);
        let title = if caption.is_empty() {
            reference
        } else {
            caption
        };
        let title_style = InlineStyle::new(
            FONT_SANS,
            TYPOGRAPHY.font_sm_px,
            600.0,
            self.palette.text_primary,
        );
        let sub_style = InlineStyle::new(
            FONT_CODE,
            TYPOGRAPHY.font_xs_px,
            400.0,
            self.palette.text_secondary,
        );
        let title_str = truncate_to_width(self.window, title, &title_style, inner_w)
            .text
            .to_string();
        let sub = format!("Surface ref  {ref_flavour}  {reference}");
        let sub_str = truncate_to_width(self.window, &sub, &sub_style, inner_w)
            .text
            .to_string();
        BlockBody {
            kind: FlowKind::SurfaceRef,
            width,
            height: card_h,
            margin_top: CARD_BLOCK_MARGIN,
            margin_bottom: CARD_BLOCK_MARGIN,
            plain: title.to_string(),
            prims: vec![
                Prim::Quad {
                    rect: FlowRect::new(0.0, 0.0, width, card_h),
                    fill: Some(self.palette.background_secondary),
                    border: Some(self.palette.card_border),
                    radius: 8.0,
                    dashed: false,
                },
                Prim::Icon {
                    rect: FlowRect::new(pad, 16.0, 20.0, 20.0),
                    icon: crate::theme::IconPaint::named("EdgelessIcon"),
                    color: self.palette.icon,
                },
                Prim::Text {
                    x: pad + 28.0,
                    y: 14.0,
                    layout: self.plain_inline(
                        &title_str,
                        title_style,
                        inner_w,
                        22.0,
                        InlineAlign::Left,
                    ),
                    clip: None,
                },
                Prim::Text {
                    x: pad + 28.0,
                    y: 42.0,
                    layout: self.plain_inline(
                        &sub_str,
                        sub_style,
                        inner_w,
                        18.0,
                        InlineAlign::Left,
                    ),
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
