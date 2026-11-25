use std::collections::BTreeMap;

use serde_json::Value as JsonValue;
use y_octo::{Any, SessionReadValue, TypeTarget};

use crate::DocResult;

use super::{RichText, SnapshotReader, number, string};

/// One row descriptor inside an `affine:table` block.
#[derive(Debug, Clone, PartialEq)]
pub struct TableRow {
    pub row_id: String,
    pub order: String,
    pub background_color: Option<String>,
}

/// One column descriptor inside an `affine:table` block.
#[derive(Debug, Clone, PartialEq)]
pub struct TableColumn {
    pub column_id: String,
    pub order: String,
    pub width: Option<f64>,
    pub background_color: Option<String>,
}

/// One cell value inside an `affine:table` block, keyed by `"{row_id}:{column_id}"`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableCell {
    pub text: RichText,
}

/// Typed `affine:table` content read from `prop:rows`, `prop:columns`, and `prop:cells`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableData {
    pub rows: Vec<TableRow>,
    pub columns: Vec<TableColumn>,
    pub cells: BTreeMap<(String, String), TableCell>,
}

impl SnapshotReader<'_> {
    pub(super) fn callout_icon(
        &self,
        entries: &BTreeMap<String, SessionReadValue>,
        props: &BTreeMap<String, JsonValue>,
    ) -> String {
        if let Some(emoji) = string(entries.get("prop:emoji")).filter(|s| !s.is_empty()) {
            return emoji;
        }
        if let Some(icon) = props.get("prop:icon") {
            if let Some(s) = icon.as_str()
                && !s.is_empty()
            {
                return s.to_string();
            }
            if let Some(unicode) = icon.get("unicode").and_then(JsonValue::as_str)
                && !unicode.is_empty()
            {
                return unicode.to_string();
            }
            if let Some(name) = icon.get("name").and_then(JsonValue::as_str)
                && !name.is_empty()
            {
                return name.to_string();
            }
        }
        "\u{1F4A1}".to_string()
    }

    pub(super) fn callout_background(props: &BTreeMap<String, JsonValue>) -> Option<String> {
        let val = props.get("prop:backgroundColor")?;
        if let Some(s) = val.as_str()
            && !s.is_empty()
        {
            return Some(s.to_string());
        }
        val.get("light")
            .or_else(|| val.get("normal"))
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    pub(super) fn read_table(
        &self,
        entries: &BTreeMap<String, SessionReadValue>,
    ) -> DocResult<TableData> {
        let session = self.page.inner();
        let mut rows = Vec::new();
        if let Some(SessionReadValue::Map(rows_id)) = entries.get("prop:rows") {
            for (key, value) in session.read_map(&TypeTarget::Identity(rows_id.clone()))? {
                let SessionReadValue::Map(row_map) = value else {
                    continue;
                };
                let target = TypeTarget::Identity(row_map);
                let row_id = string(session.get_map_value(&target, "rowId")?.as_ref())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(key);
                let order =
                    string(session.get_map_value(&target, "order")?.as_ref()).unwrap_or_default();
                let background_color =
                    string(session.get_map_value(&target, "backgroundColor")?.as_ref());
                rows.push(TableRow {
                    row_id,
                    order,
                    background_color,
                });
            }
        }
        rows.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.row_id.cmp(&b.row_id)));

        let mut columns = Vec::new();
        if let Some(SessionReadValue::Map(cols_id)) = entries.get("prop:columns") {
            for (key, value) in session.read_map(&TypeTarget::Identity(cols_id.clone()))? {
                let SessionReadValue::Map(col_map) = value else {
                    continue;
                };
                let target = TypeTarget::Identity(col_map);
                let column_id = string(session.get_map_value(&target, "columnId")?.as_ref())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(key);
                let order =
                    string(session.get_map_value(&target, "order")?.as_ref()).unwrap_or_default();
                let width =
                    number(session.get_map_value(&target, "width")?.as_ref()).filter(|w| *w > 0.0);
                let background_color =
                    string(session.get_map_value(&target, "backgroundColor")?.as_ref());
                columns.push(TableColumn {
                    column_id,
                    order,
                    width,
                    background_color,
                });
            }
        }
        columns.sort_by(|a, b| {
            a.order
                .cmp(&b.order)
                .then_with(|| a.column_id.cmp(&b.column_id))
        });

        let mut cells = BTreeMap::new();
        if let Some(SessionReadValue::Map(cells_id)) = entries.get("prop:cells") {
            for (key, value) in session.read_map(&TypeTarget::Identity(cells_id.clone()))? {
                let Some((row_id, col_id)) = key.split_once(':') else {
                    continue;
                };
                let text = match value {
                    SessionReadValue::Map(cell_map) => {
                        let target = TypeTarget::Identity(cell_map);
                        let field = session.get_map_value(&target, "text")?;
                        self.rich_text(field.as_ref())?
                    }
                    SessionReadValue::Any(Any::String(s)) => {
                        self.rich_text(Some(&SessionReadValue::Any(Any::String(s))))?
                    }
                    _ => RichText::default(),
                };
                cells.insert((row_id.to_string(), col_id.to_string()), TableCell { text });
            }
        }

        Ok(TableData {
            rows,
            columns,
            cells,
        })
    }
}

#[cfg(test)]
mod tests {
    use y_octo::{Any, SessionValue};

    use crate::{
        DocResult,
        read::BlockContent,
        schema::{BlockSpec, ParagraphType},
        session::PageDocSession,
    };

    #[test]
    fn seven_rich_blocks_round_trip_with_zero_writeback_and_table_undo() -> DocResult {
        let mut page = PageDocSession::new("doc:rich-7", 42)?;
        page.add_block(
            "page-1",
            BlockSpec::Page {
                title: "Rich Blocks".into(),
            },
            None,
            None,
            1,
        )?;
        page.add_block(
            "note-1",
            BlockSpec::Note { xywh: None },
            Some("page-1"),
            None,
            2,
        )?;
        page.add_block(
            "callout-1",
            BlockSpec::Callout {
                icon: "\u{1F4A1}".into(),
                background_color: Some("var(--affine-tag-blue)".into()),
            },
            Some("note-1"),
            None,
            3,
        )?;
        page.add_block(
            "callout-child",
            BlockSpec::Paragraph {
                paragraph_type: ParagraphType::Text,
                text: "Inside callout".into(),
            },
            Some("callout-1"),
            None,
            4,
        )?;
        page.add_block(
            "table-1",
            BlockSpec::Table {
                rows: vec![
                    vec!["Header A".into(), "Header B".into()],
                    vec!["Cell 1".into(), "Cell 2".into()],
                ],
            },
            Some("note-1"),
            None,
            5,
        )?;
        page.add_block(
            "attach-1",
            BlockSpec::Attachment {
                name: "spec.pdf".into(),
                size: 4096,
                mime_type: "application/pdf".into(),
                source_id: "blob-pdf-1".into(),
                caption: "Architecture PDF".into(),
                embed: true,
            },
            Some("note-1"),
            None,
            6,
        )?;
        page.add_block(
            "latex-1",
            BlockSpec::Latex {
                latex: r"\frac{-b \pm \sqrt{b^2-4ac}}{2a}".into(),
            },
            Some("note-1"),
            None,
            7,
        )?;
        page.add_block(
            "embed-link-1",
            BlockSpec::EmbedLinkedDoc {
                page_id: "doc:target".into(),
                style: "horizontal".into(),
                caption: String::new(),
            },
            Some("note-1"),
            None,
            8,
        )?;
        page.add_block(
            "embed-sync-1",
            BlockSpec::EmbedSyncedDoc {
                page_id: "doc:synced".into(),
                caption: "Synced note".into(),
            },
            Some("note-1"),
            None,
            9,
        )?;
        page.add_block(
            "surface-ref-1",
            BlockSpec::SurfaceRef {
                reference: "frame-1".into(),
                ref_flavour: "affine:frame".into(),
                caption: "Overview frame".into(),
            },
            Some("note-1"),
            None,
            10,
        )?;
        // Attach an unknown future property to verify 100% lossless preservation.
        page.set_block_prop(
            "callout-1",
            "prop:futureCustomField",
            SessionValue::Any(Any::String("keep-me".into())),
            11,
        )?;

        let mut reader = PageDocSession::new("doc:rich-7", 99)?;
        reader.apply_remote_update_v1(&page.encode_update_v1()?)?;
        let before_read = reader.encode_update_v1()?;
        let snap = reader.read_page_snapshot()?;
        let after_read = reader.encode_update_v1()?;
        assert_eq!(
            before_read, after_read,
            "reading 7 rich blocks must cause 0 writeback"
        );

        let callout = snap.block("callout-1").expect("callout");
        assert!(matches!(callout.content, BlockContent::Callout { .. }));
        assert_eq!(
            callout
                .props
                .get("prop:futureCustomField")
                .and_then(|v| v.as_str()),
            Some("keep-me")
        );
        assert_eq!(callout.children.len(), 1);

        let table = snap.block("table-1").expect("table");
        let BlockContent::Table(data) = &table.content else {
            panic!("expected table");
        };
        assert_eq!(data.rows.len(), 2);
        assert_eq!(data.columns.len(), 2);
        assert_eq!(
            data.cells[&("row-1".into(), "col-0".into())].text.plain,
            "Cell 1"
        );

        assert!(matches!(
            snap.block("attach-1").unwrap().content,
            BlockContent::Attachment { embed: true, .. }
        ));
        assert!(matches!(
            snap.block("latex-1").unwrap().content,
            BlockContent::Latex { .. }
        ));
        assert!(matches!(
            snap.block("embed-link-1").unwrap().content,
            BlockContent::EmbedLinkedDoc { .. }
        ));
        assert!(matches!(
            snap.block("embed-sync-1").unwrap().content,
            BlockContent::EmbedSyncedDoc { .. }
        ));
        assert!(matches!(
            snap.block("surface-ref-1").unwrap().content,
            BlockContent::SurfaceRef { .. }
        ));

        // Table cell edit and single-step undo.
        page.set_table_cell_text("table-1", "row-1", "col-0", "Updated cell", 100)?;
        let edited = page.read_page_snapshot()?;
        let BlockContent::Table(t_after) = &edited.block("table-1").unwrap().content else {
            panic!("expected table");
        };
        assert_eq!(
            t_after.cells[&("row-1".into(), "col-0".into())].text.plain,
            "Updated cell"
        );

        assert!(page.undo()?.is_some());
        let undone = page.read_page_snapshot()?;
        let BlockContent::Table(t_undone) = &undone.block("table-1").unwrap().content else {
            panic!("expected table");
        };
        assert_eq!(
            t_undone.cells[&("row-1".into(), "col-0".into())].text.plain,
            "Cell 1"
        );
        Ok(())
    }
}
