use crate::registry::SessionRegistry;
use crate::session::SessionState;
use anyhow::Result;
use cafe_types::{
    keys, Chunk, ClientMessage, ServerMessage, SessionConfig,
};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, warn};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024; // 16 MB

pub async fn handle_client(
    stream: tokio::net::UnixStream,
    registry: Arc<RwLock<SessionRegistry>>,
) {
    let (reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));

    if let Err(e) = client_loop(reader, writer, registry).await {
        debug!("client disconnected: {}", e);
    }
}

async fn client_loop(
    reader: OwnedReadHalf,
    writer: Arc<Mutex<OwnedWriteHalf>>,
    registry: Arc<RwLock<SessionRegistry>>,
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

            ClientMessage::Publish { session_id, chunk } => {
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
                // Snapshot history + get receiver while holding read lock
                let (history, mut rx) = {
                    let reg = registry.read().await;
                    match reg.get(&session_id) {
                        Some(s) => (s.history.clone(), s.subscribe()),
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

                // Replay history
                let count = history.len();
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
                    let reg = registry.read().await;
                    let snap: Vec<(String, Vec<Chunk>, tokio::sync::broadcast::Receiver<Chunk>)> = reg
                        .list()
                        .iter()
                        .filter_map(|info| {
                            reg.get(&info.session_id).map(|s| {
                                (s.session_id.clone(), s.history.clone(), s.subscribe())
                            })
                        })
                        .collect();
                    (reg.event_tx().subscribe(), snap)
                };

                for (sid, history, rx) in sessions_snapshot {
                    replay_and_forward(&writer, sid, history, rx).await;
                }

                // Listen for new sessions via registry events
                let writer3 = writer.clone();
                let reg3 = registry.clone();
                tokio::spawn(async move {
                    let mut event_rx = event_rx;
                    while let Ok(event) = event_rx.recv().await {
                        match &event {
                            ServerMessage::SessionCreated { session_id, .. } => {
                                let maybe = {
                                    let reg = reg3.read().await;
                                    reg.get(session_id)
                                        .map(|s| (s.history.clone(), s.subscribe()))
                                };
                                if let Some((history, rx)) = maybe {
                                    replay_and_forward(
                                        &writer3,
                                        session_id.clone(),
                                        history,
                                        rx,
                                    )
                                    .await;
                                }
                            }
                            _ => {
                                // Forward SessionDeleted etc.
                                send_msg(&writer3, &event).await;
                            }
                        }
                    }
                });
            }
        }
    }

    Ok(())
}

async fn replay_and_forward(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    session_id: String,
    history: Vec<Chunk>,
    mut rx: tokio::sync::broadcast::Receiver<Chunk>,
) {
    for chunk in history {
        send_msg(
            writer,
            &ServerMessage::Chunk {
                session_id: session_id.clone(),
                chunk,
            },
        )
        .await;
    }

    let writer2 = writer.clone();
    let sid2 = session_id.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(chunk) => {
                    send_msg(
                        &writer2,
                        &ServerMessage::Chunk {
                            session_id: sid2.clone(),
                            chunk,
                        },
                    )
                    .await;
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
