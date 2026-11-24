use std::sync::Arc;

use resvg::tiny_skia::{Pixmap, Transform};
use sha2::{Digest, Sha256};

use crate::error::PreviewError;
use crate::font_pool::FontPool;

/// Default maximum raster edge in physical pixels to bound memory usage.
pub const DEFAULT_MAX_EDGE_PX: u32 = 4096;

/// Compact RGBA8 pixel buffer produced by SVG or PDF rasterization.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbaImageBuffer {
    /// Physical pixel width.
    pub width_px: u32,
    /// Physical pixel height.
    pub height_px: u32,
    /// Logical width in CSS pixels or points.
    pub logical_width: f32,
    /// Logical height in CSS pixels or points.
    pub logical_height: f32,
    /// Tightly packed RGBA8 pixel bytes (`width_px * height_px * 4`).
    pub pixels: Vec<u8>,
}

impl RgbaImageBuffer {
    /// Returns the total byte size of the pixel buffer.
    pub fn byte_len(&self) -> usize {
        self.pixels.len()
    }

    /// Computes a deterministic SHA-256 digest over dimensions and pixel bytes.
    pub fn sha256(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.width_px.to_le_bytes());
        hasher.update(self.height_px.to_le_bytes());
        hasher.update(&self.pixels);
        hasher.finalize().into()
    }

    /// Formats the deterministic SHA-256 digest as lowercase hexadecimal.
    pub fn sha256_hex(&self) -> String {
        let digest = self.sha256();
        let mut out = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(&mut out, "{byte:02x}");
        }
        out
    }

    /// Counts pixels that differ from the given RGBA color by more than `tolerance`.
    pub fn count_ink_pixels(&self, background: [u8; 4], tolerance: u8) -> usize {
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| {
                px[0].abs_diff(background[0]) > tolerance
                    || px[1].abs_diff(background[1]) > tolerance
                    || px[2].abs_diff(background[2]) > tolerance
                    || px[3].abs_diff(background[3]) > tolerance
            })
            .count()
    }

    /// Counts non-transparent pixels (`alpha > 0`).
    pub fn count_non_transparent_pixels(&self) -> usize {
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] > 0)
            .count()
    }
}

/// Target sizing options for SVG rasterization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterOptions {
    /// Device scale factor applied to logical dimensions (for example `2.0`).
    pub scale_factor: f32,
    /// Optional logical width override. When set without height, aspect ratio is preserved.
    pub target_logical_width: Option<f32>,
    /// Optional logical height override. When set without width, aspect ratio is preserved.
    pub target_logical_height: Option<f32>,
    /// Maximum allowed physical width or height in pixels.
    pub max_edge_px: u32,
}

impl Default for RasterOptions {
    fn default() -> Self {
        Self {
            scale_factor: 2.0,
            target_logical_width: None,
            target_logical_height: None,
            max_edge_px: DEFAULT_MAX_EDGE_PX,
        }
    }
}

impl RasterOptions {
    /// Creates raster options with the given scale factor and default limits.
    pub fn with_scale(scale_factor: f32) -> Self {
        Self {
            scale_factor,
            ..Self::default()
        }
    }
}

/// Pure-Rust deterministic SVG rasterizer backed by `usvg` and `resvg`.
///
/// Uses only fonts explicitly registered in its [`FontPool`].
#[derive(Clone)]
pub struct SvgRasterizer {
    options: Arc<usvg::Options<'static>>,
}

impl SvgRasterizer {
    /// Builds a rasterizer bound to the given explicit [`FontPool`].
    pub fn new(font_pool: &FontPool) -> Self {
        let fontdb = font_pool.database();
        let default_selector = usvg::FontResolver::default_font_selector();
        let default_fallback = usvg::FontResolver::default_fallback_selector();

        let select_font = Box::new(
            move |font: &usvg::Font, db: &mut Arc<usvg::fontdb::Database>| {
                if let Some(id) = default_selector(font, db) {
                    return Some(id);
                }
                let sans_query = usvg::fontdb::Query {
                    families: &[usvg::fontdb::Family::SansSerif],
                    ..Default::default()
                };
                db.query(&sans_query)
                    .or_else(|| db.faces().next().map(|face| face.id))
            },
        );

        let select_fallback = Box::new(
            move |ch: char, fonts: &[usvg::fontdb::ID], db: &mut Arc<usvg::fontdb::Database>| {
                default_fallback(ch, fonts, db)
            },
        );

        let mut usvg_options = usvg::Options {
            fontdb,
            font_resolver: usvg::FontResolver {
                select_font,
                select_fallback,
            },
            ..Default::default()
        };
        if let Some(sans) = font_pool.sans_serif_family() {
            usvg_options.font_family = sans.to_string();
        }

        Self {
            options: Arc::new(usvg_options),
        }
    }

    /// Rasterizes an SVG string at the given scale factor.
    pub fn rasterize(&self, svg: &str, scale_factor: f32) -> Result<RgbaImageBuffer, PreviewError> {
        self.rasterize_with_options(svg, RasterOptions::with_scale(scale_factor))
    }

    /// Rasterizes an SVG string with explicit sizing and scale options.
    pub fn rasterize_with_options(
        &self,
        svg: &str,
        options: RasterOptions,
    ) -> Result<RgbaImageBuffer, PreviewError> {
        if svg.trim().is_empty() {
            return Err(PreviewError::EmptyInput);
        }
        if !options.scale_factor.is_finite() || options.scale_factor <= 0.0 {
            return Err(PreviewError::InvalidSize(format!(
                "scale_factor must be positive and finite, got {}",
                options.scale_factor
            )));
        }
        if options.max_edge_px == 0 {
            return Err(PreviewError::InvalidSize(
                "max_edge_px must be greater than zero".to_string(),
            ));
        }

        let tree = usvg::Tree::from_str(svg, &self.options)
            .map_err(|err| PreviewError::SvgParse(err.to_string()))?;

        rasterize_tree(&tree, options)
    }
}

fn rasterize_tree(
    tree: &usvg::Tree,
    options: RasterOptions,
) -> Result<RgbaImageBuffer, PreviewError> {
    let svg_size = tree.size();
    let src_w = svg_size.width();
    let src_h = svg_size.height();
    if !src_w.is_finite() || !src_h.is_finite() || src_w <= 0.0 || src_h <= 0.0 {
        return Err(PreviewError::InvalidSize(format!(
            "svg intrinsic size is invalid: {src_w}x{src_h}"
        )));
    }

    let (logical_width, logical_height) =
        match (options.target_logical_width, options.target_logical_height) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, src_h * (w / src_w)),
            (None, Some(h)) => (src_w * (h / src_h), h),
            (None, None) => (src_w, src_h),
        };

    if !logical_width.is_finite()
        || !logical_height.is_finite()
        || logical_width <= 0.0
        || logical_height <= 0.0
    {
        return Err(PreviewError::InvalidSize(format!(
            "logical size must be positive and finite: {logical_width}x{logical_height}"
        )));
    }

    let raw_w = (logical_width * options.scale_factor).max(1.0);
    let raw_h = (logical_height * options.scale_factor).max(1.0);
    let max_edge = options.max_edge_px as f32;
    let clamp = (max_edge / raw_w).min(max_edge / raw_h).min(1.0);
    let width_px = (raw_w * clamp).round().max(1.0) as u32;
    let height_px = (raw_h * clamp).round().max(1.0) as u32;

    let mut pixmap = Pixmap::new(width_px, height_px).ok_or_else(|| {
        PreviewError::InvalidSize(format!(
            "failed to allocate pixmap of size {width_px}x{height_px}"
        ))
    })?;

    let scale_x = width_px as f32 / src_w;
    let scale_y = height_px as f32 / src_h;
    let transform = Transform::from_scale(scale_x, scale_y);
    resvg::render(tree, transform, &mut pixmap.as_mut());

    Ok(RgbaImageBuffer {
        width_px,
        height_px,
        logical_width,
        logical_height,
        pixels: pixmap.take(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_vector_svg_deterministically_at_multiple_scales() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20">
            <rect x="2" y="2" width="36" height="16" fill="#1e66f5"/>
        </svg>"##;

        let rasterizer = SvgRasterizer::new(&FontPool::new());
        let cases = [(1.0_f32, 40_u32, 20_u32), (2.0_f32, 80_u32, 40_u32)];
        for (scale, expected_w, expected_h) in cases {
            let first = rasterizer.rasterize(svg, scale).unwrap();
            let second = rasterizer.rasterize(svg, scale).unwrap();
            assert_eq!(first.width_px, expected_w);
            assert_eq!(first.height_px, expected_h);
            assert_eq!(first.byte_len(), (expected_w * expected_h * 4) as usize);
            assert!(first.count_non_transparent_pixels() > 0);
            assert_eq!(first.sha256(), second.sha256());
        }
    }

    #[test]
    fn clamps_oversized_raster_target_to_max_edge() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="500" viewBox="0 0 1000 500">
            <rect width="1000" height="500" fill="#000000"/>
        </svg>"##;

        let rasterizer = SvgRasterizer::new(&FontPool::new());
        let opts = RasterOptions {
            scale_factor: 2.0,
            target_logical_width: None,
            target_logical_height: None,
            max_edge_px: 256,
        };
        let buf = rasterizer.rasterize_with_options(svg, opts).unwrap();
        assert_eq!(buf.width_px, 256);
        assert_eq!(buf.height_px, 128);
    }

    #[test]
    fn rejects_empty_svg_and_non_positive_scale() {
        let rasterizer = SvgRasterizer::new(&FontPool::new());
        assert!(matches!(
            rasterizer.rasterize("   ", 2.0),
            Err(PreviewError::EmptyInput)
        ));
        let valid = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>"#;
        assert!(matches!(
            rasterizer.rasterize(valid, 0.0),
            Err(PreviewError::InvalidSize(_))
        ));
        assert!(matches!(
            rasterizer.rasterize(valid, f32::NAN),
            Err(PreviewError::InvalidSize(_))
        ));
    }
}
