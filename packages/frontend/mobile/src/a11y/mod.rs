//! Production frame-1 accessibility semantic tree builder, `PlatformAccessibility` bridge sink,
//! semantic action dispatcher, and structural validator.

mod actions;

pub use actions::{
    InMemoryPlatformAccessibility, MobileA11yAction, perform_a11y_action,
    validate_accessibility_tree,
};

use std::collections::BTreeMap;

use affine_client::{
    AccessibilityBounds, AccessibilityNode, AccessibilityRole, AccessibilityState,
    AccessibilityTree, ContextState, NavTarget,
};
use ui_kit::{
    BreadcrumbBarConfig, BreadcrumbBarLayout, BreadcrumbItem, BreadcrumbItemKind,
    layout_breadcrumb_bar,
};

use crate::state::{MobileAppState, MobileRoute, MobileSheet, MobileTab};

/// Build the production accessibility tree for the current mobile shell state and viewport.
pub fn build_accessibility_tree(
    state: &mut MobileAppState,
    viewport_width: f32,
    viewport_height: f32,
) -> AccessibilityTree {
    build_accessibility_tree_with_layout(state, viewport_width, viewport_height, 0.0, 0.0, None)
}

/// Build the production accessibility tree using actual safe-area insets and optional measured
/// [`BreadcrumbBarLayout`] so screen geometry and semantic bounds stay synchronized.
pub fn build_accessibility_tree_with_layout(
    state: &mut MobileAppState,
    viewport_width: f32,
    viewport_height: f32,
    safe_top: f32,
    safe_bottom: f32,
    measured_breadcrumb: Option<&BreadcrumbBarLayout>,
) -> AccessibilityTree {
    let locale = state.locale();
    let strings = locale.strings();
    let mut nodes: BTreeMap<String, AccessibilityNode> = BTreeMap::new();
    let mut root_children: Vec<String> = Vec::new();

    let main_id = "app.main".to_string();
    let mut main_children: Vec<String> = Vec::new();
    let is_rtl = locale.direction().is_rtl();

    if let MobileRoute::Home = state.route {
        let ws_btn_id = "mobile.home.workspace_btn".to_string();
        nodes.insert(
            ws_btn_id.clone(),
            AccessibilityNode {
                id: ws_btn_id.clone(),
                role: AccessibilityRole::Button,
                name: state.active_workspace().name.clone(),
                value: None,
                state: AccessibilityState {
                    expanded: Some(state.active_sheet == MobileSheet::WorkspaceSelector),
                    ..AccessibilityState::default()
                },
                focused: false,
                bounds: AccessibilityBounds {
                    x: if is_rtl {
                        (viewport_width - 180.0).max(0.0)
                    } else {
                        16.0
                    },
                    y: safe_top + 6.0,
                    width: 164.0,
                    height: 44.0,
                },
                children: Vec::new(),
            },
        );
        main_children.push(ws_btn_id);
    }

    if let MobileRoute::Explorer = state.route
        && let Some(back_name) = explorer_back_label(state)
    {
        let back_id = "mobile.explorer.back_btn".to_string();
        let back_w = 88.0_f32;
        nodes.insert(
            back_id.clone(),
            AccessibilityNode {
                id: back_id.clone(),
                role: AccessibilityRole::Button,
                name: back_name,
                value: None,
                state: AccessibilityState::default(),
                focused: false,
                bounds: AccessibilityBounds {
                    x: if is_rtl {
                        (viewport_width - 8.0 - back_w).max(0.0)
                    } else {
                        8.0
                    },
                    y: safe_top + 6.0,
                    width: back_w,
                    height: 44.0,
                },
                children: Vec::new(),
            },
        );
        main_children.push(back_id);
    }

    if let MobileRoute::Page(doc_id) = state.route.clone() {
        let derivation = state.derive_breadcrumb();
        if !derivation.should_hide_bar_on_mobile && !derivation.crumbs.is_empty() {
            let nav_bc_id = "app.nav.breadcrumb".to_string();
            let owned_layout;
            let layout = if let Some(pre) = measured_breadcrumb {
                pre
            } else {
                let items: Vec<BreadcrumbItem> = derivation
                    .crumbs
                    .iter()
                    .map(|c| {
                        let label = if c.is_switcher {
                            match &derivation.context {
                                ContextState::MultiLocation { count, .. } => {
                                    strings.multi_location_label(locale, *count)
                                }
                                _ => c.label.clone(),
                            }
                        } else if c.id == "root:trash" {
                            strings.trash.to_string()
                        } else if c.id == "root:journals" {
                            strings.tab_journals.to_string()
                        } else {
                            c.label.clone()
                        };
                        BreadcrumbItem {
                            id: c.id.clone(),
                            label,
                            kind: if c.is_current {
                                BreadcrumbItemKind::Current
                            } else if c.is_switcher {
                                BreadcrumbItemKind::Switcher
                            } else {
                                BreadcrumbItemKind::Segment
                            },
                            clickable: !c.is_current,
                        }
                    })
                    .collect();
                owned_layout = layout_breadcrumb_bar(
                    &items,
                    &BreadcrumbBarConfig::new(viewport_width, locale.direction())
                        .with_font_scale(state.font_scale),
                );
                &owned_layout
            };

            let mut bc_children = Vec::new();
            for slot in &layout.slots {
                let node_id = format!("breadcrumb.slot.{}", slot.id);
                let is_ellipsis = slot.kind == ui_kit::BreadcrumbSlotKind::Ellipsis;
                let a11y_name = if slot.kind == ui_kit::BreadcrumbSlotKind::Switcher {
                    match &derivation.context {
                        ContextState::MultiLocation {
                            count,
                            owner_doc_id,
                            owner_doc_title,
                            ..
                        } if *owner_doc_id != doc_id => {
                            strings.owner_multi_location_label(locale, owner_doc_title, *count)
                        }
                        _ => slot.full_title.clone(),
                    }
                } else {
                    slot.full_title.clone()
                };
                nodes.insert(
                    node_id.clone(),
                    AccessibilityNode {
                        id: node_id.clone(),
                        role: if is_ellipsis {
                            AccessibilityRole::Button
                        } else {
                            AccessibilityRole::Link
                        },
                        name: a11y_name,
                        value: None,
                        state: AccessibilityState {
                            current: slot.is_current,
                            expanded: if is_ellipsis
                                || slot.kind == ui_kit::BreadcrumbSlotKind::Switcher
                            {
                                Some(state.active_sheet == MobileSheet::LocationAndTrail)
                            } else {
                                None
                            },
                            ..AccessibilityState::default()
                        },
                        focused: false,
                        bounds: AccessibilityBounds {
                            x: slot.x,
                            y: safe_top + 52.0,
                            width: slot.width,
                            height: slot.height,
                        },
                        children: Vec::new(),
                    },
                );
                bc_children.push(node_id);
            }

            nodes.insert(
                nav_bc_id.clone(),
                AccessibilityNode {
                    id: nav_bc_id.clone(),
                    role: AccessibilityRole::Navigation,
                    name: "Breadcrumb".to_string(),
                    value: None,
                    state: AccessibilityState::default(),
                    focused: false,
                    bounds: AccessibilityBounds {
                        x: 0.0,
                        y: safe_top + 52.0,
                        width: viewport_width,
                        height: layout.height,
                    },
                    children: bc_children,
                },
            );
            root_children.push(nav_bc_id);
        }

        let more_id = "mobile.page.more_btn".to_string();
        nodes.insert(
            more_id.clone(),
            AccessibilityNode {
                id: more_id.clone(),
                role: AccessibilityRole::Button,
                name: strings.location_trail_title.to_string(),
                value: None,
                state: AccessibilityState {
                    expanded: Some(state.active_sheet == MobileSheet::LocationAndTrail),
                    ..AccessibilityState::default()
                },
                focused: false,
                bounds: AccessibilityBounds {
                    x: if is_rtl {
                        8.0
                    } else {
                        (viewport_width - 52.0).max(0.0)
                    },
                    y: safe_top + 4.0,
                    width: 44.0,
                    height: 44.0,
                },
                children: Vec::new(),
            },
        );
        main_children.push(more_id);

        let backlinks_id = "page.backlinks.pill".to_string();
        let bl = state.backlinks_for(&doc_id);
        nodes.insert(
            backlinks_id.clone(),
            AccessibilityNode {
                id: backlinks_id.clone(),
                role: AccessibilityRole::Button,
                name: strings.backlinks_pill_label(
                    locale,
                    bl.items.len(),
                    bl.completeness == affine_client::BacklinkCompleteness::Ready,
                ),
                value: None,
                state: AccessibilityState {
                    expanded: Some(state.active_sheet == MobileSheet::Backlinks),
                    ..AccessibilityState::default()
                },
                focused: false,
                bounds: AccessibilityBounds {
                    x: 16.0,
                    y: safe_top + 108.0,
                    width: (viewport_width - 32.0).max(44.0),
                    height: 44.0,
                },
                children: Vec::new(),
            },
        );
        main_children.push(backlinks_id);
    }

    let main_name = match &state.route {
        MobileRoute::Home => strings.tab_home.to_string(),
        MobileRoute::Explorer => strings.tab_explorer.to_string(),
        MobileRoute::Journals => strings.tab_journals.to_string(),
        MobileRoute::Search => strings.tab_search.to_string(),
        MobileRoute::Settings => strings.settings_title.to_string(),
        MobileRoute::Page(id) => state
            .doc_by_id(id)
            .map(|d| d.title.clone())
            .unwrap_or_else(|| "Untitled".to_string()),
    };

    let show_tabbar = matches!(
        state.route,
        MobileRoute::Home | MobileRoute::Explorer | MobileRoute::Journals | MobileRoute::Search
    );

    if matches!(state.route, MobileRoute::Page(_) | MobileRoute::Settings) {
        let back_id = match state.route {
            MobileRoute::Page(_) => "mobile.page.back_btn".to_string(),
            _ => "mobile.settings.back_btn".to_string(),
        };
        let back_w = 72.0_f32;
        let back_x = if is_rtl {
            (viewport_width - 8.0 - back_w).max(0.0)
        } else {
            8.0
        };
        nodes.insert(
            back_id.clone(),
            AccessibilityNode {
                id: back_id.clone(),
                role: AccessibilityRole::Button,
                name: strings.back_label.to_string(),
                value: None,
                state: AccessibilityState::default(),
                focused: false,
                bounds: AccessibilityBounds {
                    x: back_x,
                    y: safe_top + 4.0,
                    width: back_w,
                    height: 44.0,
                },
                children: Vec::new(),
            },
        );
        main_children.push(back_id);
    }

    let tabbar_h = if show_tabbar { 64.0 } else { 0.0 };
    nodes.insert(
        main_id.clone(),
        AccessibilityNode {
            id: main_id.clone(),
            role: AccessibilityRole::Main,
            name: main_name,
            value: None,
            state: AccessibilityState::default(),
            focused: false,
            bounds: AccessibilityBounds {
                x: 0.0,
                y: safe_top,
                width: viewport_width,
                height: (viewport_height - safe_top - safe_bottom - tabbar_h).max(44.0),
            },
            children: main_children,
        },
    );
    root_children.push(main_id.clone());

    if show_tabbar {
        let tab_bar_id = "app.nav.tabbar".to_string();
        let tab_w = (viewport_width / 4.0).max(44.0);
        let tab_y = (viewport_height - safe_bottom - 64.0).max(0.0);
        let mut tab_ids = Vec::with_capacity(4);
        for (idx, tab) in MobileTab::ALL.iter().enumerate() {
            let id = tab.id().to_string();
            let label = match tab {
                MobileTab::Home => strings.tab_home,
                MobileTab::Explorer => strings.tab_explorer,
                MobileTab::Journals => strings.tab_journals,
                MobileTab::Search => strings.tab_search,
            };
            let is_active = state.active_tab == *tab;
            let tab_x = if is_rtl {
                (viewport_width - ((idx + 1) as f32) * tab_w).max(0.0)
            } else {
                (idx as f32) * tab_w
            };
            nodes.insert(
                id.clone(),
                AccessibilityNode {
                    id: id.clone(),
                    role: AccessibilityRole::Tab,
                    name: label.to_string(),
                    value: None,
                    state: AccessibilityState {
                        selected: is_active,
                        current: is_active,
                        ..AccessibilityState::default()
                    },
                    focused: false,
                    bounds: AccessibilityBounds {
                        x: tab_x,
                        y: tab_y,
                        width: tab_w,
                        height: 64.0,
                    },
                    children: Vec::new(),
                },
            );
            tab_ids.push(id);
        }
        nodes.insert(
            tab_bar_id.clone(),
            AccessibilityNode {
                id: tab_bar_id.clone(),
                role: AccessibilityRole::TabList,
                name: "Bottom Tabs".to_string(),
                value: None,
                state: AccessibilityState::default(),
                focused: false,
                bounds: AccessibilityBounds {
                    x: 0.0,
                    y: tab_y,
                    width: viewport_width,
                    height: 64.0,
                },
                children: tab_ids,
            },
        );
        root_children.push(tab_bar_id);
    }

    if state.active_sheet != MobileSheet::None {
        let sheet_id = insert_sheet_a11y_nodes(
            state,
            viewport_width,
            viewport_height,
            safe_bottom,
            is_rtl,
            &mut nodes,
        );
        root_children.push(sheet_id);
    }

    let focus_target = state
        .focused_a11y_id
        .as_deref()
        .filter(|id| nodes.contains_key(*id))
        .unwrap_or(if state.active_sheet != MobileSheet::None {
            "app.sheet.dialog"
        } else {
            main_id.as_str()
        })
        .to_string();
    if let Some(node) = nodes.get_mut(&focus_target) {
        node.focused = true;
    }

    let root_id = "app.root".to_string();
    nodes.insert(
        root_id.clone(),
        AccessibilityNode {
            id: root_id.clone(),
            role: AccessibilityRole::Application,
            name: "AFFiNE".to_string(),
            value: Some(state.active_workspace().name.clone()),
            state: AccessibilityState::default(),
            focused: false,
            bounds: AccessibilityBounds {
                x: 0.0,
                y: 0.0,
                width: viewport_width,
                height: viewport_height,
            },
            children: root_children,
        },
    );

    AccessibilityTree { root_id, nodes }
}

fn explorer_back_label(state: &mut MobileAppState) -> Option<String> {
    let strings = state.locale().strings();
    let target = state.history.current().map(|e| e.target.clone())?;
    match target {
        NavTarget::Folder(folder_id) => {
            if let affine_client::FolderForestState::Ready(forest) = state.forest_state()
                && let Some(chain) = forest.folder_chain(&folder_id)
                && chain.len() >= 2
            {
                return Some(chain[chain.len() - 2].name.clone());
            }
            Some(strings.tab_explorer.to_string())
        }
        NavTarget::Collection(id) if !id.is_empty() => Some(strings.collections.to_string()),
        NavTarget::Tag(id) if !id.is_empty() => Some(strings.tags.to_string()),
        NavTarget::Collection(_) | NavTarget::Tag(_) | NavTarget::Trash => {
            Some(strings.tab_explorer.to_string())
        }
        _ => None,
    }
}

fn insert_sheet_a11y_nodes(
    state: &mut MobileAppState,
    viewport_width: f32,
    viewport_height: f32,
    safe_bottom: f32,
    is_rtl: bool,
    nodes: &mut BTreeMap<String, AccessibilityNode>,
) -> String {
    let strings = state.locale().strings();
    let compact = viewport_height < 460.0;
    let pad = if compact { 12.0 } else { 16.0 };
    let gap = if compact { 8.0 } else { 12.0 };
    let header_block = pad + 4.0 + gap + 44.0 + gap;

    let mut cols: Vec<Vec<(String, String, bool, f32)>> = Vec::new();
    match state.active_sheet {
        MobileSheet::LocationAndTrail => {
            let derivation = state.derive_breadcrumb();
            let cur_doc_id = match &state.route {
                MobileRoute::Page(id) => id.as_str(),
                _ => "",
            };
            let trail = if derivation.can_return_to_trail {
                &derivation.recoverable_trail
            } else {
                &derivation.trail
            };
            let mut trail_col: Vec<(String, String, bool, f32)> = trail
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    (
                        format!("mobile.sheet.trail.{i}"),
                        s.title.clone(),
                        s.is_current,
                        44.0,
                    )
                })
                .collect();
            let mut owner_col = Vec::new();
            if let ContextState::MultiLocation {
                owner_doc_id,
                options,
                ..
            } = &derivation.context
                && owner_doc_id != cur_doc_id
            {
                for (i, o) in options.iter().enumerate() {
                    owner_col.push((
                        format!("mobile.sheet.owner_loc.{i}"),
                        o.label.clone(),
                        o.selected,
                        44.0,
                    ));
                }
            }
            let mut loc_col: Vec<(String, String, bool, f32)> = derivation
                .current_doc_locations
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    (
                        format!("mobile.sheet.loc.{i}"),
                        o.label.clone(),
                        o.selected,
                        46.0,
                    )
                })
                .collect();
            if derivation.can_return_to_trail {
                loc_col.push((
                    "mobile.sheet.use_trail_btn".to_string(),
                    strings.use_trail_action.to_string(),
                    false,
                    44.0,
                ));
            }
            if compact {
                cols.push(trail_col);
                if !owner_col.is_empty() {
                    cols.push(owner_col);
                }
                cols.push(loc_col);
            } else {
                trail_col.extend(owner_col);
                trail_col.extend(loc_col);
                cols.push(trail_col);
            }
        }
        MobileSheet::WorkspaceSelector => {
            cols.push(
                state
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(i, ws)| {
                        (
                            format!("mobile.sheet.ws.{}", ws.id),
                            ws.name.clone(),
                            i == state.active_workspace_idx,
                            48.0,
                        )
                    })
                    .collect(),
            );
        }
        MobileSheet::Backlinks => {
            let doc_id = match &state.route {
                MobileRoute::Page(id) => id.clone(),
                _ => String::new(),
            };
            let bl = state.backlinks_for(&doc_id);
            cols.push(
                bl.items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let title = state
                            .doc_by_id(&item.source_doc_id)
                            .map(|d| d.title.clone())
                            .unwrap_or_else(|| item.source_doc_id.clone());
                        (format!("mobile.sheet.backlink.{i}"), title, false, 46.0)
                    })
                    .collect(),
            );
        }
        MobileSheet::None => {}
    }

    let max_col_h = cols
        .iter()
        .map(|c| 20.0 + c.iter().map(|(_, _, _, h)| *h).sum::<f32>())
        .fold(44.0_f32, f32::max);
    let max_panel_h = (viewport_height - 24.0).max(180.0);
    let panel_h = (header_block + max_col_h + pad).min(max_panel_h);
    let panel_y = (viewport_height - safe_bottom - panel_h).max(0.0);

    let mut sheet_children = Vec::new();
    let done_w = 56.0_f32;
    let done_x = if is_rtl {
        pad
    } else {
        (viewport_width - pad - done_w).max(0.0)
    };
    let done_id = "mobile.sheet.done_btn".to_string();
    nodes.insert(
        done_id.clone(),
        AccessibilityNode {
            id: done_id.clone(),
            role: AccessibilityRole::Button,
            name: strings.close_label.to_string(),
            value: None,
            state: AccessibilityState::default(),
            focused: false,
            bounds: AccessibilityBounds {
                x: done_x,
                y: panel_y + pad + 4.0 + gap,
                width: done_w,
                height: 44.0,
            },
            children: Vec::new(),
        },
    );
    sheet_children.push(done_id);

    let col_count = cols.len().max(1);
    let inner_w = (viewport_width - pad * 2.0).max(44.0);
    let col_gap = if col_count > 1 { 10.0 } else { 0.0 };
    let col_w = ((inner_w - col_gap * ((col_count - 1) as f32)) / (col_count as f32)).max(44.0);
    let content_top = panel_y + header_block;
    let panel_bottom = panel_y + panel_h;

    for (c_idx, col) in cols.into_iter().enumerate() {
        let col_x = if is_rtl {
            (viewport_width - pad - ((c_idx + 1) as f32) * col_w - (c_idx as f32) * col_gap)
                .max(pad)
        } else {
            pad + (c_idx as f32) * (col_w + col_gap)
        };
        let mut y = content_top + 16.0;
        for (id, name, selected, h) in col {
            let clamped_y = y.min((panel_bottom - h).max(content_top));
            nodes.insert(
                id.clone(),
                AccessibilityNode {
                    id: id.clone(),
                    role: AccessibilityRole::Button,
                    name,
                    value: None,
                    state: AccessibilityState {
                        selected,
                        ..AccessibilityState::default()
                    },
                    focused: false,
                    bounds: AccessibilityBounds {
                        x: col_x,
                        y: clamped_y,
                        width: col_w,
                        height: h.min(panel_bottom - clamped_y).max(1.0),
                    },
                    children: Vec::new(),
                },
            );
            sheet_children.push(id);
            y += h;
        }
    }

    let sheet_id = "app.sheet.dialog".to_string();
    let sheet_name = match state.active_sheet {
        MobileSheet::WorkspaceSelector => strings.switch_workspace,
        MobileSheet::LocationAndTrail => strings.location_trail_title,
        MobileSheet::Backlinks => strings.backlinks_label,
        MobileSheet::None => "",
    };
    nodes.insert(
        sheet_id.clone(),
        AccessibilityNode {
            id: sheet_id.clone(),
            role: AccessibilityRole::Dialog,
            name: sheet_name.to_string(),
            value: None,
            state: AccessibilityState {
                expanded: Some(true),
                ..AccessibilityState::default()
            },
            focused: false,
            bounds: AccessibilityBounds {
                x: 0.0,
                y: panel_y,
                width: viewport_width,
                height: panel_h,
            },
            children: sheet_children,
        },
    );
    sheet_id
}
