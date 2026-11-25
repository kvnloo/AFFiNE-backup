//! L4 pure-Rust mobile product shell for AFFiNE (`affine-mobile`).
//!
//! Renders the 4-TabBar shell (`Home`, `Explorer`, `Journals`, `Search`), `Settings`
//! (10 groups, without `enable_editor_rtl`), `Page` header with `Breadcrumb` + `Location & Trail Sheet`
//! + independent `Backlinks` pill/sheet, `MobileBackCoordinator`, and production frame-1
//! `AccessibilityTree` export.

#![forbid(unsafe_code)]

pub mod a11y;
pub mod showcase;
pub mod state;
pub mod strings;
pub mod views;

use std::borrow::Cow;

use ui::App;
use ui_tokens::{bundled_font_bytes, bundled_script_font_bytes};

pub use a11y::{
    InMemoryPlatformAccessibility, MobileA11yAction, build_accessibility_tree, perform_a11y_action,
    validate_accessibility_tree,
};
pub use showcase::seed_showcase_state;
pub use state::{
    BackOutcome, MobileAppState, MobileBackCoordinator, MobileCollection, MobileDocRecord,
    MobileRoute, MobileSheet, MobileTab, SettingsGroupId, SettingsState, ThemePreference,
    WorkspaceData, WorkspaceSummary,
};
pub use strings::{MobileLocale, MobileStrings};
pub use views::MobileShellView;

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
const LINUX_CJK_FONT_CANDIDATES: [&str; 4] = [
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
];

/// Register bundled Inter, IBM Plex Mono, Noto Sans Arabic, and Noto Sans Hebrew fonts.
///
/// On native Linux (where headless `CosmicTextSystem::new_without_system_fonts` skips host font
/// scanning), also loads the first available system CJK fallback font so `zh-Hans` glyphs render
/// without bundling a CJK font into mobile binaries.
pub fn register_mobile_fonts(cx: &App) -> anyhow::Result<()> {
    let mut fonts: Vec<Cow<'static, [u8]>> = bundled_font_bytes()
        .into_iter()
        .chain(bundled_script_font_bytes())
        .map(Cow::Borrowed)
        .collect();

    #[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
    {
        for path in LINUX_CJK_FONT_CANDIDATES {
            if let Ok(bytes) = std::fs::read(path)
                && !bytes.is_empty()
            {
                fonts.push(Cow::Owned(bytes));
                break;
            }
        }
    }

    cx.text_system().add_fonts(fonts)
}
