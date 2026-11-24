use std::collections::{BTreeMap, BTreeSet};

use y_octo::{
    Any, DocOptions, DocSession, SessionConfig, SessionReadValue, SessionTxOrigin,
    SessionUndoConfig, SessionValue, TypePath, TypeTarget, YTypeKind,
};

use crate::error::{StoreError, StoreResult};

/// Metadata for one page entry in `rootDoc.meta.pages`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageMetaEntry {
    pub id: String,
    pub title: String,
    pub create_date_ms: i64,
    pub updated_date_ms: Option<i64>,
    pub tags: Vec<String>,
    pub trashed: bool,
    pub trash_date_ms: Option<i64>,
}

/// Result of an idempotent `create_page` call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatePageOutcome {
    pub page_id: String,
    pub created_new: bool,
    /// Encoded v1 root-doc update emitted by this call, or `None` on idempotent retry.
    pub update: Option<Vec<u8>>,
}

/// Result of applying a remote update to the root document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteRootApply {
    /// Subdocument IDs newly present in `spaces` or announced via `subdocs_added`.
    pub newly_discovered_subdocs: Vec<String>,
}

/// Authoritative coordinator for the AFFiNE workspace root YDoc (`meta.pages` + `spaces[pageId]`).
pub struct WorkspaceRootDoc {
    space_id: String,
    root: DocSession<()>,
    baseline_sv: Vec<u8>,
    local_writeback: bool,
    created_ops: BTreeMap<String, String>,
}

impl WorkspaceRootDoc {
    /// Create a fresh empty workspace root document.
    pub fn new(space_id: impl Into<String>, client_id: u64) -> Self {
        let space_id = space_id.into();
        let options = DocOptions::from_identity(client_id, space_id.clone());
        let root = DocSession::with_config(
            options,
            SessionConfig {
                undo: SessionUndoConfig::Disabled,
            },
        );
        let baseline_sv = root.encode_state_vector_v1().unwrap_or_default();
        Self {
            space_id,
            root,
            baseline_sv,
            local_writeback: false,
            created_ops: BTreeMap::new(),
        }
    }

    /// Load an existing root document from encoded v1 bytes with zero writeback.
    pub fn from_update_v1(
        space_id: impl Into<String>,
        client_id: u64,
        update: &[u8],
    ) -> StoreResult<Self> {
        let mut doc = Self::new(space_id, client_id);
        if !update.is_empty() && update != [0, 0] {
            let commits = doc
                .root
                .apply_update_v1(update)
                .map_err(|err| StoreError::InvalidBinary(err.to_string()))?;
            doc.local_writeback = commits.iter().any(|c| {
                c.origin_kind == y_octo::SessionOriginKind::FormattingCleanup
                    && !c.update.is_empty()
            });
        }
        doc.baseline_sv = doc
            .root
            .encode_state_vector_v1()
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        Ok(doc)
    }

    pub fn space_id(&self) -> &str {
        &self.space_id
    }

    /// Return `true` when local mutations have been written since load.
    pub fn has_local_writeback(&self) -> StoreResult<bool> {
        if self.local_writeback {
            return Ok(true);
        }
        let current_sv = self
            .root
            .encode_state_vector_v1()
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        Ok(current_sv != self.baseline_sv)
    }

    /// Encode the full v1 update for persistence.
    pub fn encode_update_v1(&self) -> StoreResult<Vec<u8>> {
        self.root
            .encode_diff_v1(&[0])
            .map_err(|err| StoreError::YOcto(err.to_string()))
    }

    /// Encode the current state vector.
    pub fn encode_state_vector_v1(&self) -> StoreResult<Vec<u8>> {
        self.root
            .encode_state_vector_v1()
            .map_err(|err| StoreError::YOcto(err.to_string()))
    }

    /// Encode a v1 diff against `remote_sv`.
    pub fn encode_diff_v1(&self, remote_sv: &[u8]) -> StoreResult<Vec<u8>> {
        self.root
            .encode_diff_v1(remote_sv)
            .map_err(|err| StoreError::YOcto(err.to_string()))
    }

    /// Atomically create a page in both `meta.pages` and `spaces[page_id]` in a single transaction.
    ///
    /// Retrying with the same `op_id` is strictly idempotent: it returns the original `page_id`
    /// with `created_new = false` and emits no writeback.
    pub fn create_page(
        &mut self,
        op_id: &str,
        page_id: &str,
        title: &str,
        create_date_ms: i64,
    ) -> StoreResult<CreatePageOutcome> {
        if let Some(existing_page_id) = self.created_ops.get(op_id) {
            return Ok(CreatePageOutcome {
                page_id: existing_page_id.clone(),
                created_new: false,
                update: None,
            });
        }

        if self.get_page(page_id)?.is_some() {
            self.created_ops
                .insert(op_id.to_string(), page_id.to_string());
            return Ok(CreatePageOutcome {
                page_id: page_id.to_string(),
                created_new: false,
                update: None,
            });
        }

        let meta_id = self
            .root
            .ensure_root("meta", YTypeKind::Map)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        let spaces_id = self
            .root
            .ensure_root("spaces", YTypeKind::Map)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        let meta_target = TypeTarget::from(meta_id);
        let spaces_target = TypeTarget::from(spaces_id);

        self.root
            .begin_tx(SessionTxOrigin::Untracked)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;

        if self
            .root
            .get_map_value(&meta_target, "pages")
            .map_err(|err| StoreError::YOcto(err.to_string()))?
            .is_none()
        {
            self.root
                .map_set(
                    &meta_target,
                    "pages".to_string(),
                    SessionValue::Array(Vec::new()),
                )
                .map_err(|err| StoreError::YOcto(err.to_string()))?;
        }

        let pages_target = TypeTarget::Path(TypePath::root("meta").key("pages"));
        let page_map = SessionValue::Map(vec![
            (
                "id".to_string(),
                SessionValue::Any(Any::String(page_id.to_string())),
            ),
            (
                "title".to_string(),
                SessionValue::Any(Any::String(title.to_string())),
            ),
            (
                "createDate".to_string(),
                SessionValue::Any(Any::Float64((create_date_ms as f64).into())),
            ),
            ("tags".to_string(), SessionValue::Array(Vec::new())),
        ]);
        self.root
            .array_push(&pages_target, vec![page_map])
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        self.root
            .map_set(
                &spaces_target,
                page_id.to_string(),
                SessionValue::Doc {
                    guid: page_id.to_string(),
                    gc: true,
                },
            )
            .map_err(|err| StoreError::YOcto(err.to_string()))?;

        let commit = self
            .root
            .commit_tx()
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        if commit.is_some() {
            self.local_writeback = true;
        }

        self.created_ops
            .insert(op_id.to_string(), page_id.to_string());

        Ok(CreatePageOutcome {
            page_id: page_id.to_string(),
            created_new: true,
            update: commit.map(|c| c.update),
        })
    }

    /// Update the title of `page_id` in `meta.pages`. Returns `Ok(None)` when unchanged.
    pub fn rename_page(&mut self, page_id: &str, new_title: &str) -> StoreResult<Option<Vec<u8>>> {
        let Some(target) = self.find_page_map_target(page_id)? else {
            return Err(StoreError::DocNotFound(page_id.to_string()));
        };

        if matches!(
            self.root
                .get_map_value(&target, "title")
                .map_err(|err| StoreError::YOcto(err.to_string()))?,
            Some(SessionReadValue::Any(Any::String(ref current))) if current == new_title
        ) {
            return Ok(None);
        }

        let commit = self
            .root
            .map_set(
                &target,
                "title".to_string(),
                SessionValue::Any(Any::String(new_title.to_string())),
            )
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        if commit.is_some() {
            self.local_writeback = true;
        }

        Ok(commit.map(|c| c.update))
    }

    /// Soft-delete (move to trash) `page_id` in `meta.pages`.
    pub fn trash_page(
        &mut self,
        page_id: &str,
        trash_date_ms: i64,
    ) -> StoreResult<Option<Vec<u8>>> {
        let Some(target) = self.find_page_map_target(page_id)? else {
            return Err(StoreError::DocNotFound(page_id.to_string()));
        };

        if matches!(
            self.root
                .get_map_value(&target, "trash")
                .map_err(|err| StoreError::YOcto(err.to_string()))?,
            Some(SessionReadValue::Any(Any::True))
        ) {
            return Ok(None);
        }

        self.root
            .begin_tx(SessionTxOrigin::Untracked)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        self.root
            .map_set(&target, "trash".to_string(), SessionValue::Any(Any::True))
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        self.root
            .map_set(
                &target,
                "trashDate".to_string(),
                SessionValue::Any(Any::Float64((trash_date_ms as f64).into())),
            )
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        let commit = self
            .root
            .commit_tx()
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        if commit.is_some() {
            self.local_writeback = true;
        }

        Ok(commit.map(|c| c.update))
    }

    /// Restore a soft-deleted page in `meta.pages`.
    pub fn restore_page(&mut self, page_id: &str) -> StoreResult<Option<Vec<u8>>> {
        let Some(target) = self.find_page_map_target(page_id)? else {
            return Err(StoreError::DocNotFound(page_id.to_string()));
        };

        let is_trashed = matches!(
            self.root
                .get_map_value(&target, "trash")
                .map_err(|err| StoreError::YOcto(err.to_string()))?,
            Some(SessionReadValue::Any(Any::True))
        );
        if !is_trashed {
            return Ok(None);
        }

        self.root
            .begin_tx(SessionTxOrigin::Untracked)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        self.root
            .map_set(&target, "trash".to_string(), SessionValue::Any(Any::False))
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        self.root
            .map_remove(&target, "trashDate")
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        let commit = self
            .root
            .commit_tx()
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        if commit.is_some() {
            self.local_writeback = true;
        }

        Ok(commit.map(|c| c.update))
    }

    /// Apply a remote update to the root document and report newly discovered subdoc GUIDs.
    pub fn apply_remote_update(&mut self, bytes: &[u8]) -> StoreResult<RemoteRootApply> {
        let before: BTreeSet<String> = self.subdoc_guids()?.into_values().collect();
        let commits = self
            .root
            .apply_update_v1(bytes)
            .map_err(|err| StoreError::InvalidBinary(err.to_string()))?;

        let mut discovered = BTreeSet::new();
        for commit in &commits {
            for guid in &commit.subdocs_added {
                if !before.contains(guid) {
                    discovered.insert(guid.clone());
                }
            }
        }
        for guid in self.subdoc_guids()?.into_values() {
            if !before.contains(&guid) {
                discovered.insert(guid);
            }
        }

        Ok(RemoteRootApply {
            newly_discovered_subdocs: discovered.into_iter().collect(),
        })
    }

    /// List all page metadata entries in `meta.pages` without mutating the document.
    pub fn list_pages(&self) -> StoreResult<Vec<PageMetaEntry>> {
        let pages_target = TypeTarget::Path(TypePath::root("meta").key("pages"));
        let Ok(items) = self.root.read_array(&pages_target) else {
            return Ok(Vec::new());
        };

        let mut result = Vec::new();
        for item in items {
            let SessionReadValue::Map(map_id) = item else {
                continue;
            };
            let entries = self
                .root
                .read_map(&TypeTarget::Identity(map_id))
                .map_err(|err| StoreError::YOcto(err.to_string()))?;
            let mut id = String::new();
            let mut title = String::new();
            let mut create_date_ms = 0i64;
            let mut updated_date_ms = None;
            let mut tags = Vec::new();
            let mut trashed = false;
            let mut trash_date_ms = None;

            for (key, value) in entries {
                match key.as_str() {
                    "id" => {
                        if let SessionReadValue::Any(Any::String(s)) = value {
                            id = s;
                        }
                    }
                    "title" => {
                        if let SessionReadValue::Any(Any::String(s)) = value {
                            title = s;
                        }
                    }
                    "createDate" => {
                        if let Some(n) = any_to_i64(&value) {
                            create_date_ms = n;
                        }
                    }
                    "updatedDate" => {
                        updated_date_ms = any_to_i64(&value);
                    }
                    "trash" => {
                        trashed = matches!(value, SessionReadValue::Any(Any::True));
                    }
                    "trashDate" => {
                        trash_date_ms = any_to_i64(&value);
                    }
                    "tags" => {
                        if let SessionReadValue::Array(arr_id) = value {
                            for tag in self
                                .root
                                .read_array(&TypeTarget::Identity(arr_id))
                                .map_err(|err| StoreError::YOcto(err.to_string()))?
                            {
                                if let SessionReadValue::Any(Any::String(s)) = tag {
                                    tags.push(s);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            if !id.is_empty() {
                result.push(PageMetaEntry {
                    id,
                    title,
                    create_date_ms,
                    updated_date_ms,
                    tags,
                    trashed,
                    trash_date_ms,
                });
            }
        }

        Ok(result)
    }

    /// Read a single page metadata entry by `page_id`.
    pub fn get_page(&self, page_id: &str) -> StoreResult<Option<PageMetaEntry>> {
        Ok(self
            .list_pages()?
            .into_iter()
            .find(|entry| entry.id == page_id))
    }

    /// Return `page_id -> subdoc_guid` from `spaces` without mutating the document.
    pub fn subdoc_guids(&self) -> StoreResult<BTreeMap<String, String>> {
        let spaces_target = TypeTarget::Path(TypePath::root("spaces"));
        let Ok(entries) = self.root.read_map(&spaces_target) else {
            return Ok(BTreeMap::new());
        };
        let mut out = BTreeMap::new();
        for (page_id, value) in entries {
            if let SessionReadValue::Doc { guid, .. } = value {
                out.insert(page_id, guid);
            }
        }
        Ok(out)
    }

    fn find_page_map_target(&self, page_id: &str) -> StoreResult<Option<TypeTarget>> {
        let pages_target = TypeTarget::Path(TypePath::root("meta").key("pages"));
        let Ok(items) = self.root.read_array(&pages_target) else {
            return Ok(None);
        };
        for item in items {
            if let SessionReadValue::Map(map_id) = item {
                let target = TypeTarget::Identity(map_id);
                if matches!(
                    self.root
                        .get_map_value(&target, "id")
                        .map_err(|err| StoreError::YOcto(err.to_string()))?,
                    Some(SessionReadValue::Any(Any::String(ref id))) if id == page_id
                ) {
                    return Ok(Some(target));
                }
            }
        }
        Ok(None)
    }
}

fn any_to_i64(value: &SessionReadValue) -> Option<i64> {
    match value {
        SessionReadValue::Any(Any::Integer(n)) => Some(i64::from(*n)),
        SessionReadValue::Any(Any::BigInt64(n)) => Some(*n),
        SessionReadValue::Any(Any::Float64(f)) => Some(f.0 as i64),
        SessionReadValue::Any(Any::Float32(f)) => Some(f.0 as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_rename_trash_restore_idempotent_retry_and_zero_writeback() {
        let mut root = WorkspaceRootDoc::new("ws-1", 1);
        assert!(!root.has_local_writeback().unwrap());

        let first = root.create_page("op-1", "page-1", "Alpha", 1_000).unwrap();
        assert!(first.created_new);
        assert!(first.update.is_some());
        assert_eq!(
            root.subdoc_guids()
                .unwrap()
                .get("page-1")
                .map(String::as_str),
            Some("page-1")
        );

        // Idempotent retry with the same op_id must not create a duplicate page or emit an update.
        let retry = root
            .create_page("op-1", "page-1-dup", "Alpha", 1_000)
            .unwrap();
        assert!(!retry.created_new);
        assert_eq!(retry.page_id, "page-1");
        assert!(retry.update.is_none());
        assert_eq!(root.list_pages().unwrap().len(), 1);

        // Unchanged rename produces zero writeback.
        assert!(root.rename_page("page-1", "Alpha").unwrap().is_none());
        assert!(root.rename_page("page-1", "Beta").unwrap().is_some());
        assert_eq!(root.get_page("page-1").unwrap().unwrap().title, "Beta");

        // Trash and restore.
        assert!(root.trash_page("page-1", 2_000).unwrap().is_some());
        let trashed = root.get_page("page-1").unwrap().unwrap();
        assert!(trashed.trashed);
        assert_eq!(trashed.trash_date_ms, Some(2_000));

        assert!(root.restore_page("page-1").unwrap().is_some());
        let restored = root.get_page("page-1").unwrap().unwrap();
        assert!(!restored.trashed);
        assert_eq!(restored.trash_date_ms, None);

        // Loading untouched document from bytes guarantees 0 writeback.
        let encoded = root.encode_update_v1().unwrap();
        let loaded = WorkspaceRootDoc::from_update_v1("ws-1", 2, &encoded).unwrap();
        assert!(!loaded.has_local_writeback().unwrap());
        assert_eq!(loaded.list_pages().unwrap().len(), 1);
        assert_eq!(loaded.get_page("page-1").unwrap().unwrap().title, "Beta");

        // Remote subdoc discovery.
        let mut peer = WorkspaceRootDoc::from_update_v1("ws-1", 3, &encoded).unwrap();
        let new_page = root.create_page("op-2", "page-2", "Gamma", 3_000).unwrap();
        let applied = peer
            .apply_remote_update(new_page.update.as_deref().unwrap())
            .unwrap();
        assert_eq!(applied.newly_discovered_subdocs, vec!["page-2".to_string()]);
    }
}
