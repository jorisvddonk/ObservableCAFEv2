use crate::registry::SessionRegistry;
use crate::session::SessionState;
use anyhow::Result;
use cafe_types::{
    keys, Chunk, ClientMessage, ServerMessage, SessionConfig, SubscribeFilter,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, warn};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024; // 16 MB

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

/// Shared registry of active connections, keyed by connection ID.
pub type ConnectionRegistry = Arc<RwLock<HashMap<String, Arc<Mutex<OwnedWriteHalf>>>>>;

pub async fn handle_client(
    stream: tokio::net::UnixStream,
    registry: Arc<RwLock<SessionRegistry>>,
    connections: ConnectionRegistry,
) {
    let (reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));

    let conn_id = format!("c-{}", NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed));
    connections.write().await.insert(conn_id.clone(), writer.clone());
    send_msg(&writer, &ServerMessage::Connected { connection_id: conn_id.clone() }).await;

    let conns = connections.clone();
    if let Err(e) = client_loop(reader, writer, registry, conns, conn_id.clone()).await {
        debug!("client {} disconnected: {}", conn_id, e);
    }

    connections.write().await.remove(&conn_id);
}

async fn client_loop(
    reader: OwnedReadHalf,
    writer: Arc<Mutex<OwnedWriteHalf>>,
    registry: Arc<RwLock<SessionRegistry>>,
    connections: ConnectionRegistry,
    conn_id: String,
) -> Result<()> {
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        if line.len() > MAX_MESSAGE_BYTES {
            send_error(
                &writer,
                None,
                "Payload too large",
                "PAYLOAD_TOO_LARGE",
            )
            .await;
            continue;
        }

        let msg: ClientMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                warn!("invalid message: {}", e);
                send_error(&writer, None, &format!("Invalid JSON: {}", e), "INVALID_MESSAGE")
                    .await;
                continue;
            }
        };

        match msg {
            ClientMessage::Ping => {
                send_msg(&writer, &ServerMessage::Pong).await;
            }

            ClientMessage::ListSessions => {
                let reg = registry.read().await;
                let sessions = reg.list();
                send_msg(&writer, &ServerMessage::SessionsList { sessions }).await;
            }

            ClientMessage::CreateSession {
                session_id,
                agent_id,
                config,
            } => {
                let mut reg = registry.write().await;
                if reg.contains(&session_id) {
                    send_error(
                        &writer,
                        Some(&session_id),
                        &format!("Session already exists: {}", session_id),
                        "SESSION_EXISTS",
                    )
                    .await;
                } else {
                    let mut state = SessionState::new(session_id.clone(), agent_id.clone());
                    // Emit a config null chunk if config fields are present
                    if config.backend.is_some()
                        || config.model.is_some()
                        || config.system_prompt.is_some()
                    {
                        let config_chunk = make_config_chunk(&config);
                        state.publish(config_chunk);
                    }
                    reg.insert(state);
                    drop(reg);
                    send_msg(
                        &writer,
                        &ServerMessage::SessionCreated {
                            session_id,
                            agent_id,
                        },
                    )
                    .await;
                }
            }

            ClientMessage::DeleteSession { session_id } => {
                let mut reg = registry.write().await;
                if reg.remove(&session_id) {
                    drop(reg);
                    send_msg(
                        &writer,
                        &ServerMessage::SessionDeleted {
                            session_id,
                        },
                    )
                    .await;
                } else {
                    drop(reg);
                    send_error(
                        &writer,
                        Some(&session_id),
                        &format!("Session not found: {}", session_id),
                        "SESSION_NOT_FOUND",
                    )
                    .await;
                }
            }

            ClientMessage::Publish { session_id, mut chunk } => {
                // Tag with source connection for direct-to replies
                chunk = chunk.with_annotation(keys::CAFE_SOURCE_CONNECTION, &conn_id);

                // Check for direct-to: route to specific connection instead of broadcast
                if let Some(target_id) = chunk.get_annotation::<String>(keys::CAFE_DIRECT_TO) {
                    let conns = connections.read().await;
                    if let Some(target_writer) = conns.get(&target_id) {
                        send_msg(
                            target_writer,
                            &ServerMessage::Chunk {
                                session_id,
                                chunk,
                            },
                        )
                        .await;
                    } else {
                        send_error(
                            &writer,
                            Some(&session_id),
                            &format!("Target connection not found: {}", target_id),
                            "TARGET_NOT_FOUND",
                        )
                        .await;
                    }
                    continue;
                }

                let mut reg = registry.write().await;
                if let Some(session) = reg.get_mut(&session_id) {
                    session.publish(chunk);
                } else {
                    drop(reg);
                    send_error(
                        &writer,
                        Some(&session_id),
                        &format!("Session not found: {}", session_id),
                        "SESSION_NOT_FOUND",
                    )
                    .await;
                }
            }

            ClientMessage::Subscribe { session_id } => {
                // Snapshot history + retained + get receiver while holding write lock
                let (history, retained, mut rx) = {
                    let mut reg = registry.write().await;
                    match reg.get_mut(&session_id) {
                        Some(s) => {
                            let retained = s.drain_retained();
                            (s.history.clone(), retained, s.subscribe())
                        }
                        None => {
                            drop(reg);
                            send_error(
                                &writer,
                                Some(&session_id),
                                &format!("Session not found: {}", session_id),
                                "SESSION_NOT_FOUND",
                            )
                            .await;
                            continue;
                        }
                    }
                };

                // Replay history (oldest first)
                let count = history.len() + retained.len();
                for chunk in history {
                    send_msg(
                        &writer,
                        &ServerMessage::Chunk {
                            session_id: session_id.clone(),
                            chunk,
                        },
                    )
                    .await;
                }
                // Retained transient chunks, in chronological order after history
                for chunk in retained {
                    send_msg(
                        &writer,
                        &ServerMessage::Chunk {
                            session_id: session_id.clone(),
                            chunk,
                        },
                    )
                    .await;
                }
                send_msg(
                    &writer,
                    &ServerMessage::HistoryComplete {
                        session_id: session_id.clone(),
                        count,
                    },
                )
                .await;

                // Forward live chunks in a background task
                let writer2 = writer.clone();
                let sid = session_id.clone();
                tokio::spawn(async move {
                    loop {
                        match rx.recv().await {
                            Ok(chunk) => {
                                send_msg(
                                    &writer2,
                                    &ServerMessage::Chunk {
                                        session_id: sid.clone(),
                                        chunk,
                                    },
                                )
                                .await;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("subscriber lagged by {} messages on session {}", n, sid);
                                send_error(
                                    &writer2,
                                    Some(&sid),
                                    "Subscriber lagged; re-subscribe to get full history",
                                    "SUBSCRIBER_LAGGED",
                                )
                                .await;
                                break;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                });
            }

            ClientMessage::SubscribeAll => {
                // Subscribe to all current sessions and future ones.
                let (event_rx, sessions_snapshot) = {
                    let mut reg = registry.write().await;
                    let snap: Vec<(String, String, Vec<Chunk>, Vec<Chunk>, tokio::sync::broadcast::Receiver<Chunk>)> = reg
                        .list()
                        .iter()
                        .filter_map(|info| {
                            reg.get_mut(&info.session_id).map(|s| {
                                let retained = s.drain_retained();
                                (s.session_id.clone(), s.agent_id.clone(), s.history.clone(), retained, s.subscribe())
                            })
                        })
                        .collect();
                    (reg.event_tx().subscribe(), snap)
                };

                // Announce + replay for existing sessions
                for (sid, agent_id, history, retained, rx) in sessions_snapshot {
                    send_msg(
                        &writer,
                        &ServerMessage::SessionCreated {
                            session_id: sid.clone(),
                            agent_id,
                        },
                    )
                    .await;
                    replay_and_forward(&writer, sid, history, retained, rx, None).await;
                }

                // Listen for new sessions via registry events
                let writer3 = writer.clone();
                let reg3 = registry.clone();
                tokio::spawn(async move {
                    let mut event_rx = event_rx;
                    while let Ok(event) = event_rx.recv().await {
                        // Forward registry event (SessionCreated / SessionDeleted)
                        send_msg(&writer3, &event).await;
                        // For new sessions, also replay history + forward live chunks
                        if let ServerMessage::SessionCreated { session_id, .. } = &event {
                            let maybe = {
                                let mut reg = reg3.write().await;
                                reg.get_mut(session_id)
                                    .map(|s| (s.history.clone(), s.drain_retained(), s.subscribe()))
                            };
                            if let Some((history, retained, rx)) = maybe {
                                replay_and_forward(
                                    &writer3,
                                    session_id.clone(),
                                    history,
                                    retained,
                                    rx,
                                    None,
                                )
                                .await;
                            }
                        }
                    }
                });
            }

            ClientMessage::SubscribeFiltered { filter } => {
                let filter = Arc::new(filter);

                // Snapshot matching sessions
                let (event_rx, sessions_snapshot) = {
                    let mut reg = registry.write().await;
                    let snap: Vec<(String, String, Vec<Chunk>, Vec<Chunk>, tokio::sync::broadcast::Receiver<Chunk>)> = reg
                        .list()
                        .iter()
                        .filter_map(|info| {
                            reg.get_mut(&info.session_id).and_then(|s| {
                                if !session_matches_filter(s, &filter) {
                                    return None;
                                }
                                let retained = s.drain_retained();
                                Some((s.session_id.clone(), s.agent_id.clone(), s.history.clone(), retained, s.subscribe()))
                            })
                        })
                        .collect();
                    (reg.event_tx().subscribe(), snap)
                };

                // Announce + replay for existing sessions
                for (sid, agent_id, history, retained, rx) in sessions_snapshot {
                    send_msg(
                        &writer,
                        &ServerMessage::SessionCreated {
                            session_id: sid.clone(),
                            agent_id,
                        },
                    )
                    .await;
                    replay_and_forward(&writer, sid, history, retained, rx, Some(filter.clone())).await;
                }

                // Listen for new sessions via registry events
                let writer3 = writer.clone();
                let reg3 = registry.clone();
                let filter3 = filter.clone();
                tokio::spawn(async move {
                    let mut event_rx = event_rx;
                    while let Ok(event) = event_rx.recv().await {
                        if let ServerMessage::SessionCreated { session_id, .. } = &event {
                            let maybe = {
                                let mut reg = reg3.write().await;
                                reg.get_mut(session_id).and_then(|s| {
                                    if !session_matches_filter(s, &filter3) {
                                        return None;
                                    }
                                    Some((s.history.clone(), s.drain_retained(), s.subscribe()))
                                })
                            };
                            if let Some((history, retained, rx)) = maybe {
                                // Forward SessionCreated before replay
                                send_msg(&writer3, &event).await;
                                replay_and_forward(
                                    &writer3,
                                    session_id.clone(),
                                    history,
                                    retained,
                                    rx,
                                    Some(filter3.clone()),
                                )
                                .await;
                            }
                        } else {
                            // Forward SessionDeleted etc.
                            send_msg(&writer3, &event).await;
                        }
                    }
                });
            }
        }
    }

    Ok(())
}

/// Returns true if a chunk matches the chunk-level filters (content_types, annotations).
#[doc(hidden)]
pub fn chunk_matches_filter(chunk: &Chunk, filter: &SubscribeFilter) -> bool {
    if let Some(ref types) = filter.content_types {
        if !types.contains(&chunk.content_type) {
            return false;
        }
    }
    if let Some(ref annotations) = filter.annotations {
        for (key, value) in annotations {
            match chunk.annotations.get(key) {
                Some(v) if v == value => {}
                // Boolean `false` matches both explicit false and absent
                None if value == &serde_json::Value::Bool(false) => {}
                _ => return false,
            }
        }
    }
    true
}

/// Returns true if a session-level filter matches (sessions, agents).
#[doc(hidden)]
pub fn session_matches_filter(session: &SessionState, filter: &SubscribeFilter) -> bool {
    if let Some(ref sessions) = filter.sessions {
        if !sessions.contains(&session.session_id) {
            return false;
        }
    }
    if let Some(ref agents) = filter.agents {
        if !agents.contains(&session.agent_id) {
            return false;
        }
    }
    true
}

async fn replay_and_forward(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    session_id: String,
    history: Vec<Chunk>,
    retained: Vec<Chunk>,
    mut rx: tokio::sync::broadcast::Receiver<Chunk>,
    filter: Option<Arc<SubscribeFilter>>,
) {
    for chunk in history {
        if filter.as_ref().map_or(true, |f| chunk_matches_filter(&chunk, f)) {
            send_msg(
                writer,
                &ServerMessage::Chunk {
                    session_id: session_id.clone(),
                    chunk,
                },
            )
            .await;
        }
    }
    for chunk in retained {
        if filter.as_ref().map_or(true, |f| chunk_matches_filter(&chunk, f)) {
            send_msg(
                writer,
                &ServerMessage::Chunk {
                    session_id: session_id.clone(),
                    chunk,
                },
            )
            .await;
        }
    }

    let writer2 = writer.clone();
    let sid2 = session_id.clone();
    let filter2 = filter.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(chunk) => {
                    if filter2.as_ref().map_or(true, |f| chunk_matches_filter(&chunk, f)) {
                        send_msg(
                            &writer2,
                            &ServerMessage::Chunk {
                                session_id: sid2.clone(),
                                chunk,
                            },
                        )
                        .await;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

async fn send_msg(writer: &Arc<Mutex<OwnedWriteHalf>>, msg: &ServerMessage) {
    match serde_json::to_string(msg) {
        Ok(mut json) => {
            json.push('\n');
            let mut w = writer.lock().await;
            if let Err(e) = w.write_all(json.as_bytes()).await {
                debug!("write error: {}", e);
            }
        }
        Err(e) => error!("failed to serialize message: {}", e),
    }
}

async fn send_error(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    session_id: Option<&str>,
    message: &str,
    code: &str,
) {
    send_msg(
        writer,
        &ServerMessage::Error {
            session_id: session_id.map(String::from),
            message: message.to_string(),
            code: code.to_string(),
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use cafe_types::ContentType;

    fn test_chunk() -> Chunk {
        Chunk::new_null("test").with_annotation("key1", "value1")
    }

    fn test_session(id: &str, agent: &str) -> SessionState {
        SessionState::new(id.into(), agent.into())
    }

    #[test]
    fn chunk_filter_no_filter() {
        let f = SubscribeFilter::default();
        assert!(chunk_matches_filter(&test_chunk(), &f));
    }

    #[test]
    fn chunk_filter_by_content_type() {
        let f = SubscribeFilter {
            content_types: Some(vec![ContentType::Text]),
            ..Default::default()
        };
        let text = Chunk::new_text("hello", "test");
        let bin = Chunk::new_binary(vec![1, 2], "test", "test");
        assert!(chunk_matches_filter(&text, &f));
        assert!(!chunk_matches_filter(&bin, &f));
        assert!(!chunk_matches_filter(&Chunk::new_null("test"), &f));
    }

    #[test]
    fn chunk_filter_by_annotation() {
        let f = SubscribeFilter {
            annotations: Some(HashMap::from([(
                "key1".into(),
                serde_json::Value::String("value1".into()),
            )])),
            ..Default::default()
        };
        let matching = test_chunk();
        let non_matching = Chunk::new_null("test").with_annotation("key1", "wrong");
        assert!(chunk_matches_filter(&matching, &f));
        assert!(!chunk_matches_filter(&non_matching, &f));
    }

    #[test]
    fn chunk_filter_both_type_and_annotation() {
        let f = SubscribeFilter {
            content_types: Some(vec![ContentType::Text]),
            annotations: Some(HashMap::from([(
                "key1".into(),
                serde_json::Value::String("value1".into()),
            )])),
            ..Default::default()
        };
        let chunk = Chunk::new_text("hello", "test").with_annotation("key1", "value1");
        assert!(chunk_matches_filter(&chunk, &f));

        // Wrong type
        let wrong_type = Chunk::new_binary(vec![1], "test", "test").with_annotation("key1", "value1");
        assert!(!chunk_matches_filter(&wrong_type, &f));

        // Wrong annotation
        let wrong_ann = Chunk::new_text("hello", "test").with_annotation("key1", "wrong");
        assert!(!chunk_matches_filter(&wrong_ann, &f));
    }

    #[test]
    fn session_filter_empty() {
        let s = test_session("s1", "agent1");
        let f = SubscribeFilter::default();
        assert!(session_matches_filter(&s, &f));
    }

    #[test]
    fn session_filter_by_id() {
        let s = test_session("s1", "agent1");
        let f = SubscribeFilter {
            sessions: Some(vec!["s1".into(), "s2".into()]),
            ..Default::default()
        };
        assert!(session_matches_filter(&s, &f));

        let f2 = SubscribeFilter {
            sessions: Some(vec!["s3".into()]),
            ..Default::default()
        };
        assert!(!session_matches_filter(&s, &f2));
    }

    #[test]
    fn session_filter_by_agent() {
        let s = test_session("s1", "agent1");
        let f = SubscribeFilter {
            agents: Some(vec!["agent1".into()]),
            ..Default::default()
        };
        assert!(session_matches_filter(&s, &f));

        let f2 = SubscribeFilter {
            agents: Some(vec!["agent2".into()]),
            ..Default::default()
        };
        assert!(!session_matches_filter(&s, &f2));
    }

    // ── Property-based tests (proptest) ──

    use proptest::prelude::*;

    fn any_content_type() -> impl Strategy<Value = ContentType> {
        prop_oneof![
            Just(ContentType::Text),
            Just(ContentType::Binary),
            Just(ContentType::BinaryRef),
            Just(ContentType::Null),
        ]
    }

    fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
        prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            ".{0,20}".prop_map(serde_json::Value::String),
            (any::<i64>()).prop_map(|n| serde_json::json!(n)),
        ]
    }

    fn annotation_map() -> impl Strategy<Value = HashMap<String, serde_json::Value>> {
        prop::collection::hash_map("[a-z._-]{1,15}", arb_json_value(), 0..5)
    }

    fn any_chunk() -> impl Strategy<Value = Chunk> {
        (
            "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            any_content_type(),
            proptest::option::of(".{0,50}"),
            proptest::option::of(prop::collection::vec(any::<u8>(), 0..50)),
            proptest::option::of("[a-z/._-]{0,30}"),
            "[a-zA-Z0-9._-]{1,30}",
            annotation_map(),
            any::<i64>(),
        )
            .prop_map(
                |(id, content_type, content, data, mime_type, producer, annotations, timestamp)| {
                    Chunk {
                        id,
                        content_type,
                        content,
                        data,
                        mime_type,
                        producer,
                        annotations,
                        timestamp,
                    }
                },
            )
    }

    fn any_session_state() -> impl Strategy<Value = SessionState> {
        (".{0,20}", ".{0,20}")
            .prop_map(|(session_id, agent_id)| SessionState::new(session_id, agent_id))
    }

    proptest! {
        // ── chunk_matches_filter properties ──

        #[test]
        fn chunk_filter_empty_matches_all(chunk in any_chunk()) {
            let filter = SubscribeFilter::default();
            prop_assert!(chunk_matches_filter(&chunk, &filter));
        }

        #[test]
        fn chunk_filter_content_type_present(chunk in any_chunk()) {
            let ct = chunk.content_type.clone();
            let filter = SubscribeFilter {
                content_types: Some(vec![ct]),
                ..Default::default()
            };
            prop_assert!(chunk_matches_filter(&chunk, &filter));
        }

        #[test]
        fn chunk_filter_content_type_rejects_other(
            chunk in any_chunk(),
            other_ct in any_content_type(),
        ) {
            prop_assume!(chunk.content_type != other_ct);
            let filter = SubscribeFilter {
                content_types: Some(vec![other_ct]),
                ..Default::default()
            };
            prop_assert!(!chunk_matches_filter(&chunk, &filter));
        }

        #[test]
        fn chunk_filter_empty_types_rejects_all(chunk in any_chunk()) {
            let filter = SubscribeFilter {
                content_types: Some(vec![]),
                ..Default::default()
            };
            prop_assert!(!chunk_matches_filter(&chunk, &filter));
        }

        #[test]
        fn chunk_filter_annotation_match(
            chunk in any_chunk(),
            key in "[a-z._-]{1,10}",
            value in arb_json_value(),
        ) {
            let annotated = chunk.clone().with_annotation(&key, &value);
            let mut ann_map = HashMap::new();
            ann_map.insert(key, value);
            let filter = SubscribeFilter {
                annotations: Some(ann_map),
                ..Default::default()
            };
            prop_assert!(chunk_matches_filter(&annotated, &filter));
        }

        #[test]
        fn chunk_filter_annotation_rejects_when_missing(
            chunk in any_chunk(),
            key in "[a-z._-]{1,10}",
            value in arb_json_value(),
        ) {
            // Skip cases where the chunk coincidentally has the annotation
            prop_assume!(chunk.annotations.get(&key) != Some(&value));
            // Also skip false — false is special (matches absent)
            prop_assume!(value != serde_json::Value::Bool(false));

            let mut ann_map = HashMap::new();
            ann_map.insert(key, value);
            let filter = SubscribeFilter {
                annotations: Some(ann_map),
                ..Default::default()
            };
            prop_assert!(!chunk_matches_filter(&chunk, &filter));
        }

        #[test]
        fn chunk_filter_annotation_false_matches_absent(
            chunk in any_chunk(),
            key in "[a-z._-]{1,10}",
        ) {
            prop_assume!(!chunk.annotations.contains_key(&key));
            let mut ann_map = HashMap::new();
            ann_map.insert(key, serde_json::Value::Bool(false));
            let filter = SubscribeFilter {
                annotations: Some(ann_map),
                ..Default::default()
            };
            prop_assert!(chunk_matches_filter(&chunk, &filter));
        }

        #[test]
        fn chunk_filter_and_semantics(
            chunk in any_chunk(),
            other_type in any_content_type(),
            key in "[a-z._-]{1,10}",
            value in arb_json_value(),
        ) {
            // Both content_type and annotation must fail independently
            prop_assume!(chunk.content_type != other_type);
            prop_assume!(!chunk.annotations.contains_key(&key));

            let mut ann_map = HashMap::new();
            ann_map.insert(key, value);
            let filter = SubscribeFilter {
                content_types: Some(vec![other_type]),
                annotations: Some(ann_map),
                ..Default::default()
            };
            prop_assert!(!chunk_matches_filter(&chunk, &filter));

            // type matches but annotation fails
            let mut ann_map2 = HashMap::new();
            ann_map2.insert("nonexistent.key".to_string(), serde_json::Value::String("x".into()));
            let filter2 = SubscribeFilter {
                content_types: Some(vec![chunk.content_type.clone()]),
                annotations: Some(ann_map2),
                ..Default::default()
            };
            prop_assert!(!chunk_matches_filter(&chunk, &filter2));
        }

        // ── session_matches_filter properties ──

        #[test]
        fn session_filter_empty_matches_all(s in any_session_state()) {
            let filter = SubscribeFilter::default();
            prop_assert!(session_matches_filter(&s, &filter));
        }

        #[test]
        fn session_filter_by_id_match(s in any_session_state()) {
            let sid = s.session_id.clone();
            let filter = SubscribeFilter {
                sessions: Some(vec![sid]),
                ..Default::default()
            };
            prop_assert!(session_matches_filter(&s, &filter));
        }

        #[test]
        fn session_filter_by_id_no_match(
            s in any_session_state(),
            other_id in ".{0,20}",
        ) {
            prop_assume!(s.session_id != other_id);
            let filter = SubscribeFilter {
                sessions: Some(vec![other_id]),
                ..Default::default()
            };
            prop_assert!(!session_matches_filter(&s, &filter));
        }

        #[test]
        fn session_filter_by_agent_match(s in any_session_state()) {
            let aid = s.agent_id.clone();
            let filter = SubscribeFilter {
                agents: Some(vec![aid]),
                ..Default::default()
            };
            prop_assert!(session_matches_filter(&s, &filter));
        }

        #[test]
        fn session_filter_by_agent_no_match(
            s in any_session_state(),
            other_agent in ".{0,20}",
        ) {
            prop_assume!(s.agent_id != other_agent);
            let filter = SubscribeFilter {
                agents: Some(vec![other_agent]),
                ..Default::default()
            };
            prop_assert!(!session_matches_filter(&s, &filter));
        }

        #[test]
        fn session_filter_and_semantics(
            s in any_session_state(),
            other_id in ".{0,20}",
            other_agent in ".{0,20}",
        ) {
            // Both sessions and agents must match (AND semantics)
            prop_assume!(s.session_id != other_id);
            prop_assume!(s.agent_id != other_agent);

            // Neither matches → fail
            let filter = SubscribeFilter {
                sessions: Some(vec![other_id.clone()]),
                agents: Some(vec![other_agent.clone()]),
                ..Default::default()
            };
            prop_assert!(!session_matches_filter(&s, &filter));

            // Sessions matches, agents doesn't → fail (AND)
            let filter2 = SubscribeFilter {
                sessions: Some(vec![s.session_id.clone()]),
                agents: Some(vec![other_agent.clone()]),
                ..Default::default()
            };
            prop_assert!(!session_matches_filter(&s, &filter2));

            // Agents matches, sessions doesn't → fail (AND)
            let filter3 = SubscribeFilter {
                sessions: Some(vec![other_id.clone()]),
                agents: Some(vec![s.agent_id.clone()]),
                ..Default::default()
            };
            prop_assert!(!session_matches_filter(&s, &filter3));

            // Both match → pass
            let filter4 = SubscribeFilter {
                sessions: Some(vec![s.session_id.clone()]),
                agents: Some(vec![s.agent_id.clone()]),
                ..Default::default()
            };
            prop_assert!(session_matches_filter(&s, &filter4));
        }

        #[test]
        fn session_filter_empty_vecs_are_empty(
            s in any_session_state(),
        ) {
            // Empty sessions vec → nothing matches sessions filter
            let filter = SubscribeFilter {
                sessions: Some(vec![]),
                ..Default::default()
            };
            prop_assert!(!session_matches_filter(&s, &filter));

            // Empty agents vec → nothing matches agents filter
            let filter2 = SubscribeFilter {
                agents: Some(vec![]),
                ..Default::default()
            };
            prop_assert!(!session_matches_filter(&s, &filter2));
        }

        // ── Filter monotonic property ──
        // Adding constraints can only reduce matches.

        #[test]
        fn chunk_filter_monotonic_content_type(
            chunk in any_chunk(),
            ct in any_content_type(),
        ) {
            let empty = SubscribeFilter::default();
            let restricted = SubscribeFilter {
                content_types: Some(vec![ct]),
                ..Default::default()
            };
            // If the chunk matches restricted, it must also match empty
            if chunk_matches_filter(&chunk, &restricted) {
                prop_assert!(chunk_matches_filter(&chunk, &empty));
            }
        }

        #[test]
        fn chunk_filter_monotonic_annotation(
            chunk in any_chunk(),
            key in "[a-z._-]{1,10}",
            value in arb_json_value(),
        ) {
            let mut ann = HashMap::new();
            ann.insert(key, value);
            let restricted = SubscribeFilter {
                annotations: Some(ann),
                ..Default::default()
            };
            let empty = SubscribeFilter::default();
            if chunk_matches_filter(&chunk, &restricted) {
                prop_assert!(chunk_matches_filter(&chunk, &empty));
            }
        }

        #[test]
        fn chunk_filter_monotonic_combined(
            chunk in any_chunk(),
            ct in any_content_type(),
            key in "[a-z._-]{1,10}",
            value in arb_json_value(),
        ) {
            let mut ann = HashMap::new();
            ann.insert(key.clone(), value.clone());
            let restricted = SubscribeFilter {
                content_types: Some(vec![ct.clone()]),
                annotations: Some(ann),
                ..Default::default()
            };
            let type_only = SubscribeFilter {
                content_types: Some(vec![ct]),
                ..Default::default()
            };
            let ann_only = SubscribeFilter {
                annotations: Some(HashMap::from([(key.clone(), value)])),
                ..Default::default()
            };
            // If chunk matches combined, it must match each component
            if chunk_matches_filter(&chunk, &restricted) {
                prop_assert!(chunk_matches_filter(&chunk, &type_only),
                    "matched combined filter but not content_type-only filter");
                prop_assert!(chunk_matches_filter(&chunk, &ann_only),
                    "matched combined filter but not annotation-only filter");
            }
        }

        #[test]
        fn chunk_filter_monotonic_annotation_values(
            chunk in any_chunk(),
            key in "[a-z._-]{1,10}",
            value1 in arb_json_value(),
            value2 in arb_json_value(),
        ) {
            let mut ann_restrictive = HashMap::new();
            ann_restrictive.insert(key.clone(), value2.clone());
            let restrictive = SubscribeFilter {
                annotations: Some(ann_restrictive),
                ..Default::default()
            };
            // 2-key filter (more restrictive) implies 1-key filter (less restrictive)
            // But only if the chunk doesn't have key mapped to a different value
            let mut ann_broad = HashMap::new();
            ann_broad.insert(key.clone(), value1);
            ann_broad.insert("another.key".to_string(), value2);
            let broad = SubscribeFilter {
                annotations: Some(ann_broad),
                ..Default::default()
            };
            if chunk_matches_filter(&chunk, &broad) {
                prop_assert!(chunk_matches_filter(&chunk, &restrictive));
            }
        }

        #[test]
        fn session_filter_monotonic_sessions(
            s in any_session_state(),
            extra_id in ".{0,20}",
        ) {
            let specific = SubscribeFilter {
                sessions: Some(vec![s.session_id.clone()]),
                ..Default::default()
            };
            let broader = SubscribeFilter {
                sessions: Some(vec![s.session_id.clone(), extra_id]),
                ..Default::default()
            };
            // If the session matches the broader set, it must also match the specific one
            if session_matches_filter(&s, &broader) {
                prop_assert!(session_matches_filter(&s, &specific));
            }
        }

        #[test]
        fn session_filter_monotonic_agents(
            s in any_session_state(),
            extra_agent in ".{0,20}",
        ) {
            let specific = SubscribeFilter {
                agents: Some(vec![s.agent_id.clone()]),
                ..Default::default()
            };
            let broader = SubscribeFilter {
                agents: Some(vec![s.agent_id.clone(), extra_agent]),
                ..Default::default()
            };
            if session_matches_filter(&s, &broader) {
                prop_assert!(session_matches_filter(&s, &specific));
            }
        }

        // ── Source connection injection (ADR-101) ──

        #[test]
        fn source_connection_injected_on_publish(
            chunk in any_chunk(),
            conn_id in "[a-zA-Z0-9._-]{1,30}",
        ) {
            let ct = chunk.content_type.clone();
            let producer = chunk.producer.clone();
            let tagged = chunk.with_annotation(keys::CAFE_SOURCE_CONNECTION, &conn_id);
            prop_assert_eq!(
                tagged.get_annotation::<String>(keys::CAFE_SOURCE_CONNECTION),
                Some(conn_id)
            );
            // Other fields unchanged
            prop_assert_eq!(tagged.content_type, ct);
            prop_assert_eq!(tagged.producer, producer);
        }

        // ── Direct-to precondition (ADR-102) ──

        #[test]
        fn direct_to_present_when_annotated(
            chunk in any_chunk(),
            target in "[a-zA-Z0-9._-]{1,30}",
        ) {
            let with_direct = chunk.with_annotation(keys::CAFE_DIRECT_TO, &target);
            prop_assert_eq!(
                with_direct.get_annotation::<String>(keys::CAFE_DIRECT_TO),
                Some(target)
            );
        }

        #[test]
        fn direct_to_absent_when_not_annotated(chunk in any_chunk()) {
            prop_assert!(chunk.get_annotation::<String>(keys::CAFE_DIRECT_TO).is_none());
        }

        #[test]
        fn direct_to_skips_session_publish(
            chunk in any_chunk(),
            target in "[a-zA-Z0-9._-]{1,30}",
        ) {
            let with_direct = chunk.with_annotation(keys::CAFE_DIRECT_TO, &target);
            // SessionState::publish does NOT check for direct_to, so the chunk
            // is appended to history normally. The skip-broadcast behavior is
            // enforced at the handler level (client_loop in client.rs).
            // This test verifies the annotation precondition holds.
            let mut state = SessionState::new("test".into(), "test".into());
            state.publish(with_direct.clone());
            // The chunk goes to history because SessionState.publish doesn't
            // implement direct_to logic
            if !with_direct.is_transient() {
                assert_eq!(state.history.len(), 1);
                assert_eq!(
                    state.history[0].get_annotation::<String>(keys::CAFE_DIRECT_TO),
                    Some(target)
                );
            }
        }
    }

    // ── Connection ID properties (ADR-101) ──

    #[test]
    fn connection_ids_are_monotonic() {
        let first = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
        let second = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
        assert!(second > first, "connection IDs must be strictly increasing");
        assert_eq!(second, first + 1, "connection IDs must be sequential");
    }

    #[test]
    fn connection_id_format_is_c_prefix() {
        let id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
        let conn_id = format!("c-{}", id);
        assert!(conn_id.starts_with("c-"), "connection ID must start with 'c-'");
        // Verify it parses back to the same number
        let parsed: u64 = conn_id.strip_prefix("c-").unwrap().parse().unwrap();
        assert_eq!(parsed, id);
    }
}

fn make_config_chunk(config: &SessionConfig) -> Chunk {
    let mut chunk = Chunk::new_null("com.nominal.cafe-bus")
        .with_annotation(keys::CONFIG_TYPE, "runtime");
    if let Some(b) = &config.backend {
        chunk = chunk.with_annotation(keys::CONFIG_BACKEND, b);
    }
    if let Some(m) = &config.model {
        chunk = chunk.with_annotation(keys::CONFIG_MODEL, m);
    }
    if let Some(sp) = &config.system_prompt {
        chunk = chunk.with_annotation(keys::CONFIG_SYSTEM_PROMPT, sp);
    }
    if let Some(t) = config.temperature {
        chunk = chunk.with_annotation(keys::CONFIG_TEMPERATURE, t);
    }
    if let Some(mt) = config.max_tokens {
        chunk = chunk.with_annotation(keys::CONFIG_MAX_TOKENS, mt);
    }
    chunk
}
