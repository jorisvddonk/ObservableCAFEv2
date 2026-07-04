use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::http::HttpClient;
use cafe_sdk::{keys, Chunk, ContentType, ServerMessage, SubscribeFilter};
use clap::{Parser, Subcommand};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use std::io::Write;

/// Find binary-store read credentials from session history.
fn find_read_creds(history: &[Chunk], chunk_id: &str) -> Result<(String, String)> {
    for c in history.iter().rev() {
        if let Some(target) = c.is_mutation() {
            if target == chunk_id {
                if let Some(ru) = c.annotations.get("binary.read_url").and_then(|v| v.as_str()) {
                    let rt = c.annotations.get("binary.read_token").and_then(|v| v.as_str()).unwrap_or("");
                    return Ok((ru.to_string(), rt.to_string()));
                }
            }
        }
    }
    anyhow::bail!("no read credentials found for chunk {}", chunk_id)
}

fn parse_keyval(s: &str) -> Result<(String, String)> {
    let mut parts = s.splitn(2, '=');
    let key = parts.next().ok_or_else(|| anyhow::anyhow!("missing key"))?.to_string();
    let val = parts.next().unwrap_or("").to_string();
    Ok((key, val))
}

#[derive(Subcommand)]
enum StoreAction {
    /// Upload a file to the binary-store
    Upload {
        path: String,
        #[arg(long)]
        mime: Option<String>,
        /// Binary-store URL (default: http://localhost:4002)
        #[arg(long, default_value = "http://localhost:4002")]
        store_url: String,
        /// Session to use (default: auto-create)
        #[arg(long)]
        session: Option<String>,
    },
    /// Download a file from the binary-store
    Download {
        chunk_id: String,
        #[arg(long)]
        output: Option<String>,
        #[arg(long, default_value = "http://localhost:4002")]
        store_url: String,
        #[arg(long)]
        session: Option<String>,
    },
    /// Stream a file from the binary-store to stdout
    Stream {
        chunk_id: String,
        #[arg(long, default_value = "http://localhost:4002")]
        store_url: String,
        #[arg(long)]
        session: Option<String>,
    },
}

#[derive(Parser)]
#[command(name = "cafe-cli")]
struct Cli {
    #[arg(long, default_value = "/tmp/cafe-bus.sock")]
    bus: String,

    /// Cafe-server HTTP URL (needed for chat command)
    #[arg(long, default_value = "http://localhost:4000")]
    server: String,

    /// Auth token (needed for HTTP commands)
    #[arg(long)]
    token: Option<String>,

    #[arg(short, long, help = "Logging to stderr")]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Publish a chunk to a session
    Publish {
        session_id: String,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        mime: Option<String>,
        #[arg(long)]
        binary_ref: bool,
        /// Publish a null chunk (specify --annotation for config keys)
        #[arg(long)]
        null: bool,
        /// Annotation in key=value format (repeat for multiple)
        #[arg(long = "annotation", value_parser = parse_keyval)]
        annotations: Vec<(String, String)>,
        #[arg(long)]
        transient: bool,
        /// Seconds to wait for mutations on the published chunk (uses long-lived connection)
        #[arg(long)]
        wait: Option<u64>,
    },
    /// Subscribe to a session and print chunks as JSON lines
    Subscribe {
        session_id: String,
        #[arg(long, default_value = "5")]
        timeout_secs: u64,
    },
    /// Subscribe to all sessions (filtered) and print as JSON lines
    SubscribeAll {
        #[arg(long)]
        content_type: Option<String>,
        #[arg(long, default_value = "5")]
        timeout_secs: u64,
    },
    /// List sessions as JSON
    ListSessions,
    /// List available LLM models
    ListModels,
    /// Create a session, prints the session ID
    CreateSession {
        /// Session ID (omit for auto-generated)
        session_id: Option<String>,
        #[arg(long, default_value = "default")]
        agent: String,
    },
    /// Delete a session
    DeleteSession {
        session_id: String,
    },
    /// Print session history as JSON lines
    History {
        session_id: String,
    },
    /// Binary-store operations (upload/download/stream)
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },
    /// Send a chat message and print SSE response chunks as JSON lines
    Chat {
        session_id: String,
        message: String,
        /// Wait up to this many seconds for a response (default 30)
        #[arg(long, default_value = "30")]
        timeout_secs: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt::init();
    }

    let client = BusClient::new(&cli.bus);

    match cli.command {
        Command::Publish {
            session_id,
            text,
            file,
            mime,
            binary_ref,
            null,
            annotations,
            transient,
            wait,
        } => {
            let chunk_id = uuid::Uuid::new_v4().to_string();
            let mut chunk = if null {
                let mut c = Chunk::new_null("cafe-cli");
                for (k, v) in &annotations {
                    c = c.with_annotation(k.as_str(), v.as_str());
                }
                c
            } else if binary_ref {
                let mime = mime.unwrap_or_else(|| "application/octet-stream".into());
                Chunk::new_binary_ref(mime, "cafe-cli")
            } else if let Some(content) = text {
                Chunk::new_text(content, "cafe-cli")
                    .with_annotation(cafe_sdk::keys::CHAT_ROLE, "user")
            } else if let Some(path) = file {
                let data = tokio::fs::read(&path).await?;
                let mime = mime.unwrap_or_else(|| "application/octet-stream".into());
                Chunk::new_binary(data, mime, "cafe-cli")
            } else {
                anyhow::bail!("specify --text, --file, --binary-ref, or --null");
            };
            chunk.id = chunk_id.clone();

            if transient {
                chunk = chunk.as_transient();
            }

            if let Some(wait_secs) = wait {
                // Use a raw socket so we share the same connection — needed to receive
                // direct_to mutations (binary-store sends write credentials back).
                let stream = tokio::net::UnixStream::connect(&cli.bus).await?;
                let (reader, mut writer) = tokio::io::split(stream);
                let mut lines = tokio::io::BufReader::new(reader).lines();

                // Read Connected
                if let Some(line) = lines.next_line().await? {
                    if let Ok(ServerMessage::Connected { .. }) = serde_json::from_str(&line) {
                        // consumed
                    }
                }

                // SubscribeAll so we receive direct_to mutations
                writer
                    .write_all(b"{\"op\":\"subscribe_all\"}\n")
                    .await?;

                // Create session if needed (fire-and-forget is fine — SESSION_EXISTS ignored)
                let create = serde_json::json!({
                    "op": "create_session",
                    "session_id": session_id,
                    "agent_id": "default",
                    "config": {}
                });
                writer.write_all(create.to_string().as_bytes()).await?;
                writer.write_all(b"\n").await?;

                // Publish the chunk
                let publish = serde_json::json!({
                    "op": "publish",
                    "session_id": session_id,
                    "chunk": &chunk,
                });
                writer.write_all(publish.to_string().as_bytes()).await?;
                writer.write_all(b"\n").await?;

                // Wait for mutations targeting our chunk_id
                let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);
                while tokio::time::Instant::now() < deadline {
                    tokio::select! {
                        line = lines.next_line() => {
                            match line {
                                Ok(Some(line)) => {
                                    if let Ok(msg) = serde_json::from_str::<ServerMessage>(&line) {
                                        if let ServerMessage::Chunk { chunk, .. } = msg {
                                            if let Some(target) = chunk.is_mutation() {
                                                if target == chunk_id {
                                                    // Found a mutation targeting our chunk — print it
                                                    let json = serde_json::to_string(&chunk)?;
                                                    println!("{}", json);
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => break,
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                }
            } else {
                client.publish(&session_id, chunk).await?;
            }
        }

        Command::Subscribe {
            session_id,
            timeout_secs,
        } => {
            let mut rx = client.subscribe(&session_id).await?;
            eprintln!("subscribed session={} timeout={}s", session_id, timeout_secs);

            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Some(ServerMessage::Chunk { chunk, .. }) => {
                                let json = serde_json::to_string(&chunk)?;
                                println!("{}", json);
                            }
                            Some(ServerMessage::HistoryComplete { count, .. }) => {
                                eprintln!("history_complete count={}", count);
                            }
                            Some(ServerMessage::SessionDeleted { .. }) => break,
                            Some(_) => {}
                            None => break,
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                        break;
                    }
                }
            }
        }

        Command::SubscribeAll {
            content_type,
            timeout_secs,
        } => {
            let mut rx = if let Some(ct_str) = content_type {
                let ct = match ct_str.as_str() {
                    "text" => ContentType::Text,
                    "binary" => ContentType::Binary,
                    "binary-ref" | "binary_ref" => ContentType::BinaryRef,
                    "null" => ContentType::Null,
                    _ => anyhow::bail!("unknown content type: {}", ct_str),
                };
                let filter = SubscribeFilter {
                    content_types: Some(vec![ct]),
                    ..Default::default()
                };
                client.subscribe_filtered(filter).await?
            } else {
                client.subscribe_all().await?
            };

            eprintln!("subscribed_all timeout={}s", timeout_secs);

            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Some(ServerMessage::Chunk { session_id, chunk }) => {
                                let mut map = serde_json::Map::new();
                                map.insert("event".into(), "chunk".into());
                                map.insert("session_id".into(), session_id.into());
                                if let Ok(v) = serde_json::to_value(&chunk) {
                                    map.insert("chunk".into(), v);
                                }
                                println!("{}", serde_json::Value::Object(map));
                            }
                            Some(ServerMessage::SessionCreated { session_id, agent_id }) => {
                                eprintln!("session_created id={} agent={}", session_id, agent_id);
                            }
                            Some(ServerMessage::SessionDeleted { session_id }) => {
                                eprintln!("session_deleted id={}", session_id);
                            }
                            Some(_) => {}
                            None => break,
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                        break;
                    }
                }
            }
        }

        Command::ListSessions => {
            let sessions = client.list_sessions().await?;
            let json = serde_json::to_string(&sessions)?;
            println!("{}", json);
        }

        Command::ListModels => {
            let chunks = client.get_history("_cafe_llm_registry").await?;
            let models: Vec<String> = chunks
                .iter()
                .filter_map(|c| {
                    if c.content_type == ContentType::Null {
                        c.get_annotation::<String>("config.available_models")
                    } else {
                        None
                    }
                })
                .last()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let json = serde_json::to_string(&models)?;
            println!("{}", json);
        }

        Command::CreateSession { session_id, agent } => {
            let id = match session_id {
                Some(sid) => {
                    client.create_session(&sid, &agent, Default::default()).await?;
                    sid
                }
                None => {
                    let sid = uuid::Uuid::new_v4().to_string();
                    client.create_session(&sid, &agent, Default::default()).await?;
                    sid
                }
            };
            println!("{}", id);
        }

        Command::DeleteSession { session_id } => {
            client.delete_session(&session_id).await?;
        }

        Command::History { session_id } => {
            let chunks = client.get_history(&session_id).await?;
            for chunk in &chunks {
                let json = serde_json::to_string(chunk)?;
                println!("{}", json);
            }
        }

        Command::Store { action } => {
            match action {
                StoreAction::Upload { path, mime, store_url, session } => {
                    let session_id = session.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    let chunk_id = uuid::Uuid::new_v4().to_string();
                    let store_url = store_url.trim_end_matches('/').to_string();

                    // Open raw socket for credential exchange
                    let stream = tokio::net::UnixStream::connect(&cli.bus).await?;
                    let (reader, mut writer) = tokio::io::split(stream);
                    let mut lines = BufReader::new(reader).lines();

                    // Read Connected
                    if let Some(line) = lines.next_line().await? {
                        let _ = serde_json::from_str::<ServerMessage>(&line);
                    }

                    // SubscribeAll, then wait briefly for snapshot to settle
                    writer.write_all(b"{\"op\":\"subscribe_all\"}\n").await?;
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    // Create session (auto-generated UUID, always succeeds)
                    let create = serde_json::json!({"op":"create_session","session_id":session_id,"agent_id":"default","config":{}});
                    writer.write_all(create.to_string().as_bytes()).await?;
                    writer.write_all(b"\n").await?;

                    // Drain until we see the SessionCreated for our session
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                    while tokio::time::Instant::now() < deadline {
                        if let Ok(Some(line)) = lines.next_line().await {
                            if line.contains(&format!(r#""session_id":"{}""#, session_id))
                                && line.contains(r#""event":"session_created""#)
                            {
                                break;
                            }
                        } else {
                            break;
                        }
                    }

                    // Publish BinaryRef
                    let mut binref = Chunk::new_binary_ref(mime.as_deref().unwrap_or("application/octet-stream"), "cafe-cli");
                    binref.id = chunk_id.clone();
                    let publish = serde_json::json!({"op":"publish","session_id":session_id,"chunk":&binref});
                    writer.write_all(publish.to_string().as_bytes()).await?;
                    writer.write_all(b"\n").await?;

                    // Wait for write credentials mutation
                    let mut write_url = None;
                    let mut write_token = None;
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                    while tokio::time::Instant::now() < deadline {
                        tokio::select! {
                            line = lines.next_line() => {
                                match line {
                                    Ok(Some(line)) => {
                                        if let Ok(msg) = serde_json::from_str::<ServerMessage>(&line) {
                                            if let ServerMessage::Chunk { chunk, .. } = msg {
                                                let ann = &chunk.annotations;
                                                if ann.contains_key("cafe.binary.write_url") {
                                                    write_url = ann.get("cafe.binary.write_url").and_then(|v| v.as_str().map(String::from));
                                                    write_token = ann.get("cafe.binary.write_token").and_then(|v| v.as_str().map(String::from));
                                                    if write_url.is_some() && write_token.is_some() {
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    _ => break,
                                }
                            }
                            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                        }
                    }

                    let write_url = write_url.ok_or_else(|| anyhow::anyhow!("no write credentials received"))?;
                    let write_token = write_token.unwrap_or_default();

                    // Read file and upload
                    let data = tokio::fs::read(&path).await?;
                    let mime_type = mime.unwrap_or_else(|| {
                        path.rsplit('.').next().map(|ext| {
                            match ext {
                                "wav" => "audio/wav",
                                "mp3" => "audio/mpeg",
                                "png" => "image/png",
                                "jpg" | "jpeg" => "image/jpeg",
                                "gif" => "image/gif",
                                "txt" => "text/plain",
                                "json" => "application/json",
                                _ => "application/octet-stream",
                            }
                        }).unwrap_or("application/octet-stream").to_string()
                    });

                    let upload_url = format!("{}?token={}&session_id={}", write_url, write_token, session_id);
                    let client = reqwest::Client::new();
                    client.post(&upload_url)
                        .header("Content-Type", &mime_type)
                        .body(data)
                        .send()
                        .await?;

                    // Wait for read credentials mutation
                    let mut read_url = None;
                    let mut read_token = None;
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                    while tokio::time::Instant::now() < deadline {
                        tokio::select! {
                            line = lines.next_line() => {
                                match line {
                                    Ok(Some(line)) => {
                                        if let Ok(msg) = serde_json::from_str::<ServerMessage>(&line) {
                                            if let ServerMessage::Chunk { chunk, .. } = msg {
                                                let ann = &chunk.annotations;
                                                if ann.contains_key("binary.read_url") {
                                                    read_url = ann.get("binary.read_url").and_then(|v| v.as_str().map(String::from));
                                                    read_token = ann.get("binary.read_token").and_then(|v| v.as_str().map(String::from));
                                                    if read_url.is_some() {
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    _ => break,
                                }
                            }
                            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                        }
                    }

                    // Fallback: read from history
                    if read_url.is_none() {
                        let bus = BusClient::new(&cli.bus);
                        if let Ok(history) = bus.get_history(&session_id).await {
                            for c in &history {
                                if let Some(target) = c.is_mutation() {
                                    if target == chunk_id {
                                        if let Some(ru) = c.annotations.get("binary.read_url").and_then(|v| v.as_str()) {
                                            read_url = Some(ru.to_string());
                                            read_token = c.annotations.get("binary.read_token").and_then(|v| v.as_str().map(String::from));
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Print credentials for scripting
                    let result = serde_json::json!({
                        "chunk_id": chunk_id,
                        "session_id": session_id,
                        "write_url": write_url,
                        "write_token": write_token,
                        "read_url": read_url,
                        "read_token": read_token,
                        "store_url": store_url,
                    });
                    println!("{}", serde_json::to_string(&result)?);
                }

                StoreAction::Download { chunk_id, output, store_url, session } => {
                    let session_id = session.as_deref().unwrap_or("_store_ops");
                    let bus = BusClient::new(&cli.bus);

                    // Find read credentials from history
                    let history = bus.get_history(session_id).await?;
                    let (read_url, read_token) = find_read_creds(&history, &chunk_id)?;

                    let client = reqwest::Client::new();
                    let resp = client.get(format!("{}?token={}", read_url, read_token))
                        .send().await?;
                    let bytes = resp.bytes().await?;

                    if let Some(out) = output {
                        tokio::fs::write(&out, &bytes).await?;
                        eprintln!("downloaded {} bytes to {}", bytes.len(), out);
                    } else {
                        std::io::stdout().lock().write_all(&bytes)?;
                    }
                }

                StoreAction::Stream { chunk_id, store_url, session } => {
                    let session_id = session.as_deref().unwrap_or("_store_ops");
                    let bus = BusClient::new(&cli.bus);

                    let history = bus.get_history(session_id).await?;
                    let (read_url, read_token) = find_read_creds(&history, &chunk_id)?;

                    let client = reqwest::Client::new();
                    let mut resp = client.get(format!("{}?token={}", read_url, read_token))
                        .send().await?;

                    let mut stdout = std::io::stdout().lock();
                    while let Some(chunk) = resp.chunk().await? {
                        stdout.write_all(&chunk)?;
                    }
                }
            }
        }

        Command::Chat {
            session_id,
            message,
            timeout_secs,
        } => {
            let token = cli.token.as_deref().unwrap_or("");
            let http = HttpClient::new(&cli.server, token);
            let (tx, mut rx) = mpsc::channel::<Chunk>(256);

            tokio::select! {
                result = http.stream_chat(&session_id, &message, tx) => {
                    if let Err(e) = result {
                        eprintln!("chat error: {}", e);
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                    eprintln!("chat timeout after {}s", timeout_secs);
                }
            }

            // Drain all chunks (stream_chat closes tx when done)
            while let Some(chunk) = rx.recv().await {
                let json = serde_json::to_string(&chunk)?;
                println!("{}", json);
            }
        }
    }

    Ok(())
}
