mod handlers;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::{
    BroadcastDocInvalidationMessage, BroadcastDocUpdatesMessage, DEFAULT_CLIENT_VERSION,
    DeleteDocMessage, DocLifecycleAction, DocLifecycleMessage, DocLifecycleResponse,
    EngineIoClient, EngineIoConfig, EngineIoOutput, JoinSpaceBatchEntry, JoinSpaceBatchMessage,
    JoinSpaceBatchResponse, LeaveSpaceBatchMessage, LoadDocMessage, LoadDocResponse,
    LoadDocTimestampsMessage, PushDocIntent, PushDocUpdateMessage, PushDocUpdateResponse,
    SPACE_BROADCAST_DOC_INVALIDATION, SPACE_BROADCAST_DOC_UPDATES, SPACE_DELETE_DOC,
    SPACE_DOC_LIFECYCLE, SPACE_JOIN_BATCH, SPACE_JOIN_BATCH_LIMIT, SPACE_LEAVE_BATCH,
    SPACE_LOAD_DOC, SPACE_LOAD_DOC_TIMESTAMPS, SPACE_PUSH_DOC_UPDATE, SocketIoClient,
    SocketIoEvent, SpaceType, TransportMode, WebsocketResponse,
};
use crate::{
    error::{StoreError, StoreResult},
    id_converter::IdConverter,
    workspace_db::WORKSPACE_SYNC_PROTOCOL_VERSION,
};

/// Deterministic exponential backoff policy for reconnects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy {
    pub initial_delay_ms: i64,
    pub max_delay_ms: i64,
    pub ack_timeout_ms: i64,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay_ms: 1_000,
            max_delay_ms: 30_000,
            ack_timeout_ms: 15_000,
        }
    }
}

impl ReconnectPolicy {
    pub fn backoff_ms(&self, attempt: u32) -> i64 {
        let shift = attempt.min(20);
        let factor = 1i64.checked_shl(shift).unwrap_or(i64::MAX);
        self.initial_delay_ms
            .saturating_mul(factor)
            .min(self.max_delay_ms)
    }
}

/// Configuration for [`SpaceSyncEngine`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpaceSyncConfig {
    pub space_type: SpaceType,
    pub space_id: String,
    pub client_version: String,
    pub protocol_version: u8,
    pub transport_mode: TransportMode,
    pub reconnect: ReconnectPolicy,
}

impl SpaceSyncConfig {
    pub fn workspace(space_id: impl Into<String>, is_self_hosted: bool) -> Self {
        Self {
            space_type: SpaceType::Workspace,
            space_id: space_id.into(),
            client_version: DEFAULT_CLIENT_VERSION.to_string(),
            protocol_version: WORKSPACE_SYNC_PROTOCOL_VERSION,
            transport_mode: if is_self_hosted {
                TransportMode::PollingThenWebsocket
            } else {
                TransportMode::WebsocketOnly
            },
            reconnect: ReconnectPolicy::default(),
        }
    }
}

/// Persisted offline update queued for ordered replay once connected and joined.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OfflinePendingUpdate {
    pub doc_id: String,
    pub update: Vec<u8>,
    pub intent: PushDocIntent,
}

/// Connection and join lifecycle state of [`SpaceSyncEngine`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpaceSyncState {
    Offline,
    Connecting,
    Joining,
    Ready,
    ReconnectScheduled { next_attempt_at_ms: i64 },
    PermissionRevoked,
}

/// Outbound transport commands emitted by [`SpaceSyncEngine`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpaceSyncOutbound {
    Transport(EngineIoOutput),
    ScheduleTimerAtMs(i64),
}

/// Domain events delivered to the application by [`SpaceSyncEngine`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpaceSyncEvent {
    Joined {
        client_id: String,
        protocol_version: u8,
    },
    DocsJoined {
        doc_ids: Vec<String>,
    },
    DocLoaded {
        doc_id: String,
        missing: Vec<u8>,
        state: Vec<u8>,
        timestamp: i64,
    },
    DocNotFound {
        doc_id: String,
    },
    DocUpdatePushed {
        doc_id: String,
        timestamp: i64,
        created: bool,
    },
    DocTimestampsLoaded {
        timestamps: BTreeMap<String, i64>,
    },
    DocDeleted {
        doc_id: String,
    },
    DocLifecycleApplied {
        doc_id: String,
        lifecycle: DocLifecycleAction,
        timestamp: i64,
        deleted_doc_ids: Option<Vec<String>>,
    },
    RemoteDocUpdates {
        doc_id: String,
        updates: Vec<Vec<u8>>,
        timestamp: i64,
        editor: Option<String>,
    },
    DocInvalidated {
        timestamp: Option<i64>,
        inventory_changed: bool,
    },
    PermissionRevoked {
        doc_id: Option<String>,
        code: String,
        message: String,
    },
    RequestFailed {
        op: &'static str,
        doc_id: Option<String>,
        code: String,
        message: String,
    },
    Disconnected {
        reason: String,
        next_reconnect_ms: i64,
    },
}

pub(super) enum DeferredAfterJoin {
    LoadDoc {
        doc_id: String,
        state_vector: Option<Vec<u8>>,
        retried_not_in_space: bool,
    },
    PushDocUpdate {
        entry: OfflinePendingUpdate,
        retried_not_in_space: bool,
    },
}

pub(super) enum InFlightRequest {
    InitialJoin {
        root_wire_id: String,
    },
    JoinDocs {
        wire_doc_ids: Vec<String>,
        deferred: Option<DeferredAfterJoin>,
    },
    LoadDoc {
        doc_id: String,
        wire_doc_id: String,
        state_vector: Option<Vec<u8>>,
        retried_not_in_space: bool,
    },
    PushDocUpdate {
        entry: OfflinePendingUpdate,
        wire_doc_id: String,
        retried_not_in_space: bool,
    },
    LoadDocTimestamps,
    DeleteDoc {
        doc_id: String,
    },
    DocLifecycle {
        doc_id: String,
        lifecycle: DocLifecycleAction,
    },
}

pub(super) struct PendingAck {
    pub(super) sent_at_ms: i64,
    pub(super) request: InFlightRequest,
}

/// Pure deterministic sans-I/O state machine for AFFiNE `SpaceSync` over Engine.IO v4 + Socket.IO v4.
pub struct SpaceSyncEngine {
    pub(super) config: SpaceSyncConfig,
    pub(super) state: SpaceSyncState,
    pub(super) engine_io: EngineIoClient,
    pub(super) socket_io: SocketIoClient,
    pub(super) id_converter: IdConverter,
    pub(super) joined_doc_ids: BTreeSet<String>,
    pub(super) pending_deletes: BTreeMap<String, String>,
    pub(super) offline_queue: VecDeque<OfflinePendingUpdate>,
    pub(super) in_flight: BTreeMap<u64, PendingAck>,
    pub(super) reconnect_attempts: u32,
    pub(super) outbound: Vec<SpaceSyncOutbound>,
    pub(super) events: Vec<SpaceSyncEvent>,
}

impl SpaceSyncEngine {
    pub fn new(config: SpaceSyncConfig, id_converter: IdConverter) -> Self {
        let engine_io = EngineIoClient::new(EngineIoConfig {
            transport_mode: config.transport_mode,
        });
        Self {
            config,
            state: SpaceSyncState::Offline,
            engine_io,
            socket_io: SocketIoClient::new(),
            id_converter,
            joined_doc_ids: BTreeSet::new(),
            pending_deletes: BTreeMap::new(),
            offline_queue: VecDeque::new(),
            in_flight: BTreeMap::new(),
            reconnect_attempts: 0,
            outbound: Vec::new(),
            events: Vec::new(),
        }
    }

    pub fn state(&self) -> SpaceSyncState {
        self.state
    }

    pub fn id_converter(&self) -> &IdConverter {
        &self.id_converter
    }

    pub fn set_id_converter(&mut self, converter: IdConverter) {
        self.id_converter = converter;
    }

    /// Return the set of wire document IDs currently subscribed on this connection.
    pub fn joined_wire_doc_ids(&self) -> &BTreeSet<String> {
        &self.joined_doc_ids
    }

    /// Snapshot the offline queue for persistence across restarts.
    pub fn offline_queue(&self) -> &VecDeque<OfflinePendingUpdate> {
        &self.offline_queue
    }

    /// Restore a previously persisted offline queue before connecting.
    pub fn restore_offline_queue(&mut self, pending: Vec<OfflinePendingUpdate>) {
        self.offline_queue.extend(pending);
    }

    /// Begin connecting to the remote sync server at deterministic timestamp `now_ms`.
    pub fn connect(&mut self, now_ms: i64) {
        if self.state == SpaceSyncState::PermissionRevoked {
            return;
        }
        self.state = SpaceSyncState::Connecting;
        self.engine_io.connect(now_ms);
        self.flush_engine_io_outputs(now_ms);
    }

    /// Explicitly subscribe to a list of local document IDs in batches of `SPACE_JOIN_BATCH_LIMIT`.
    pub fn join_docs(&mut self, local_doc_ids: &[&str], now_ms: i64) -> StoreResult<Vec<u64>> {
        self.ensure_ready()?;
        let wire_ids: Vec<String> = local_doc_ids
            .iter()
            .map(|id| self.id_converter.new_id_to_old_id(id))
            .filter(|wire| !self.joined_doc_ids.contains(wire))
            .collect();
        let mut ack_ids = Vec::new();
        for chunk in wire_ids.chunks(SPACE_JOIN_BATCH_LIMIT) {
            let ack = self.send_join_docs_batch(chunk.to_vec(), None, now_ms)?;
            ack_ids.push(ack);
        }
        Ok(ack_ids)
    }

    /// Emit `space:leave-batch` in batches of `SPACE_JOIN_BATCH_LIMIT` followed by an empty
    /// `docIds: []` leave packet, matching `CloudDocStorageConnection.doDisconnect`.
    pub fn leave_space(&mut self, now_ms: i64) -> StoreResult<()> {
        let doc_ids: Vec<String> = std::mem::take(&mut self.joined_doc_ids)
            .into_iter()
            .collect();
        for chunk in doc_ids.chunks(SPACE_JOIN_BATCH_LIMIT) {
            let msg = LeaveSpaceBatchMessage {
                space_type: self.config.space_type,
                space_id: self.config.space_id.clone(),
                doc_ids: chunk.to_vec(),
            };
            let pkt = self
                .socket_io
                .encode_event(SPACE_LEAVE_BATCH, serde_json::to_value(msg)?)?;
            self.engine_io.send_message(&pkt)?;
        }
        let final_msg = LeaveSpaceBatchMessage {
            space_type: self.config.space_type,
            space_id: self.config.space_id.clone(),
            doc_ids: Vec::new(),
        };
        let pkt = self
            .socket_io
            .encode_event(SPACE_LEAVE_BATCH, serde_json::to_value(final_msg)?)?;
        self.engine_io.send_message(&pkt)?;
        self.flush_engine_io_outputs(now_ms);
        Ok(())
    }

    /// Queue or immediately push a local document update.
    ///
    /// When `protocol_version != 1` and `intent == Update`, automatically joins the document
    /// first if it is not yet in `joined_doc_ids`.
    pub fn push_doc_update(
        &mut self,
        doc_id: impl Into<String>,
        update: Vec<u8>,
        intent: PushDocIntent,
        now_ms: i64,
    ) -> StoreResult<()> {
        let entry = OfflinePendingUpdate {
            doc_id: doc_id.into(),
            update,
            intent,
        };
        if self.state == SpaceSyncState::Ready {
            self.dispatch_push_doc_update(entry, false, now_ms)
        } else {
            self.offline_queue.push_back(entry);
            Ok(())
        }
    }

    /// Request a document snapshot/diff via `space:load-doc`.
    ///
    /// Automatically sends `space:join-batch` first if the document is not yet subscribed on
    /// this connection, then dispatches `space:load-doc` once the join ACK succeeds.
    pub fn load_doc(
        &mut self,
        doc_id: &str,
        state_vector: Option<&[u8]>,
        now_ms: i64,
    ) -> StoreResult<u64> {
        self.ensure_ready()?;
        self.dispatch_load_doc(
            doc_id.to_string(),
            state_vector.map(|sv| sv.to_vec()),
            false,
            now_ms,
        )
    }

    /// Request document timestamps via `space:load-doc-timestamps`.
    pub fn load_doc_timestamps(&mut self, after_ms: Option<i64>, now_ms: i64) -> StoreResult<u64> {
        self.ensure_ready()?;
        let msg = LoadDocTimestampsMessage {
            space_type: self.config.space_type,
            space_id: self.config.space_id.clone(),
            timestamp: after_ms,
        };
        let (ack_id, packet) = self
            .socket_io
            .encode_event_with_ack(SPACE_LOAD_DOC_TIMESTAMPS, serde_json::to_value(msg)?)?;
        self.in_flight.insert(
            ack_id,
            PendingAck {
                sent_at_ms: now_ms,
                request: InFlightRequest::LoadDocTimestamps,
            },
        );
        self.engine_io.send_message(&packet)?;
        self.flush_engine_io_outputs(now_ms);
        Ok(ack_id)
    }

    /// Request document deletion via `space:delete-doc`.
    pub fn delete_doc(&mut self, doc_id: &str, now_ms: i64) -> StoreResult<u64> {
        self.ensure_ready()?;
        let msg = DeleteDocMessage {
            space_type: self.config.space_type,
            space_id: self.config.space_id.clone(),
            doc_id: self.id_converter.new_id_to_old_id(doc_id),
        };
        let (ack_id, packet) = self
            .socket_io
            .encode_event_with_ack(SPACE_DELETE_DOC, serde_json::to_value(msg)?)?;
        self.in_flight.insert(
            ack_id,
            PendingAck {
                sent_at_ms: now_ms,
                request: InFlightRequest::DeleteDoc {
                    doc_id: doc_id.to_string(),
                },
            },
        );
        self.engine_io.send_message(&packet)?;
        self.flush_engine_io_outputs(now_ms);
        Ok(ack_id)
    }

    /// Apply a document lifecycle transition (`trash`, `restore`, `delete`) via `space:doc-lifecycle`,
    /// preserving `physicalDocId` across delete retries matching `CloudDocStorage.applyDocLifecycle`.
    pub fn apply_doc_lifecycle(
        &mut self,
        doc_id: &str,
        lifecycle: DocLifecycleAction,
        now_ms: i64,
    ) -> StoreResult<u64> {
        self.ensure_ready()?;
        let (wire_doc_id, physical_doc_id) = match lifecycle {
            DocLifecycleAction::Delete => {
                let phys = self
                    .pending_deletes
                    .entry(doc_id.to_string())
                    .or_insert_with(|| self.id_converter.new_id_to_old_id(doc_id))
                    .clone();
                (doc_id.to_string(), Some(phys))
            }
            DocLifecycleAction::Trash | DocLifecycleAction::Restore => {
                (self.id_converter.new_id_to_old_id(doc_id), None)
            }
        };
        let msg = DocLifecycleMessage {
            space_type: self.config.space_type,
            space_id: self.config.space_id.clone(),
            doc_id: wire_doc_id,
            lifecycle,
            physical_doc_id,
        };
        let (ack_id, packet) = self
            .socket_io
            .encode_event_with_ack(SPACE_DOC_LIFECYCLE, serde_json::to_value(msg)?)?;
        self.in_flight.insert(
            ack_id,
            PendingAck {
                sent_at_ms: now_ms,
                request: InFlightRequest::DocLifecycle {
                    doc_id: doc_id.to_string(),
                    lifecycle,
                },
            },
        );
        self.engine_io.send_message(&packet)?;
        self.flush_engine_io_outputs(now_ms);
        Ok(ack_id)
    }

    /// Feed an HTTP long-polling response body at `now_ms`.
    pub fn on_polling_response(&mut self, body: &str, now_ms: i64) -> StoreResult<()> {
        self.engine_io.on_polling_response(body, now_ms)?;
        self.flush_engine_io_outputs(now_ms);
        Ok(())
    }

    /// Notify that the WebSocket connection opened at `now_ms`.
    pub fn on_ws_open(&mut self, now_ms: i64) {
        self.engine_io.on_ws_open(now_ms);
        self.flush_engine_io_outputs(now_ms);
    }

    /// Notify that the WebSocket transport failed or closed.
    ///
    /// During a polling-to-WebSocket upgrade probe, Engine.IO smoothly degrades to
    /// `PollingOpen` without disconnecting `SpaceSyncEngine`.
    pub fn on_ws_error(&mut self, reason: &str, now_ms: i64) {
        self.engine_io.on_ws_error(reason, now_ms);
        self.flush_engine_io_outputs(now_ms);
    }

    /// Feed an inbound WebSocket text frame at `now_ms`.
    pub fn on_ws_text(&mut self, frame: &str, now_ms: i64) -> StoreResult<()> {
        self.engine_io.on_ws_text(frame, now_ms)?;
        self.flush_engine_io_outputs(now_ms);
        Ok(())
    }

    /// Feed an inbound WebSocket binary frame at `now_ms`.
    pub fn on_ws_binary(&mut self, bytes: Vec<u8>, now_ms: i64) -> StoreResult<()> {
        self.engine_io.on_ws_binary(bytes, now_ms);
        self.flush_engine_io_outputs(now_ms);
        Ok(())
    }

    /// Notify the engine that the underlying transport dropped at `now_ms`.
    pub fn on_transport_error(&mut self, reason: &str, now_ms: i64) {
        self.handle_disconnect(reason, now_ms);
    }

    /// Advance deterministic timers (heartbeat timeout, ACK timeout retries, reconnect backoff).
    pub fn on_tick(&mut self, now_ms: i64) -> StoreResult<()> {
        if let SpaceSyncState::ReconnectScheduled { next_attempt_at_ms } = self.state {
            if now_ms >= next_attempt_at_ms {
                self.connect(now_ms);
            }
            return Ok(());
        }

        if self.engine_io.poll_timeout(now_ms) {
            self.flush_engine_io_outputs(now_ms);
            return Ok(());
        }

        let timed_out_ids: Vec<u64> = self
            .in_flight
            .iter()
            .filter(|(_, pending)| {
                now_ms.saturating_sub(pending.sent_at_ms) >= self.config.reconnect.ack_timeout_ms
            })
            .map(|(id, _)| *id)
            .collect();

        for ack_id in timed_out_ids {
            if let Some(pending) = self.in_flight.remove(&ack_id) {
                self.handle_ack_timeout(pending.request, now_ms)?;
            }
        }

        Ok(())
    }

    pub fn drain_outbound(&mut self) -> Vec<SpaceSyncOutbound> {
        std::mem::take(&mut self.outbound)
    }

    pub fn drain_events(&mut self) -> Vec<SpaceSyncEvent> {
        std::mem::take(&mut self.events)
    }

    fn ensure_ready(&self) -> StoreResult<()> {
        if self.state != SpaceSyncState::Ready {
            return Err(StoreError::InvalidProtocol(
                "SpaceSyncEngine is not in Ready state".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    use super::*;

    fn connect_and_join_ws(engine: &mut SpaceSyncEngine) {
        engine.connect(1_000);
        let _ = engine.drain_outbound();
        let open =
            r#"0{"sid":"e1","upgrades":["websocket"],"pingInterval":25000,"pingTimeout":20000}"#;
        engine.on_polling_response(open, 1_050).unwrap();
        engine.on_ws_open(1_100);
        engine.on_ws_text("3probe", 1_150).unwrap();
        engine.on_ws_text(r#"40{"sid":"sio-1"}"#, 1_200).unwrap();
        let _ = engine.drain_outbound();
        engine
            .on_ws_text(
                r#"430[{"data":{"clientId":"c1","success":true,"protocolVersion":1}}]"#,
                1_300,
            )
            .unwrap();
        let _ = engine.drain_events();
        let _ = engine.drain_outbound();
    }

    #[test]
    fn offline_queue_replay_ack_timeout_retry_and_permission_revocation() {
        let config = SpaceSyncConfig::workspace("ws-1", true);
        let mut engine = SpaceSyncEngine::new(config, IdConverter::new("ws-1"));

        engine
            .push_doc_update("doc-1", vec![1, 2, 3], PushDocIntent::Update, 100)
            .unwrap();
        assert_eq!(engine.offline_queue().len(), 1);

        engine.connect(1_000);
        let _ = engine.drain_outbound();
        let open =
            r#"0{"sid":"e1","upgrades":["websocket"],"pingInterval":25000,"pingTimeout":20000}"#;
        engine.on_polling_response(open, 1_050).unwrap();
        engine.on_ws_open(1_100);
        engine.on_ws_text("3probe", 1_150).unwrap();
        engine.on_ws_text(r#"40{"sid":"sio-1"}"#, 1_200).unwrap();
        let out = engine.drain_outbound();
        assert!(out.iter().any(|o| matches!(
            o,
            SpaceSyncOutbound::Transport(EngineIoOutput::SendWsText(t))
                if t.contains(SPACE_JOIN_BATCH) && t.contains("\"clientVersion\":\"0.27.5\"")
        )));

        engine
            .on_ws_text(
                r#"430[{"data":{"clientId":"c1","success":true,"protocolVersion":1}}]"#,
                1_300,
            )
            .unwrap();
        assert_eq!(engine.state(), SpaceSyncState::Ready);
        assert!(engine.joined_wire_doc_ids().contains("ws-1"));
        assert!(engine.offline_queue().is_empty());

        // ACK timeout retries push_doc_update with new ack_id = 2.
        let _ = engine.drain_outbound();
        engine.on_tick(1_300 + 15_000).unwrap();
        let retry_out = engine.drain_outbound();
        assert!(retry_out.iter().any(|o| matches!(
            o,
            SpaceSyncOutbound::Transport(EngineIoOutput::SendWsText(t))
                if t.starts_with("422[\"space:push-doc-update\"")
        )));

        engine
            .on_ws_text(
                r#"432[{"data":{"accepted":true,"timestamp":9999,"created":true}}]"#,
                16_500,
            )
            .unwrap();
        assert!(
            engine
                .drain_events()
                .contains(&SpaceSyncEvent::DocUpdatePushed {
                    doc_id: "doc-1".into(),
                    timestamp: 9_999,
                    created: true,
                })
        );

        // Inbound broadcast maps oldId -> newId and decodes base64.
        let bcast = format!(
            r#"42["space:broadcast-doc-updates",{{"spaceType":"workspace","spaceId":"ws-1","docId":"db$ws-1$folders","updates":["{}"],"timestamp":10001}}]"#,
            BASE64.encode([9, 8, 7])
        );
        engine.on_ws_text(&bcast, 17_000).unwrap();
        assert_eq!(
            engine.drain_events(),
            vec![SpaceSyncEvent::RemoteDocUpdates {
                doc_id: "db$folders".into(),
                updates: vec![vec![9, 8, 7]],
                timestamp: 10_001,
                editor: None,
            }]
        );

        // Load root doc (already joined) -> permission revocation halts sync.
        let ack_id = engine.load_doc("ws-1", None, 17_500).unwrap();
        let err_ack = format!(
            r#"43{ack_id}[{{"error":{{"status":403,"code":"SPACE_ACCESS_DENIED","name":"SPACE_ACCESS_DENIED","message":"denied"}}}}]"#
        );
        engine.on_ws_text(&err_ack, 17_600).unwrap();
        assert_eq!(engine.state(), SpaceSyncState::PermissionRevoked);
        assert!(matches!(
            engine.drain_events().as_slice(),
            [SpaceSyncEvent::PermissionRevoked { code, .. }] if code == "SPACE_ACCESS_DENIED"
        ));
    }

    #[test]
    fn load_doc_auto_joins_unjoined_doc_and_retries_once_on_not_in_space() {
        let mut engine = SpaceSyncEngine::new(
            SpaceSyncConfig::workspace("ws-1", true),
            IdConverter::new("ws-1"),
        );
        connect_and_join_ws(&mut engine);

        // doc-42 is not yet joined -> load_doc sends space:join-batch first (ack_id = 1).
        let join_ack = engine.load_doc("doc-42", None, 2_000).unwrap();
        let out = engine.drain_outbound();
        assert!(out.iter().any(|o| matches!(
            o,
            SpaceSyncOutbound::Transport(EngineIoOutput::SendWsText(t))
                if t.contains(SPACE_JOIN_BATCH) && t.contains("\"docId\":\"doc-42\"")
        )));

        // Join ACK succeeds -> automatically emits space:load-doc (ack_id = 2).
        engine
            .on_ws_text(
                &format!(
                    r#"43{join_ack}[{{"data":{{"clientId":"c1","success":true,"protocolVersion":1}}}}]"#
                ),
                2_100,
            )
            .unwrap();
        assert!(engine.joined_wire_doc_ids().contains("doc-42"));
        let out = engine.drain_outbound();
        assert!(out.iter().any(|o| matches!(
            o,
            SpaceSyncOutbound::Transport(EngineIoOutput::SendWsText(t))
                if t.starts_with("422[\"space:load-doc\"")
        )));

        // Server rejects load-doc with NOT_IN_SPACE (e.g. permission generation bump)
        // -> removes doc-42 and retries join + load-doc once (join ack_id = 3).
        engine
            .on_ws_text(
                r#"432[{"error":{"status":400,"code":"NOT_IN_SPACE","name":"NOT_IN_SPACE","message":"not in space"}}]"#,
                2_200,
            )
            .unwrap();
        assert!(!engine.joined_wire_doc_ids().contains("doc-42"));
        let retry_join_out = engine.drain_outbound();
        assert!(retry_join_out.iter().any(|o| matches!(
            o,
            SpaceSyncOutbound::Transport(EngineIoOutput::SendWsText(t))
                if t.starts_with("423[\"space:join-batch\"")
        )));

        // Retry join succeeds -> emits load-doc (ack_id = 4) -> succeeds!
        engine
            .on_ws_text(
                r#"433[{"data":{"clientId":"c1","success":true,"protocolVersion":1}}]"#,
                2_300,
            )
            .unwrap();
        let _ = engine.drain_outbound();
        let payload = BASE64.encode([0, 0]);
        engine
            .on_ws_text(
                &format!(
                    r#"434[{{"data":{{"missing":"{payload}","state":"{payload}","timestamp":5000}}}}]"#
                ),
                2_400,
            )
            .unwrap();
        assert!(engine.drain_events().contains(&SpaceSyncEvent::DocLoaded {
            doc_id: "doc-42".into(),
            missing: vec![0, 0],
            state: vec![0, 0],
            timestamp: 5_000,
        }));
    }

    #[test]
    fn leave_space_doc_lifecycle_and_explicit_request_failures() {
        let mut converter = IdConverter::new("ws-1");
        converter.register_page_mapping("logical-1", "physical-guid-1");
        let mut engine = SpaceSyncEngine::new(SpaceSyncConfig::workspace("ws-1", true), converter);
        connect_and_join_ws(&mut engine);

        // apply_doc_lifecycle(Delete) sends physicalDocId and remembers it across retries.
        let ack = engine
            .apply_doc_lifecycle("logical-1", DocLifecycleAction::Delete, 3_000)
            .unwrap();
        let out = engine.drain_outbound();
        assert!(out.iter().any(|o| matches!(
            o,
            SpaceSyncOutbound::Transport(EngineIoOutput::SendWsText(t))
                if t.contains(SPACE_DOC_LIFECYCLE)
                    && t.contains("\"physicalDocId\":\"physical-guid-1\"")
        )));
        engine
            .on_ws_text(
                &format!(
                    r#"43{ack}[{{"data":{{"timestamp":7777,"deletedDocIds":["physical-guid-1"]}}}}]"#
                ),
                3_100,
            )
            .unwrap();
        assert_eq!(
            engine.drain_events(),
            vec![SpaceSyncEvent::DocLifecycleApplied {
                doc_id: "logical-1".into(),
                lifecycle: DocLifecycleAction::Delete,
                timestamp: 7_777,
                deleted_doc_ids: Some(vec!["physical-guid-1".into()]),
            }]
        );

        // Timeout on load_doc_timestamps emits explicit RequestFailed (never silently dropped).
        let _ = engine.load_doc_timestamps(None, 4_000).unwrap();
        engine.on_tick(4_000 + 15_000).unwrap();
        assert!(matches!(
            engine.drain_events().as_slice(),
            [SpaceSyncEvent::RequestFailed {
                op: SPACE_LOAD_DOC_TIMESTAMPS,
                code,
                ..
            }] if code == "ACK_TIMEOUT"
        ));

        // leave_space emits batched leave + trailing empty docIds: [].
        engine.leave_space(20_000).unwrap();
        let leave_out = engine.drain_outbound();
        let leave_frames: Vec<&str> = leave_out
            .iter()
            .filter_map(|o| match o {
                SpaceSyncOutbound::Transport(EngineIoOutput::SendWsText(t))
                    if t.contains(SPACE_LEAVE_BATCH) =>
                {
                    Some(t.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(leave_frames.len(), 2);
        assert!(leave_frames[0].contains("\"docIds\":[\"ws-1\"]"));
        assert!(leave_frames[1].contains("\"docIds\":[]"));
    }

    #[test]
    fn direct_websocket_connect_and_ws_upgrade_error_keeps_polling_alive() {
        // Direct WebSocket mode reconnects with exponential backoff.
        let mut cloud = SpaceSyncEngine::new(
            SpaceSyncConfig::workspace("ws-cloud", false),
            IdConverter::new("ws-cloud"),
        );
        cloud.connect(1_000);
        let _ = cloud.drain_outbound();
        cloud.on_transport_error("network drop 1", 2_000);
        assert_eq!(
            cloud.state(),
            SpaceSyncState::ReconnectScheduled {
                next_attempt_at_ms: 3_000,
            }
        );
        cloud.on_tick(3_000).unwrap();
        assert_eq!(cloud.state(), SpaceSyncState::Connecting);
        cloud.on_transport_error("network drop 2", 3_500);
        assert_eq!(
            cloud.state(),
            SpaceSyncState::ReconnectScheduled {
                next_attempt_at_ms: 5_500,
            }
        );

        // Self-hosted polling -> WS upgrade error degrades to polling without disconnecting engine.
        let mut selfhost = SpaceSyncEngine::new(
            SpaceSyncConfig::workspace("ws-sh", true),
            IdConverter::new("ws-sh"),
        );
        selfhost.connect(1_000);
        let _ = selfhost.drain_outbound();
        let open =
            r#"0{"sid":"sh1","upgrades":["websocket"],"pingInterval":25000,"pingTimeout":20000}"#;
        selfhost.on_polling_response(open, 1_050).unwrap();
        selfhost.on_ws_error("proxy rejected ws", 1_100);
        assert_eq!(selfhost.state(), SpaceSyncState::Connecting);
        assert!(selfhost.drain_events().is_empty());
    }

    #[test]
    fn v2_push_update_requeues_when_deferred_join_times_out_or_disconnects() {
        let mut config = SpaceSyncConfig::workspace("ws-v2", true);
        config.protocol_version = 2;
        let mut engine = SpaceSyncEngine::new(config, IdConverter::new("ws-v2"));
        engine.connect(1_000);
        let _ = engine.drain_outbound();
        let open =
            r#"0{"sid":"e2","upgrades":["websocket"],"pingInterval":25000,"pingTimeout":20000}"#;
        engine.on_polling_response(open, 1_050).unwrap();
        engine.on_ws_open(1_100);
        engine.on_ws_text("3probe", 1_150).unwrap();
        engine.on_ws_text(r#"40{"sid":"sio-2"}"#, 1_200).unwrap();
        engine
            .on_ws_text(
                r#"430[{"data":{"clientId":"c2","success":true,"protocolVersion":2}}]"#,
                1_300,
            )
            .unwrap();
        let _ = engine.drain_events();
        let _ = engine.drain_outbound();

        // Push to unjoined doc triggers JoinDocs with deferred PushDocUpdate; timeout requeues it.
        engine
            .push_doc_update("doc-v2", vec![4, 5, 6], PushDocIntent::Update, 2_000)
            .unwrap();
        assert!(engine.offline_queue().is_empty());
        engine.on_tick(2_000 + 15_000).unwrap();
        assert_eq!(engine.offline_queue().len(), 1);
        assert_eq!(engine.offline_queue()[0].doc_id, "doc-v2");

        // Second attempt interrupted by disconnect also requeues the deferred PushDocUpdate.
        let pending = engine.offline_queue.pop_front().unwrap();
        engine
            .push_doc_update(pending.doc_id, pending.update, pending.intent, 18_000)
            .unwrap();
        engine.on_transport_error("drop during join", 18_500);
        assert_eq!(engine.offline_queue().len(), 1);
        assert_eq!(engine.offline_queue()[0].doc_id, "doc-v2");
    }
}
