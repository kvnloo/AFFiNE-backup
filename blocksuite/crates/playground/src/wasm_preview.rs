//! WebAssembly (`wasm32`) worker-backed preview providers (`WasmMathPreviewProvider` +
//! `WasmPdfPageProvider`) bridging Dedicated Web Workers (`math-worker.js`, `pdf-worker.js`)
//! to `blocksuite-editor`'s preview cache and automatic GPUI window repaint.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    sync::{Arc, OnceLock, RwLock},
};

use ui::{Hsla, WeakEntity};
use wasm_bindgen::prelude::*;

use crate::{
    PlaygroundView,
    block_flow::{
        MathPreviewProvider, MathRenderResult, MathRenderSlot, PdfPageProvider, PdfPreviewPage,
        bump_preview_revision, hsla_to_css_hex, register_math_preview_provider,
        register_pdf_page_provider, rgba_pixels_to_render_image,
    },
};

const MAX_MATH_CACHE_ENTRIES: usize = 256;
const MAX_PDF_CACHE_ENTRIES: usize = 64;

type MathKey = (String, bool, u32, String, u32);
type PdfPageKey = (u64, usize, u32);

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__playgroundPreview"], js_name = requestMath, catch)]
    fn js_request_math(
        latex: &str,
        display: bool,
        font_size_pt: f32,
        color_css: &str,
        dpr: f32,
    ) -> Result<(), JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__playgroundPreview"], js_name = requestPdfPageCount, catch)]
    fn js_request_pdf_page_count(pdf_hash: String, bytes: &[u8]) -> Result<(), JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__playgroundPreview"], js_name = requestPdfPage, catch)]
    fn js_request_pdf_page(
        pdf_hash: String,
        bytes: &[u8],
        page_index: usize,
        scale: f32,
    ) -> Result<(), JsValue>;
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut h);
    bytes.hash(&mut h);
    h.finish()
}

fn math_cache() -> &'static RwLock<HashMap<MathKey, MathRenderResult>> {
    static CACHE: OnceLock<RwLock<HashMap<MathKey, MathRenderResult>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn math_in_flight() -> &'static RwLock<HashSet<MathKey>> {
    static SET: OnceLock<RwLock<HashSet<MathKey>>> = OnceLock::new();
    SET.get_or_init(|| RwLock::new(HashSet::new()))
}

fn pdf_counts() -> &'static RwLock<HashMap<u64, usize>> {
    static CACHE: OnceLock<RwLock<HashMap<u64, usize>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn pdf_pages() -> &'static RwLock<HashMap<PdfPageKey, PdfPreviewPage>> {
    static CACHE: OnceLock<RwLock<HashMap<PdfPageKey, PdfPreviewPage>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn pdf_in_flight() -> &'static RwLock<HashSet<PdfPageKey>> {
    static SET: OnceLock<RwLock<HashSet<PdfPageKey>>> = OnceLock::new();
    SET.get_or_init(|| RwLock::new(HashSet::new()))
}

std::thread_local! {
    static ACTIVE_VIEW: RefCell<Option<(WeakEntity<PlaygroundView>, ui::AsyncApp)>> =
        const { RefCell::new(None) };
}

fn notify_active_view(refresh_pdf: bool) {
    ACTIVE_VIEW.with(|slot| {
        if let Some((weak, app)) = slot.borrow().as_ref() {
            let weak = weak.clone();
            let app = app.clone();
            let _ = app.update(|cx| {
                let _ = weak.update(cx, |view, cx| {
                    if refresh_pdf {
                        let attachments = view.active_pdf_attachments();
                        view.refresh_pdf_previews(&attachments);
                    }
                    cx.notify();
                });
            });
        }
    });
}

struct WasmMathPreviewProvider;

impl MathPreviewProvider for WasmMathPreviewProvider {
    fn render_math(
        &self,
        latex: &str,
        display: bool,
        font_size_pt: f32,
        color: Hsla,
        dpr: f32,
    ) -> MathRenderResult {
        let color_css = hsla_to_css_hex(color);
        let key: MathKey = (
            latex.to_string(),
            display,
            font_size_pt.to_bits(),
            color_css.clone(),
            dpr.to_bits(),
        );
        if let Ok(guard) = math_cache().read()
            && let Some(hit) = guard.get(&key)
        {
            return hit.clone();
        }
        let should_request = math_in_flight()
            .write()
            .map(|mut set| set.insert(key.clone()))
            .unwrap_or(false);
        if should_request {
            let _ = js_request_math(latex, display, font_size_pt, &color_css, dpr);
        }
        MathRenderResult::Pending
    }
}

struct WasmPdfPageProvider;

impl PdfPageProvider for WasmPdfPageProvider {
    fn page_count(&self, bytes: &[u8]) -> Option<usize> {
        let hash = hash_bytes(bytes);
        if let Ok(guard) = pdf_counts().read()
            && let Some(&count) = guard.get(&hash)
        {
            return (count > 0).then_some(count);
        }
        let _ = js_request_pdf_page_count(format!("{hash:016x}"), bytes);
        None
    }

    fn render_pdf_page(
        &self,
        bytes: &[u8],
        page_index: usize,
        scale: f32,
    ) -> Option<PdfPreviewPage> {
        let hash = hash_bytes(bytes);
        let key: PdfPageKey = (hash, page_index, scale.to_bits());
        if let Ok(guard) = pdf_pages().read()
            && let Some(hit) = guard.get(&key)
        {
            return Some(hit.clone());
        }
        let should_request = pdf_in_flight()
            .write()
            .map(|mut set| set.insert(key))
            .unwrap_or(false);
        if should_request {
            let _ = js_request_pdf_page(format!("{hash:016x}"), bytes, page_index, scale);
        }
        None
    }
}

/// Registers the Web Worker preview providers and binds `view` for automatic repaint wakeups.
pub(crate) fn install_wasm_preview_providers(view: WeakEntity<PlaygroundView>, cx: &ui::App) {
    ACTIVE_VIEW.with(|slot| {
        *slot.borrow_mut() = Some((view, cx.to_async()));
    });
    register_math_preview_provider(Some(Arc::new(WasmMathPreviewProvider)));
    register_pdf_page_provider(Some(Arc::new(WasmPdfPageProvider)));
}

/// Callback invoked by `preview-workers.js` when a RaTeX math formula finishes rasterizing.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn deliver_math_preview(
    latex: String,
    display: bool,
    font_size_pt: f32,
    color_css: String,
    dpr: f32,
    width_pt: f32,
    ascent_pt: f32,
    descent_pt: f32,
    width_px: u32,
    height_px: u32,
    rgba: &[u8],
) {
    let key: MathKey = (
        latex,
        display,
        font_size_pt.to_bits(),
        color_css,
        dpr.to_bits(),
    );
    if let Ok(mut set) = math_in_flight().write() {
        set.remove(&key);
    }
    let result = match rgba_pixels_to_render_image(width_px, height_px, rgba) {
        Some(image) => MathRenderResult::Ready(MathRenderSlot {
            image,
            width_pt,
            ascent_pt,
            descent_pt,
        }),
        None => MathRenderResult::Error,
    };
    if let Ok(mut guard) = math_cache().write() {
        if guard.len() >= MAX_MATH_CACHE_ENTRIES {
            guard.clear();
        }
        guard.insert(key, result);
    }
    bump_preview_revision();
    notify_active_view(false);
}

/// Callback invoked by `preview-workers.js` when RaTeX reports a definitive syntax/parse error.
#[wasm_bindgen]
pub fn deliver_math_error(
    latex: String,
    display: bool,
    font_size_pt: f32,
    color_css: String,
    dpr: f32,
) {
    let key: MathKey = (
        latex,
        display,
        font_size_pt.to_bits(),
        color_css,
        dpr.to_bits(),
    );
    if let Ok(mut set) = math_in_flight().write() {
        set.remove(&key);
    }
    if let Ok(mut guard) = math_cache().write() {
        if guard.len() >= MAX_MATH_CACHE_ENTRIES {
            guard.clear();
        }
        guard.insert(key, MathRenderResult::Error);
    }
    bump_preview_revision();
    notify_active_view(false);
}

/// Callback invoked by `preview-workers.js` when Hayro reports a PDF's page count.
#[wasm_bindgen]
pub fn deliver_pdf_page_count(pdf_hash_hex: String, page_count: usize) {
    let Ok(hash) = u64::from_str_radix(&pdf_hash_hex, 16) else {
        return;
    };
    if let Ok(mut guard) = pdf_counts().write() {
        guard.insert(hash, page_count);
    }
    bump_preview_revision();
    notify_active_view(true);
}

/// Callback invoked by `preview-workers.js` when Hayro finishes rasterizing a PDF page.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn deliver_pdf_page(
    pdf_hash_hex: String,
    page_index: usize,
    page_count: usize,
    scale: f32,
    width_px: u32,
    height_px: u32,
    rgba: &[u8],
) {
    let Ok(hash) = u64::from_str_radix(&pdf_hash_hex, 16) else {
        return;
    };
    let key: PdfPageKey = (hash, page_index, scale.to_bits());
    if let Ok(mut set) = pdf_in_flight().write() {
        set.remove(&key);
    }
    if let Some(image) = rgba_pixels_to_render_image(width_px, height_px, rgba) {
        if let Ok(mut counts) = pdf_counts().write() {
            counts.insert(hash, page_count);
        }
        if let Ok(mut pages) = pdf_pages().write() {
            if pages.len() >= MAX_PDF_CACHE_ENTRIES {
                pages.clear();
            }
            pages.insert(
                key,
                PdfPreviewPage {
                    page_index,
                    page_count,
                    image,
                    width_pt: (width_px as f32) / scale.max(0.25),
                    height_pt: (height_px as f32) / scale.max(0.25),
                },
            );
        }
        bump_preview_revision();
        notify_active_view(true);
    }
}
