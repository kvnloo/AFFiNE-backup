use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_svg::{SvgColorSyntax, SvgOptions, render_to_svg_with_color_syntax};
use ratex_types::math_style::MathStyle;

use crate::error::PreviewError;
use crate::svg_raster::{RasterOptions, RgbaImageBuffer, SvgRasterizer};

/// Display mode for LaTeX mathematical formulas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathDisplayMode {
    /// Inline math aligned to the surrounding text baseline (`$...$`).
    Inline,
    /// Centered display math block (`$$...$$`).
    Display,
}

/// Options controlling LaTeX math typesetting and output metrics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MathRenderOptions {
    /// Inline or display style.
    pub mode: MathDisplayMode,
    /// Base font size in points (for example `16.0`).
    pub font_size_pt: f64,
    /// Optional padding around the SVG box in points.
    pub padding_pt: f64,
    /// Embed KaTeX math glyphs as standalone `<path>` outlines in SVG output.
    pub embed_glyphs: bool,
}

impl Default for MathRenderOptions {
    fn default() -> Self {
        Self {
            mode: MathDisplayMode::Inline,
            font_size_pt: 16.0,
            padding_pt: 0.0,
            embed_glyphs: true,
        }
    }
}

/// Typeset math box metrics and SVG output aligned for `LineLayout` baseline placement.
#[derive(Debug, Clone, PartialEq)]
pub struct MathSvgOutput {
    /// Total box width in points.
    pub width_pt: f64,
    /// Height above the baseline in points.
    pub ascent_pt: f64,
    /// Depth below the baseline in points.
    pub descent_pt: f64,
    /// Standalone SVG string.
    pub svg: String,
}

/// Rasterized math formula with exact baseline metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct MathRgbaOutput {
    /// Total box width in points.
    pub width_pt: f64,
    /// Height above the baseline in points.
    pub ascent_pt: f64,
    /// Depth below the baseline in points.
    pub descent_pt: f64,
    /// Rasterized RGBA8 pixel buffer.
    pub image: RgbaImageBuffer,
}

/// Renders a LaTeX math expression using the pure-Rust RaTeX KaTeX-compatible engine.
pub fn render_latex_svg(
    latex: &str,
    options: MathRenderOptions,
) -> Result<MathSvgOutput, PreviewError> {
    if latex.trim().is_empty() {
        return Err(PreviewError::EmptyInput);
    }
    if !options.font_size_pt.is_finite() || options.font_size_pt <= 0.0 {
        return Err(PreviewError::InvalidSize(format!(
            "font_size_pt must be positive and finite, got {}",
            options.font_size_pt
        )));
    }

    let nodes = ratex_parser::parse(latex).map_err(|err| PreviewError::Math(err.to_string()))?;
    let style = match options.mode {
        MathDisplayMode::Inline => MathStyle::Text,
        MathDisplayMode::Display => MathStyle::Display,
    };
    let layout_opts = LayoutOptions::default().with_style(style);
    let layout_box = layout(&nodes, &layout_opts);
    let display_list = to_display_list(&layout_box);

    let svg_opts = SvgOptions {
        font_size: options.font_size_pt,
        padding: options.padding_pt,
        stroke_width: (options.font_size_pt * 0.04).max(0.5),
        embed_glyphs: options.embed_glyphs,
        font_dir: String::new(),
    };
    let svg = render_to_svg_with_color_syntax(&display_list, &svg_opts, SvgColorSyntax::Rgb);

    let width_pt = display_list.width * options.font_size_pt + 2.0 * options.padding_pt;
    let ascent_pt = display_list.height * options.font_size_pt + options.padding_pt;
    let descent_pt = display_list.depth * options.font_size_pt + options.padding_pt;

    Ok(MathSvgOutput {
        width_pt,
        ascent_pt,
        descent_pt,
        svg,
    })
}

/// Renders a LaTeX math formula to RGBA8 pixels with exact baseline metrics.
pub fn render_latex_rgba(
    latex: &str,
    options: MathRenderOptions,
    rasterizer: &SvgRasterizer,
    scale_factor: f32,
) -> Result<MathRgbaOutput, PreviewError> {
    let svg_out = render_latex_svg(latex, options)?;
    let image =
        rasterizer.rasterize_with_options(&svg_out.svg, RasterOptions::with_scale(scale_factor))?;
    Ok(MathRgbaOutput {
        width_pt: svg_out.width_pt,
        ascent_pt: svg_out.ascent_pt,
        descent_pt: svg_out.descent_pt,
        image,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_pool::FontPool;
    use crate::test_support::build_test_font_pool;

    /// 48 representative 0.27.5 KaTeX formulas spanning AFFiNE tests and real documents.
    pub(crate) const KATEX_CORPUS: &[(&str, &str, MathDisplayMode)] = &[
        ("einstein", r"E = mc^2", MathDisplayMode::Inline),
        ("pythagoras", r"a^2 + b^2 = c^2", MathDisplayMode::Inline),
        (
            "sub_sup_multi",
            r"x_{i,j}^{n+1} + y_k^{(2)}",
            MathDisplayMode::Inline,
        ),
        (
            "complex_exp",
            r"e^{-i\omega t} = \cos(\omega t) - i\sin(\omega t)",
            MathDisplayMode::Inline,
        ),
        (
            "binomial",
            r"\binom{n}{k} = \frac{n!}{k!(n-k)!}",
            MathDisplayMode::Display,
        ),
        (
            "tensor",
            r"T^{\mu\nu}_{\alpha\beta} = g^{\mu\rho} g^{\nu\sigma} T_{\rho\sigma\alpha\beta}",
            MathDisplayMode::Display,
        ),
        ("simple_frac", r"\frac{a+b}{c+d}", MathDisplayMode::Inline),
        (
            "continued_frac",
            r"1 + \frac{1}{1 + \frac{1}{1 + \frac{1}{x}}}",
            MathDisplayMode::Display,
        ),
        (
            "partial_deriv",
            r"\dfrac{\partial f}{\partial x} + \dfrac{\partial f}{\partial y} = 0",
            MathDisplayMode::Display,
        ),
        (
            "tfrac_inline",
            r"\tfrac{1}{2} m v^2",
            MathDisplayMode::Inline,
        ),
        ("sqrt_simple", r"\sqrt{2}", MathDisplayMode::Inline),
        ("nth_root", r"\sqrt[3]{x^2 + y^2}", MathDisplayMode::Inline),
        (
            "nested_root",
            r"\sqrt{\frac{a^2 + b^2}{2}}",
            MathDisplayMode::Display,
        ),
        (
            "quadratic",
            r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
            MathDisplayMode::Display,
        ),
        (
            "pmatrix",
            r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            MathDisplayMode::Display,
        ),
        (
            "bmatrix_3x3",
            r"\begin{bmatrix} 1 & 0 & 0 \\ 0 & 1 & 0 \\ 0 & 0 & 1 \end{bmatrix}",
            MathDisplayMode::Display,
        ),
        (
            "vmatrix_det",
            r"\begin{vmatrix} a & b \\ c & d \end{vmatrix} = ad - bc",
            MathDisplayMode::Display,
        ),
        (
            "smallmatrix",
            r"\bigl(\begin{smallmatrix} a & b \\ c & d \end{smallmatrix}\bigr)",
            MathDisplayMode::Inline,
        ),
        (
            "sum_squares",
            r"\sum_{i=1}^{n} i^2 = \frac{n(n+1)(2n+1)}{6}",
            MathDisplayMode::Display,
        ),
        (
            "product_k",
            r"\prod_{k=1}^{n} (1 + x_k)",
            MathDisplayMode::Display,
        ),
        (
            "gaussian_integral",
            r"\int_{0}^{\infty} e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}",
            MathDisplayMode::Display,
        ),
        (
            "contour_integral",
            r"\oint_{C} \vec{F}\cdot d\vec{r}",
            MathDisplayMode::Display,
        ),
        (
            "double_integral",
            r"\iint_{D} f(x,y)\,dx\,dy",
            MathDisplayMode::Display,
        ),
        (
            "sinc_limit",
            r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
            MathDisplayMode::Display,
        ),
        (
            "euler_limit",
            r"\lim_{n \to \infty} \left(1 + \frac{1}{n}\right)^n = e",
            MathDisplayMode::Display,
        ),
        (
            "greek_lower",
            r"\alpha + \beta + \gamma = \pi",
            MathDisplayMode::Inline,
        ),
        (
            "gamma_fn",
            r"\Gamma(z) = \int_0^\infty t^{z-1} e^{-t}\,dt",
            MathDisplayMode::Display,
        ),
        (
            "blackboard",
            r"f: \mathbb{R}^n \to \mathbb{C}",
            MathDisplayMode::Inline,
        ),
        (
            "calligraphic",
            r"\mathcal{L}\{f(t)\} = F(s)",
            MathDisplayMode::Inline,
        ),
        (
            "fraktur_bold",
            r"\mathfrak{g} \subset \mathbf{GL}_n(\mathbb{C})",
            MathDisplayMode::Inline,
        ),
        (
            "text_cjk_frac",
            r"\text{速度} = \frac{\text{距离}}{\text{时间}}",
            MathDisplayMode::Display,
        ),
        (
            "text_cjk_inline",
            r"f(x) = \text{最大值} + \alpha",
            MathDisplayMode::Inline,
        ),
        (
            "text_cjk_cond",
            r"\text{当 } x > 0 \text{ 时}",
            MathDisplayMode::Inline,
        ),
        (
            "aligned_env",
            r"\begin{aligned} a &= b + c \\ d &= e + f \end{aligned}",
            MathDisplayMode::Display,
        ),
        (
            "cases_env",
            r"f(x) = \begin{cases} x^2 & x \ge 0 \\ -x & x < 0 \end{cases}",
            MathDisplayMode::Display,
        ),
        (
            "array_hline",
            r"\begin{array}{c|c} a & b \\ \hline c & d \end{array}",
            MathDisplayMode::Display,
        ),
        (
            "left_right_paren",
            r"\left(\frac{a}{b}\right)^2",
            MathDisplayMode::Inline,
        ),
        (
            "left_right_bracket",
            r"\left[\frac{1}{2}, \frac{3}{4}\right]",
            MathDisplayMode::Inline,
        ),
        (
            "left_right_brace",
            r"\left\{x \in \mathbb{R} \mid x^2 < 2\right\}",
            MathDisplayMode::Inline,
        ),
        (
            "left_right_norm",
            r"\left\| \mathbf{x} \right\|_2 = \sqrt{\sum_i x_i^2}",
            MathDisplayMode::Display,
        ),
        (
            "left_right_angle",
            r"\left\langle u, v \right\rangle",
            MathDisplayMode::Inline,
        ),
        (
            "left_dot_eval",
            r"\left. \frac{df}{dx} \right|_{x=0}",
            MathDisplayMode::Display,
        ),
        (
            "accents",
            r"\hat{x} + \bar{y} + \tilde{z} + \vec{v}",
            MathDisplayMode::Inline,
        ),
        (
            "over_under",
            r"\overline{x + y} + \underbrace{a + b + \cdots + z}_{26}",
            MathDisplayMode::Display,
        ),
        (
            "logic_quantifiers",
            r"\forall x \in X, \exists y \in Y : x \le y",
            MathDisplayMode::Inline,
        ),
        (
            "maxwell_div",
            r"\nabla \cdot \mathbf{E} = \frac{\rho}{\varepsilon_0}",
            MathDisplayMode::Display,
        ),
        (
            "color_boxed",
            r"\boxed{\color{blue}{x^2} + \color{red}{y^2} = r^2}",
            MathDisplayMode::Display,
        ),
        (
            "affine_nested_frac",
            r"a + \left(\frac{\frac{a}{b}}{c}\right)",
            MathDisplayMode::Inline,
        ),
    ];

    #[test]
    fn ratex_renders_full_katex_corpus_with_embedded_outlines_and_cjk() {
        assert!(KATEX_CORPUS.len() >= 40);
        let pool = build_test_font_pool();
        let full_rasterizer = SvgRasterizer::new(&pool);
        let empty_rasterizer = SvgRasterizer::new(&FontPool::new());

        for (name, latex, mode) in KATEX_CORPUS {
            let opts = MathRenderOptions {
                mode: *mode,
                font_size_pt: 16.0,
                padding_pt: 1.0,
                embed_glyphs: true,
            };
            let out = render_latex_svg(latex, opts)
                .unwrap_or_else(|err| panic!("ratex failed on {name} ({latex}): {err}"));
            assert!(out.width_pt > 0.0, "{name}: width must be positive");
            assert!(out.ascent_pt > 0.0, "{name}: ascent must be positive");
            assert!(
                out.descent_pt >= 0.0,
                "{name}: descent must be non-negative"
            );
            assert!(out.svg.contains("<svg"), "{name}: missing <svg>");
            assert!(
                out.svg.contains("<path"),
                "{name}: expected standalone vector <path>"
            );

            let rgba = full_rasterizer
                .rasterize(&out.svg, 2.0)
                .unwrap_or_else(|err| panic!("resvg failed on ratex svg for {name}: {err}"));
            assert!(
                rgba.count_non_transparent_pixels() > 0,
                "{name}: rasterized formula has zero ink"
            );

            // Non-CJK formulas must render visible ink even with an empty FontPool.
            if !name.contains("cjk") {
                let standalone_rgba = empty_rasterizer.rasterize(&out.svg, 2.0).unwrap();
                assert!(
                    standalone_rgba.count_non_transparent_pixels() > 0,
                    "{name}: standalone outline SVG must render without external fonts"
                );
            }
        }
    }

    #[cfg(feature = "typst")]
    #[test]
    fn mitex_typst_candidate_runs_for_parity_comparison() {
        let pool = build_test_font_pool();
        let mut world = crate::typst_world::InMemoryTypstWorld::new(&pool);
        let samples = [
            r"E = mc^2",
            r"\frac{a+b}{c+d}",
            r"\sum_{i=1}^{n} i^2 = \frac{n(n+1)(2n+1)}{6}",
        ];
        for latex in samples {
            let typst_math = mitex::convert_math(latex, None).expect("mitex conversion");
            let wrapped = format!(
                "#set page(width: auto, height: auto, margin: 0pt)\n#set text(size: 16pt)\n${typst_math}$"
            );
            let out = world.compile_svg(&wrapped).expect("typst compile");
            assert!(out.width_pt > 0.0 && out.ascent_pt > 0.0);
        }
    }
}
