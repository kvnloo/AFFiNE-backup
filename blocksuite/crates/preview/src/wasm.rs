use wasm_bindgen::prelude::*;

#[cfg(any(feature = "mermaid", feature = "typst"))]
use crate::PreviewDate;
#[cfg(feature = "mermaid")]
use crate::mermaid::{MermaidOptions, MermaidRenderer};
#[cfg(feature = "pdf")]
use crate::pdf::{HayroPdfRenderer, PdfPageRenderer, PdfRenderRequest};

/// Math SVG output and baseline metrics returned across the Wasm Worker boundary.
#[cfg(feature = "math")]
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug, Clone)]
pub struct WasmMathOutput {
    /// Total box width in points.
    pub width_pt: f64,
    /// Distance from the top edge to the baseline in points.
    pub ascent_pt: f64,
    /// Distance from the baseline to the bottom edge in points.
    pub descent_pt: f64,
    /// Standalone SVG string.
    pub svg: String,
}

/// Typst SVG output and layout metrics returned across the Wasm Worker boundary.
#[cfg(feature = "typst")]
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug, Clone)]
pub struct WasmTypstOutput {
    /// Page width in Typst points.
    pub width_pt: f64,
    /// Page height in Typst points.
    pub height_pt: f64,
    /// Distance from the top edge to the baseline in points.
    pub ascent_pt: f64,
    /// Distance from the baseline to the bottom edge in points.
    pub descent_pt: f64,
    /// Compiled SVG document string.
    pub svg: String,
}

/// PDF page RGBA buffer and dimensions returned across the Wasm Worker boundary.
#[cfg(feature = "pdf")]
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug, Clone)]
pub struct WasmPdfPageOutput {
    /// Physical pixel width.
    pub width_px: u32,
    /// Physical pixel height.
    pub height_px: u32,
    /// Logical page width in PDF points.
    pub logical_width: f32,
    /// Logical page height in PDF points.
    pub logical_height: f32,
    /// Total number of pages in the PDF document.
    pub page_count: usize,
    /// Tightly packed RGBA8 pixel bytes (`width_px * height_px * 4`).
    pub pixels: Vec<u8>,
}

/// Renders a Mermaid diagram to `resvg`-safe SVG inside a Web Worker.
#[cfg(feature = "mermaid")]
#[wasm_bindgen]
pub fn render_mermaid_svg_wasm(
    source: &str,
    year: i32,
    month: u8,
    day: u8,
    timezone_offset_minutes: i16,
) -> Result<String, JsValue> {
    let renderer = MermaidRenderer::new();
    let options = MermaidOptions {
        today: PreviewDate::new(year, month, day),
        timezone_offset_minutes,
        ..MermaidOptions::default()
    };
    renderer
        .render_svg(source, &options)
        .map_err(|err| JsValue::from_str(&err.to_string()))
}

/// Renders a LaTeX formula to standalone SVG with exact baseline metrics inside a Web Worker.
#[cfg(feature = "math")]
#[wasm_bindgen]
pub fn render_latex_svg_wasm(
    latex: &str,
    display_mode: bool,
    font_size_pt: f64,
) -> Result<WasmMathOutput, JsValue> {
    let mode = if display_mode {
        crate::math::MathDisplayMode::Display
    } else {
        crate::math::MathDisplayMode::Inline
    };
    let out = crate::math::render_latex_svg(
        latex,
        crate::math::MathRenderOptions {
            mode,
            font_size_pt,
            padding_pt: 0.0,
            embed_glyphs: true,
        },
    )
    .map_err(|err| JsValue::from_str(&err.to_string()))?;
    Ok(WasmMathOutput {
        width_pt: out.width_pt,
        ascent_pt: out.ascent_pt,
        descent_pt: out.descent_pt,
        svg: out.svg,
    })
}

/// Compiles native Typst markup into SVG with layout metrics inside a Web Worker.
///
/// Pass optional font bytes (`font_bytes`) to register an in-memory font before compilation.
#[cfg(feature = "typst")]
#[wasm_bindgen]
pub fn compile_typst_svg_wasm(
    source: &str,
    year: i32,
    month: u8,
    day: u8,
    font_bytes: Option<Vec<u8>>,
) -> Result<WasmTypstOutput, JsValue> {
    let mut pool = crate::font_pool::FontPool::new();
    if let Some(bytes) = font_bytes
        && !bytes.is_empty()
    {
        pool.register_font_bytes(bytes)
            .map_err(|err| JsValue::from_str(&err.to_string()))?;
    }
    let mut world = crate::typst_world::InMemoryTypstWorld::new(&pool);
    world.set_today(Some(PreviewDate::new(year, month, day)));
    let out = world
        .compile_svg(source)
        .map_err(|err| JsValue::from_str(&err.to_string()))?;
    Ok(WasmTypstOutput {
        width_pt: out.width_pt,
        height_pt: out.height_pt,
        ascent_pt: out.ascent_pt,
        descent_pt: out.descent_pt,
        svg: out.svg,
    })
}

/// Renders a single PDF page to RGBA8 pixels with dimensions and page count inside a Web Worker.
#[cfg(feature = "pdf")]
#[wasm_bindgen]
pub fn render_pdf_page_rgba_wasm(
    pdf_bytes: &[u8],
    page_index: usize,
    scale: f32,
    max_edge_px: u32,
) -> Result<WasmPdfPageOutput, JsValue> {
    let renderer = HayroPdfRenderer::from_bytes(pdf_bytes.to_vec())
        .map_err(|err| JsValue::from_str(&err.to_string()))?;
    let page_count = renderer.page_count();
    let request = PdfRenderRequest {
        page_index,
        scale,
        max_edge_px,
        ..PdfRenderRequest::default()
    };
    let buf = renderer
        .render_page(request)
        .map_err(|err| JsValue::from_str(&err.to_string()))?;
    Ok(WasmPdfPageOutput {
        width_px: buf.width_px,
        height_px: buf.height_px,
        logical_width: buf.logical_width,
        logical_height: buf.logical_height,
        page_count,
        pixels: buf.pixels,
    })
}
