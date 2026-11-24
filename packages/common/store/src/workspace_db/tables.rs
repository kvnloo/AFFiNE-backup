use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use y_octo::{AHashMap, Any, Doc, DocOptions, HashMapExt, StateVector, Value};

use crate::error::{StoreError, StoreResult};

/// Soft-delete marker written by `YjsTableAdapter` (`packages/common/infra/src/orm/core/adapters/yjs/table.ts`).
pub const DELETED_FLAG_KEY: &str = "$$DELETED";

/// All 5 workspace `db$` table names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum WorkspaceDbTableName {
    Folders,
    DocProperties,
    DocCustomPropertyInfo,
    PinnedCollections,
    ExplorerIcon,
}

pub const WORKSPACE_DB_TABLE_NAMES: &[WorkspaceDbTableName] = &[
    WorkspaceDbTableName::Folders,
    WorkspaceDbTableName::DocProperties,
    WorkspaceDbTableName::DocCustomPropertyInfo,
    WorkspaceDbTableName::PinnedCollections,
    WorkspaceDbTableName::ExplorerIcon,
];

impl WorkspaceDbTableName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Folders => "folders",
            Self::DocProperties => "docProperties",
            Self::DocCustomPropertyInfo => "docCustomPropertyInfo",
            Self::PinnedCollections => "pinnedCollections",
            Self::ExplorerIcon => "explorerIcon",
        }
    }

    pub fn local_doc_id(self) -> String {
        format!("db${}", self.as_str())
    }

    pub const fn primary_key_field(self) -> &'static str {
        match self {
            Self::PinnedCollections => "collectionId",
            Self::Folders
            | Self::DocProperties
            | Self::DocCustomPropertyInfo
            | Self::ExplorerIcon => "id",
        }
    }

    pub const fn known_fields(self) -> &'static [&'static str] {
        match self {
            Self::Folders => &["id", "parentId", "data", "type", "index"],
            Self::DocProperties => &[
                "id",
                "primaryMode",
                "edgelessColorTheme",
                "journal",
                "pageWidth",
                "isTemplate",
                "integrationType",
                "createdBy",
                "updatedBy",
            ],
            Self::DocCustomPropertyInfo => &[
                "id",
                "name",
                "type",
                "show",
                "index",
                "icon",
                "additionalData",
                "isDeleted",
            ],
            Self::PinnedCollections => &["collectionId", "index"],
            Self::ExplorerIcon => &["id", "icon"],
        }
    }
}

/// All 3 `userdata$` table names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum UserdataTableName {
    Favorite,
    Settings,
    DocIntegrationRef,
}

pub const USERDATA_TABLE_NAMES: &[UserdataTableName] = &[
    UserdataTableName::Favorite,
    UserdataTableName::Settings,
    UserdataTableName::DocIntegrationRef,
];

impl UserdataTableName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Favorite => "favorite",
            Self::Settings => "settings",
            Self::DocIntegrationRef => "docIntegrationRef",
        }
    }

    pub fn local_doc_id(self, user_id: &str) -> String {
        format!("userdata${user_id}${}", self.as_str())
    }

    pub const fn primary_key_field(self) -> &'static str {
        match self {
            Self::Favorite | Self::Settings => "key",
            Self::DocIntegrationRef => "id",
        }
    }

    pub const fn known_fields(self) -> &'static [&'static str] {
        match self {
            Self::Favorite => &["key", "index"],
            Self::Settings => &["key", "value"],
            Self::DocIntegrationRef => &["id", "type", "integrationId", "refMeta"],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRow {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub data: String,
    #[serde(rename = "type")]
    pub folder_type: String,
    pub index: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocPropertiesRow {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edgeless_color_theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_width: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_template: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    #[serde(flatten)]
    pub custom_fields: BTreeMap<String, JsonValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocCustomPropertyInfoRow {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub property_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_data: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_deleted: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedCollectionRow {
    pub collection_id: String,
    pub index: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerIconRow {
    pub id: String,
    pub icon: JsonValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteRow {
    pub key: String,
    pub index: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsRow {
    pub key: String,
    pub value: JsonValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocIntegrationRefRow {
    pub id: String,
    #[serde(rename = "type")]
    pub integration_type: String,
    pub integration_id: String,
    pub ref_meta: JsonValue,
}

/// Primary-key `Y.Map` table backed by a single `y-octo::Doc`, matching `YjsTableAdapter`.
pub struct YjsTableDoc {
    doc_id: String,
    key_field: &'static str,
    known_fields: &'static [&'static str],
    doc: Doc,
    baseline_sv: StateVector,
    local_writeback: bool,
}

impl YjsTableDoc {
    pub fn new(
        doc_id: impl Into<String>,
        key_field: &'static str,
        known_fields: &'static [&'static str],
        client_id: u64,
    ) -> Self {
        let doc_id = doc_id.into();
        let doc = DocOptions::from_identity(client_id, doc_id.clone()).build();
        let baseline_sv = doc.get_state_vector();
        Self {
            doc_id,
            key_field,
            known_fields,
            doc,
            baseline_sv,
            local_writeback: false,
        }
    }

    /// Load a table document from encoded v1 bytes with zero writeback.
    pub fn from_update_v1(
        doc_id: impl Into<String>,
        key_field: &'static str,
        known_fields: &'static [&'static str],
        client_id: u64,
        update: &[u8],
    ) -> StoreResult<Self> {
        let mut table = Self::new(doc_id, key_field, known_fields, client_id);
        if !update.is_empty() && update != [0, 0] {
            table
                .doc
                .apply_update_from_binary_v1(update)
                .map_err(|err| StoreError::InvalidBinary(err.to_string()))?;
        }
        table.baseline_sv = table.doc.get_state_vector();
        table.local_writeback = false;
        Ok(table)
    }

    pub fn doc_id(&self) -> &str {
        &self.doc_id
    }

    /// Return `true` when local writes have occurred since load.
    pub fn has_local_writeback(&self) -> StoreResult<bool> {
        Ok(self.local_writeback || self.doc.get_state_vector() != self.baseline_sv)
    }

    pub fn encode_update_v1(&self) -> StoreResult<Vec<u8>> {
        self.doc
            .encode_update_v1()
            .map_err(|err| StoreError::YOcto(err.to_string()))
    }

    /// Upsert a typed row by primary key, removing `$$DELETED` if previously soft-deleted.
    pub fn upsert_row<T: Serialize>(&mut self, row: &T) -> StoreResult<Vec<u8>> {
        let before_sv = self.doc.get_state_vector();
        let JsonValue::Object(obj) = serde_json::to_value(row)? else {
            return Err(StoreError::InvalidProtocol(
                "table row must serialize as a JSON object".into(),
            ));
        };
        let Some(key) = obj.get(self.key_field).and_then(JsonValue::as_str) else {
            return Err(StoreError::InvalidProtocol(format!(
                "missing string primary key field `{}`",
                self.key_field
            )));
        };

        let mut map = self
            .doc
            .get_or_create_map(key)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        for (field, val) in &obj {
            map.insert(field.clone(), Value::Any(json_to_any(val)))
                .map_err(|err| StoreError::YOcto(err.to_string()))?;
        }
        map.remove(DELETED_FLAG_KEY);
        self.local_writeback = true;

        self.doc
            .encode_state_as_update_v1(&before_sv)
            .map_err(|err| StoreError::YOcto(err.to_string()))
    }

    /// Soft-delete a row matching `YjsTableAdapter.deleteTy`: remove all non-key fields and set
    /// `$$DELETED = true`.
    pub fn soft_delete(&mut self, primary_key: &str) -> StoreResult<Option<Vec<u8>>> {
        if !self.doc.keys().iter().any(|k| k == primary_key) {
            return Ok(None);
        }
        let before_sv = self.doc.get_state_vector();
        let mut map = self
            .doc
            .get_or_create_map(primary_key)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        if is_map_deleted(&map) {
            return Ok(None);
        }

        let existing_keys: Vec<String> = map.iter().map(|(k, _)| k.to_string()).collect();
        for field in self.known_fields {
            if *field != self.key_field {
                map.remove(field);
            }
        }
        for field in existing_keys {
            if field != self.key_field && field != DELETED_FLAG_KEY {
                map.remove(&field);
            }
        }
        map.insert(DELETED_FLAG_KEY.to_string(), Value::Any(Any::True))
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        self.local_writeback = true;

        let diff = self
            .doc
            .encode_state_as_update_v1(&before_sv)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        Ok(Some(diff))
    }

    /// Read a single non-deleted row by primary key without creating empty maps.
    pub fn get_row<T: for<'de> Deserialize<'de>>(
        &self,
        primary_key: &str,
    ) -> StoreResult<Option<T>> {
        if !self.doc.keys().iter().any(|k| k == primary_key) {
            return Ok(None);
        }
        let map = self
            .doc
            .get_or_create_map(primary_key)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        if is_map_deleted(&map) {
            return Ok(None);
        }
        let json = map_to_json_object(&map);
        Ok(Some(serde_json::from_value(json)?))
    }

    /// List all non-deleted rows in deterministic key order without mutating the document.
    pub fn list_rows<T: for<'de> Deserialize<'de>>(&self) -> StoreResult<Vec<T>> {
        let mut keys = self.doc.keys();
        keys.sort();
        let mut rows = Vec::new();
        for key in keys {
            if let Some(row) = self.get_row::<T>(&key)? {
                rows.push(row);
            }
        }
        Ok(rows)
    }

    /// Return `true` if `primary_key` exists in the underlying YDoc with `$$DELETED = true`.
    pub fn is_soft_deleted(&self, primary_key: &str) -> StoreResult<bool> {
        if !self.doc.keys().iter().any(|k| k == primary_key) {
            return Ok(false);
        }
        let map = self
            .doc
            .get_or_create_map(primary_key)
            .map_err(|err| StoreError::YOcto(err.to_string()))?;
        Ok(matches!(
            map.get(DELETED_FLAG_KEY),
            Some(Value::Any(Any::True))
        ))
    }
}

fn is_map_deleted(map: &y_octo::Map) -> bool {
    if map.is_empty() {
        return true;
    }
    matches!(map.get(DELETED_FLAG_KEY), Some(Value::Any(Any::True)))
}

fn map_to_json_object(map: &y_octo::Map) -> JsonValue {
    let mut obj = serde_json::Map::new();
    for (k, v) in map.iter() {
        if k == DELETED_FLAG_KEY {
            continue;
        }
        if let Value::Any(any) = v {
            obj.insert(k.to_string(), any_to_json(&any));
        }
    }
    JsonValue::Object(obj)
}

fn any_to_json(any: &Any) -> JsonValue {
    match any {
        Any::Null | Any::Undefined => JsonValue::Null,
        Any::False => JsonValue::Bool(false),
        Any::True => JsonValue::Bool(true),
        Any::Integer(n) => JsonValue::Number((*n).into()),
        Any::BigInt64(n) => JsonValue::Number((*n).into()),
        Any::Float32(f) => serde_json::Number::from_f64(f64::from(f.0))
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Any::Float64(f) => serde_json::Number::from_f64(f.0)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Any::String(s) => JsonValue::String(s.clone()),
        Any::Array(arr) => JsonValue::Array(arr.iter().map(any_to_json).collect()),
        Any::Object(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map.iter() {
                obj.insert(k.clone(), any_to_json(v));
            }
            JsonValue::Object(obj)
        }
        Any::Binary(bytes) => JsonValue::Array(bytes.iter().map(|b| (*b).into()).collect()),
    }
}

fn json_to_any(value: &JsonValue) -> Any {
    match value {
        JsonValue::Null => Any::Null,
        JsonValue::Bool(true) => Any::True,
        JsonValue::Bool(false) => Any::False,
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                if let Ok(i32_val) = i32::try_from(i) {
                    Any::Integer(i32_val)
                } else {
                    Any::BigInt64(i)
                }
            } else if let Some(f) = n.as_f64() {
                Any::Float64(f.into())
            } else {
                Any::Null
            }
        }
        JsonValue::String(s) => Any::String(s.clone()),
        JsonValue::Array(arr) => Any::Array(arr.iter().map(json_to_any).collect()),
        JsonValue::Object(obj) => {
            let mut map = AHashMap::with_capacity(obj.len());
            for (k, v) in obj {
                map.insert(k.clone(), json_to_any(v));
            }
            Any::Object(Box::new(map))
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn all_eight_tables_support_typed_upsert_soft_delete_reinsert_and_zero_writeback() {
        // 1. folders (db$)
        let mut folders = YjsTableDoc::new(
            WorkspaceDbTableName::Folders.local_doc_id(),
            WorkspaceDbTableName::Folders.primary_key_field(),
            WorkspaceDbTableName::Folders.known_fields(),
            1,
        );
        folders
            .upsert_row(&FolderRow {
                id: "f1".into(),
                parent_id: None,
                data: "Root".into(),
                folder_type: "folder".into(),
                index: "a0".into(),
            })
            .unwrap();
        assert_eq!(folders.list_rows::<FolderRow>().unwrap().len(), 1);
        folders.soft_delete("f1").unwrap();
        assert!(folders.is_soft_deleted("f1").unwrap());
        assert!(folders.get_row::<FolderRow>("f1").unwrap().is_none());
        // Re-insert clears $$DELETED
        folders
            .upsert_row(&FolderRow {
                id: "f1".into(),
                parent_id: Some("root".into()),
                data: "Restored".into(),
                folder_type: "folder".into(),
                index: "a1".into(),
            })
            .unwrap();
        assert!(!folders.is_soft_deleted("f1").unwrap());
        assert_eq!(
            folders.get_row::<FolderRow>("f1").unwrap().unwrap().data,
            "Restored"
        );

        // 2. docProperties (db$, including custom:* fields)
        let mut props = YjsTableDoc::new(
            WorkspaceDbTableName::DocProperties.local_doc_id(),
            WorkspaceDbTableName::DocProperties.primary_key_field(),
            WorkspaceDbTableName::DocProperties.known_fields(),
            1,
        );
        let mut custom = BTreeMap::new();
        custom.insert("custom:priority".to_string(), json!("high"));
        props
            .upsert_row(&DocPropertiesRow {
                id: "doc-1".into(),
                primary_mode: Some("edgeless".into()),
                edgeless_color_theme: None,
                journal: None,
                page_width: Some("fullWidth".into()),
                is_template: Some(false),
                integration_type: None,
                created_by: Some("u1".into()),
                updated_by: None,
                custom_fields: custom,
            })
            .unwrap();
        let loaded_props = YjsTableDoc::from_update_v1(
            WorkspaceDbTableName::DocProperties.local_doc_id(),
            WorkspaceDbTableName::DocProperties.primary_key_field(),
            WorkspaceDbTableName::DocProperties.known_fields(),
            2,
            &props.encode_update_v1().unwrap(),
        )
        .unwrap();
        assert!(!loaded_props.has_local_writeback().unwrap());
        let row = loaded_props
            .get_row::<DocPropertiesRow>("doc-1")
            .unwrap()
            .unwrap();
        assert_eq!(row.primary_mode.as_deref(), Some("edgeless"));
        assert_eq!(
            row.custom_fields.get("custom:priority"),
            Some(&json!("high"))
        );

        // 3-5. docCustomPropertyInfo, pinnedCollections, explorerIcon
        let mut custom_info = YjsTableDoc::new(
            WorkspaceDbTableName::DocCustomPropertyInfo.local_doc_id(),
            WorkspaceDbTableName::DocCustomPropertyInfo.primary_key_field(),
            WorkspaceDbTableName::DocCustomPropertyInfo.known_fields(),
            1,
        );
        custom_info
            .upsert_row(&DocCustomPropertyInfoRow {
                id: "cp1".into(),
                name: Some("Status".into()),
                property_type: "text".into(),
                show: Some("always-show".into()),
                index: Some("a0".into()),
                icon: None,
                additional_data: Some(json!({"choices": ["todo", "done"]})),
                is_deleted: Some(false),
            })
            .unwrap();
        assert_eq!(
            custom_info
                .get_row::<DocCustomPropertyInfoRow>("cp1")
                .unwrap()
                .unwrap()
                .name
                .as_deref(),
            Some("Status")
        );

        let mut pinned = YjsTableDoc::new(
            WorkspaceDbTableName::PinnedCollections.local_doc_id(),
            WorkspaceDbTableName::PinnedCollections.primary_key_field(),
            WorkspaceDbTableName::PinnedCollections.known_fields(),
            1,
        );
        pinned
            .upsert_row(&PinnedCollectionRow {
                collection_id: "col-1".into(),
                index: "a0".into(),
            })
            .unwrap();
        assert_eq!(pinned.list_rows::<PinnedCollectionRow>().unwrap().len(), 1);

        let mut icons = YjsTableDoc::new(
            WorkspaceDbTableName::ExplorerIcon.local_doc_id(),
            WorkspaceDbTableName::ExplorerIcon.primary_key_field(),
            WorkspaceDbTableName::ExplorerIcon.known_fields(),
            1,
        );
        icons
            .upsert_row(&ExplorerIconRow {
                id: "doc:doc-1".into(),
                icon: json!({"type": "emoji", "unicode": "1f4dd"}),
            })
            .unwrap();
        assert_eq!(icons.list_rows::<ExplorerIconRow>().unwrap().len(), 1);

        // 6-8. userdata$ tables: favorite, settings, docIntegrationRef
        let mut fav = YjsTableDoc::new(
            UserdataTableName::Favorite.local_doc_id("u1"),
            UserdataTableName::Favorite.primary_key_field(),
            UserdataTableName::Favorite.known_fields(),
            1,
        );
        fav.upsert_row(&FavoriteRow {
            key: "doc:doc-1".into(),
            index: "a0".into(),
        })
        .unwrap();
        assert_eq!(fav.list_rows::<FavoriteRow>().unwrap().len(), 1);

        let mut settings = YjsTableDoc::new(
            UserdataTableName::Settings.local_doc_id("u1"),
            UserdataTableName::Settings.primary_key_field(),
            UserdataTableName::Settings.known_fields(),
            1,
        );
        settings
            .upsert_row(&SettingsRow {
                key: "theme".into(),
                value: json!("dark"),
            })
            .unwrap();
        assert_eq!(
            settings
                .get_row::<SettingsRow>("theme")
                .unwrap()
                .unwrap()
                .value,
            json!("dark")
        );

        let mut refs = YjsTableDoc::new(
            UserdataTableName::DocIntegrationRef.local_doc_id("u1"),
            UserdataTableName::DocIntegrationRef.primary_key_field(),
            UserdataTableName::DocIntegrationRef.known_fields(),
            1,
        );
        refs.upsert_row(&DocIntegrationRefRow {
            id: "doc-1".into(),
            integration_type: "readwise".into(),
            integration_id: "rw-1".into(),
            ref_meta: json!({"bookId": 42}),
        })
        .unwrap();
        assert_eq!(refs.list_rows::<DocIntegrationRefRow>().unwrap().len(), 1);
    }
}
