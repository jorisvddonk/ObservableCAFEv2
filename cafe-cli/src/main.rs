use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, ContentType, ServerMessage, SubscribeFilter};
use clap::{Parser, Subcommand};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "cafe-cli")]
struct Cli {
    #[arg(long, default_value = "/tmp/cafe-bus.sock")]
    bus: String,

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
        #[arg(long)]
        transient: bool,
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
            transient,
        } => {
            let mut chunk = if binary_ref {
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
                anyhow::bail!("specify --text, --file, or --binary-ref");
            };

            if transient {
                chunk = chunk.as_transient();
            }

            client.publish(&session_id, chunk).await?;
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
    }

    Ok(())
}
