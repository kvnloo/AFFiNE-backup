//! Mobile `KeyboardToolbar`, long-press ICU word selection, embedded PDF paging,
//! table cell editing, and embedded doc navigation handlers for `PlaygroundView`.

use blocksuite_playground_doc::schema::{ListType, ParagraphType};
use ui::{Context, Window, point, px};
use y_octo::{Any, TextAttributes, TextDeltaOp, TextInsert};

use crate::{
    PlaygroundView,
    block_flow::{parse_pdf_renderer, render_pdf_page},
    editor_session::{DocPoint, DocumentSelection, word_range},
};

use super::*;

impl PlaygroundView {
    /// Handles mobile keyboard toolbar, PDF paging, table cell edit, embed doc open,
    /// and touch long-press word selection events. Returns `true` when consumed.
    pub(super) fn handle_mobile_and_rich_event(
        &mut self,
        event: &OverlayEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            OverlayEvent::SetKeyboardToolbarVisible(visible) => {
                self.overlays.set_keyboard_toolbar_visible(*visible);
                cx.notify();
                true
            }
            OverlayEvent::KeyboardToolbar(action) => {
                self.run_keyboard_toolbar_action(*action, window, cx);
                true
            }
            OverlayEvent::AttachmentPdfPage { block_id, delta } => {
                self.step_attachment_pdf_page(block_id, *delta, cx);
                true
            }
            OverlayEvent::OpenEmbedDoc { page_id } => {
                self.last_opened_embed_doc = Some(page_id.clone());
                cx.notify();
                true
            }
            OverlayEvent::EditTableCell {
                block_id,
                row_id,
                column_id,
                text,
            } => {
                let now = self.now_ms(cx) as u64;
                if let Ok(page) = self.page_mut()
                    && let Ok(commit) =
                        page.set_table_cell_text(block_id, row_id, column_id, text, now)
                {
                    self.editor.last_typing = None;
                    self.did_edit(commit, None, window, cx);
                }
                true
            }
            OverlayEvent::LongPressWord { x, y } => {
                self.long_press_word(*x, *y, window, cx);
                true
            }
            _ => false,
        }
    }

    fn run_keyboard_toolbar_action(
        &mut self,
        action: KeyboardToolbarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let block_id = self
            .editor
            .selection
            .as_ref()
            .map(|sel| sel.focus.block_id.clone());
        match action {
            KeyboardToolbarAction::Undo => {
                let _ = self.history(false, window, cx);
            }
            KeyboardToolbarAction::Redo => {
                let _ = self.history(true, window, cx);
            }
            KeyboardToolbarAction::Bold => {
                let _ = self.format_mark("bold", window, cx);
            }
            KeyboardToolbarAction::Italic => {
                let _ = self.format_mark("italic", window, cx);
            }
            KeyboardToolbarAction::Underline => {
                let _ = self.format_mark("underline", window, cx);
            }
            KeyboardToolbarAction::InlineCode => {
                let _ = self.format_mark("code", window, cx);
            }
            KeyboardToolbarAction::InlineMath => {
                if let Some(sel) = self.editor.selection.clone() {
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
                        if let Ok(commit) =
                            page.apply_text_delta(&sel.focus.block_id, "prop:text", &delta, now)
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
            }
            KeyboardToolbarAction::H1
            | KeyboardToolbarAction::H2
            | KeyboardToolbarAction::H3
            | KeyboardToolbarAction::Todo
            | KeyboardToolbarAction::Bullet
            | KeyboardToolbarAction::Numbered
            | KeyboardToolbarAction::Quote
            | KeyboardToolbarAction::CodeBlock => {
                let Some(id) = block_id else {
                    return;
                };
                let cmd = match action {
                    KeyboardToolbarAction::H1 => SlashCommand::Paragraph(ParagraphType::H1),
                    KeyboardToolbarAction::H2 => SlashCommand::Paragraph(ParagraphType::H2),
                    KeyboardToolbarAction::H3 => SlashCommand::Paragraph(ParagraphType::H3),
                    KeyboardToolbarAction::Todo => SlashCommand::List(ListType::Todo),
                    KeyboardToolbarAction::Bullet => SlashCommand::List(ListType::Bulleted),
                    KeyboardToolbarAction::Numbered => SlashCommand::List(ListType::Numbered),
                    KeyboardToolbarAction::Quote => SlashCommand::Paragraph(ParagraphType::Quote),
                    _ => SlashCommand::Code,
                };
                let _ = self.run_slash_command(&id, cmd, false, window, cx);
            }
            KeyboardToolbarAction::Indent => {
                let _ = self.edit_key("tab", false, window, cx);
            }
            KeyboardToolbarAction::Outdent => {
                let _ = self.edit_key("tab", true, window, cx);
            }
            KeyboardToolbarAction::Mention => {
                if let Some(sel) = self.editor.selection.clone() {
                    let _ = self.run_linked_doc_slash_command(
                        &sel.focus.block_id,
                        sel.focus.offset(),
                        None,
                        window,
                        cx,
                    );
                }
            }
            KeyboardToolbarAction::Dismiss => {
                self.overlays.set_keyboard_toolbar_visible(false);
                cx.notify();
            }
        }
    }

    fn step_attachment_pdf_page(&mut self, block_id: &str, delta: i32, cx: &mut Context<Self>) {
        let Some(source_id) = self.snapshot.as_ref().and_then(|snap| {
            snap.block(block_id).and_then(|node| match &node.content {
                blocksuite_playground_doc::read::BlockContent::Attachment { source_id, .. } => {
                    Some(source_id.clone())
                }
                _ => None,
            })
        }) else {
            return;
        };
        let Some(bytes) = self.state.blobs.get(&source_id) else {
            return;
        };
        let Some(renderer) = parse_pdf_renderer(bytes) else {
            return;
        };
        let total = blocksuite_editor::block_flow::render_pdf_page(&renderer, 0, 1.0)
            .map_or(0, |p| p.page_count);
        if total == 0 {
            return;
        }
        let current = self.pdf_page_indices.get(block_id).copied().unwrap_or(0) as i64;
        let next = (current + i64::from(delta)).clamp(0, (total - 1) as i64) as usize;
        if let Some(preview) = render_pdf_page(&renderer, next, 1.5) {
            self.pdf_page_indices
                .insert(block_id.to_string(), preview.page_index);
            self.pdf_previews.insert(block_id.to_string(), preview);
            self.env_rev += 1;
            cx.notify();
        }
    }

    /// Touch long-press at window coordinates `(x, y)`: selects the ICU word at that point.
    pub fn long_press_word(&mut self, x: f32, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.input_point(point(px(x), px(y)), window) else {
            return;
        };
        let text = self.block_text(&p.block_id).unwrap_or("");
        let range = word_range(text, p.offset());
        self.editor.dragging = false;
        self.editor_focus.focus(window, cx);
        self.change_selection(
            DocumentSelection {
                anchor: DocPoint::new(&p.block_id, range.start),
                focus: DocPoint::new(&p.block_id, range.end),
            },
            window,
            cx,
        );
    }
}
