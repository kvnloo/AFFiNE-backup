//! Copilot audio transcription GraphQL types, mutations, and deprecated
//! `Copilot.transcriptTask` query resolver aligned with `v0.27.5` `schema.gql`.

use std::io::Read as _;
use std::sync::Arc;

use async_graphql::{Context, Enum, ID, InputObject, Object, Result, SimpleObject, Upload};
use server_spi::{
  AudioSliceManifestItemRecord, AudioSliceUpload, MAX_TRANSCRIPT_SLICE_BYTES, MAX_TRANSCRIPT_SLICE_COUNT,
  MAX_TRANSCRIPT_TOTAL_BYTES, MeetingActionItemRecord, MeetingSummaryV2Record, NormalizedTranscriptSegmentRecord,
  SubmitTranscriptTaskSpiInput, TranscriptItemRecord, TranscriptJobStatus, TranscriptQualityRecord,
  TranscriptSourceAudioInputRecord, TranscriptSourceAudioRecord, TranscriptTaskRecord,
};
use server_types::DomainError;

use super::{enforce_default_throttle_if_anonymous_graphql, require_authenticated_user};
use crate::AppState;
use crate::legacy_compat::errors::domain_error_to_graphql;

#[derive(Enum, Copy, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "AiJobStatus")]
pub enum AiJobStatus {
  #[graphql(name = "claimed")]
  Claimed,
  #[graphql(name = "failed")]
  Failed,
  #[graphql(name = "finished")]
  Finished,
  #[graphql(name = "pending")]
  Pending,
  #[graphql(name = "running")]
  Running,
}

impl From<TranscriptJobStatus> for AiJobStatus {
  fn from(status: TranscriptJobStatus) -> Self {
    match status {
      TranscriptJobStatus::Pending => Self::Pending,
      TranscriptJobStatus::Running => Self::Running,
      TranscriptJobStatus::Finished => Self::Finished,
      TranscriptJobStatus::Failed => Self::Failed,
      TranscriptJobStatus::Claimed => Self::Claimed,
    }
  }
}

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "TranscriptionItemType")]
pub struct TranscriptionItemType {
  pub end: String,
  pub speaker: String,
  pub start: String,
  pub transcription: String,
}

impl From<TranscriptItemRecord> for TranscriptionItemType {
  fn from(item: TranscriptItemRecord) -> Self {
    Self {
      end: item.end,
      speaker: item.speaker,
      start: item.start,
      transcription: item.transcription,
    }
  }
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
#[graphql(name = "AudioSliceManifestItemType")]
pub struct AudioSliceManifestItemType {
  pub byte_size: Option<i32>,
  pub duration_sec: f64,
  pub file_name: String,
  pub index: i32,
  pub mime_type: String,
  pub start_sec: f64,
}

impl From<AudioSliceManifestItemRecord> for AudioSliceManifestItemType {
  fn from(item: AudioSliceManifestItemRecord) -> Self {
    Self {
      byte_size: item.byte_size,
      duration_sec: item.duration_sec,
      file_name: item.file_name,
      index: item.index,
      mime_type: item.mime_type,
      start_sec: item.start_sec,
    }
  }
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
#[graphql(name = "NormalizedTranscriptSegmentType")]
pub struct NormalizedTranscriptSegmentType {
  pub end: String,
  pub end_sec: f64,
  pub speaker: String,
  pub start: String,
  pub start_sec: f64,
  pub text: String,
}

impl From<NormalizedTranscriptSegmentRecord> for NormalizedTranscriptSegmentType {
  fn from(seg: NormalizedTranscriptSegmentRecord) -> Self {
    Self {
      end: seg.end,
      end_sec: seg.end_sec,
      speaker: seg.speaker,
      start: seg.start,
      start_sec: seg.start_sec,
      text: seg.text,
    }
  }
}

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "MeetingActionItemType")]
pub struct MeetingActionItemType {
  pub deadline: Option<String>,
  pub description: String,
  pub owner: Option<String>,
}

impl From<MeetingActionItemRecord> for MeetingActionItemType {
  fn from(item: MeetingActionItemRecord) -> Self {
    Self {
      deadline: item.deadline,
      description: item.description,
      owner: item.owner,
    }
  }
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
#[graphql(name = "MeetingSummaryV2Type")]
pub struct MeetingSummaryV2Type {
  pub action_items: Vec<MeetingActionItemType>,
  pub attendees: Vec<String>,
  pub blockers: Vec<String>,
  pub decisions: Vec<String>,
  pub duration_minutes: f64,
  pub key_points: Vec<String>,
  pub open_questions: Vec<String>,
  pub title: String,
}

impl From<MeetingSummaryV2Record> for MeetingSummaryV2Type {
  fn from(summary: MeetingSummaryV2Record) -> Self {
    Self {
      action_items: summary.action_items.into_iter().map(Into::into).collect(),
      attendees: summary.attendees,
      blockers: summary.blockers,
      decisions: summary.decisions,
      duration_minutes: summary.duration_minutes,
      key_points: summary.key_points,
      open_questions: summary.open_questions,
      title: summary.title,
    }
  }
}

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "TranscriptionSourceAudioType")]
pub struct TranscriptionSourceAudioType {
  pub blob_id: Option<String>,
  pub channels: Option<i32>,
  pub duration_ms: Option<i32>,
  pub mime_type: Option<String>,
  pub sample_rate: Option<i32>,
}

impl From<TranscriptSourceAudioRecord> for TranscriptionSourceAudioType {
  fn from(audio: TranscriptSourceAudioRecord) -> Self {
    Self {
      blob_id: audio.blob_id,
      channels: audio.channels,
      duration_ms: audio.duration_ms,
      mime_type: audio.mime_type,
      sample_rate: audio.sample_rate,
    }
  }
}

#[derive(SimpleObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "TranscriptionQualityType")]
pub struct TranscriptionQualityType {
  pub degraded: Option<bool>,
  pub overflow_count: Option<i32>,
}

impl From<TranscriptQualityRecord> for TranscriptionQualityType {
  fn from(quality: TranscriptQualityRecord) -> Self {
    Self {
      degraded: quality.degraded,
      overflow_count: quality.overflow_count,
    }
  }
}

#[derive(InputObject, Clone, Debug, PartialEq)]
#[graphql(name = "AudioSliceManifestItemInput")]
pub struct AudioSliceManifestItemInput {
  pub byte_size: Option<i32>,
  pub duration_sec: f64,
  pub file_name: String,
  pub index: i32,
  pub mime_type: String,
  pub start_sec: f64,
}

impl From<AudioSliceManifestItemInput> for AudioSliceManifestItemRecord {
  fn from(item: AudioSliceManifestItemInput) -> Self {
    Self {
      index: item.index,
      file_name: item.file_name,
      mime_type: item.mime_type,
      start_sec: item.start_sec,
      duration_sec: item.duration_sec,
      byte_size: item.byte_size,
    }
  }
}

#[derive(InputObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "TranscriptionSourceAudioInput")]
pub struct TranscriptionSourceAudioInput {
  pub channels: Option<i32>,
  pub duration_ms: Option<i32>,
  pub mime_type: Option<String>,
  pub sample_rate: Option<i32>,
}

impl From<TranscriptionSourceAudioInput> for TranscriptSourceAudioInputRecord {
  fn from(audio: TranscriptionSourceAudioInput) -> Self {
    Self {
      mime_type: audio.mime_type,
      duration_ms: audio.duration_ms,
      sample_rate: audio.sample_rate,
      channels: audio.channels,
    }
  }
}

#[derive(InputObject, Clone, Debug, Eq, PartialEq)]
#[graphql(name = "TranscriptionQualityInput")]
pub struct TranscriptionQualityInput {
  pub degraded: Option<bool>,
  pub overflow_count: Option<i32>,
}

impl From<TranscriptionQualityInput> for TranscriptQualityRecord {
  fn from(quality: TranscriptionQualityInput) -> Self {
    Self {
      degraded: quality.degraded,
      overflow_count: quality.overflow_count,
    }
  }
}

#[derive(InputObject, Clone, Debug, PartialEq)]
#[graphql(name = "SubmitAudioTranscriptionInput")]
pub struct SubmitAudioTranscriptionInput {
  pub quality: Option<TranscriptionQualityInput>,
  pub slice_manifest: Option<Vec<AudioSliceManifestItemInput>>,
  pub source_audio: Option<TranscriptionSourceAudioInput>,
}

#[derive(SimpleObject, Clone, Debug, PartialEq)]
#[graphql(name = "TranscriptionResultType")]
pub struct TranscriptionResultType {
  pub actions: Option<String>,
  pub id: ID,
  pub normalized_segments: Option<Vec<NormalizedTranscriptSegmentType>>,
  pub normalized_transcript: Option<String>,
  pub quality: Option<TranscriptionQualityType>,
  pub slice_manifest: Option<Vec<AudioSliceManifestItemType>>,
  pub source_audio: Option<TranscriptionSourceAudioType>,
  pub status: AiJobStatus,
  pub summary: Option<String>,
  pub summary_json: Option<MeetingSummaryV2Type>,
  pub title: Option<String>,
  pub transcription: Option<Vec<TranscriptionItemType>>,
  pub version: Option<String>,
}

impl From<TranscriptTaskRecord> for TranscriptionResultType {
  fn from(record: TranscriptTaskRecord) -> Self {
    let status: AiJobStatus = record.status.into();
    let is_finished = matches!(status, AiJobStatus::Finished | AiJobStatus::Claimed);
    if !is_finished {
      return Self {
        id: ID(record.id),
        status,
        actions: None,
        normalized_segments: None,
        normalized_transcript: None,
        quality: None,
        slice_manifest: None,
        source_audio: None,
        summary: None,
        summary_json: None,
        title: None,
        transcription: None,
        version: None,
      };
    }
    Self {
      actions: record.actions,
      id: ID(record.id),
      normalized_segments: record
        .normalized_segments
        .map(|items| items.into_iter().map(Into::into).collect()),
      normalized_transcript: record.normalized_transcript,
      quality: record.quality.map(Into::into),
      slice_manifest: record
        .slice_manifest
        .map(|items| items.into_iter().map(Into::into).collect()),
      source_audio: record.source_audio.map(Into::into),
      status,
      summary: record.summary,
      summary_json: record.summary_json.map(Into::into),
      title: record.title,
      transcription: record
        .transcription
        .map(|items| items.into_iter().map(Into::into).collect()),
      version: record.version,
    }
  }
}

fn gql_err(e: DomainError) -> async_graphql::Error {
  domain_error_to_graphql(&e)
}

async fn enforce_copilot_enabled(state: &AppState) -> std::result::Result<(), DomainError> {
  state.extension.copilot_runtime().assert_enabled().await?;
  if let Ok(cfg) = state.app_config.get_effective_app_config().await
    && cfg.pointer("/copilot/enabled").and_then(serde_json::Value::as_bool) == Some(false)
  {
    return Err(DomainError::Forbidden("Copilot is disabled.".to_owned()));
  }
  Ok(())
}

fn read_bounded_upload(
  ctx: &Context<'_>,
  upload: Upload,
  remaining_total: usize,
) -> std::result::Result<AudioSliceUpload, DomainError> {
  let value = upload
    .value(ctx)
    .map_err(|e| DomainError::Validation(format!("invalid transcript audio upload: {e}")))?;
  let file_name = value.filename.clone();
  let mime_type = value
    .content_type
    .as_deref()
    .filter(|s| !s.trim().is_empty())
    .unwrap_or("application/octet-stream")
    .to_owned();
  let max_read = MAX_TRANSCRIPT_SLICE_BYTES.min(remaining_total);
  let mut bytes = Vec::new();
  value
    .content
    .take((max_read as u64).saturating_add(1))
    .read_to_end(&mut bytes)
    .map_err(|e| DomainError::Validation(format!("failed to read transcript audio upload: {e}")))?;
  if bytes.len() > MAX_TRANSCRIPT_SLICE_BYTES {
    return Err(DomainError::Validation(format!(
      "transcript audio slice exceeds maximum size of {MAX_TRANSCRIPT_SLICE_BYTES} bytes"
    )));
  }
  if bytes.len() > remaining_total {
    return Err(DomainError::Validation(format!(
      "transcript audio total size exceeds maximum of {MAX_TRANSCRIPT_TOTAL_BYTES} bytes"
    )));
  }
  Ok(AudioSliceUpload {
    file_name,
    mime_type,
    bytes,
  })
}

/// Shared resolver for `Copilot.transcriptTask(blobId, taskId)`.
pub async fn resolve_copilot_transcript_task(
  ctx: &Context<'_>,
  workspace_id: Option<&str>,
  blob_id: Option<String>,
  task_id: Option<String>,
) -> Result<Option<TranscriptionResultType>> {
  enforce_default_throttle_if_anonymous_graphql(ctx).await?;
  let user = require_authenticated_user(ctx).await?;
  let state = Arc::clone(ctx.data_unchecked::<Arc<AppState>>());
  enforce_copilot_enabled(&state).await.map_err(gql_err)?;
  let Some(workspace_id) = workspace_id.filter(|id| !id.is_empty()) else {
    return Ok(None);
  };
  if task_id.is_none() && blob_id.is_none() {
    return Ok(None);
  }
  let task = state
    .extension
    .copilot_transcript_spi()
    .query_transcript_task(&user.id, workspace_id, task_id.as_deref(), blob_id.as_deref())
    .await
    .map_err(gql_err)?;
  Ok(task.map(Into::into))
}

/// Standalone Copilot transcript view exposing the deprecated `transcriptTask`
/// field for composition into `type Copilot`.
#[derive(Clone, Debug, Default)]
pub struct TranscriptCopilotView {
  pub workspace_id: Option<String>,
}

#[Object]
impl TranscriptCopilotView {
  #[graphql(deprecation = "Use realtime subscription \"copilot.transcript.task.changed\" instead.")]
  async fn transcript_task(
    &self,
    ctx: &Context<'_>,
    blob_id: Option<String>,
    task_id: Option<String>,
  ) -> Result<Option<TranscriptionResultType>> {
    resolve_copilot_transcript_task(ctx, self.workspace_id.as_deref(), blob_id, task_id).await
  }
}

#[derive(Default)]
pub struct TranscriptMutation;

#[Object]
impl TranscriptMutation {
  async fn submit_transcript_task(
    &self,
    ctx: &Context<'_>,
    blob: Option<Upload>,
    blob_id: String,
    blobs: Option<Vec<Upload>>,
    input: Option<SubmitAudioTranscriptionInput>,
    workspace_id: String,
  ) -> Result<Option<TranscriptionResultType>> {
    enforce_default_throttle_if_anonymous_graphql(ctx).await?;
    let user = require_authenticated_user(ctx).await?;
    let state = Arc::clone(ctx.data_unchecked::<Arc<AppState>>());
    enforce_copilot_enabled(&state).await.map_err(gql_err)?;

    let mut uploads = Vec::new();
    if let Some(single) = blob {
      uploads.push(single);
    }
    if let Some(list) = blobs {
      uploads.extend(list);
    }
    if uploads.is_empty() {
      return Err(gql_err(DomainError::Validation(
        "copilot_transcription_audio_not_provided".to_owned(),
      )));
    }
    if uploads.len() > MAX_TRANSCRIPT_SLICE_COUNT {
      return Err(gql_err(DomainError::Validation(format!(
        "transcript audio slice count exceeds maximum of {MAX_TRANSCRIPT_SLICE_COUNT}"
      ))));
    }

    let mut remaining_total = MAX_TRANSCRIPT_TOTAL_BYTES;
    let mut slices = Vec::with_capacity(uploads.len());
    for upload in uploads {
      let slice = read_bounded_upload(ctx, upload, remaining_total).map_err(gql_err)?;
      remaining_total = remaining_total.saturating_sub(slice.bytes.len());
      slices.push(slice);
    }

    let (source_audio, quality, slice_manifest) = match input {
      Some(v) => (
        v.source_audio.map(Into::into),
        v.quality.map(Into::into),
        v.slice_manifest.map(|list| list.into_iter().map(Into::into).collect()),
      ),
      None => (None, None, None),
    };

    let task = state
      .extension
      .copilot_transcript_spi()
      .submit_transcript_task(SubmitTranscriptTaskSpiInput {
        user_id: user.id,
        workspace_id,
        blob_id,
        blobs: slices,
        source_audio,
        quality,
        slice_manifest,
      })
      .await
      .map_err(gql_err)?;
    Ok(task.map(Into::into))
  }

  async fn retry_transcript_task(
    &self,
    ctx: &Context<'_>,
    task_id: String,
    workspace_id: String,
  ) -> Result<Option<TranscriptionResultType>> {
    enforce_default_throttle_if_anonymous_graphql(ctx).await?;
    let user = require_authenticated_user(ctx).await?;
    let state = Arc::clone(ctx.data_unchecked::<Arc<AppState>>());
    enforce_copilot_enabled(&state).await.map_err(gql_err)?;
    let task = state
      .extension
      .copilot_transcript_spi()
      .retry_transcript_task(&user.id, &workspace_id, &task_id)
      .await
      .map_err(gql_err)?;
    Ok(task.map(Into::into))
  }

  async fn settle_transcript_task(
    &self,
    ctx: &Context<'_>,
    task_id: String,
    workspace_id: String,
  ) -> Result<Option<TranscriptionResultType>> {
    enforce_default_throttle_if_anonymous_graphql(ctx).await?;
    let user = require_authenticated_user(ctx).await?;
    let state = Arc::clone(ctx.data_unchecked::<Arc<AppState>>());
    enforce_copilot_enabled(&state).await.map_err(gql_err)?;
    let task = state
      .extension
      .copilot_transcript_spi()
      .settle_transcript_task(&user.id, &workspace_id, &task_id)
      .await
      .map_err(gql_err)?;
    Ok(task.map(Into::into))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use async_graphql::{EmptySubscription, Schema};

  #[test]
  fn unfinished_transcript_record_strips_result_payload() {
    let pending = TranscriptTaskRecord {
      id: "task-1".into(),
      status: TranscriptJobStatus::Pending,
      title: Some("secret".into()),
      summary: Some("secret".into()),
      actions: Some("secret".into()),
      transcription: Some(vec![TranscriptItemRecord {
        speaker: "A".into(),
        start: "00:00:00".into(),
        end: "00:00:05".into(),
        transcription: "hello".into(),
      }]),
      source_audio: None,
      quality: None,
      slice_manifest: None,
      normalized_segments: None,
      normalized_transcript: Some("00:00:00 A: hello".into()),
      summary_json: None,
      version: Some("transcript-result-v1".into()),
    };
    let gql: TranscriptionResultType = pending.into();
    assert_eq!(gql.id, ID("task-1".into()));
    assert_eq!(gql.status, AiJobStatus::Pending);
    assert!(gql.title.is_none());
    assert!(gql.summary.is_none());
    assert!(gql.transcription.is_none());
    assert!(gql.normalized_transcript.is_none());
  }

  #[test]
  fn transcript_copilot_view_sdl_contains_deprecated_transcript_task() {
    let schema = Schema::build(TranscriptCopilotView::default(), TranscriptMutation, EmptySubscription).finish();
    let sdl = schema.sdl();
    assert!(
      sdl.contains("transcriptTask(blobId: String, taskId: String): TranscriptionResultType")
        && sdl.contains("@deprecated(reason:")
        && sdl.contains("copilot.transcript.task.changed"),
      "unexpected SDL:\n{sdl}"
    );
    assert!(sdl.contains("enum AiJobStatus"));
    assert!(sdl.contains("submitTranscriptTask("));
    assert!(sdl.contains("retryTranscriptTask("));
    assert!(sdl.contains("settleTranscriptTask("));
  }

  #[test]
  fn transcript_error_codes_match_v0275_bad_request_envelopes() {
    use async_graphql::Value;

    let cases = [
      (
        "copilot_transcription_audio_not_provided",
        "COPILOT_TRANSCRIPTION_AUDIO_NOT_PROVIDED",
      ),
      ("copilot_transcription_job_exists", "COPILOT_TRANSCRIPTION_JOB_EXISTS"),
      (
        "copilot_transcription_job_not_found",
        "COPILOT_TRANSCRIPTION_JOB_NOT_FOUND",
      ),
    ];
    for (raw, expected_name) in cases {
      let err = gql_err(DomainError::Validation(raw.to_owned()));
      let ext = err.extensions.as_ref().expect("extensions present");
      assert_eq!(ext.get("status"), Some(&Value::from(400)), "raw={raw}");
      assert_eq!(ext.get("type"), Some(&Value::String("BAD_REQUEST".into())), "raw={raw}");
      assert_eq!(ext.get("name"), Some(&Value::String(expected_name.into())), "raw={raw}");
    }
  }
}
