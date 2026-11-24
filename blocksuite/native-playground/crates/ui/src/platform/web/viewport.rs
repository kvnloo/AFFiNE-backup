//! Browser viewport and safe-area frame snapshots in canvas coordinates.
//!
//! Browser geometry is sampled before rendering, never by geometry getters.
//! Safe-area padding is resolved by CSS on a single zero-size `aria-hidden`
//! non-interactive probe so display cutouts use the browser's own environment
//! values without multiplying persistent DOM nodes.

use crate::{Bounds, Edges, Pixels, WindowInsets, point, px, size};
#[cfg(target_family = "wasm")]
use wasm_bindgen::JsCast;

/// Raw CSS-pixel viewport and safe-area measurements used to compute
/// [`Bounds<Pixels>`] and [`WindowInsets`] deterministically.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RawViewportMetrics {
    pub(crate) canvas_left: f64,
    pub(crate) canvas_top: f64,
    pub(crate) canvas_width: f64,
    pub(crate) canvas_height: f64,
    pub(crate) layout_width: f64,
    pub(crate) layout_height: f64,
    pub(crate) visual_offset_left: f64,
    pub(crate) visual_offset_top: f64,
    pub(crate) visual_width: f64,
    pub(crate) visual_height: f64,
    pub(crate) visual_scale: f64,
    pub(crate) safe_area_css: [f64; 4], // [top, right, bottom, left]
}

/// Pure viewport calculation shared by runtime and unit tests.
///
/// Prevents double-subtracting the software keyboard when `visualViewport.height`
/// has already shrunk: `visible_bounds` reflects the shortened `visualViewport`,
/// while `WindowInsets.ime.bottom` reports only the portion of the canvas
/// obscured below `visible_bounds` (0 when the browser already resized the layout
/// viewport / canvas, or during pinch zoom).
pub(crate) fn compute_viewport_snapshot(m: RawViewportMetrics) -> (Bounds<Pixels>, WindowInsets) {
    let canvas_right = m.canvas_left + m.canvas_width;
    let canvas_bottom = m.canvas_top + m.canvas_height;

    let x = (m.visual_offset_left - m.canvas_left).clamp(0.0, m.canvas_width);
    let y = (m.visual_offset_top - m.canvas_top).clamp(0.0, m.canvas_height);
    let right = (m.visual_offset_left + m.visual_width - m.canvas_left).clamp(x, m.canvas_width);
    let bottom = (m.visual_offset_top + m.visual_height - m.canvas_top).clamp(y, m.canvas_height);
    let visible_bounds = Bounds::new(
        point(px(x as f32), px(y as f32)),
        size(px((right - x) as f32), px((bottom - y) as f32)),
    );

    let [sa_top, sa_right, sa_bottom, sa_left] = m.safe_area_css;
    let safe_area = Edges {
        top: px((sa_top - m.canvas_top).max(0.0) as f32),
        right: px((canvas_right - m.layout_width + sa_right).max(0.0) as f32),
        bottom: px((canvas_bottom - m.layout_height + sa_bottom).max(0.0) as f32),
        left: px((sa_left - m.canvas_left).max(0.0) as f32),
    };

    // Only report IME bottom inset for the region of the canvas actually obscured
    // below `visualViewport` at unzoomed scale (~1.0). When the browser already
    // shrunk the canvas/layout viewport to fit above the keyboard, `obscured_bottom`
    // is 0 so callers never subtract the keyboard height twice.
    let unzoomed = (m.visual_scale - 1.0).abs() < 0.01;
    let visual_bottom_in_canvas = bottom;
    let obscured_bottom = if unzoomed && m.visual_height < m.layout_height - 1.0 {
        (m.canvas_height - visual_bottom_in_canvas).max(0.0)
    } else {
        0.0
    };
    // Subtract only the excess beyond visible_bounds when consumers lay out against
    // the full canvas; when `visible_bounds.size.height` is already shortened to
    // `visual_height`, `ime.bottom` equals the exact obscured band without overlap.
    let ime = Edges {
        top: px(0.0),
        right: px(0.0),
        bottom: px(obscured_bottom as f32),
        left: px(0.0),
    };

    let insets = WindowInsets {
        safe_area,
        ime,
        ..WindowInsets::default()
    };
    (visible_bounds, insets)
}

/// Collision-free CSS dimension encoding for a single zero-content-size probe.
///
/// By encoding `(left, right)` and `(top, bottom)` with distinct radix weights
/// into the probe's invisible `width`/`height` (or comparing all 4 edges read via
/// `getComputedStyle`), redistributing insets across opposite sides — such as
/// flipping an iPhone landscape notch from `(44, 0, 0, 0)` to `(0, 44, 0, 0)`
/// where the total sum is unchanged — still changes the probe's observed box size
/// and triggers `ResizeObserver` immediately.
pub(crate) fn encode_collision_free_probe_size(
    top: f64,
    right: f64,
    bottom: f64,
    left: f64,
) -> (f64, f64) {
    // Radix 1024 exceeds any physical display cutout (< 200 CSS px), so every
    // distinct `(left, right)` and `(top, bottom)` pair maps to a unique size.
    (left + right * 1024.0, top + bottom * 1024.0)
}

#[cfg(target_family = "wasm")]
pub(crate) struct WebViewport {
    safe_area_probe: web_sys::HtmlElement,
    pub(crate) visible_bounds: Bounds<Pixels>,
    pub(crate) insets: WindowInsets,
}

#[cfg(target_family = "wasm")]
impl WebViewport {
    pub(crate) fn new(document: &web_sys::Document) -> anyhow::Result<Self> {
        let safe_area_probe = create_single_safe_area_probe(document)?;
        let body = document
            .body()
            .ok_or_else(|| anyhow::anyhow!("Missing document body"))?;
        body.append_child(&safe_area_probe)
            .map_err(|error| anyhow::anyhow!("Failed to attach safe-area probe: {error:?}"))?;
        Ok(Self {
            safe_area_probe,
            visible_bounds: Bounds::default(),
            insets: WindowInsets::default(),
        })
    }

    pub(crate) fn observe_safe_area(&self, observer: &web_sys::ResizeObserver) {
        // Single zero-visual-footprint probe uses collision-free weighted
        // width/height expressions so edge redistributions (e.g. landscape
        // left-to-right notch flip with identical total sum) still fire
        // ResizeObserver alongside orientation/visualViewport listeners.
        observer.observe(&self.safe_area_probe);
    }

    pub(crate) fn update(
        &mut self,
        window: &web_sys::Window,
        canvas: &web_sys::HtmlCanvasElement,
    ) -> anyhow::Result<bool> {
        let canvas_bounds = canvas.get_bounding_client_rect();
        let document = window
            .document()
            .ok_or_else(|| anyhow::anyhow!("Missing document"))?;
        let root = document
            .document_element()
            .ok_or_else(|| anyhow::anyhow!("Missing root"))?;
        let layout_width = root.client_width() as f64;
        let layout_height = root.client_height() as f64;
        let (visual_offset_left, visual_offset_top, visual_width, visual_height, visual_scale) =
            match window.visual_viewport() {
                Some(viewport) => (
                    viewport.offset_left(),
                    viewport.offset_top(),
                    viewport.width(),
                    viewport.height(),
                    viewport.scale(),
                ),
                None => (0.0, 0.0, layout_width, layout_height, 1.0),
            };

        let safe_area_css = read_safe_area_padding(window, &self.safe_area_probe);
        let (visible_bounds, insets) = compute_viewport_snapshot(RawViewportMetrics {
            canvas_left: canvas_bounds.left(),
            canvas_top: canvas_bounds.top(),
            canvas_width: canvas_bounds.width(),
            canvas_height: canvas_bounds.height(),
            layout_width,
            layout_height,
            visual_offset_left,
            visual_offset_top,
            visual_width,
            visual_height,
            visual_scale,
            safe_area_css,
        });

        let changed = self.visible_bounds != visible_bounds || self.insets != insets;
        self.visible_bounds = visible_bounds;
        self.insets = insets;
        Ok(changed)
    }
}

#[cfg(target_family = "wasm")]
impl Drop for WebViewport {
    fn drop(&mut self) {
        self.safe_area_probe.remove();
    }
}

#[cfg(target_family = "wasm")]
fn create_single_safe_area_probe(
    document: &web_sys::Document,
) -> anyhow::Result<web_sys::HtmlElement> {
    let probe = document
        .create_element("div")
        .map_err(|error| anyhow::anyhow!("Failed to create safe-area probe: {error:?}"))?
        .dyn_into::<web_sys::HtmlElement>()
        .map_err(|error| anyhow::anyhow!("Invalid safe-area probe element: {error:?}"))?;
    probe
        .set_attribute("aria-hidden", "true")
        .map_err(|error| {
            anyhow::anyhow!("Failed to mark safe-area probe aria-hidden: {error:?}")
        })?;
    // Single non-interactive hidden probe:
    // - `padding` carries the 4 exact `env(safe-area-inset-*)` values read via `getComputedStyle`.
    // - `width`/`height` encode weighted combinations so edge redistribution with identical sum
    //   (e.g. landscape notch moving left <-> right) changes the ResizeObserver box immediately.
    probe
        .set_attribute(
            "style",
            "position:fixed;left:0;top:0;border:0;margin:0;visibility:hidden;pointer-events:none;\
             box-sizing:content-box;\
             padding:env(safe-area-inset-top,0px) env(safe-area-inset-right,0px) env(safe-area-inset-bottom,0px) env(safe-area-inset-left,0px);\
             width:calc(env(safe-area-inset-left,0px) + env(safe-area-inset-right,0px) * 1024);\
             height:calc(env(safe-area-inset-top,0px) + env(safe-area-inset-bottom,0px) * 1024)",
        )
        .map_err(|error| anyhow::anyhow!("Failed to style safe-area probe: {error:?}"))?;
    Ok(probe)
}

#[cfg(target_family = "wasm")]
fn read_safe_area_padding(window: &web_sys::Window, probe: &web_sys::HtmlElement) -> [f64; 4] {
    let Ok(Some(computed)) = window.get_computed_style(probe) else {
        return [0.0; 4];
    };
    [
        "padding-top",
        "padding-right",
        "padding-bottom",
        "padding-left",
    ]
    .map(|prop| {
        computed
            .get_property_value(prop)
            .ok()
            .and_then(|val| val.trim_end_matches("px").parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(0.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_single_probe_detects_landscape_flip_and_avoids_ime_double_subtraction() {
        // Landscape notch flip: (top=0, right=0, bottom=0, left=44) -> (top=0, right=44, bottom=0, left=0)
        // Total horizontal inset sum is identical (44), yet both encoded probe size and resolved
        // WindowInsets must detect the change without collision.
        let left_notch = RawViewportMetrics {
            canvas_left: 0.0,
            canvas_top: 0.0,
            canvas_width: 844.0,
            canvas_height: 390.0,
            layout_width: 844.0,
            layout_height: 390.0,
            visual_offset_left: 0.0,
            visual_offset_top: 0.0,
            visual_width: 844.0,
            visual_height: 390.0,
            visual_scale: 1.0,
            safe_area_css: [0.0, 0.0, 0.0, 44.0],
        };
        let right_notch = RawViewportMetrics {
            safe_area_css: [0.0, 44.0, 0.0, 0.0],
            ..left_notch
        };
        let (_, insets_left) = compute_viewport_snapshot(left_notch);
        let (_, insets_right) = compute_viewport_snapshot(right_notch);
        assert_ne!(insets_left, insets_right);
        assert_eq!(insets_left.safe_area.left, px(44.0));
        assert_eq!(insets_left.safe_area.right, px(0.0));
        assert_eq!(insets_right.safe_area.left, px(0.0));
        assert_eq!(insets_right.safe_area.right, px(44.0));

        let size_left = encode_collision_free_probe_size(0.0, 0.0, 0.0, 44.0);
        let size_right = encode_collision_free_probe_size(0.0, 44.0, 0.0, 0.0);
        assert_ne!(size_left, size_right);

        // Also check top/bottom redistribution with equal sum: (44, 0, 0, 0) -> (0, 0, 44, 0).
        let size_top = encode_collision_free_probe_size(44.0, 0.0, 0.0, 0.0);
        let size_bottom = encode_collision_free_probe_size(0.0, 0.0, 44.0, 0.0);
        assert_ne!(size_top, size_bottom);

        // IME inset check:
        // Case 1: browser resizes layout & canvas to 500px (Android resizes-content).
        // visible_bounds is already 500px; ime.bottom must be 0 so height is never subtracted twice.
        let resized_content = RawViewportMetrics {
            canvas_width: 390.0,
            canvas_height: 500.0,
            layout_width: 390.0,
            layout_height: 500.0,
            visual_width: 390.0,
            visual_height: 500.0,
            safe_area_css: [0.0; 4],
            ..left_notch
        };
        let (vis1, ins1) = compute_viewport_snapshot(resized_content);
        assert_eq!(vis1.size.height, px(500.0));
        assert_eq!(ins1.ime.bottom, px(0.0));

        // Case 2: browser keeps layout/canvas at 844px and shrinks visualViewport to 500px (iOS Safari).
        // visible_bounds is 500px and obscured canvas region below it is 344px.
        let resized_visual = RawViewportMetrics {
            canvas_width: 390.0,
            canvas_height: 844.0,
            layout_width: 390.0,
            layout_height: 844.0,
            visual_width: 390.0,
            visual_height: 500.0,
            safe_area_css: [0.0; 4],
            ..left_notch
        };
        let (vis2, ins2) = compute_viewport_snapshot(resized_visual);
        assert_eq!(vis2.size.height, px(500.0));
        assert_eq!(ins2.ime.bottom, px(344.0));
        assert_eq!(vis2.size.height + ins2.ime.bottom, px(844.0));
    }
}
