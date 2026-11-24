use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use super::{
    BroadcastDocInvalidationMessage, BroadcastDocUpdatesMessage, DeferredAfterJoin,
    DocLifecycleAction, DocLifecycleResponse, EngineIoOutput, InFlightRequest, JoinSpaceBatchEntry,
    JoinSpaceBatchMessage, JoinSpaceBatchResponse, LoadDocMessage, LoadDocResponse,
    OfflinePendingUpdate, PendingAck, PushDocIntent, PushDocUpdateMessage, PushDocUpdateResponse,
    SPACE_BROADCAST_DOC_INVALIDATION, SPACE_BROADCAST_DOC_UPDATES, SPACE_DELETE_DOC,
    SPACE_DOC_LIFECYCLE, SPACE_JOIN_BATCH, SPACE_LOAD_DOC, SPACE_LOAD_DOC_TIMESTAMPS,
    SPACE_PUSH_DOC_UPDATE, SocketIoEvent, SpaceSyncEngine, SpaceSyncEvent, SpaceSyncOutbound,
    SpaceSyncState, WebsocketResponse,
};
use crate::error::StoreResult;

impl SpaceSyncEngine {
    pub(super) fn send_join_docs_batch(
        &mut self,
        wire_doc_ids: Vec<String>,
        deferred: Option<DeferredAfterJoin>,
        now_ms: i64,
    ) -> StoreResult<u64> {
        let spaces = wire_doc_ids
            .iter()
            .map(|wire_id| JoinSpaceBatchEntry {
                space_type: self.config.space_type,
                space_id: self.config.space_id.clone(),
                doc_id: Some(wire_id.clone()),
            })
            .collect();
        let join = JoinSpaceBatchMessage {
            spaces,
            client_version: self.config.client_version.clone(),
            protocol_version: self.config.protocol_version,
        };
        let (ack_id, pkt) = self
            .socket_io
            .encode_event_with_ack(SPACE_JOIN_BATCH, serde_json::to_value(join)?)?;
        self.in_flight.insert(
            ack_id,
            PendingAck {
                sent_at_ms: now_ms,
                request: InFlightRequest::JoinDocs {
                    wire_doc_ids,
                    deferred,
                },
            },
        );
        self.engine_io.send_message(&pkt)?;
        self.flush_engine_io_outputs(now_ms);
        Ok(ack_id)
    }

    pub(super) fn dispatch_load_doc(
        &mut self,
        doc_id: String,
        state_vector: Option<Vec<u8>>,
        retried_not_in_space: bool,
        now_ms: i64,
    ) -> StoreResult<u64> {
        let wire_doc_id = self.id_converter.new_id_to_old_id(&doc_id);
        if !self.joined_doc_ids.contains(&wire_doc_id) {
            return self.send_join_docs_batch(
                vec![wire_doc_id],
                Some(DeferredAfterJoin::LoadDoc {
                    doc_id,
                    state_vector,
                    retried_not_in_space,
                }),
                now_ms,
            );
        }
        let msg = LoadDocMessage {
            space_type: self.config.space_type,
            space_id: self.config.space_id.clone(),
            doc_id: wire_doc_id.clone(),
            state_vector: state_vector.as_deref().map(|sv| BASE64.encode(sv)),
        };
        let (ack_id, packet) = self
            .socket_io
            .encode_event_with_ack(SPACE_LOAD_DOC, serde_json::to_value(msg)?)?;
        self.in_flight.insert(
            ack_id,
            PendingAck {
                sent_at_ms: now_ms,
                request: InFlightRequest::LoadDoc {
                    doc_id,
                    wire_doc_id,
                    state_vector,
                    retried_not_in_space,
                },
            },
        );
        self.engine_io.send_message(&packet)?;
        self.flush_engine_io_outputs(now_ms);
        Ok(ack_id)
    }

    pub(super) fn dispatch_push_doc_update(
        &mut self,
        entry: OfflinePendingUpdate,
        retried_not_in_space: bool,
        now_ms: i64,
    ) -> StoreResult<()> {
        let wire_doc_id = self.id_converter.new_id_to_old_id(&entry.doc_id);
        let needs_join = self.config.protocol_version != 1
            && entry.intent == PushDocIntent::Update
            && !self.joined_doc_ids.contains(&wire_doc_id);
        if needs_join {
            self.send_join_docs_batch(
                vec![wire_doc_id],
                Some(DeferredAfterJoin::PushDocUpdate {
                    entry,
                    retried_not_in_space,
                }),
                now_ms,
            )?;
            return Ok(());
        }
        let intent = if self.config.protocol_version == 1 {
            PushDocIntent::Create
        } else {
            entry.intent
        };
        let msg = PushDocUpdateMessage {
            space_type: self.config.space_type,
            space_id: self.config.space_id.clone(),
            doc_id: wire_doc_id.clone(),
            update: BASE64.encode(&entry.update),
            intent,
        };
        let (ack_id, pkt) = self
            .socket_io
            .encode_event_with_ack(SPACE_PUSH_DOC_UPDATE, serde_json::to_value(msg)?)?;
        self.in_flight.insert(
            ack_id,
            PendingAck {
                sent_at_ms: now_ms,
                request: InFlightRequest::PushDocUpdate {
                    entry,
                    wire_doc_id,
                    retried_not_in_space,
                },
            },
        );
        self.engine_io.send_message(&pkt)?;
        self.flush_engine_io_outputs(now_ms);
        Ok(())
    }

    pub(super) fn flush_engine_io_outputs(&mut self, now_ms: i64) {
        let outputs = self.engine_io.drain_outputs();
        for output in outputs {
            match output {
                EngineIoOutput::Connected { sid, upgraded_ws } => {
                    self.outbound
                        .push(SpaceSyncOutbound::Transport(EngineIoOutput::Connected {
                            sid,
                            upgraded_ws,
                        }));
                    if let Ok(connect_pkt) = self.socket_io.encode_connect(None)
                        && self.engine_io.send_message(&connect_pkt).is_ok()
                    {
                        self.flush_engine_io_outputs(now_ms);
                    }
                }
                EngineIoOutput::Message(sio_raw) => {
                    if let Ok(Some(ev)) = self.socket_io.on_text_packet(&sio_raw) {
                        self.handle_socket_io_event(ev, now_ms);
                    }
                }
                EngineIoOutput::BinaryMessage(bytes) => {
                    if let Ok(Some(ev)) = self.socket_io.on_binary_attachment(bytes) {
                        self.handle_socket_io_event(ev, now_ms);
                    }
                }
                EngineIoOutput::Closed { reason } => {
                    self.handle_disconnect(&reason, now_ms);
                }
                EngineIoOutput::HeartbeatTimeout => {
                    self.handle_disconnect("heartbeat timeout", now_ms);
                }
                other => {
                    self.outbound.push(SpaceSyncOutbound::Transport(other));
                }
            }
        }
    }

    fn handle_socket_io_event(&mut self, event: SocketIoEvent, now_ms: i64) {
        match event {
            SocketIoEvent::Connected { .. } => {
                self.state = SpaceSyncState::Joining;
                let root_wire_id = self.id_converter.new_id_to_old_id(&self.config.space_id);
                let join = JoinSpaceBatchMessage {
                    spaces: vec![JoinSpaceBatchEntry {
                        space_type: self.config.space_type,
                        space_id: self.config.space_id.clone(),
                        doc_id: Some(root_wire_id.clone()),
                    }],
                    client_version: self.config.client_version.clone(),
                    protocol_version: self.config.protocol_version,
                };
                if let Ok(val) = serde_json::to_value(join)
                    && let Ok((ack_id, pkt)) =
                        self.socket_io.encode_event_with_ack(SPACE_JOIN_BATCH, val)
                {
                    self.in_flight.insert(
                        ack_id,
                        PendingAck {
                            sent_at_ms: now_ms,
                            request: InFlightRequest::InitialJoin { root_wire_id },
                        },
                    );
                    if self.engine_io.send_message(&pkt).is_ok() {
                        self.flush_engine_io_outputs(now_ms);
                    }
                }
            }
            SocketIoEvent::ConnectError(err) => {
                self.handle_disconnect(&err.to_string(), now_ms);
            }
            SocketIoEvent::Disconnected => {
                self.handle_disconnect("socket.io disconnect", now_ms);
            }
            SocketIoEvent::Event {
                event: name,
                payload,
                ..
            } => {
                self.handle_server_event(&name, payload);
            }
            SocketIoEvent::Ack { ack_id, payload } => {
                self.handle_ack(ack_id, payload, now_ms);
            }
        }
    }

    fn handle_server_event(&mut self, name: &str, payload: Value) {
        match name {
            SPACE_BROADCAST_DOC_UPDATES => {
                if let Ok(msg) = serde_json::from_value::<BroadcastDocUpdatesMessage>(payload) {
                    if msg.space_type != self.config.space_type
                        || msg.space_id != self.config.space_id
                    {
                        return;
                    }
                    let doc_id = self.id_converter.old_id_to_new_id(&msg.doc_id);
                    if self.id_converter.new_id_to_old_id(&doc_id) != msg.doc_id {
                        return;
                    }
                    let mut decoded = Vec::with_capacity(msg.updates.len());
                    for encoded in msg.updates {
                        if let Ok(bytes) = BASE64.decode(encoded) {
                            decoded.push(bytes);
                        }
                    }
                    self.events.push(SpaceSyncEvent::RemoteDocUpdates {
                        doc_id,
                        updates: decoded,
                        timestamp: msg.timestamp,
                        editor: msg.editor,
                    });
                }
            }
            SPACE_BROADCAST_DOC_INVALIDATION => {
                if let Ok(msg) = serde_json::from_value::<BroadcastDocInvalidationMessage>(payload)
                {
                    if msg.space_type != self.config.space_type
                        || msg.space_id != self.config.space_id
                    {
                        return;
                    }
                    self.events.push(SpaceSyncEvent::DocInvalidated {
                        timestamp: msg.timestamp,
                        inventory_changed: msg.inventory_changed.unwrap_or(false),
                    });
                }
            }
            _ => {}
        }
    }

    fn handle_ack(&mut self, ack_id: u64, payload: Value, now_ms: i64) {
        let Some(pending) = self.in_flight.remove(&ack_id) else {
            return;
        };

        match pending.request {
            InFlightRequest::InitialJoin { root_wire_id } => {
                match serde_json::from_value::<WebsocketResponse<JoinSpaceBatchResponse>>(payload) {
                    Ok(WebsocketResponse::Ok { data })
                        if data.success
                            && data.protocol_version == self.config.protocol_version =>
                    {
                        self.joined_doc_ids.insert(root_wire_id);
                        self.state = SpaceSyncState::Ready;
                        self.reconnect_attempts = 0;
                        self.events.push(SpaceSyncEvent::Joined {
                            client_id: data.client_id,
                            protocol_version: data.protocol_version,
                        });
                        self.replay_offline_queue(now_ms);
                    }
                    Ok(WebsocketResponse::Err { error }) if error.is_permission_revoked() => {
                        let code = error.canonical_code().to_string();
                        self.handle_permission_revoked(None, &code, error.message);
                    }
                    _ => {
                        self.handle_disconnect("space:join-batch rejected", now_ms);
                    }
                }
            }
            InFlightRequest::JoinDocs {
                wire_doc_ids,
                deferred,
            } => {
                match serde_json::from_value::<WebsocketResponse<JoinSpaceBatchResponse>>(payload) {
                    Ok(WebsocketResponse::Ok { data })
                        if data.success
                            && data.protocol_version == self.config.protocol_version =>
                    {
                        let mut local_ids = Vec::with_capacity(wire_doc_ids.len());
                        for wire_id in wire_doc_ids {
                            local_ids.push(self.id_converter.old_id_to_new_id(&wire_id));
                            self.joined_doc_ids.insert(wire_id);
                        }
                        self.events
                            .push(SpaceSyncEvent::DocsJoined { doc_ids: local_ids });
                        if let Some(next_op) = deferred {
                            self.run_deferred_after_join(next_op, now_ms);
                        }
                    }
                    Ok(WebsocketResponse::Err { error }) if error.is_permission_revoked() => {
                        let doc_id = deferred_doc_id(deferred.as_ref());
                        let code = error.canonical_code().to_string();
                        self.handle_permission_revoked(doc_id, &code, error.message);
                    }
                    Ok(WebsocketResponse::Err { error }) => {
                        let doc_id = deferred_doc_id(deferred.as_ref());
                        if let Some(DeferredAfterJoin::PushDocUpdate { entry, .. }) = deferred {
                            self.offline_queue.push_front(entry);
                        }
                        self.events.push(SpaceSyncEvent::RequestFailed {
                            op: SPACE_JOIN_BATCH,
                            doc_id,
                            code: error.canonical_code().to_string(),
                            message: error.message,
                        });
                    }
                    _ => {
                        let doc_id = deferred_doc_id(deferred.as_ref());
                        if let Some(DeferredAfterJoin::PushDocUpdate { entry, .. }) = deferred {
                            self.offline_queue.push_front(entry);
                        }
                        self.events.push(SpaceSyncEvent::RequestFailed {
                            op: SPACE_JOIN_BATCH,
                            doc_id,
                            code: "INVALID_RESPONSE".into(),
                            message: "space:join-batch rejected".into(),
                        });
                    }
                }
            }
            InFlightRequest::LoadDoc {
                doc_id,
                wire_doc_id,
                state_vector,
                retried_not_in_space,
            } => match serde_json::from_value::<WebsocketResponse<LoadDocResponse>>(payload) {
                Ok(WebsocketResponse::Ok { data }) => {
                    match (BASE64.decode(&data.missing), BASE64.decode(&data.state)) {
                        (Ok(missing), Ok(state)) => {
                            self.events.push(SpaceSyncEvent::DocLoaded {
                                doc_id,
                                missing,
                                state,
                                timestamp: data.timestamp,
                            });
                        }
                        _ => {
                            self.events.push(SpaceSyncEvent::RequestFailed {
                                op: SPACE_LOAD_DOC,
                                doc_id: Some(doc_id),
                                code: "INVALID_BASE64".into(),
                                message: "invalid base64 in space:load-doc response".into(),
                            });
                        }
                    }
                }
                Ok(WebsocketResponse::Err { error }) if error.is_doc_not_found() => {
                    self.events.push(SpaceSyncEvent::DocNotFound { doc_id });
                }
                Ok(WebsocketResponse::Err { error })
                    if error.is_not_in_space() && !retried_not_in_space =>
                {
                    self.joined_doc_ids.remove(&wire_doc_id);
                    let _ = self.dispatch_load_doc(doc_id, state_vector, true, now_ms);
                }
                Ok(WebsocketResponse::Err { error }) if error.is_permission_revoked() => {
                    let code = error.canonical_code().to_string();
                    self.handle_permission_revoked(Some(doc_id), &code, error.message);
                }
                Ok(WebsocketResponse::Err { error }) => {
                    self.events.push(SpaceSyncEvent::RequestFailed {
                        op: SPACE_LOAD_DOC,
                        doc_id: Some(doc_id),
                        code: error.canonical_code().to_string(),
                        message: error.message,
                    });
                }
                Err(err) => {
                    self.events.push(SpaceSyncEvent::RequestFailed {
                        op: SPACE_LOAD_DOC,
                        doc_id: Some(doc_id),
                        code: "INVALID_RESPONSE".into(),
                        message: err.to_string(),
                    });
                }
            },
            InFlightRequest::PushDocUpdate {
                entry,
                wire_doc_id,
                retried_not_in_space,
            } => {
                match serde_json::from_value::<WebsocketResponse<PushDocUpdateResponse>>(payload) {
                    Ok(WebsocketResponse::Ok { data }) => {
                        self.events.push(SpaceSyncEvent::DocUpdatePushed {
                            doc_id: entry.doc_id,
                            timestamp: data.timestamp,
                            created: data.created,
                        });
                    }
                    Ok(WebsocketResponse::Err { error })
                        if error.is_not_in_space() && !retried_not_in_space =>
                    {
                        self.joined_doc_ids.remove(&wire_doc_id);
                        let _ = self.dispatch_push_doc_update(entry, true, now_ms);
                    }
                    Ok(WebsocketResponse::Err { error }) if error.is_permission_revoked() => {
                        let code = error.canonical_code().to_string();
                        self.handle_permission_revoked(Some(entry.doc_id), &code, error.message);
                    }
                    Ok(WebsocketResponse::Err { error }) => {
                        let doc_id = entry.doc_id.clone();
                        self.offline_queue.push_front(entry);
                        self.events.push(SpaceSyncEvent::RequestFailed {
                            op: SPACE_PUSH_DOC_UPDATE,
                            doc_id: Some(doc_id),
                            code: error.canonical_code().to_string(),
                            message: error.message,
                        });
                    }
                    Err(err) => {
                        let doc_id = entry.doc_id.clone();
                        self.offline_queue.push_front(entry);
                        self.events.push(SpaceSyncEvent::RequestFailed {
                            op: SPACE_PUSH_DOC_UPDATE,
                            doc_id: Some(doc_id),
                            code: "INVALID_RESPONSE".into(),
                            message: err.to_string(),
                        });
                    }
                }
            }
            InFlightRequest::LoadDocTimestamps => {
                match serde_json::from_value::<WebsocketResponse<BTreeMap<String, i64>>>(payload) {
                    Ok(WebsocketResponse::Ok { data }) => {
                        let mut normalized = BTreeMap::new();
                        for (wire_id, ts) in data {
                            let local_id = self.id_converter.old_id_to_new_id(&wire_id);
                            if self.id_converter.new_id_to_old_id(&local_id) == wire_id {
                                normalized.insert(local_id, ts);
                            }
                        }
                        self.events.push(SpaceSyncEvent::DocTimestampsLoaded {
                            timestamps: normalized,
                        });
                    }
                    Ok(WebsocketResponse::Err { error }) if error.is_permission_revoked() => {
                        let code = error.canonical_code().to_string();
                        self.handle_permission_revoked(None, &code, error.message);
                    }
                    Ok(WebsocketResponse::Err { error }) => {
                        self.events.push(SpaceSyncEvent::RequestFailed {
                            op: SPACE_LOAD_DOC_TIMESTAMPS,
                            doc_id: None,
                            code: error.canonical_code().to_string(),
                            message: error.message,
                        });
                    }
                    Err(err) => {
                        self.events.push(SpaceSyncEvent::RequestFailed {
                            op: SPACE_LOAD_DOC_TIMESTAMPS,
                            doc_id: None,
                            code: "INVALID_RESPONSE".into(),
                            message: err.to_string(),
                        });
                    }
                }
            }
            InFlightRequest::DeleteDoc { doc_id } => {
                match serde_json::from_value::<WebsocketResponse<Value>>(payload) {
                    Ok(WebsocketResponse::Ok { .. }) => {
                        self.events.push(SpaceSyncEvent::DocDeleted { doc_id });
                    }
                    Ok(WebsocketResponse::Err { error }) if error.is_permission_revoked() => {
                        let code = error.canonical_code().to_string();
                        self.handle_permission_revoked(Some(doc_id), &code, error.message);
                    }
                    Ok(WebsocketResponse::Err { error }) => {
                        self.events.push(SpaceSyncEvent::RequestFailed {
                            op: SPACE_DELETE_DOC,
                            doc_id: Some(doc_id),
                            code: error.canonical_code().to_string(),
                            message: error.message,
                        });
                    }
                    Err(err) => {
                        self.events.push(SpaceSyncEvent::RequestFailed {
                            op: SPACE_DELETE_DOC,
                            doc_id: Some(doc_id),
                            code: "INVALID_RESPONSE".into(),
                            message: err.to_string(),
                        });
                    }
                }
            }
            InFlightRequest::DocLifecycle { doc_id, lifecycle } => {
                match serde_json::from_value::<WebsocketResponse<DocLifecycleResponse>>(payload) {
                    Ok(WebsocketResponse::Ok { data }) => {
                        if lifecycle == DocLifecycleAction::Delete {
                            self.pending_deletes.remove(&doc_id);
                        }
                        self.events.push(SpaceSyncEvent::DocLifecycleApplied {
                            doc_id,
                            lifecycle,
                            timestamp: data.timestamp,
                            deleted_doc_ids: data.deleted_doc_ids,
                        });
                    }
                    Ok(WebsocketResponse::Err { error }) => {
                        if lifecycle == DocLifecycleAction::Delete
                            && error.is_terminal_lifecycle_error()
                        {
                            self.pending_deletes.remove(&doc_id);
                        }
                        if error.is_permission_revoked() {
                            let code = error.canonical_code().to_string();
                            self.handle_permission_revoked(Some(doc_id), &code, error.message);
                        } else {
                            self.events.push(SpaceSyncEvent::RequestFailed {
                                op: SPACE_DOC_LIFECYCLE,
                                doc_id: Some(doc_id),
                                code: error.canonical_code().to_string(),
                                message: error.message,
                            });
                        }
                    }
                    Err(err) => {
                        self.events.push(SpaceSyncEvent::RequestFailed {
                            op: SPACE_DOC_LIFECYCLE,
                            doc_id: Some(doc_id),
                            code: "INVALID_RESPONSE".into(),
                            message: err.to_string(),
                        });
                    }
                }
            }
        }
    }

    pub(super) fn handle_ack_timeout(
        &mut self,
        request: InFlightRequest,
        now_ms: i64,
    ) -> StoreResult<()> {
        match request {
            InFlightRequest::PushDocUpdate { entry, .. } => {
                if self.state == SpaceSyncState::Ready {
                    self.dispatch_push_doc_update(entry, false, now_ms)?;
                } else {
                    self.offline_queue.push_front(entry);
                }
            }
            InFlightRequest::InitialJoin { .. } => {
                self.handle_disconnect("join ACK timeout", now_ms);
            }
            InFlightRequest::JoinDocs { deferred, .. } => {
                let doc_id = deferred_doc_id(deferred.as_ref());
                if let Some(DeferredAfterJoin::PushDocUpdate { entry, .. }) = deferred {
                    self.offline_queue.push_front(entry);
                }
                self.events.push(SpaceSyncEvent::RequestFailed {
                    op: SPACE_JOIN_BATCH,
                    doc_id,
                    code: "ACK_TIMEOUT".into(),
                    message: "space:join-batch ACK timed out".into(),
                });
            }
            InFlightRequest::LoadDoc { doc_id, .. } => {
                self.events.push(SpaceSyncEvent::RequestFailed {
                    op: SPACE_LOAD_DOC,
                    doc_id: Some(doc_id),
                    code: "ACK_TIMEOUT".into(),
                    message: "space:load-doc ACK timed out".into(),
                });
            }
            InFlightRequest::LoadDocTimestamps => {
                self.events.push(SpaceSyncEvent::RequestFailed {
                    op: SPACE_LOAD_DOC_TIMESTAMPS,
                    doc_id: None,
                    code: "ACK_TIMEOUT".into(),
                    message: "space:load-doc-timestamps ACK timed out".into(),
                });
            }
            InFlightRequest::DeleteDoc { doc_id } => {
                self.events.push(SpaceSyncEvent::RequestFailed {
                    op: SPACE_DELETE_DOC,
                    doc_id: Some(doc_id),
                    code: "ACK_TIMEOUT".into(),
                    message: "space:delete-doc ACK timed out".into(),
                });
            }
            InFlightRequest::DocLifecycle { doc_id, .. } => {
                self.events.push(SpaceSyncEvent::RequestFailed {
                    op: SPACE_DOC_LIFECYCLE,
                    doc_id: Some(doc_id),
                    code: "ACK_TIMEOUT".into(),
                    message: "space:doc-lifecycle ACK timed out".into(),
                });
            }
        }
        Ok(())
    }

    fn run_deferred_after_join(&mut self, op: DeferredAfterJoin, now_ms: i64) {
        match op {
            DeferredAfterJoin::LoadDoc {
                doc_id,
                state_vector,
                retried_not_in_space,
            } => {
                let _ = self.dispatch_load_doc(doc_id, state_vector, retried_not_in_space, now_ms);
            }
            DeferredAfterJoin::PushDocUpdate {
                entry,
                retried_not_in_space,
            } => {
                let _ = self.dispatch_push_doc_update(entry, retried_not_in_space, now_ms);
            }
        }
    }

    fn handle_permission_revoked(&mut self, doc_id: Option<String>, code: &str, message: String) {
        if matches!(
            code,
            "SPACE_ACCESS_DENIED" | "AUTHENTICATION_REQUIRED" | "UNSUPPORTED_CLIENT_VERSION"
        ) {
            self.state = SpaceSyncState::PermissionRevoked;
            self.joined_doc_ids.clear();
            self.offline_queue.clear();
            self.in_flight.clear();
        }
        self.events.push(SpaceSyncEvent::PermissionRevoked {
            doc_id,
            code: code.to_string(),
            message,
        });
    }

    fn replay_offline_queue(&mut self, now_ms: i64) {
        while let Some(entry) = self.offline_queue.pop_front() {
            if self
                .dispatch_push_doc_update(entry.clone(), false, now_ms)
                .is_err()
            {
                self.offline_queue.push_front(entry);
                break;
            }
        }
    }

    pub(super) fn handle_disconnect(&mut self, reason: &str, now_ms: i64) {
        if self.state == SpaceSyncState::PermissionRevoked {
            return;
        }
        self.joined_doc_ids.clear();
        let mut requeue = Vec::new();
        for (_, pending) in std::mem::take(&mut self.in_flight) {
            match pending.request {
                InFlightRequest::PushDocUpdate { entry, .. }
                | InFlightRequest::JoinDocs {
                    deferred: Some(DeferredAfterJoin::PushDocUpdate { entry, .. }),
                    ..
                } => {
                    requeue.push(entry);
                }
                _ => {}
            }
        }
        for entry in requeue.into_iter().rev() {
            self.offline_queue.push_front(entry);
        }

        let delay = self.config.reconnect.backoff_ms(self.reconnect_attempts);
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        let next_at = now_ms.saturating_add(delay);
        self.state = SpaceSyncState::ReconnectScheduled {
            next_attempt_at_ms: next_at,
        };
        self.outbound
            .push(SpaceSyncOutbound::ScheduleTimerAtMs(next_at));
        self.events.push(SpaceSyncEvent::Disconnected {
            reason: reason.to_string(),
            next_reconnect_ms: next_at,
        });
    }
}

fn deferred_doc_id(deferred: Option<&DeferredAfterJoin>) -> Option<String> {
    match deferred {
        Some(DeferredAfterJoin::LoadDoc { doc_id, .. }) => Some(doc_id.clone()),
        Some(DeferredAfterJoin::PushDocUpdate { entry, .. }) => Some(entry.doc_id.clone()),
        None => None,
    }
}
