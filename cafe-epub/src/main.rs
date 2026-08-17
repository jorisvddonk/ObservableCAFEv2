mod commands;
mod epub;
mod narrator;

use cafe_sdk::bus::{BusClient, SessionSubscription};
use cafe_sdk::{
    keys, roles, rpc_errors, Chunk, ContentType, EvaluatorSchema, JsonRpcRequest, JsonRpcResponse,
    ServerMessage,
};
use narrator::Narrator;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

const PRODUCER: &str = "com.nominal.cafe-epub";
const EPUB_MIME: &str = "application/epub+zip";
const TTS_RPC_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let socket_path =
        std::env::var("CAFE_BUS_SOCKET").unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());
    let narrator = Arc::new(Narrator::new());

    cafe_sdk::bus::run_with_reconnect("cafe-epub", move || {
        let sp = socket_path.clone();
        let n = narrator.clone();
        async move { subscribe_all(&sp, n).await }
    })
    .await;

    Ok(())
}

async fn subscribe_all(socket_path: &str, narrator: Arc<Narrator>) -> anyhow::Result<()> {
    info!("cafe-epub: starting on {}", socket_path);
    let client = BusClient::unix(socket_path);

    let schema = EvaluatorSchema {
        name: "epub".into(),
        description: "EPUB narrator — loads EPUB books and reads chapters aloud via TTS".into(),
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "config.tts.profile": { "type": "string", "description": "TTS voice profile" },
                "config.epub.path": { "type": "string", "description": "Loaded EPUB source path" },
                "config.epub.chapter": { "type": "integer", "description": "Current chapter index" }
            }
        }),
        rpc_params_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "string" },
                "text": { "type": "string", "description": "User message / command text" }
            }
        }),
    };
    if let Err(e) = cafe_sdk::schema::announce_schema(&client, schema).await {
        warn!("cafe-epub: failed to announce schema: {}", e);
    }

    let mut rx = client.subscribe_all().await?;
    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let c = client.clone();
            let sp = socket_path.to_string();
            let n = narrator.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, sp, c, n).await {
                    warn!("cafe-epub: session error: {}", e);
                }
            });
        }
    }
    Ok(())
}

/// Bytes awaiting upload to the binary-store for a published BinaryRef chunk.
struct PendingUpload {
    bytes: Vec<u8>,
}

async fn run_session(
    session_id: String,
    _socket_path: String,
    client: BusClient,
    narrator: Arc<Narrator>,
) -> anyhow::Result<()> {
    let mut sub = client.subscribe_session(&session_id).await?;
    let http = reqwest::Client::new();
    let mut pending: HashMap<String, PendingUpload> = HashMap::new();

    // Restore a previously loaded book from persisted session history.
    restore_state(&client, &session_id, &narrator).await;

    // Gate dispatch on history replay completion (ADR-123).
    let mut history_complete = false;

    while let Some(msg) = sub.rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            ServerMessage::HistoryComplete { .. } => {
                history_complete = true;
                continue;
            }
            _ => continue,
        };

        if !history_complete {
            continue;
        }

        // Handle binary-store write credentials for pending book-blob uploads.
        if let Some(target_id) = chunk.is_mutation() {
            if chunk.producer == "com.nominal.cafe-binary-store" {
                if let Some(upload) = pending.remove(&target_id) {
                    let write_url = chunk.get_annotation::<String>(keys::CAFE_BINARY_WRITE_URL);
                    let write_token = chunk.get_annotation::<String>(keys::CAFE_BINARY_WRITE_TOKEN);
                    if let (Some(url), Some(token)) = (write_url, write_token) {
                        let sid = session_id.clone();
                        let http = http.clone();
                        tokio::spawn(async move {
                            let upload_url =
                                format!("{}?token={}&session_id={}", url, token, sid);
                            match http
                                .post(&upload_url)
                                .header("Content-Type", EPUB_MIME)
                                .body(upload.bytes)
                                .send()
                                .await
                            {
                                Ok(r) if r.status().is_success() => info!(
                                    "cafe-epub: uploaded book blob {} ({} bytes)",
                                    target_id,
                                    r.content_length().unwrap_or(0)
                                ),
                                Ok(r) => warn!(
                                    "cafe-epub: blob upload failed for {}: HTTP {}",
                                    target_id,
                                    r.status()
                                ),
                                Err(e) => {
                                    warn!("cafe-epub: blob upload error for {}: {}", target_id, e)
                                }
                            }
                        });
                    }
                }
                continue;
            }
        }

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if request.method != "epub.invoke" {
            continue;
        }

        info!(
            "cafe-epub: handling RPC request id={} session={}",
            request.id, session_id
        );
        let call_id = request.id.clone();
        let result =
            handle_epub_request(&request, &session_id, &mut sub, &client, &narrator, &mut pending)
                .await;

        let response = match result {
            Ok(payload) => JsonRpcResponse::ok(&call_id, payload),
            Err(e) => {
                error!("cafe-epub: command error for call {}: {}", call_id, e);
                JsonRpcResponse::err(&call_id, rpc_errors::INTERNAL_ERROR, e.to_string())
            }
        };
        let resp_chunk = Chunk::new_null(PRODUCER)
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = sub.publish(resp_chunk).await;
    }

    Ok(())
}

async fn handle_epub_request(
    request: &JsonRpcRequest,
    session_id: &str,
    sub: &mut SessionSubscription,
    client: &BusClient,
    narrator: &Arc<Narrator>,
    pending: &mut HashMap<String, PendingUpload>,
) -> anyhow::Result<serde_json::Value> {
    let text = request.params["text"].as_str().unwrap_or("");
    let Some(cmd) = commands::parse(text) else {
        return Ok(serde_json::json!({ "handled": false }));
    };

    match cmd {
        commands::Command::Load(path) => {
            handle_load(session_id, sub, narrator, pending, &path).await
        }
        commands::Command::Next => handle_read(session_id, sub, client, narrator, Move::Next).await,
        commands::Command::Prev => handle_read(session_id, sub, client, narrator, Move::Prev).await,
        commands::Command::Chapter(n) => {
            handle_read(session_id, sub, client, narrator, Move::Chapter(n)).await
        }
        commands::Command::List => handle_list(session_id, sub, narrator).await,
        commands::Command::Current => handle_current(session_id, sub, narrator).await,
        commands::Command::Help => handle_help(sub).await,
    }
}

// ── Command handlers ─────────────────────────────────────────────────────────

async fn handle_load(
    session_id: &str,
    sub: &mut SessionSubscription,
    narrator: &Arc<Narrator>,
    pending: &mut HashMap<String, PendingUpload>,
    path: &str,
) -> anyhow::Result<serde_json::Value> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| anyhow::anyhow!("cannot read '{path}': {e}"))?;
    let book = epub::parse_epub_bytes(bytes.clone())
        .map_err(|e| anyhow::anyhow!("failed to parse '{path}': {e}"))?;

    // Persist the book as a blob in the session: publish a BinaryRef chunk, then
    // upload the bytes when the binary-store delivers write credentials.
    let blob_chunk = Chunk::new_binary_ref(EPUB_MIME, PRODUCER)
        .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT)
        .with_annotation(keys::CAFE_BINARY_BYTE_SIZE, bytes.len() as u64);
    let blob_id = blob_chunk.id.clone();
    sub.publish(blob_chunk).await?;
    pending.insert(blob_id.clone(), PendingUpload { bytes });

    narrator
        .with_session(session_id, |s| {
            s.book = Some(book.clone());
            s.current = 0;
            s.blob_chunk_id = Some(blob_id.clone());
            s.source_path = Some(path.to_string());
        })
        .await;
    publish_state_annotations(sub, session_id, narrator).await?;

    let summary = format!(
        "Loaded \"{}\"{}. {} chapters. Say !next to start, !list for the table of contents, or !chapter N to jump.",
        book.title,
        book.creator
            .as_ref()
            .map(|c| format!(" by {c}"))
            .unwrap_or_default(),
        book.chapter_count(),
    );
    publish_assistant_text(sub, &summary).await?;

    Ok(serde_json::json!({
        "handled": true,
        "book": book.title,
        "chapters": book.chapter_count(),
    }))
}

enum Move {
    Next,
    Prev,
    Chapter(usize),
}

async fn handle_read(
    session_id: &str,
    sub: &mut SessionSubscription,
    client: &BusClient,
    narrator: &Arc<Narrator>,
    mv: Move,
) -> anyhow::Result<serde_json::Value> {
    let moved = narrator
        .with_session(session_id, |s| -> anyhow::Result<usize> {
            if s.book.is_none() {
                return Err(anyhow::anyhow!("No book loaded. Use !load <path> first."));
            }
            let total = s.book.as_ref().expect("checked above").chapter_count();
            match mv {
                Move::Next => s
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Already at the last chapter."))?,
                Move::Prev => s
                    .prev()
                    .ok_or_else(|| anyhow::anyhow!("Already at the first chapter."))?,
                Move::Chapter(n) => s.goto(n).ok_or_else(|| {
                    anyhow::anyhow!("Chapter {n} is out of range (1-{total}).")
                })?,
            };
            Ok(s.current)
        })
        .await?;

    let (chapter_title, text) = narrator
        .peek(session_id, |s| {
            let ch = s.current_chapter().expect("book is loaded");
            (ch.title.clone(), ch.text.clone())
        })
        .await;

    // Publish the chapter text to the session so it appears in the transcript.
    let display = format!("Chapter {} — {chapter_title}\n\n{text}", moved + 1);
    publish_assistant_text(sub, &display).await?;

    // Persist the new position so it survives a restart.
    publish_state_annotations(sub, session_id, narrator).await?;

    // Vocalize the chapter by emitting a tts.invoke RPC request on the session.
    let profile = resolve_tts_profile(client, session_id).await;
    match emit_tts(sub, client, session_id, &text, profile.as_deref()).await {
        Ok(()) => Ok(serde_json::json!({
            "handled": true,
            "chapter": moved + 1,
            "narrated": true,
        })),
        Err(e) => {
            warn!("cafe-epub: TTS failed for session {}: {}", session_id, e);
            let note = format!("(TTS failed: {e})");
            publish_assistant_text(sub, &note).await?;
            Ok(serde_json::json!({
                "handled": true,
                "chapter": moved + 1,
                "narrated": false,
                "error": e.to_string(),
            }))
        }
    }
}

async fn handle_list(
    session_id: &str,
    sub: &mut SessionSubscription,
    narrator: &Arc<Narrator>,
) -> anyhow::Result<serde_json::Value> {
    let (book_title, current, chapters) = narrator
        .peek(session_id, |s| {
            let book = s.book.as_ref();
            let title = book.map(|b| b.title.clone());
            let chapters: Vec<(String, bool)> = book
                .map(|b| {
                    b.chapters
                        .iter()
                        .enumerate()
                        .map(|(i, c)| (c.title.clone(), i == s.current))
                        .collect()
                })
                .unwrap_or_default();
            (title, s.current, chapters)
        })
        .await;

    if chapters.is_empty() {
        publish_assistant_text(sub, "No book loaded. Use !load <path> first.").await?;
        return Ok(serde_json::json!({ "handled": true, "chapters": 0 }));
    }

    let mut lines = vec![format!(
        "\"{}\" — {} chapters:",
        book_title.unwrap_or_default(),
        chapters.len()
    )];
    for (i, (title, is_current)) in chapters.iter().enumerate() {
        let marker = if *is_current { "*" } else { " " };
        lines.push(format!("{marker} {:>3}. {title}", i + 1));
    }
    publish_assistant_text(sub, &lines.join("\n")).await?;

    Ok(serde_json::json!({ "handled": true, "chapters": chapters.len(), "current": current }))
}

async fn handle_current(
    session_id: &str,
    sub: &mut SessionSubscription,
    narrator: &Arc<Narrator>,
) -> anyhow::Result<serde_json::Value> {
    let (book_title, chapter_title, current, total) = narrator
        .peek(session_id, |s| {
            let current = s.current;
            let total = s.book.as_ref().map(|b| b.chapter_count()).unwrap_or(0);
            let chapter_title = s.current_chapter().map(|c| c.title.clone());
            (s.book.as_ref().map(|b| b.title.clone()), chapter_title, current, total)
        })
        .await;

    let msg = match (book_title, chapter_title) {
        (Some(b), Some(c)) => {
            format!("\"{b}\" — chapter {} of {total}: {c}", current + 1)
        }
        _ => "No book loaded. Use !load <path> first.".to_string(),
    };
    publish_assistant_text(sub, &msg).await?;

    Ok(serde_json::json!({ "handled": true }))
}

async fn handle_help(sub: &mut SessionSubscription) -> anyhow::Result<serde_json::Value> {
    let help = "EPUB narrator commands:\n\
        !load <path> — load an EPUB\n\
        !next / !n — read the next chapter aloud\n\
        !prev / !p — read the previous chapter\n\
        !chapter <n> / !ch <n> — jump to chapter n\n\
        !list / !l — list all chapters\n\
        !current / !c — show current chapter\n\
        !help — show this help";
    publish_assistant_text(sub, help).await?;
    Ok(serde_json::json!({ "handled": true }))
}

// ── TTS RPC emission ─────────────────────────────────────────────────────────

/// Publish a `tts.invoke` JSON-RPC request for `text` and await the response.
async fn emit_tts(
    sub: &mut SessionSubscription,
    client: &BusClient,
    session_id: &str,
    text: &str,
    profile: Option<&str>,
) -> anyhow::Result<()> {
    if text.trim().is_empty() {
        anyhow::bail!("chapter has no readable text");
    }

    let mut params = serde_json::json!({ "text": text });
    if let Some(p) = profile {
        params["profile"] = serde_json::Value::String(p.to_string());
    }
    let request = JsonRpcRequest::new("tts.invoke", params);
    let call_id = request.id.clone();

    let req_chunk = Chunk::new_null(PRODUCER)
        .with_annotation(keys::CAFE_JSONRPC_REQUEST, &request)
        .as_transient()
        .with_retain(60);
    sub.publish(req_chunk).await?;

    // Await the TTS response on a dedicated subscription. cafe-tts publishes
    // its response only after receiving our request, so no race here.
    let mut rx = client.subscribe(session_id).await?;
    tokio::time::timeout(TTS_RPC_TIMEOUT, async {
        loop {
            match rx.recv().await {
                Some(ServerMessage::Chunk { chunk, .. }) => {
                    if chunk.is_rpc_response_for(&call_id) {
                        return chunk.as_rpc_response().ok_or_else(|| {
                            anyhow::anyhow!("malformed tts response for call {call_id}")
                        });
                    }
                }
                Some(_) => continue,
                None => {
                    return Err(anyhow::anyhow!(
                        "bus disconnected while awaiting tts response"
                    ));
                }
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out after {}s waiting for TTS", TTS_RPC_TIMEOUT.as_secs()))?
    .and_then(|resp| {
        if resp.is_ok() {
            Ok(())
        } else {
            let err = resp
                .error
                .ok_or_else(|| anyhow::anyhow!("tts response carried no error detail"))?;
            Err(anyhow::anyhow!("TTS error [{}]: {}", err.code, err.message))
        }
    })
}

/// Resolve the TTS voice profile from session config annotations.
async fn resolve_tts_profile(client: &BusClient, session_id: &str) -> Option<String> {
    let history = client.get_history(session_id).await.ok()?;
    history
        .iter()
        .rev()
        .find_map(|c| {
            c.annotations
                .get("config.tts.profile")
                .and_then(|v| v.as_str().map(String::from))
        })
}

// ── Session state persistence ─────────────────────────────────────────────────

/// Publish a config annotation chunk recording the current book + position so
/// the narrator can restore state after a restart.
async fn publish_state_annotations(
    sub: &mut SessionSubscription,
    session_id: &str,
    narrator: &Arc<Narrator>,
) -> anyhow::Result<()> {
    let (path, current, started) = narrator
        .peek(session_id, |s| (s.source_path.clone(), s.current, s.started))
        .await;
    let mut chunk = Chunk::new_null(PRODUCER).with_annotation("config.type", "runtime");
    if let Some(path) = path {
        chunk = chunk.with_annotation("config.epub.path", &path);
    }
    chunk = chunk.with_annotation("config.epub.chapter", current);
    chunk = chunk.with_annotation("config.epub.started", started);
    sub.publish(chunk).await?;
    Ok(())
}

/// Restore a previously loaded book from persisted session history.
async fn restore_state(client: &BusClient, session_id: &str, narrator: &Arc<Narrator>) {
    let Ok(history) = client.get_history(session_id).await else {
        return;
    };

    // Most recent EPUB blob BinaryRef published by this evaluator.
    let blob_ref = history
        .iter()
        .rev()
        .find(|c| {
            c.content_type == ContentType::BinaryRef
                && c.producer == PRODUCER
                && c.mime_type.as_deref() == Some(EPUB_MIME)
        })
        .cloned();
    let Some(blob_ref) = blob_ref else { return };

    let read_creds = find_read_credentials(&history, &blob_ref.id);
    let source_path = latest_annotation(&history, "config.epub.path")
        .and_then(|v| v.as_str().map(String::from));
    let current = latest_annotation(&history, "config.epub.chapter")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let started = latest_annotation(&history, "config.epub.started")
        .and_then(|v| v.as_bool())
        .unwrap_or(current > 0);

    // Prefer the persisted blob; fall back to re-reading the filesystem path.
    let mut book = None;
    if let Some((url, token)) = read_creds {
        if let Ok(bytes) = fetch_bytes(&url, &token).await {
            book = epub::parse_epub_bytes(bytes).ok();
        }
    }
    if book.is_none() {
        if let Some(path) = &source_path {
            book = epub::parse_epub_path(path).ok();
        }
    }

    if let Some(book) = book {
        let total = book.chapter_count();
        narrator
            .with_session(session_id, |s| {
                s.book = Some(book);
                s.current = current.min(total.saturating_sub(1));
                // Resumed state resumes narration: if a chapter beyond the first
                // was persisted, the book is considered started.
                s.started = started;
                s.blob_chunk_id = Some(blob_ref.id.clone());
                s.source_path = source_path.clone();
            })
            .await;
        info!("cafe-epub: restored book for session {}", session_id);
    }
}

/// Scan history for a mutation chunk carrying read credentials for `chunk_id`.
fn find_read_credentials(
    history: &[Chunk],
    chunk_id: &str,
) -> Option<(String, String)> {
    for chunk in history {
        if chunk.content_type != ContentType::Null {
            continue;
        }
        let ann = &chunk.annotations;
        if ann.get("cafe.mutates.target_id").and_then(|v| v.as_str()) != Some(chunk_id) {
            continue;
        }
        let url = ann.get("cafe.binary.read_url").and_then(|v| v.as_str())?;
        let token = ann.get("cafe.binary.read_token").and_then(|v| v.as_str())?;
        return Some((url.to_string(), token.to_string()));
    }
    None
}

/// Find the most recent annotation value for `key` in history.
fn latest_annotation<'a>(history: &'a [Chunk], key: &str) -> Option<&'a serde_json::Value> {
    history
        .iter()
        .rev()
        .find_map(|c| c.annotations.get(key))
}

async fn fetch_bytes(url: &str, token: &str) -> anyhow::Result<Vec<u8>> {
    let resp = reqwest::Client::new()
        .get(format!("{}?token={}", url, token))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("binary-store returned {} for {}", resp.status(), url);
    }
    Ok(resp.bytes().await?.to_vec())
}

/// Publish an assistant text chunk to the session (for display in the transcript).
async fn publish_assistant_text(
    sub: &mut SessionSubscription,
    text: &str,
) -> anyhow::Result<()> {
    let chunk = Chunk::new_text(text, PRODUCER).with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);
    sub.publish(chunk).await?;
    Ok(())
}
