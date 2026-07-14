use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::bus::IrohConfig;
use cafe_sdk::http::HttpClient;
use cafe_sdk::{keys, Chunk, ContentType, ServerMessage, SubscribeFilter};
use clap::{Parser, Subcommand};
use std::time::Duration;
use tokio::sync::mpsc;
use std::io::Write;

/// Find binary-store read credentials from session history.
fn find_read_creds(history: &[Chunk], chunk_id: &str) -> Result<(String, String)> {
    for c in history.iter().rev() {
        if let Some(target) = c.is_mutation() {
            if target == chunk_id {
                if let Some(ru) = c.annotations.get(keys::CAFE_BINARY_READ_URL).and_then(|v| v.as_str()) {
                    let rt = c.annotations.get(keys::CAFE_BINARY_READ_TOKEN).and_then(|v| v.as_str()).unwrap_or("");
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

/// Manage the bus iroh peer-ID allowlist database.
async fn run_allowlist(db_arg: &str, action: &AllowlistAction) -> Result<()> {
    use std::str::FromStr;

    if let AllowlistAction::MyId = action {
        let secret = std::env::var("CAFE_BUS_IROH_CLIENT_SECRET_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .and_then(|s| iroh::SecretKey::from_str(&s).ok());
        match secret {
            Some(key) => {
                // Stable key is configured; report its public peer id.
                println!("{}", key.public());
            }
            None => {
                // Generate a fresh key and print both the secret (to deploy as
                // the env var) and its public peer id (to register in the allowlist).
                let key = iroh::SecretKey::generate();
                let hex: String = key
                    .to_bytes()
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect();
                let pk = key.public();
                println!("secret (set CAFE_BUS_IROH_CLIENT_SECRET_KEY to this): {}", hex);
                println!("peer id (register with `iroh-allowlist add`):      {}", pk);
            }
        }
        return Ok(());
    }

    let path = if db_arg.is_empty() {
        std::env::var("CAFE_BUS_IROH_ALLOWLIST_DB").map_err(|_| {
            anyhow::anyhow!("specify --db <path> or set CAFE_BUS_IROH_ALLOWLIST_DB")
        })?
    } else {
        db_arg.to_string()
    };

    let allow = cafe_bus::allowlist::Allowlist::connect(&path).await?;
    match action {
        AllowlistAction::Add { peer_id, label } => {
            allow.add(peer_id, label.as_deref()).await?;
            println!("added {}", peer_id);
        }
        AllowlistAction::Remove { peer_id } => {
            if allow.remove(peer_id).await? {
                println!("removed {}", peer_id);
            } else {
                println!("{} not found", peer_id);
            }
        }
        AllowlistAction::List => {
            for (peer, label) in allow.list().await? {
                match label {
                    Some(l) => println!("{}\t{}", peer, l),
                    None => println!("{}", peer),
                }
            }
        }
        AllowlistAction::MyId => unreachable!(),
    }
    Ok(())
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

    /// iroh: bus public key (EndpointId)
    #[arg(long)]
    bus_iroh_key: Option<String>,

    /// iroh: relay URL
    #[arg(long)]
    bus_iroh_relay: Option<String>,

    /// iroh: ALPN (default: cafe-bus/0)
    #[arg(long)]
    bus_iroh_alpn: Option<String>,

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
    /// List available agents
    ListAgents,
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
    /// Manage the bus iroh peer-ID allowlist database
    IrohAllowlist {
        /// Path to the allowlist SQLite DB (default: $CAFE_BUS_IROH_ALLOWLIST_DB)
        #[arg(long, default_value = "")]
        db: String,
        #[command(subcommand)]
        action: AllowlistAction,
    },
}

#[derive(Subcommand)]
enum AllowlistAction {
    /// Add a peer ID to the allowlist
    Add {
        peer_id: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// Remove a peer ID from the allowlist
    Remove {
        peer_id: String,
    },
    /// List allowed peer IDs
    List,
    /// Print this client's stable peer ID (from CAFE_BUS_IROH_CLIENT_SECRET_KEY, or generate one)
    MyId,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt::init();
    }

    // Allowlist admin operates on the DB directly and doesn't need a bus connection.
    if let Command::IrohAllowlist { db, action } = &cli.command {
        return run_allowlist(db, action).await;
    }

    let iroh_requested = cli.bus_iroh_key.is_some()
        || cli.bus_iroh_relay.is_some()
        || cli.bus_iroh_alpn.is_some();

    let client = if let Some(cfg) = IrohConfig::from_cli(
        cli.bus_iroh_key.as_deref(),
        cli.bus_iroh_relay.as_deref(),
        cli.bus_iroh_alpn.as_deref(),
    ) {
        BusClient::from_iroh_config(cfg).await?
    } else if iroh_requested {
        anyhow::bail!("--bus-iroh-key is required and must be a valid EndpointId");
    } else {
        // Try local addr file from the bus
        let addr_file = format!("{}.iroh-addr", cli.bus);
        if let Ok(json) = std::fs::read_to_string(&addr_file) {
            if let Some(cfg) = IrohConfig::from_bus_addr_json(&json) {
                eprintln!("Using iroh addr from {}", addr_file);
                BusClient::from_iroh_config(cfg).await?
            } else {
                BusClient::unix(&cli.bus)
            }
        } else {
            BusClient::unix(&cli.bus)
        }
    };

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
                // Use SessionSubscription for shared-connection direct_to replies
                let _ = client.create_session(&session_id, "default", Default::default()).await;
                let mut sub = client.subscribe_session(&session_id).await?;

                // Publish the chunk through the shared connection
                sub.publish(chunk).await?;

                // Wait for mutations targeting our chunk_id
                let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_secs);
                while tokio::time::Instant::now() < deadline {
                    tokio::select! {
                        msg = sub.rx.recv() => {
                            match msg {
                                Some(ServerMessage::Chunk { chunk, .. }) => {
                                    if let Some(target) = chunk.is_mutation() {
                                        if target == chunk_id {
                                            let json = serde_json::to_string(&chunk)?;
                                            println!("{}", json);
                                        }
                                    }
                                }
                                Some(ServerMessage::SessionDeleted { .. }) => break,
                                None => break,
                                _ => {}
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

        Command::ListAgents => {
            let token = cli.token.as_deref().unwrap_or("");
            let http = HttpClient::new(&cli.server, token);
            let agents = http.list_agents().await?;
            let json = serde_json::to_string(&agents)?;
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

                    // Create session, then subscribe with a shared connection for credential exchange
                    client.create_session(&session_id, "default", Default::default()).await?;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let mut sub = client.subscribe_session(&session_id).await?;

                    // Publish BinaryRef
                    let mut binref = Chunk::new_binary_ref(mime.as_deref().unwrap_or("application/octet-stream"), "cafe-cli");
                    binref.id = chunk_id.clone();
                    sub.publish(binref).await?;

                    // Wait for write credentials mutation
                    let mut write_url = None;
                    let mut write_token = None;
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                    while tokio::time::Instant::now() < deadline {
                        tokio::select! {
                            msg = sub.rx.recv() => {
                                match msg {
                                    Some(ServerMessage::Chunk { chunk, .. }) => {
                                        let ann = &chunk.annotations;
                                        if ann.contains_key("cafe.binary.write_url") {
                                            write_url = ann.get("cafe.binary.write_url").and_then(|v| v.as_str().map(String::from));
                                            write_token = ann.get("cafe.binary.write_token").and_then(|v| v.as_str().map(String::from));
                                            if write_url.is_some() && write_token.is_some() {
                                                break;
                                            }
                                        }
                                    }
                                    Some(ServerMessage::SessionDeleted { .. }) => break,
                                    None => break,
                                    _ => {}
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
                    let http_client = reqwest::Client::new();
                    http_client.post(&upload_url)
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
                            msg = sub.rx.recv() => {
                                match msg {
                                    Some(ServerMessage::Chunk { chunk, .. }) => {
                                        let ann = &chunk.annotations;
                                        if ann.contains_key(keys::CAFE_BINARY_READ_URL) {
                                            read_url = ann.get(keys::CAFE_BINARY_READ_URL).and_then(|v| v.as_str().map(String::from));
                                            read_token = ann.get(keys::CAFE_BINARY_READ_TOKEN).and_then(|v| v.as_str().map(String::from));
                                            if read_url.is_some() {
                                                break;
                                            }
                                        }
                                    }
                                    Some(ServerMessage::SessionDeleted { .. }) => break,
                                    None => break,
                                    _ => {}
                                }
                            }
                            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                        }
                    }

                    // Fallback: read from history
                    if read_url.is_none() {
                        if let Ok(history) = client.get_history(&session_id).await {
                            for c in &history {
                                if let Some(target) = c.is_mutation() {
                                    if target == chunk_id {
                                        if let Some(ru) = c.annotations.get(keys::CAFE_BINARY_READ_URL).and_then(|v| v.as_str()) {
                                            read_url = Some(ru.to_string());
                                            read_token = c.annotations.get(keys::CAFE_BINARY_READ_TOKEN).and_then(|v| v.as_str().map(String::from));
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
                    let bus = BusClient::unix(&cli.bus);

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
                    let bus = BusClient::unix(&cli.bus);

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
        Command::IrohAllowlist { .. } => {
            // Handled earlier (before connecting to the bus); this arm is unreachable.
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_read_creds_uses_cafe_prefixed_keys() {
        let chunk_id = "abc";
        // The binary-store emits cafe.binary.read_url / read_token, NOT the
        // unprefixed binary.read_url the old code looked up.
        let creds = Chunk::mutation(chunk_id, "cafe-cli")
            .with_annotation(keys::CAFE_BINARY_READ_URL, &"https://read/abc".to_string())
            .with_annotation(keys::CAFE_BINARY_READ_TOKEN, &"tok".to_string());
        let history = vec![creds];
        let (url, token) = find_read_creds(&history, chunk_id).expect("creds found");
        assert_eq!(url, "https://read/abc");
        assert_eq!(token, "tok");
    }
}
