//! Workspace Copilot embedding configuration, artifacts, and ignored-document GraphQL resolvers.

use std::io::Read as _;
use std::sync::Arc;

use async_graphql::{Context, Object, Result, SimpleObject, Upload};
use chrono::{DateTime, Utc};
use server_spi::{AddWorkspaceArtifactInput, EmbeddingPagination, WorkspaceArtifactRecord, WorkspaceIgnoredDocRecord};
use server_types::DomainError;

use super::doc::{PageInfo, PaginationInput, encode_cursor};
use super::{enforce_default_throttle_if_anonymous_graphql, require_authenticated_user};
use crate::AppState;
use crate::legacy_compat::errors::domain_error_to_graphql;
use crate::legacy_compat::quota::SafeInt;

const MAX_EMBEDDABLE_SIZE: usize = 50 * 1024 * 1024;

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "CopilotWorkspaceIgnoredDoc")]
pub struct CopilotWorkspaceIgnoredDoc {
  pub created_at: DateTime<Utc>,
  pub created_by: Option<String>,
  pub created_by_avatar: Option<String>,
  pub doc_created_at: Option<DateTime<Utc>>,
  pub doc_id: String,
  pub doc_updated_at: Option<DateTime<Utc>>,
  pub title: Option<String>,
  pub updated_by: Option<String>,
}

impl From<WorkspaceIgnoredDocRecord> for CopilotWorkspaceIgnoredDoc {
  fn from(r: WorkspaceIgnoredDocRecord) -> Self {
    Self {
      created_at: r.created_at,
      created_by: r.created_by,
      created_by_avatar: r.created_by_avatar,
      doc_created_at: r.doc_created_at,
      doc_id: r.doc_id,
      doc_updated_at: r.doc_updated_at,
      title: r.title,
      updated_by: r.updated_by,
    }
  }
}

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "CopilotWorkspaceIgnoredDocTypeEdge")]
pub struct CopilotWorkspaceIgnoredDocTypeEdge {
  pub cursor: String,
  pub node: CopilotWorkspaceIgnoredDoc,
}

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "PaginatedIgnoredDocsType")]
pub struct PaginatedIgnoredDocsType {
  pub edges: Vec<CopilotWorkspaceIgnoredDocTypeEdge>,
  pub page_info: PageInfo,
  pub total_count: i32,
}

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "CopilotWorkspaceArtifact")]
pub struct CopilotWorkspaceArtifact {
  pub artifact_id: String,
  pub content_hash: String,
  pub created_at: DateTime<Utc>,
  pub embedding_status: String,
  pub file_name: String,
  pub media_type: String,
  pub size: SafeInt,
  pub workspace_id: String,
}

impl From<WorkspaceArtifactRecord> for CopilotWorkspaceArtifact {
  fn from(r: WorkspaceArtifactRecord) -> Self {
    Self {
      artifact_id: r.artifact_id,
      content_hash: r.content_hash,
      created_at: r.created_at,
      embedding_status: r.embedding_status,
      file_name: r.file_name,
      media_type: r.media_type,
      size: SafeInt(r.size),
      workspace_id: r.workspace_id,
    }
  }
}

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "CopilotWorkspaceArtifactTypeEdge")]
pub struct CopilotWorkspaceArtifactTypeEdge {
  pub cursor: String,
  pub node: CopilotWorkspaceArtifact,
}

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "PaginatedCopilotWorkspaceArtifactType")]
pub struct PaginatedCopilotWorkspaceArtifactType {
  pub edges: Vec<CopilotWorkspaceArtifactTypeEdge>,
  pub page_info: PageInfo,
  pub total_count: i32,
}

pub struct CopilotWorkspaceConfig {
  pub workspace_id: String,
}

fn parse_embedding_pagination(pagination: &PaginationInput) -> Result<(EmbeddingPagination, i32, i32)> {
  let first = pagination.resolved_first(20);
  let offset = pagination.resolved_offset();
  let after_raw = pagination.validate_and_resolve_after()?;
  let after_created_at = match after_raw {
    Some(raw) => {
      let trimmed = raw.trim().trim_matches('"');
      let parsed = DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
          trimmed
            .parse::<i64>()
            .ok()
            .and_then(DateTime::from_timestamp_millis)
            .ok_or(())
        })
        .map_err(|_| domain_error_to_graphql(&DomainError::Validation("invalid pagination cursor".into())))?;
      Some(parsed)
    }
    None => None,
  };
  Ok((
    EmbeddingPagination {
      first: Some(i64::from(first)),
      offset: Some(i64::from(offset)),
      after_created_at,
    },
    first,
    offset,
  ))
}

fn build_page_info(cursors: &[String], offset: i32, total_count: i64, has_after: bool) -> PageInfo {
  let start_cursor = cursors.first().cloned();
  let end_cursor = cursors.last().cloned();
  let returned = cursors.len() as i64;
  let has_next_page = if has_after {
    returned > 0 && returned < total_count
  } else {
    i64::from(offset) + returned < total_count
  };
  let has_previous_page = has_after || offset > 0;
  PageInfo {
    start_cursor,
    end_cursor,
    has_next_page,
    has_previous_page,
  }
}

#[Object(name = "CopilotWorkspaceConfig")]
impl CopilotWorkspaceConfig {
  async fn workspace_id(&self) -> &str {
    &self.workspace_id
  }

  async fn all_ignored_docs(&self, ctx: &Context<'_>) -> Result<Vec<CopilotWorkspaceIgnoredDoc>> {
    enforce_default_throttle_if_anonymous_graphql(ctx).await?;
    let state = ctx.data_unchecked::<Arc<AppState>>();
    state
      .extension
      .copilot_runtime()
      .assert_enabled()
      .await
      .map_err(|e| domain_error_to_graphql(&e))?;
    let rows = state
      .extension
      .copilot_embedding_spi()
      .list_all_ignored_docs(&self.workspace_id)
      .await
      .map_err(|e| domain_error_to_graphql(&e))?;
    Ok(rows.into_iter().map(CopilotWorkspaceIgnoredDoc::from).collect())
  }

  async fn ignored_docs(&self, ctx: &Context<'_>, pagination: PaginationInput) -> Result<PaginatedIgnoredDocsType> {
    enforce_default_throttle_if_anonymous_graphql(ctx).await?;
    let (spi_pagination, _first, offset) = parse_embedding_pagination(&pagination)?;
    let has_after = spi_pagination.after_created_at.is_some();
    let state = ctx.data_unchecked::<Arc<AppState>>();
    state
      .extension
      .copilot_runtime()
      .assert_enabled()
      .await
      .map_err(|e| domain_error_to_graphql(&e))?;
    let page = state
      .extension
      .copilot_embedding_spi()
      .list_ignored_docs(&self.workspace_id, spi_pagination)
      .await
      .map_err(|e| domain_error_to_graphql(&e))?;

    let mut cursors = Vec::with_capacity(page.items.len());
    let mut edges = Vec::with_capacity(page.items.len());
    for item in page.items {
      let cursor = encode_cursor(&item.created_at.to_rfc3339());
      cursors.push(cursor.clone());
      edges.push(CopilotWorkspaceIgnoredDocTypeEdge {
        cursor,
        node: CopilotWorkspaceIgnoredDoc::from(item),
      });
    }
    let page_info = build_page_info(&cursors, offset, page.total_count, has_after);
    Ok(PaginatedIgnoredDocsType {
      edges,
      page_info,
      total_count: i32::try_from(page.total_count).unwrap_or(i32::MAX),
    })
  }

  async fn artifacts(
    &self,
    ctx: &Context<'_>,
    pagination: PaginationInput,
  ) -> Result<PaginatedCopilotWorkspaceArtifactType> {
    enforce_default_throttle_if_anonymous_graphql(ctx).await?;
    let (spi_pagination, _first, offset) = parse_embedding_pagination(&pagination)?;
    let has_after = spi_pagination.after_created_at.is_some();
    let state = ctx.data_unchecked::<Arc<AppState>>();
    state
      .extension
      .copilot_runtime()
      .assert_enabled()
      .await
      .map_err(|e| domain_error_to_graphql(&e))?;
    let page = state
      .extension
      .copilot_embedding_spi()
      .list_artifacts(&self.workspace_id, spi_pagination)
      .await
      .map_err(|e| domain_error_to_graphql(&e))?;

    let mut cursors = Vec::with_capacity(page.items.len());
    let mut edges = Vec::with_capacity(page.items.len());
    for item in page.items {
      let cursor = encode_cursor(&item.created_at.to_rfc3339());
      cursors.push(cursor.clone());
      edges.push(CopilotWorkspaceArtifactTypeEdge {
        cursor,
        node: CopilotWorkspaceArtifact::from(item),
      });
    }
    let page_info = build_page_info(&cursors, offset, page.total_count, has_after);
    Ok(PaginatedCopilotWorkspaceArtifactType {
      edges,
      page_info,
      total_count: i32::try_from(page.total_count).unwrap_or(i32::MAX),
    })
  }
}

pub(crate) async fn resolve_workspace_embedding(
  ctx: &Context<'_>,
  workspace_id: &str,
) -> Result<CopilotWorkspaceConfig> {
  enforce_default_throttle_if_anonymous_graphql(ctx).await?;
  let state = ctx.data_unchecked::<Arc<AppState>>();
  state
    .extension
    .copilot_runtime()
    .assert_enabled()
    .await
    .map_err(|e| domain_error_to_graphql(&e))?;
  let user = require_authenticated_user(ctx).await?;
  let perms = state
    .workspaces
    .resolve_workspace_permissions(&user.id, workspace_id)
    .await
    .map_err(|e| domain_error_to_graphql(&e))?;
  if !perms.read {
    return Err(domain_error_to_graphql(&DomainError::Forbidden(format!(
      "space_access_denied:{workspace_id}"
    ))));
  }
  Ok(CopilotWorkspaceConfig {
    workspace_id: workspace_id.to_owned(),
  })
}

async fn require_workspace_settings_update<'a>(ctx: &'a Context<'_>, workspace_id: &str) -> Result<&'a Arc<AppState>> {
  enforce_default_throttle_if_anonymous_graphql(ctx).await?;
  let state = ctx.data_unchecked::<Arc<AppState>>();
  state
    .extension
    .copilot_runtime()
    .assert_enabled()
    .await
    .map_err(|e| domain_error_to_graphql(&e))?;
  let user = require_authenticated_user(ctx).await?;
  let perms = state
    .workspaces
    .resolve_workspace_permissions(&user.id, workspace_id)
    .await
    .map_err(|e| domain_error_to_graphql(&e))?;
  if !perms.settings_update {
    return Err(domain_error_to_graphql(&DomainError::Forbidden(format!(
      "space_access_denied:{workspace_id}"
    ))));
  }
  Ok(state)
}

#[derive(Default)]
pub struct EmbeddingMutation;

#[Object]
impl EmbeddingMutation {
  /// Add a workspace artifact
  async fn add_workspace_artifact(
    &self,
    ctx: &Context<'_>,
    blob: Upload,
    workspace_id: String,
  ) -> Result<CopilotWorkspaceArtifact> {
    let state = require_workspace_settings_update(ctx, &workspace_id).await?;
    let spi = state.extension.copilot_embedding_spi();
    if !spi.is_enabled() {
      return Err(domain_error_to_graphql(&DomainError::Forbidden(
        "copilot_embedding_unavailable".into(),
      )));
    }

    let mut upload = blob
      .value(ctx)
      .map_err(|e| domain_error_to_graphql(&DomainError::Validation(format!("invalid artifact upload: {e}"))))?;
    let file_name = upload.filename.clone();
    let mime_type = upload
      .content_type
      .as_deref()
      .filter(|s| !s.trim().is_empty())
      .unwrap_or("application/octet-stream")
      .to_owned();

    let mut bytes = Vec::new();
    let limit = (MAX_EMBEDDABLE_SIZE as u64).saturating_add(1);
    (&mut upload.content).take(limit).read_to_end(&mut bytes).map_err(|e| {
      domain_error_to_graphql(&DomainError::Internal(format!(
        "copilot_failed_to_add_workspace_artifact:{e}"
      )))
    })?;
    if bytes.len() > MAX_EMBEDDABLE_SIZE {
      return Err(domain_error_to_graphql(&DomainError::QuotaExceeded {
        kind: "blob_limit",
        message: "Blob size exceeds maximum embeddable limit.".into(),
      }));
    }

    let record = spi
      .add_workspace_artifact(AddWorkspaceArtifactInput {
        workspace_id,
        file_name,
        mime_type,
        bytes,
      })
      .await
      .map_err(|err| match err {
        DomainError::Unauthorized(_)
        | DomainError::Forbidden(_)
        | DomainError::QuotaExceeded { .. }
        | DomainError::RateLimited(_)
        | DomainError::Validation(_) => domain_error_to_graphql(&err),
        DomainError::Internal(ref msg) if msg.starts_with("copilot_failed_to_add_workspace_artifact:") => {
          domain_error_to_graphql(&err)
        }
        other => domain_error_to_graphql(&DomainError::Internal(format!(
          "copilot_failed_to_add_workspace_artifact:{other}"
        ))),
      })?;
    Ok(CopilotWorkspaceArtifact::from(record))
  }

  /// Remove a workspace artifact
  async fn remove_workspace_artifact(
    &self,
    ctx: &Context<'_>,
    artifact_id: String,
    workspace_id: String,
  ) -> Result<bool> {
    let state = require_workspace_settings_update(ctx, &workspace_id).await?;
    state
      .extension
      .copilot_embedding_spi()
      .remove_workspace_artifact(&workspace_id, &artifact_id)
      .await
      .map_err(|e| domain_error_to_graphql(&e))
  }

  /// Update ignored docs
  async fn update_workspace_embedding_ignored_docs(
    &self,
    ctx: &Context<'_>,
    add: Option<Vec<String>>,
    remove: Option<Vec<String>>,
    workspace_id: String,
  ) -> Result<i32> {
    let state = require_workspace_settings_update(ctx, &workspace_id).await?;
    state
      .extension
      .copilot_embedding_spi()
      .update_ignored_docs(&workspace_id, add.unwrap_or_default(), remove.unwrap_or_default())
      .await
      .map_err(|e| domain_error_to_graphql(&e))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::legacy_compat::errors::domain_error_to_envelope;

  #[test]
  fn community_embedding_error_extensions_match_contract() {
    let unavail = domain_error_to_envelope(&DomainError::Forbidden("copilot_embedding_unavailable".into()));
    assert_eq!(unavail.status, 403);
    assert_eq!(unavail.name, "COPILOT_EMBEDDING_UNAVAILABLE");
    assert_eq!(unavail.error_type, "ACTION_FORBIDDEN");

    let failed = domain_error_to_envelope(&DomainError::Internal(
      "copilot_failed_to_add_workspace_artifact:unsupported media type".into(),
    ));
    assert_eq!(failed.status, 500);
    assert_eq!(failed.name, "COPILOT_FAILED_TO_ADD_WORKSPACE_ARTIFACT");
    assert_eq!(failed.error_type, "INTERNAL_SERVER_ERROR");
    assert_eq!(
      failed.data,
      Some(serde_json::json!({ "message": "unsupported media type" }))
    );

    let gen_err = domain_error_to_envelope(&DomainError::Internal(
      "copilot_failed_to_generate_embedding:openai:upstream timeout".into(),
    ));
    assert_eq!(gen_err.status, 500);
    assert_eq!(gen_err.name, "COPILOT_FAILED_TO_GENERATE_EMBEDDING");
    assert_eq!(
      gen_err.data,
      Some(serde_json::json!({ "provider": "openai", "message": "upstream timeout" }))
    );
  }

  #[test]
  fn embedding_sdl_matches_frozen_v0275_contract() {
    let schema = async_graphql::Schema::build(
      super::super::QueryRoot::default(),
      super::super::MutationRoot::default(),
      async_graphql::EmptySubscription,
    )
    .finish();
    let sdl = schema.sdl();
    for expected in [
      "type CopilotWorkspaceConfig",
      "allIgnoredDocs: [CopilotWorkspaceIgnoredDoc!]!",
      "artifacts(pagination: PaginationInput!): PaginatedCopilotWorkspaceArtifactType!",
      "ignoredDocs(pagination: PaginationInput!): PaginatedIgnoredDocsType!",
      "type CopilotWorkspaceIgnoredDoc",
      "docId: String!",
      "createdByAvatar: String",
      "docCreatedAt: DateTime",
      "docUpdatedAt: DateTime",
      "type CopilotWorkspaceArtifact",
      "artifactId: String!",
      "contentHash: String!",
      "embeddingStatus: String!",
      "mediaType: String!",
      "size: SafeInt!",
      "embedding: CopilotWorkspaceConfig!",
      "addWorkspaceArtifact(blob: Upload!, workspaceId: String!): CopilotWorkspaceArtifact!",
      "removeWorkspaceArtifact(artifactId: String!, workspaceId: String!): Boolean!",
      "updateWorkspaceEmbeddingIgnoredDocs(add: [String!], remove: [String!], workspaceId: String!): Int!",
    ] {
      assert!(sdl.contains(expected), "missing expected SDL fragment: {expected}");
    }
  }
}
