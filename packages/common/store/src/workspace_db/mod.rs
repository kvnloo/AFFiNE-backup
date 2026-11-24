pub mod root_doc;
pub mod tables;

pub use root_doc::{CreatePageOutcome, PageMetaEntry, RemoteRootApply, WorkspaceRootDoc};
pub use tables::{
    DELETED_FLAG_KEY, DocCustomPropertyInfoRow, DocIntegrationRefRow, DocPropertiesRow,
    ExplorerIconRow, FavoriteRow, FolderRow, PinnedCollectionRow, SettingsRow,
    USERDATA_TABLE_NAMES, UserdataTableName, WORKSPACE_DB_TABLE_NAMES, WorkspaceDbTableName,
    YjsTableDoc,
};

use std::collections::BTreeMap;

use crate::error::StoreResult;

/// Protocol version 1: creating a document writes both `meta.pages` and `spaces[pageId]` atomically.
pub const WORKSPACE_SYNC_PROTOCOL_VERSION: u8 = 1;

/// Authoritative writer for a single workspace's root document and its 8 metadata Yjs tables
/// (5 `db$` workspace tables + 3 `userdata$` user tables).
pub struct WorkspaceDb {
    space_id: String,
    user_id: String,
    client_id: u64,
    root: WorkspaceRootDoc,
    db_tables: BTreeMap<WorkspaceDbTableName, YjsTableDoc>,
    userdata_tables: BTreeMap<UserdataTableName, YjsTableDoc>,
}

impl WorkspaceDb {
    /// Create a fresh in-memory workspace DB coordinator.
    pub fn new(space_id: impl Into<String>, user_id: impl Into<String>, client_id: u64) -> Self {
        let space_id = space_id.into();
        let user_id = user_id.into();
        let root = WorkspaceRootDoc::new(space_id.clone(), client_id);
        Self {
            space_id,
            user_id,
            client_id,
            root,
            db_tables: BTreeMap::new(),
            userdata_tables: BTreeMap::new(),
        }
    }

    /// Load an existing root document snapshot with zero writeback.
    pub fn from_root_update_v1(
        space_id: impl Into<String>,
        user_id: impl Into<String>,
        client_id: u64,
        root_update: &[u8],
    ) -> StoreResult<Self> {
        let space_id = space_id.into();
        let user_id = user_id.into();
        let root = WorkspaceRootDoc::from_update_v1(space_id.clone(), client_id, root_update)?;
        Ok(Self {
            space_id,
            user_id,
            client_id,
            root,
            db_tables: BTreeMap::new(),
            userdata_tables: BTreeMap::new(),
        })
    }

    pub fn space_id(&self) -> &str {
        &self.space_id
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn root(&self) -> &WorkspaceRootDoc {
        &self.root
    }

    pub fn root_mut(&mut self) -> &mut WorkspaceRootDoc {
        &mut self.root
    }

    /// Load a workspace `db$` table document from encoded v1 bytes with zero writeback.
    pub fn load_db_table(&mut self, table: WorkspaceDbTableName, update: &[u8]) -> StoreResult<()> {
        let doc = YjsTableDoc::from_update_v1(
            table.local_doc_id(),
            table.primary_key_field(),
            table.known_fields(),
            self.client_id,
            update,
        )?;
        self.db_tables.insert(table, doc);
        Ok(())
    }

    /// Load a `userdata$` table document from encoded v1 bytes with zero writeback.
    pub fn load_userdata_table(
        &mut self,
        table: UserdataTableName,
        update: &[u8],
    ) -> StoreResult<()> {
        let doc = YjsTableDoc::from_update_v1(
            table.local_doc_id(&self.user_id),
            table.primary_key_field(),
            table.known_fields(),
            self.client_id,
            update,
        )?;
        self.userdata_tables.insert(table, doc);
        Ok(())
    }

    /// Access a loaded `db$` table without creating or mutating it.
    pub fn db_table(&self, table: WorkspaceDbTableName) -> Option<&YjsTableDoc> {
        self.db_tables.get(&table)
    }

    /// Access or lazily initialize a `db$` table for writes.
    pub fn db_table_mut(&mut self, table: WorkspaceDbTableName) -> &mut YjsTableDoc {
        let client_id = self.client_id;
        self.db_tables.entry(table).or_insert_with(|| {
            YjsTableDoc::new(
                table.local_doc_id(),
                table.primary_key_field(),
                table.known_fields(),
                client_id,
            )
        })
    }

    /// Access a loaded `userdata$` table without creating or mutating it.
    pub fn userdata_table(&self, table: UserdataTableName) -> Option<&YjsTableDoc> {
        self.userdata_tables.get(&table)
    }

    /// Access or lazily initialize a `userdata$` table for writes.
    pub fn userdata_table_mut(&mut self, table: UserdataTableName) -> &mut YjsTableDoc {
        let client_id = self.client_id;
        let doc_id = table.local_doc_id(&self.user_id);
        self.userdata_tables.entry(table).or_insert_with(|| {
            YjsTableDoc::new(
                doc_id,
                table.primary_key_field(),
                table.known_fields(),
                client_id,
            )
        })
    }
}
