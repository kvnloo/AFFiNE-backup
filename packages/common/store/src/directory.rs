use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    error::{StoreError, StoreResult},
    workspace_db::{DocPropertiesRow, WorkspaceDb, WorkspaceDbTableName},
};

/// Canonical document identifier exposed across the narrow `WorkspaceDocDirectory` seam.
/// Never carries `db$`, `userdata$`, legacy subdoc GUIDs, or network ACK metadata.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct DocId(String);

impl DocId {
    pub fn new(id: impl Into<String>) -> StoreResult<Self> {
        let id = id.into();
        if id.is_empty() || id.starts_with("db$") || id.starts_with("userdata$") {
            return Err(StoreError::InvalidProtocol(format!(
                "invalid canonical DocId: {id:?}"
            )));
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Primary view mode of a document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocMode {
    Page,
    Edgeless,
}

impl DocMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Edgeless => "edgeless",
        }
    }

    pub fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("edgeless") => Self::Edgeless,
            _ => Self::Page,
        }
    }
}

/// Projected directory entry for a workspace document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocDirectoryEntry {
    pub id: DocId,
    pub title: String,
    pub mode: DocMode,
    pub create_date_ms: i64,
    pub updated_date_ms: Option<i64>,
    pub trashed: bool,
    pub tags: Vec<String>,
}

/// Deterministic cancellation handle for directory queries.
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Query filter and pagination parameters for `WorkspaceDocDirectory::query_docs`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocQuery {
    /// Optional case-insensitive title prefix filter.
    pub title_prefix: Option<String>,
    /// Optional case-insensitive exact substring filter (mirrors `fuse.js` `threshold: 0`).
    pub title_contains: Option<String>,
    /// Whether to include soft-deleted (trashed) documents.
    pub include_trashed: bool,
    pub offset: usize,
    pub limit: usize,
}

/// Paginated query response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocQueryPage {
    pub entries: Vec<DocDirectoryEntry>,
    pub total: usize,
    pub has_more: bool,
}

/// Command to create a new linked document (for `@` mention or Database row-to-doc).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateLinkedDocRequest {
    pub op_id: String,
    pub doc_id: DocId,
    pub title: String,
    pub mode: DocMode,
    pub now_ms: i64,
}

/// Outcome of `WorkspaceDocDirectory::create_linked_doc`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateLinkedDocResult {
    pub doc_id: DocId,
    pub created_new: bool,
}

/// Narrow decoupling port consumed by the editor (`blocksuite/crates/editor`) for document
/// listing, linked-doc creation, opening, and committed title synchronization.
pub trait WorkspaceDocDirectory {
    fn query_docs(
        &self,
        query: &DocQuery,
        cancel: Option<&CancelToken>,
    ) -> StoreResult<DocQueryPage>;

    fn visible_doc_ids(&self) -> StoreResult<Vec<DocId>>;

    fn get_doc(&self, doc_id: &DocId) -> StoreResult<Option<DocDirectoryEntry>>;

    fn create_linked_doc(
        &mut self,
        request: CreateLinkedDocRequest,
    ) -> StoreResult<CreateLinkedDocResult>;

    fn open_doc(&mut self, doc_id: &DocId) -> StoreResult<DocDirectoryEntry>;

    fn sync_committed_title(&mut self, doc_id: &DocId, title: &str) -> StoreResult<bool>;
}

fn filter_and_paginate(
    all: Vec<DocDirectoryEntry>,
    query: &DocQuery,
    cancel: Option<&CancelToken>,
) -> StoreResult<DocQueryPage> {
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return Err(StoreError::Cancelled);
    }

    let prefix_lower = query.title_prefix.as_deref().map(str::to_lowercase);
    let contains_lower = query.title_contains.as_deref().map(str::to_lowercase);

    let mut matched = Vec::new();
    for entry in all {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(StoreError::Cancelled);
        }
        if !query.include_trashed && entry.trashed {
            continue;
        }
        let title_lower = entry.title.to_lowercase();
        if let Some(ref prefix) = prefix_lower
            && !title_lower.starts_with(prefix)
        {
            continue;
        }
        if let Some(ref substr) = contains_lower
            && !title_lower.contains(substr)
        {
            continue;
        }
        matched.push(entry);
    }

    let total = matched.len();
    let limit = if query.limit == 0 { total } else { query.limit };
    let start = query.offset.min(total);
    let end = start.saturating_add(limit).min(total);
    let entries = matched[start..end].to_vec();
    let has_more = end < total;

    Ok(DocQueryPage {
        entries,
        total,
        has_more,
    })
}

/// Pure in-memory reference adapter for standalone editor hosts and deterministic tests.
#[derive(Default)]
pub struct InMemoryDocDirectory {
    entries: BTreeMap<DocId, DocDirectoryEntry>,
    created_ops: BTreeMap<String, DocId>,
    last_opened: Option<DocId>,
}

impl InMemoryDocDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_entry(&mut self, entry: DocDirectoryEntry) {
        self.entries.insert(entry.id.clone(), entry);
    }

    pub fn last_opened(&self) -> Option<&DocId> {
        self.last_opened.as_ref()
    }
}

impl WorkspaceDocDirectory for InMemoryDocDirectory {
    fn query_docs(
        &self,
        query: &DocQuery,
        cancel: Option<&CancelToken>,
    ) -> StoreResult<DocQueryPage> {
        let all: Vec<DocDirectoryEntry> = self.entries.values().cloned().collect();
        filter_and_paginate(all, query, cancel)
    }

    fn visible_doc_ids(&self) -> StoreResult<Vec<DocId>> {
        Ok(self
            .entries
            .values()
            .filter(|e| !e.trashed)
            .map(|e| e.id.clone())
            .collect())
    }

    fn get_doc(&self, doc_id: &DocId) -> StoreResult<Option<DocDirectoryEntry>> {
        Ok(self.entries.get(doc_id).cloned())
    }

    fn create_linked_doc(
        &mut self,
        request: CreateLinkedDocRequest,
    ) -> StoreResult<CreateLinkedDocResult> {
        if let Some(existing) = self.created_ops.get(&request.op_id) {
            return Ok(CreateLinkedDocResult {
                doc_id: existing.clone(),
                created_new: false,
            });
        }
        let entry = DocDirectoryEntry {
            id: request.doc_id.clone(),
            title: request.title,
            mode: request.mode,
            create_date_ms: request.now_ms,
            updated_date_ms: None,
            trashed: false,
            tags: Vec::new(),
        };
        self.entries.insert(request.doc_id.clone(), entry);
        self.created_ops
            .insert(request.op_id, request.doc_id.clone());
        Ok(CreateLinkedDocResult {
            doc_id: request.doc_id,
            created_new: true,
        })
    }

    fn open_doc(&mut self, doc_id: &DocId) -> StoreResult<DocDirectoryEntry> {
        let entry = self
            .entries
            .get(doc_id)
            .cloned()
            .ok_or_else(|| StoreError::DocNotFound(doc_id.as_str().to_string()))?;
        self.last_opened = Some(doc_id.clone());
        Ok(entry)
    }

    fn sync_committed_title(&mut self, doc_id: &DocId, title: &str) -> StoreResult<bool> {
        let entry = self
            .entries
            .get_mut(doc_id)
            .ok_or_else(|| StoreError::DocNotFound(doc_id.as_str().to_string()))?;
        if entry.title == title {
            return Ok(false);
        }
        entry.title = title.to_string();
        Ok(true)
    }
}

/// Production `WorkspaceDocDirectory` adapter backed by the authoritative `WorkspaceDb`.
pub struct WorkspaceDbDocDirectory<'a> {
    db: &'a mut WorkspaceDb,
    last_opened: Option<DocId>,
}

impl<'a> WorkspaceDbDocDirectory<'a> {
    pub fn new(db: &'a mut WorkspaceDb) -> Self {
        Self {
            db,
            last_opened: None,
        }
    }

    pub fn last_opened(&self) -> Option<&DocId> {
        self.last_opened.as_ref()
    }

    fn project_entries(&self) -> StoreResult<Vec<DocDirectoryEntry>> {
        let pages = self.db.root().list_pages()?;
        let props_table = self.db.db_table(WorkspaceDbTableName::DocProperties);
        let mut entries = Vec::with_capacity(pages.len());

        for page in pages {
            let Ok(doc_id) = DocId::new(page.id) else {
                continue;
            };
            let mode = match props_table {
                Some(table) => table
                    .get_row::<DocPropertiesRow>(doc_id.as_str())?
                    .map(|row| DocMode::from_wire(row.primary_mode.as_deref()))
                    .unwrap_or(DocMode::Page),
                None => DocMode::Page,
            };
            entries.push(DocDirectoryEntry {
                id: doc_id,
                title: page.title,
                mode,
                create_date_ms: page.create_date_ms,
                updated_date_ms: page.updated_date_ms,
                trashed: page.trashed,
                tags: page.tags,
            });
        }
        Ok(entries)
    }
}

impl WorkspaceDocDirectory for WorkspaceDbDocDirectory<'_> {
    fn query_docs(
        &self,
        query: &DocQuery,
        cancel: Option<&CancelToken>,
    ) -> StoreResult<DocQueryPage> {
        let all = self.project_entries()?;
        filter_and_paginate(all, query, cancel)
    }

    fn visible_doc_ids(&self) -> StoreResult<Vec<DocId>> {
        Ok(self
            .project_entries()?
            .into_iter()
            .filter(|e| !e.trashed)
            .map(|e| e.id)
            .collect())
    }

    fn get_doc(&self, doc_id: &DocId) -> StoreResult<Option<DocDirectoryEntry>> {
        Ok(self
            .project_entries()?
            .into_iter()
            .find(|entry| &entry.id == doc_id))
    }

    fn create_linked_doc(
        &mut self,
        request: CreateLinkedDocRequest,
    ) -> StoreResult<CreateLinkedDocResult> {
        let outcome = self.db.root_mut().create_page(
            &request.op_id,
            request.doc_id.as_str(),
            &request.title,
            request.now_ms,
        )?;
        if outcome.created_new && request.mode == DocMode::Edgeless {
            let props = self.db.db_table_mut(WorkspaceDbTableName::DocProperties);
            props.upsert_row(&DocPropertiesRow {
                id: outcome.page_id.clone(),
                primary_mode: Some(request.mode.as_str().to_string()),
                edgeless_color_theme: None,
                journal: None,
                page_width: None,
                is_template: None,
                integration_type: None,
                created_by: None,
                updated_by: None,
                custom_fields: BTreeMap::new(),
            })?;
        }
        Ok(CreateLinkedDocResult {
            doc_id: DocId::new(outcome.page_id)?,
            created_new: outcome.created_new,
        })
    }

    fn open_doc(&mut self, doc_id: &DocId) -> StoreResult<DocDirectoryEntry> {
        let entry = self
            .get_doc(doc_id)?
            .ok_or_else(|| StoreError::DocNotFound(doc_id.as_str().to_string()))?;
        self.last_opened = Some(doc_id.clone());
        Ok(entry)
    }

    fn sync_committed_title(&mut self, doc_id: &DocId, title: &str) -> StoreResult<bool> {
        Ok(self
            .db
            .root_mut()
            .rename_page(doc_id.as_str(), title)?
            .is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_and_workspace_db_adapters_satisfy_directory_contract() {
        assert!(DocId::new("db$folders").is_err());
        assert!(DocId::new("userdata$u1$favorite").is_err());
        assert!(DocId::new("").is_err());

        let mut mem = InMemoryDocDirectory::new();
        run_directory_contract(&mut mem);

        let mut db = WorkspaceDb::new("ws-1", "u1", 1);
        let mut prod = WorkspaceDbDocDirectory::new(&mut db);
        run_directory_contract(&mut prod);
    }

    fn run_directory_contract(dir: &mut dyn WorkspaceDocDirectory) {
        let d1 = DocId::new("doc-1").unwrap();
        let d2 = DocId::new("doc-2").unwrap();
        let d3 = DocId::new("doc-3").unwrap();

        let r1 = dir
            .create_linked_doc(CreateLinkedDocRequest {
                op_id: "op-1".into(),
                doc_id: d1.clone(),
                title: "Design Spec".into(),
                mode: DocMode::Page,
                now_ms: 100,
            })
            .unwrap();
        assert!(r1.created_new);
        let r1_dup = dir
            .create_linked_doc(CreateLinkedDocRequest {
                op_id: "op-1".into(),
                doc_id: d1.clone(),
                title: "Design Spec".into(),
                mode: DocMode::Page,
                now_ms: 100,
            })
            .unwrap();
        assert!(!r1_dup.created_new);

        dir.create_linked_doc(CreateLinkedDocRequest {
            op_id: "op-2".into(),
            doc_id: d2.clone(),
            title: "Design Board".into(),
            mode: DocMode::Edgeless,
            now_ms: 200,
        })
        .unwrap();
        dir.create_linked_doc(CreateLinkedDocRequest {
            op_id: "op-3".into(),
            doc_id: d3.clone(),
            title: "Release Notes".into(),
            mode: DocMode::Page,
            now_ms: 300,
        })
        .unwrap();

        // Prefix + pagination
        let page1 = dir
            .query_docs(
                &DocQuery {
                    title_prefix: Some("design".into()),
                    title_contains: None,
                    include_trashed: false,
                    offset: 0,
                    limit: 1,
                },
                None,
            )
            .unwrap();
        assert_eq!(page1.total, 2);
        assert_eq!(page1.entries.len(), 1);
        assert!(page1.has_more);

        // Substring filter
        let sub = dir
            .query_docs(
                &DocQuery {
                    title_prefix: None,
                    title_contains: Some("board".into()),
                    include_trashed: false,
                    offset: 0,
                    limit: 10,
                },
                None,
            )
            .unwrap();
        assert_eq!(sub.entries.len(), 1);
        assert_eq!(sub.entries[0].mode, DocMode::Edgeless);

        // Cancellation
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            dir.query_docs(&DocQuery::default(), Some(&cancel)),
            Err(StoreError::Cancelled)
        ));

        // Open and sync committed title
        let opened = dir.open_doc(&d1).unwrap();
        assert_eq!(opened.title, "Design Spec");
        assert!(!dir.sync_committed_title(&d1, "Design Spec").unwrap());
        assert!(dir.sync_committed_title(&d1, "Design Spec v2").unwrap());
        assert_eq!(dir.get_doc(&d1).unwrap().unwrap().title, "Design Spec v2");
        assert_eq!(dir.visible_doc_ids().unwrap().len(), 3);
    }

    #[test]
    fn unicode_case_folding_matches_prefix_and_substring() {
        let mut dir = InMemoryDocDirectory::new();
        let cases = [
            ("u1", "École de Paris", Some("école"), None),
            ("u2", "Überblick", None, Some("über")),
            ("u3", "Привет Мир", Some("привет"), Some("мир")),
        ];
        for (id, title, _, _) in cases {
            dir.create_linked_doc(CreateLinkedDocRequest {
                op_id: format!("op-{id}"),
                doc_id: DocId::new(id).unwrap(),
                title: title.into(),
                mode: DocMode::Page,
                now_ms: 100,
            })
            .unwrap();
        }
        for (expected_id, _, prefix, contains) in cases {
            let page = dir
                .query_docs(
                    &DocQuery {
                        title_prefix: prefix.map(str::to_string),
                        title_contains: contains.map(str::to_string),
                        include_trashed: false,
                        offset: 0,
                        limit: 10,
                    },
                    None,
                )
                .unwrap();
            assert_eq!(page.entries.len(), 1);
            assert_eq!(page.entries[0].id.as_str(), expected_id);
        }
    }
}
