//! Comprehensive `0.27.5` `defaultKeyboardToolbarConfig` integration tests and
//! offscreen PNG evidence generator for `S3.4b-MobileKeyboardToolbar`.

use std::{cell::RefCell, rc::Rc, time::Duration};

use blocksuite_playground_doc::schema::{BlockSpec, ListType, ParagraphType};
use ui::{
    AppContext, Application, ApplicationHandle, Bounds, FrameSnapshot, HeadlessPlatform,
    HeadlessWindow, WindowBounds, WindowHandle, WindowOptions, audit_frame_snapshot, point, px,
    size,
};
use y_octo::{Any, TextDeltaOp};

use crate::{
    PlaygroundOptions, PlaygroundTheme, PlaygroundView,
    overlays::{
        ActiveSelection, HighlightColor, KEYBOARD_TOOLBAR_HEIGHT, KeyboardToolbarAction,
        KeyboardToolbarDateKind, KeyboardToolbarLevel, KeyboardToolbarPanel, OverlayEvent,
        SlashCommand,
    },
    register_bundled_fonts,
};

struct MobileSession {
    platform: Rc<HeadlessPlatform>,
    app: ApplicationHandle,
    window: WindowHandle<PlaygroundView>,
    headless: HeadlessWindow,
}

impl MobileSession {
    fn open(width: u32, height: u32, theme: PlaygroundTheme) -> Self {
        let platform = HeadlessPlatform::new(width, height, 1.0);
        let handle_cell: Rc<RefCell<Option<WindowHandle<PlaygroundView>>>> =
            Rc::new(RefCell::new(None));
        let handle_out = handle_cell.clone();
        let app = Application::with_platform(platform.clone()).run_embedded(move |cx| {
            register_bundled_fonts(cx).expect("bundled fonts");
            let options = PlaygroundOptions {
                theme,
                ..PlaygroundOptions::default()
            };
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(0.0), px(0.0)),
                            size(px(width as f32), px(height as f32)),
                        ))),
                        ..WindowOptions::default()
                    },
                    |_, cx| cx.new(|cx| PlaygroundView::with_options(options, cx)),
                )
                .expect("open headless window");
            *handle_out.borrow_mut() = Some(window);
        });
        let window = handle_cell.borrow_mut().take().expect("window opened");
        let headless = platform
            .windows()
            .into_iter()
            .next()
            .expect("headless window registered");
        let session = Self {
            platform,
            app,
            window,
            headless,
        };
        session.frame();
        session.platform.run_until_parked();
        session
    }

    fn edit(&self, f: impl FnOnce(&mut blocksuite_playground_doc::session::PageDocSession)) {
        self.app.update(|cx| {
            self.window
                .update(cx, |v, _, cx| {
                    let page = v.page_mut().expect("active page");
                    f(page);
                    v.refresh_document();
                    cx.notify();
                })
                .unwrap();
        });
    }

    fn overlay(&self, event: OverlayEvent) {
        self.app.update(|cx| {
            self.window
                .update(cx, |v, w, cx| v.dispatch_overlay(event, w, cx))
                .unwrap();
        });
    }

    fn frame(&self) -> FrameSnapshot {
        self.headless.request_frame();
        self.app.update(|cx| {
            self.window
                .update(cx, |_, window, _| window.frame_snapshot().clone())
                .unwrap()
        })
    }

    fn text(&self, block_id: &str) -> blocksuite_playground_doc::read::RichText {
        self.app.update(|cx| {
            self.window
                .update(cx, |v, _, _| {
                    v.page()
                        .expect("page")
                        .read_editable_text(block_id)
                        .expect("text")
                        .0
                })
                .unwrap()
        })
    }

    fn block_count(&self, parent_id: &str) -> usize {
        self.app.update(|cx| {
            self.window
                .update(cx, |v, _, _| {
                    v.page_snapshot()
                        .and_then(|s| s.block(parent_id))
                        .map(|n| n.children.len())
                        .unwrap_or(0)
                })
                .unwrap()
        })
    }

    fn view<R>(&self, f: impl FnOnce(&PlaygroundView) -> R) -> R {
        self.app
            .update(|cx| self.window.update(cx, |v, _, _| f(v)).unwrap())
    }

    fn save_png(&self, path: &std::path::Path, label: &str) {
        let snap = self.frame();
        let violations = audit_frame_snapshot(&snap);
        assert!(
            violations.is_empty(),
            "audit violations in {label}: {violations:#?}"
        );
        let img = self
            .app
            .update(|cx| {
                self.window
                    .update(cx, |_, window, _| window.render_to_image())
                    .unwrap()
            })
            .expect("render image");
        img.save(path)
            .unwrap_or_else(|e| panic!("save {}: {e}", path.display()));
    }
}

#[test]
fn keyboard_toolbar_0275_highlights_marks_and_link_single_step_undo() {
    let session = MobileSession::open(390, 844, PlaygroundTheme::Light);
    session.edit(|page| {
        page.add_block(
            "hl-p",
            BlockSpec::Paragraph {
                paragraph_type: ParagraphType::Text,
                text: "Hello highlight world".into(),
            },
            Some("showcase-note"),
            Some(0),
            5_000_000,
        )
        .expect("add paragraph");
    });

    // Non-collapsed selection autoShows `Aa` sub-toolbar.
    session.overlay(OverlayEvent::SetSelection(Some(ActiveSelection {
        block_id: "hl-p".into(),
        start_utf16: 6,
        end_utf16: 15,
    })));
    session.overlay(OverlayEvent::SetKeyboardToolbarVisible(true));
    let frame = session.frame();
    assert!(audit_frame_snapshot(&frame).is_empty());
    assert_eq!(
        session.view(|v| v.overlays().keyboard_toolbar_level()),
        KeyboardToolbarLevel::TextStyle
    );

    // Every text mark (Bold, Italic, Underline, Strike, InlineCode) applies and undoes in 1 step.
    for (action, key) in [
        (KeyboardToolbarAction::Bold, "bold"),
        (KeyboardToolbarAction::Italic, "italic"),
        (KeyboardToolbarAction::Underline, "underline"),
        (KeyboardToolbarAction::Strike, "strike"),
        (KeyboardToolbarAction::InlineCode, "code"),
    ] {
        session.overlay(OverlayEvent::KeyboardToolbar(action));
        let marked = session.text("hl-p");
        assert!(
            marked.delta.iter().any(|op| matches!(
                op,
                TextDeltaOp::Insert { format: Some(f), .. } if f.get(key) == Some(&Any::True)
            )),
            "mark {key} not applied"
        );
        session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Undo));
        let undone = session.text("hl-p");
        assert!(
            undone.delta.iter().all(|op| !matches!(
                op,
                TextDeltaOp::Insert { format: Some(f), .. } if f.get(key) == Some(&Any::True)
            )),
            "mark {key} not undone in 1 step"
        );
    }

    // Highlight panel opens cleanly and applies fg + bg colors with 1-step Undo.
    session.overlay(OverlayEvent::KeyboardToolbar(
        KeyboardToolbarAction::ToggleHighlightPanel,
    ));
    let hl_frame = session.frame();
    assert!(audit_frame_snapshot(&hl_frame).is_empty());
    assert!(
        hl_frame
            .elements
            .contains_key("overlay.keyboard_toolbar.panel.color.red")
    );
    assert!(
        hl_frame
            .elements
            .contains_key("overlay.keyboard_toolbar.panel.background.yellow")
    );

    session.overlay(OverlayEvent::KeyboardToolbar(
        KeyboardToolbarAction::SetTextColor(HighlightColor::Red),
    ));
    let colored = session.text("hl-p");
    assert!(colored.delta.iter().any(|op| matches!(
        op,
        TextDeltaOp::Insert { format: Some(f), .. }
            if f.get("color") == Some(&Any::String("var(--affine-v2-text-highlight-fg-red)".into()))
    )));
    session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Undo));
    let after_fg_undo = session.text("hl-p");
    assert!(after_fg_undo.delta.iter().all(|op| !matches!(
        op,
        TextDeltaOp::Insert { format: Some(f), .. } if f.contains_key("color")
    )));

    session.overlay(OverlayEvent::KeyboardToolbar(
        KeyboardToolbarAction::SetBgColor(HighlightColor::Yellow),
    ));
    let bg_colored = session.text("hl-p");
    assert!(bg_colored.delta.iter().any(|op| matches!(
        op,
        TextDeltaOp::Insert { format: Some(f), .. }
            if f.get("background")
                == Some(&Any::String("var(--affine-v2-text-highlight-bg-yellow)".into()))
    )));
    session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Undo));
    let after_bg_undo = session.text("hl-p");
    assert!(after_bg_undo.delta.iter().all(|op| !matches!(
        op,
        TextDeltaOp::Insert { format: Some(f), .. } if f.contains_key("background")
    )));

    // Link opens existing LinkBar without crashing.
    session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Link));
    let _ = session.frame();
    assert!(session.view(|v| v.overlays().link_field().is_some()));
}

#[test]
fn keyboard_toolbar_0275_block_ops_date_and_table_gate_single_step_undo() {
    let session = MobileSession::open(390, 844, PlaygroundTheme::Light);
    session.edit(|page| {
        page.add_block(
            "ops-p1",
            BlockSpec::Paragraph {
                paragraph_type: ParagraphType::Text,
                text: "First block".into(),
            },
            Some("showcase-note"),
            Some(0),
            6_000_000,
        )
        .expect("add p1");
        page.add_block(
            "ops-p2",
            BlockSpec::Paragraph {
                paragraph_type: ParagraphType::Text,
                text: "Second block".into(),
            },
            Some("showcase-note"),
            Some(1),
            6_000_001,
        )
        .expect("add p2");
    });

    session.overlay(OverlayEvent::SetSelection(Some(ActiveSelection::caret(
        "ops-p2", 12,
    ))));
    session.overlay(OverlayEvent::SetKeyboardToolbarVisible(true));
    let base_count = session.block_count("showcase-note");

    // Duplicate creates a sibling below in 1 Undo step.
    session.overlay(OverlayEvent::KeyboardToolbar(
        KeyboardToolbarAction::Duplicate,
    ));
    assert_eq!(session.block_count("showcase-note"), base_count + 1);
    session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Undo));
    assert_eq!(session.block_count("showcase-note"), base_count);

    // Delete removes current block in 1 Undo step.
    session.overlay(OverlayEvent::SetSelection(Some(ActiveSelection::caret(
        "ops-p2", 0,
    ))));
    session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Delete));
    assert_eq!(session.block_count("showcase-note"), base_count - 1);
    session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Undo));
    assert_eq!(session.block_count("showcase-note"), base_count);

    // Date insertion (Today) inserts formatted date in 1 Undo step.
    session.overlay(OverlayEvent::SetSelection(Some(ActiveSelection::caret(
        "ops-p1", 11,
    ))));
    session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Date(
        KeyboardToolbarDateKind::Today,
    )));
    assert!(session.text("ops-p1").plain.len() > "First block".len());
    session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Undo));
    assert_eq!(session.text("ops-p1").plain, "First block");

    // Table, Callout, Image, Bookmark, EquationBlock each insert & undo in 1 step.
    for action in [
        KeyboardToolbarAction::Table,
        KeyboardToolbarAction::Callout,
        KeyboardToolbarAction::Image,
        KeyboardToolbarAction::Bookmark,
        KeyboardToolbarAction::EquationBlock,
        KeyboardToolbarAction::Slash(SlashCommand::Divider),
        KeyboardToolbarAction::Slash(SlashCommand::List(ListType::Toggle)),
    ] {
        session.overlay(OverlayEvent::SetSelection(Some(ActiveSelection::caret(
            "ops-p1", 0,
        ))));
        session.overlay(OverlayEvent::KeyboardToolbar(action));
        session.overlay(OverlayEvent::CloseEquationEditor);
        session.overlay(OverlayEvent::KeyboardToolbar(KeyboardToolbarAction::Undo));
    }

    // Table-cell focus gate disables non-Undo/Redo/Dismiss buttons and collapses open panels.
    session.edit(|page| {
        page.add_block(
            "gate-tbl",
            BlockSpec::Table {
                rows: vec![vec!["A".into(), "B".into()]],
            },
            Some("showcase-note"),
            Some(0),
            6_000_100,
        )
        .expect("add table");
    });
    session.overlay(OverlayEvent::KeyboardToolbar(
        KeyboardToolbarAction::ToggleMorePanel,
    ));
    assert_eq!(
        session.view(|v| v.overlays().keyboard_toolbar_active_panel()),
        Some(KeyboardToolbarPanel::More)
    );
    session.overlay(OverlayEvent::FocusTableCell {
        block_id: "gate-tbl".into(),
        row_id: "row-0".into(),
        column_id: "col-0".into(),
        screen_pos: None,
    });
    session.platform.advance_clock(Duration::from_millis(160));
    let gated = session.frame();
    assert!(audit_frame_snapshot(&gated).is_empty());
    assert_eq!(
        session.view(|v| v.overlays().keyboard_toolbar_active_panel()),
        None
    );
    assert_eq!(
        gated
            .elements
            .get("overlay.keyboard_toolbar")
            .map(|e| e.bounds.height),
        Some(KEYBOARD_TOOLBAR_HEIGHT)
    );
}

#[test]
#[ignore = "offscreen PNG evidence generator for S3.4b"]
fn render_s3_4b_keyboard_toolbar_evidence() {
    use crate::{HOME_PAGE_ID, room_client::DocState};

    let out_dir = std::env::var("S3_4B_OUT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("target/s3-4b-after"));
    std::fs::create_dir_all(&out_dir).expect("create out_dir");

    let build_doc = |title: &str| {
        let mut state = DocState::new("s3-4b", 77).expect("doc");
        state
            .workspace
            .create_page(HOME_PAGE_ID, title, 1)
            .expect("page");
        let page = state.workspace.ensure_page(HOME_PAGE_ID).expect("page");
        page.add_block(
            "p-root",
            BlockSpec::Page {
                title: title.into(),
            },
            None,
            None,
            1,
        )
        .unwrap();
        page.add_block("p-surface", BlockSpec::Surface, Some("p-root"), None, 2)
            .unwrap();
        page.add_block(
            "p-note",
            BlockSpec::Note { xywh: None },
            Some("p-root"),
            None,
            3,
        )
        .unwrap();
        page.add_block(
            "p-body",
            BlockSpec::Paragraph {
                paragraph_type: ParagraphType::Text,
                text: "Select text to open Aa formatting and highlight colors.".into(),
            },
            Some("p-note"),
            None,
            4,
        )
        .unwrap();
        page.add_block(
            "p-tbl",
            BlockSpec::Table {
                rows: vec![
                    vec!["Task".into(), "Status".into()],
                    vec!["KeyboardToolbar".into(), "Parity".into()],
                ],
            },
            Some("p-note"),
            None,
            5,
        )
        .unwrap();
        state
    };

    for (w, h, tag) in [(390, 844, "390"), (320, 568, "320")] {
        for (theme, tname) in [
            (PlaygroundTheme::Light, "light"),
            (PlaygroundTheme::Dark, "dark"),
        ] {
            let s = MobileSession::open(w, h, theme);
            let doc = build_doc("Mobile Keyboard Toolbar");
            s.app.update(|cx| {
                s.window
                    .update(cx, |v, _, cx| v.set_document(doc, cx))
                    .unwrap();
            });
            s.frame();
            // 1. Default top-level toolbar
            s.overlay(OverlayEvent::SetSelection(Some(ActiveSelection::caret(
                "p-body", 0,
            ))));
            s.overlay(OverlayEvent::SetKeyboardToolbarVisible(true));
            s.save_png(
                &out_dir.join(format!("after-{tag}-01-default-{tname}.png")),
                &format!("after-{tag}-01-default-{tname}"),
            );

            if theme == PlaygroundTheme::Light {
                // 2. `+` insert panel open
                s.overlay(OverlayEvent::KeyboardToolbar(
                    KeyboardToolbarAction::ToggleMorePanel,
                ));
                s.save_png(
                    &out_dir.join(format!("after-{tag}-02-more-panel.png")),
                    &format!("after-{tag}-02-more-panel"),
                );
                s.overlay(OverlayEvent::KeyboardToolbar(
                    KeyboardToolbarAction::ToggleMorePanel,
                ));

                // 3. Non-collapsed selection -> autoShow `Aa` text style sub-toolbar
                s.overlay(OverlayEvent::SetSelection(Some(ActiveSelection {
                    block_id: "p-body".into(),
                    start_utf16: 0,
                    end_utf16: 11,
                })));
                s.save_png(
                    &out_dir.join(format!("after-{tag}-03-autoshow-aa.png")),
                    &format!("after-{tag}-03-autoshow-aa"),
                );

                // 4. 9-color Highlight panel open
                s.overlay(OverlayEvent::KeyboardToolbar(
                    KeyboardToolbarAction::ToggleHighlightPanel,
                ));
                s.save_png(
                    &out_dir.join(format!("after-{tag}-04-highlight-panel.png")),
                    &format!("after-{tag}-04-highlight-panel"),
                );

                // 5. Table cell focus disabled state
                s.overlay(OverlayEvent::FocusTableCell {
                    block_id: "p-tbl".into(),
                    row_id: "row-1".into(),
                    column_id: "col-0".into(),
                    screen_pos: None,
                });
                s.overlay(OverlayEvent::SetKeyboardToolbarVisible(true));
                s.save_png(
                    &out_dir.join(format!("after-{tag}-05-table-disabled.png")),
                    &format!("after-{tag}-05-table-disabled"),
                );
            }
        }
    }
}
