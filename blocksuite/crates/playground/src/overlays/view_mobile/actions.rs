//! Single-step Undo document commands driven by the mobile `KeyboardToolbar`:
//! inline/block color highlights, block duplicate/delete, block inserts
//! (equation, table, callout, image, bookmark), inline math, and deterministic
//! scheduler-clock date formatting.

use blocksuite_playground_doc::{
    DocError, DocResult,
    read::{BlockContent, BlockNode},
    schema::{BlockSpec, ListType, ParagraphType},
};
use ui::{Context, Window};
use y_octo::{Any, TextAttributes, TextDeltaOp, TextInsert};

use crate::{
    PlaygroundView,
    block_flow::FlowKind,
    editor_session::{DocPoint, DocumentSelection},
    overlays::{EquationTarget, KeyboardToolbarDateKind, OverlayEvent, find_parent},
};

/// Howard Hinnant's civil-from-days algorithm: converts days since 1970-01-01 to `(year, month, day)`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    (year, m as u32, d as u32)
}

/// Format a deterministic scheduler timestamp (`epoch_ms`) into BlockSuite `0.27.5`
/// `formatDate` (`yyyy-mm-dd`) or `formatTime` (`mm-dd hh:mm`) without wall-clock I/O.
pub(super) fn format_keyboard_toolbar_date(epoch_ms: i64, kind: KeyboardToolbarDateKind) -> String {
    let total_secs = epoch_ms.div_euclid(1_000);
    let days = total_secs.div_euclid(86_400);
    let sec_of_day = total_secs.rem_euclid(86_400);
    match kind {
        KeyboardToolbarDateKind::Today => {
            let (y, m, d) = civil_from_days(days);
            format!("{y:04}-{m:02}-{d:02}")
        }
        KeyboardToolbarDateKind::Tomorrow => {
            let (y, m, d) = civil_from_days(days + 1);
            format!("{y:04}-{m:02}-{d:02}")
        }
        KeyboardToolbarDateKind::Yesterday => {
            let (y, m, d) = civil_from_days(days - 1);
            format!("{y:04}-{m:02}-{d:02}")
        }
        KeyboardToolbarDateKind::Now => {
            let (_, m, d) = civil_from_days(days);
            let hh = sec_of_day / 3_600;
            let mm = (sec_of_day % 3_600) / 60;
            format!("{m:02}-{d:02} {hh:02}:{mm:02}")
        }
    }
}

fn clone_block_spec(node: &BlockNode) -> BlockSpec {
    match &node.content {
        BlockContent::Paragraph { kind, .. } => BlockSpec::Paragraph {
            paragraph_type: *kind,
            text: String::new(),
        },
        BlockContent::List {
            kind,
            checked,
            order,
            ..
        } => BlockSpec::List {
            list_type: *kind,
            text: String::new(),
            checked: *checked,
            order: order.and_then(|v| i32::try_from(v).ok()),
        },
        BlockContent::Code {
            language, caption, ..
        } => BlockSpec::Code {
            text: String::new(),
            language: (!language.is_empty()).then(|| language.clone()),
            caption: caption.clone(),
        },
        BlockContent::Divider => BlockSpec::Divider,
        BlockContent::Latex { latex } => BlockSpec::Latex {
            latex: latex.clone(),
        },
        BlockContent::Callout {
            icon,
            background_color,
            ..
        } => BlockSpec::Callout {
            icon: icon.clone(),
            background_color: background_color.clone(),
        },
        BlockContent::Image {
            source_id,
            caption,
            width,
            height,
        } => BlockSpec::Image {
            source_id: source_id.clone(),
            caption: caption.clone(),
            width: width.unwrap_or(320.0),
            height: height.unwrap_or(180.0),
            xywh: None,
        },
        BlockContent::Bookmark {
            url,
            title,
            description,
            ..
        } => BlockSpec::Bookmark {
            url: url.clone(),
            title: title.clone(),
            description: description.clone(),
        },
        BlockContent::Table(table) => {
            let rows = table
                .rows
                .iter()
                .map(|r| {
                    table
                        .columns
                        .iter()
                        .map(|c| {
                            table
                                .cells
                                .get(&(r.row_id.clone(), c.column_id.clone()))
                                .map(|cell| cell.text.plain.clone())
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .collect();
            BlockSpec::Table { rows }
        }
        _ => BlockSpec::Paragraph {
            paragraph_type: ParagraphType::Text,
            text: String::new(),
        },
    }
}

impl PlaygroundView {
    pub(super) fn can_indent_current_block(&self) -> bool {
        let Some(id) = self
            .editor
            .selection
            .as_ref()
            .map(|s| s.focus.block_id.as_str())
        else {
            return false;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        if snapshot
            .block(id)
            .is_some_and(|n| matches!(n.content, BlockContent::Code { .. }))
        {
            return true;
        }
        let Some(root) = snapshot.root.as_ref() else {
            return false;
        };
        let Some((parent, pos)) = find_parent(root, id) else {
            return false;
        };
        pos.checked_sub(1)
            .and_then(|i| parent.children.get(i))
            .is_some_and(|b| {
                matches!(
                    b.content,
                    BlockContent::List { .. } | BlockContent::Paragraph { .. }
                )
            })
    }

    pub(super) fn can_dedent_current_block(&self) -> bool {
        let Some(id) = self
            .editor
            .selection
            .as_ref()
            .map(|s| s.focus.block_id.as_str())
        else {
            return false;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        let Some(root) = snapshot.root.as_ref() else {
            return false;
        };
        let Some((parent, _)) = find_parent(root, id) else {
            return false;
        };
        matches!(
            parent.content,
            BlockContent::List { .. } | BlockContent::Paragraph { .. }
        ) && find_parent(root, &parent.id).is_some()
    }

    /// Apply or clear a `color` or `background` rich-text highlight mark in one undo step.
    pub(super) fn format_color_mark(
        &mut self,
        key: &str,
        value: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocResult {
        let Some(sel) = self.editor.selection.clone() else {
            return Ok(());
        };
        if sel.empty() {
            self.editor
                .pending_marks
                .insert(key.into(), value.map(|v| Any::String(v.to_owned())));
            return Ok(());
        }
        let Some(layout) = self.flow_layout() else {
            return Ok(());
        };
        let Some((start, end, a, b, _)) = sel.ordered(&layout) else {
            return Ok(());
        };
        let now = self.now_ms(cx) as u64;
        let mut ranges = Vec::new();
        for block in layout.blocks[a..=b].iter().filter(|b| b.editable()) {
            let id = &block.block_id;
            let from = if id == &start.block_id {
                start.offset()
            } else {
                0
            };
            let to = if id == &end.block_id {
                end.offset()
            } else {
                block.body.plain.encode_utf16().count() as u64
            };
            if to <= from {
                continue;
            }
            let prop = if matches!(block.body.kind, FlowKind::Title) {
                "prop:title"
            } else {
                "prop:text"
            };
            ranges.push((id.as_str(), prop, from, to - from));
        }
        if ranges.is_empty() {
            return Ok(());
        }
        let page = self.page_mut()?;
        let snap = sel.snapshot(page)?;
        page.stop_capturing()?;
        let attr_val = value.map_or(Any::Null, |v| Any::String(v.to_owned()));
        let commit = page.structural_edit(now, |page| {
            for (id, prop, from, len) in ranges {
                page.format_text(
                    id,
                    prop,
                    from,
                    len,
                    TextAttributes::from([(key.to_owned(), attr_val.clone())]),
                    now,
                )?;
            }
            Ok(())
        })?;
        if commit.is_some() {
            page.set_undo_selection_snapshot(snap)?;
        }
        self.editor.last_typing = None;
        self.did_edit(commit, (a == b).then_some(&start.block_id), window, cx);
        Ok(())
    }

    /// Insert an inline LaTeX equation placeholder at the caret and open the equation editor.
    pub(super) fn insert_inline_math_from_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sel) = self.editor.selection.clone() else {
            return;
        };
        let offset = sel.focus.offset();
        let now = self.now_ms(cx) as u64;
        let before = self.selection_snapshot();
        if let Ok(page) = self.page_mut() {
            let _ = page.stop_capturing();
            let mut delta = Vec::new();
            if offset > 0 {
                delta.push(TextDeltaOp::Retain {
                    retain: offset,
                    format: None,
                });
            }
            delta.push(TextDeltaOp::Insert {
                insert: TextInsert::Text(" ".into()),
                format: Some(TextAttributes::from([(
                    "latex".to_string(),
                    Any::String(String::new()),
                )])),
            });
            if let Ok(commit) = page.apply_text_delta(&sel.focus.block_id, "prop:text", &delta, now)
            {
                if commit.is_some()
                    && let Some(before) = before
                {
                    let _ = page.set_undo_selection_snapshot(before);
                }
                self.editor.selection = Some(DocumentSelection::caret(DocPoint::new(
                    &sel.focus.block_id,
                    offset + 1,
                )));
                self.did_edit(commit, Some(&sel.focus.block_id), window, cx);
                self.dispatch_overlay(
                    OverlayEvent::OpenEquationEditor {
                        target: EquationTarget::Inline {
                            block_id: sel.focus.block_id,
                            utf16_offset: offset,
                        },
                        initial: String::new(),
                    },
                    window,
                    cx,
                );
            }
        }
    }

    /// Clone the currently focused block directly below it as one undoable `structural_edit`.
    pub(super) fn duplicate_current_block(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocResult {
        let Some(sel) = self.editor.selection.clone() else {
            return Ok(());
        };
        let block_id = sel.focus.block_id.clone();
        let snapshot = self
            .snapshot
            .clone()
            .ok_or_else(|| DocError::InvalidState("missing page".into()))?;
        let Some((parent, slot)) = snapshot
            .root
            .as_ref()
            .and_then(|root| find_parent(root, &block_id))
        else {
            return Ok(());
        };
        let node = &parent.children[slot];
        let spec = clone_block_spec(node);
        let is_numbered = matches!(
            node.content,
            BlockContent::List {
                kind: ListType::Numbered,
                ..
            }
        );
        let has_text = matches!(
            node.content,
            BlockContent::Paragraph { .. } | BlockContent::List { .. } | BlockContent::Code { .. }
        );
        let existing_delta = if has_text {
            self.page()
                .and_then(|p| p.read_editable_text(&block_id).ok())
                .map(|(rich, _)| rich.delta)
        } else {
            None
        };
        let fresh = self.fresh_block_id();
        let parent_id = parent.id.clone();
        let now = self.now_ms(cx) as u64;
        let before = self.selection_snapshot();
        let page = self.page_mut()?;
        page.stop_capturing()?;
        let commit = page.structural_edit(now, |page| {
            page.add_block(&fresh, spec, Some(&parent_id), Some(slot as u64 + 1), now)?;
            if let Some(delta) = existing_delta.as_ref()
                && !delta.is_empty()
            {
                page.apply_text_delta(&fresh, "prop:text", delta, now)?;
            }
            if is_numbered {
                page.correct_numbered_order_from(&parent_id, slot + 1, 1)?;
            }
            Ok(())
        })?;
        if commit.is_some()
            && let Some(before) = before
        {
            page.set_undo_selection_snapshot(before)?;
        }
        if has_text {
            self.editor.selection = Some(DocumentSelection::caret(DocPoint::new(&fresh, 0)));
        }
        self.editor.last_typing = None;
        self.did_edit(commit, None, window, cx);
        Ok(())
    }

    /// Delete the currently focused block in one undoable `structural_edit`.
    /// When deleting the last remaining block of its parent, inserts a fresh empty paragraph.
    pub(super) fn delete_current_block(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocResult {
        let Some(sel) = self.editor.selection.clone() else {
            return Ok(());
        };
        let block_id = sel.focus.block_id.clone();
        let snapshot = self
            .snapshot
            .clone()
            .ok_or_else(|| DocError::InvalidState("missing page".into()))?;
        let Some((parent, slot)) = snapshot
            .root
            .as_ref()
            .and_then(|root| find_parent(root, &block_id))
        else {
            return Ok(());
        };
        let parent_id = parent.id.clone();
        let is_last_child = parent.children.len() <= 1;
        let fallback_id = if is_last_child {
            Some(self.fresh_block_id())
        } else {
            None
        };
        let next_focus_id = if let Some(ref fresh) = fallback_id {
            Some(fresh.clone())
        } else if slot > 0 {
            Some(parent.children[slot - 1].id.clone())
        } else {
            parent.children.get(slot + 1).map(|n| n.id.clone())
        };
        let now = self.now_ms(cx) as u64;
        let before = self.selection_snapshot();
        let page = self.page_mut()?;
        page.stop_capturing()?;
        let commit = page.structural_edit(now, |page| {
            if let Some(ref fresh) = fallback_id {
                page.add_block(
                    fresh,
                    BlockSpec::Paragraph {
                        paragraph_type: ParagraphType::Text,
                        text: String::new(),
                    },
                    Some(&parent_id),
                    Some(0),
                    now,
                )?;
            }
            page.delete_block_subtree(&parent_id, &block_id, now)?;
            Ok(())
        })?;
        if commit.is_some()
            && let Some(before) = before
        {
            page.set_undo_selection_snapshot(before)?;
        }
        self.editor.selection =
            next_focus_id.map(|id| DocumentSelection::caret(DocPoint::new(id, 0)));
        self.editor.last_typing = None;
        self.did_edit(commit, None, window, cx);
        Ok(())
    }

    fn insert_sibling_block(
        &mut self,
        spec: BlockSpec,
        child_paragraph: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocResult<Option<String>> {
        let Some(sel) = self.editor.selection.clone() else {
            return Ok(None);
        };
        let block_id = sel.focus.block_id.clone();
        let snapshot = self
            .snapshot
            .clone()
            .ok_or_else(|| DocError::InvalidState("missing page".into()))?;
        let Some((parent, slot)) = snapshot
            .root
            .as_ref()
            .and_then(|root| find_parent(root, &block_id))
        else {
            return Ok(None);
        };
        let parent_id = parent.id.clone();
        let fresh = self.fresh_block_id();
        let child_id = child_paragraph.then(|| self.fresh_block_id());
        let now = self.now_ms(cx) as u64;
        let before = self.selection_snapshot();
        let page = self.page_mut()?;
        page.stop_capturing()?;
        let commit = page.structural_edit(now, |page| {
            page.add_block(&fresh, spec, Some(&parent_id), Some(slot as u64 + 1), now)?;
            if let Some(ref cid) = child_id {
                page.add_block(
                    cid,
                    BlockSpec::Paragraph {
                        paragraph_type: ParagraphType::Text,
                        text: String::new(),
                    },
                    Some(&fresh),
                    Some(0),
                    now,
                )?;
            }
            Ok(())
        })?;
        if commit.is_some()
            && let Some(before) = before
        {
            page.set_undo_selection_snapshot(before)?;
        }
        if let Some(ref cid) = child_id {
            self.editor.selection = Some(DocumentSelection::caret(DocPoint::new(cid, 0)));
        }
        self.editor.last_typing = None;
        self.did_edit(commit, None, window, cx);
        Ok(Some(fresh))
    }

    pub(super) fn insert_equation_block_from_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocResult {
        if let Some(block_id) = self.insert_sibling_block(
            BlockSpec::Latex {
                latex: String::new(),
            },
            false,
            window,
            cx,
        )? {
            self.dispatch_overlay(
                OverlayEvent::OpenEquationEditor {
                    target: EquationTarget::Block { block_id },
                    initial: String::new(),
                },
                window,
                cx,
            );
        }
        Ok(())
    }

    pub(super) fn insert_table_block_from_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocResult {
        let _ = self.insert_sibling_block(
            BlockSpec::Table {
                rows: vec![
                    vec![String::new(), String::new()],
                    vec![String::new(), String::new()],
                ],
            },
            false,
            window,
            cx,
        )?;
        Ok(())
    }

    pub(super) fn insert_callout_block_from_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocResult {
        let _ = self.insert_sibling_block(
            BlockSpec::Callout {
                icon: "\u{1F4A1}".into(),
                background_color: None,
            },
            true,
            window,
            cx,
        )?;
        Ok(())
    }

    pub(super) fn insert_image_block_from_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocResult {
        let _ = self.insert_sibling_block(
            BlockSpec::Image {
                source_id: String::new(),
                caption: String::new(),
                width: 320.0,
                height: 180.0,
                xywh: None,
            },
            false,
            window,
            cx,
        )?;
        Ok(())
    }

    pub(super) fn insert_bookmark_block_from_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocResult {
        let _ = self.insert_sibling_block(
            BlockSpec::Bookmark {
                url: "https://affine.pro".into(),
                title: Some("Link".into()),
                description: None,
            },
            false,
            window,
            cx,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_date_formatter_matches_0275_formats() {
        // 2026-11-26T14:05:00Z = 1795701900000 ms
        let ts = 1_795_701_900_000_i64;
        assert_eq!(
            format_keyboard_toolbar_date(ts, KeyboardToolbarDateKind::Today),
            "2026-11-26"
        );
        assert_eq!(
            format_keyboard_toolbar_date(ts, KeyboardToolbarDateKind::Tomorrow),
            "2026-11-27"
        );
        assert_eq!(
            format_keyboard_toolbar_date(ts, KeyboardToolbarDateKind::Yesterday),
            "2026-11-25"
        );
        assert_eq!(
            format_keyboard_toolbar_date(ts, KeyboardToolbarDateKind::Now),
            "11-26 14:05"
        );
    }
}
