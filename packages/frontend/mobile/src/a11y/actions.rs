//! `PlatformAccessibility` bridge sink, semantic action dispatcher, and structural validator.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use affine_client::{AccessibilityTree, ContextState, LocationChoice, PlatformAccessibility};

use crate::state::{MobileAppState, MobileBackCoordinator, MobileSheet, MobileTab};

/// Semantic action invoked by a screen reader or platform accessibility bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MobileA11yAction {
    Activate,
    Dismiss,
}

/// Thread-safe in-memory [`PlatformAccessibility`] sink for hosts and integration tests.
#[derive(Clone, Debug, Default)]
pub struct InMemoryPlatformAccessibility {
    latest: Arc<Mutex<Option<AccessibilityTree>>>,
    publish_count: Arc<Mutex<usize>>,
    announcements: Arc<Mutex<Vec<(String, bool)>>>,
}

impl InMemoryPlatformAccessibility {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn latest_tree(&self) -> Option<AccessibilityTree> {
        self.latest.lock().ok().and_then(|g| g.clone())
    }

    pub fn publish_count(&self) -> usize {
        self.publish_count.lock().map(|g| *g).unwrap_or(0)
    }

    pub fn announcements(&self) -> Vec<(String, bool)> {
        self.announcements
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }
}

impl PlatformAccessibility for InMemoryPlatformAccessibility {
    fn publish_tree(&mut self, tree: &AccessibilityTree) {
        if let Ok(mut slot) = self.latest.lock() {
            *slot = Some(tree.clone());
        }
        if let Ok(mut count) = self.publish_count.lock() {
            *count += 1;
        }
    }

    fn announce(&mut self, message: &str, polite: bool) {
        if let Ok(mut log) = self.announcements.lock() {
            log.push((message.to_string(), polite));
        }
    }
}

/// Dispatch a semantic accessibility action against `node_id` on `state`.
pub fn perform_a11y_action(
    state: &mut MobileAppState,
    node_id: &str,
    action: MobileA11yAction,
    now_ms: f64,
) -> bool {
    if action == MobileA11yAction::Dismiss {
        if state.active_sheet != MobileSheet::None {
            state.close_sheet(now_ms);
            return true;
        }
        return false;
    }

    match node_id {
        "mobile.tab.home" => {
            state.switch_tab(MobileTab::Home, now_ms);
            true
        }
        "mobile.tab.explorer" => {
            state.switch_tab(MobileTab::Explorer, now_ms);
            true
        }
        "mobile.tab.journals" => {
            state.switch_tab(MobileTab::Journals, now_ms);
            true
        }
        "mobile.tab.search" => {
            state.switch_tab(MobileTab::Search, now_ms);
            true
        }
        "mobile.page.back_btn" | "mobile.settings.back_btn" => {
            MobileBackCoordinator::handle_back(state, now_ms);
            true
        }
        "mobile.page.more_btn" => {
            state.open_sheet_from(
                MobileSheet::LocationAndTrail,
                Some("mobile.page.more_btn"),
                now_ms,
            );
            true
        }
        "mobile.home.workspace_btn" => {
            state.open_sheet_from(
                MobileSheet::WorkspaceSelector,
                Some("mobile.home.workspace_btn"),
                now_ms,
            );
            true
        }
        "page.backlinks.pill" => {
            state.open_sheet_from(MobileSheet::Backlinks, Some("page.backlinks.pill"), now_ms);
            true
        }
        "mobile.sheet.done_btn" => {
            state.close_sheet(now_ms);
            true
        }
        "mobile.sheet.use_trail_btn" => {
            let cur_seq = state.history.current().map(|e| e.seq).unwrap_or(0);
            state.pick_location(cur_seq, Some(LocationChoice::UseTrail), now_ms);
            true
        }
        _ if node_id.starts_with("mobile.sheet.loc.") => {
            let Ok(idx) = node_id["mobile.sheet.loc.".len()..].parse::<usize>() else {
                return false;
            };
            let cur_seq = state.history.current().map(|e| e.seq).unwrap_or(0);
            let derivation = state.derive_breadcrumb();
            let Some(opt) = derivation.current_doc_locations.get(idx) else {
                return false;
            };
            state.pick_location(
                cur_seq,
                Some(LocationChoice::Location(opt.location.clone())),
                now_ms,
            );
            true
        }
        _ if node_id.starts_with("mobile.sheet.owner_loc.") => {
            let Ok(idx) = node_id["mobile.sheet.owner_loc.".len()..].parse::<usize>() else {
                return false;
            };
            let derivation = state.derive_breadcrumb();
            let ContextState::MultiLocation {
                owner_seq, options, ..
            } = derivation.context
            else {
                return false;
            };
            let Some(opt) = options.get(idx) else {
                return false;
            };
            state.pick_location(
                owner_seq,
                Some(LocationChoice::Location(opt.location.clone())),
                now_ms,
            );
            true
        }
        _ if node_id.starts_with("mobile.sheet.ws.") => {
            let ws_id = &node_id["mobile.sheet.ws.".len()..];
            let Some(idx) = state.workspaces.iter().position(|w| w.id == ws_id) else {
                return false;
            };
            state.select_workspace(idx, now_ms);
            true
        }
        _ => false,
    }
}

/// Validate that `tree` has no orphaned nodes, no missing children, and at most one `current`
/// node inside the active navigation landmark.
pub fn validate_accessibility_tree(tree: &AccessibilityTree) -> Vec<String> {
    let mut errors = Vec::new();
    if !tree.nodes.contains_key(&tree.root_id) {
        errors.push(format!("missing root node {}", tree.root_id));
        return errors;
    }

    let mut reachable: HashSet<&str> = HashSet::new();
    let mut stack = vec![tree.root_id.as_str()];
    while let Some(id) = stack.pop() {
        if !reachable.insert(id) {
            continue;
        }
        if let Some(node) = tree.nodes.get(id) {
            for child in &node.children {
                if !tree.nodes.contains_key(child) {
                    errors.push(format!("node {id} references missing child {child}"));
                } else {
                    stack.push(child.as_str());
                }
            }
        }
    }

    for id in tree.nodes.keys() {
        if !reachable.contains(id.as_str()) {
            errors.push(format!("orphaned accessibility node {id}"));
        }
    }

    let current_count = tree.nodes.values().filter(|n| n.state.current).count();
    if current_count > 1 {
        errors.push(format!(
            "expected at most 1 aria-current node, found {current_count}"
        ));
    }

    errors
}
