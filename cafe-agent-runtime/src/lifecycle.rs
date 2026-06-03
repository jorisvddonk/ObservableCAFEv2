use anyhow::Result;
use cafe_types::{keys, Chunk, ClientMessage, SessionConfig};
use std::collections::HashMap;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::UnixStream;

/// Send a single message to the bus and close the connection.
async fn send_once(socket_path: &str, msg: &ClientMessage) -> Result<()> {
    let stream = UnixStream::connect(socket_path).await?;
    let (_, writer) = stream.into_split();
    let mut bw = BufWriter::new(writer);
    let mut json = serde_json::to_string(msg)?;
    json.push('\n');
    bw.write_all(json.as_bytes()).await?;
    bw.flush().await?;
    Ok(())
}

/// Create a background session for an agent, optionally sending an initial chunk.
pub async fn create_agent_session(
    socket_path: &str,
    agent_name: &str,
    initial_chunk_content: Option<String>,
    initial_chunk_type: Option<String>,
    initial_chunk_data: Option<Vec<u8>>,
    initial_chunk_mime_type: Option<String>,
    initial_chunk_annotations: HashMap<String, serde_json::Value>,
) -> Result<()> {
    let msg = ClientMessage::CreateSession {
        session_id: agent_name.to_string(),
        agent_id: agent_name.to_string(),
        config: SessionConfig::default(),
    };
    send_once(socket_path, &msg).await?;

    // Determine chunk type and content, with defaults
    let chunk_type = initial_chunk_type.as_deref().unwrap_or("text");
    let chunk_content = initial_chunk_content.unwrap_or_default();
    let chunk_data = initial_chunk_data;
    let chunk_mime_type = initial_chunk_mime_type;

    // Create and send initial chunk if specified
    match chunk_type {
        "text" => {
            if !chunk_content.trim().is_empty() {
                let mut chunk = Chunk::new_text(chunk_content, agent_name)
                    .with_annotation(keys::CHAT_ROLE, "user");
                
                // Add any additional annotations from config
                for (key, value) in initial_chunk_annotations {
                    chunk = chunk.with_annotation(key, value);
                }
                
                let publish_msg = ClientMessage::Publish {
                    session_id: agent_name.to_string(),
                    chunk,
                };
                send_once(socket_path, &publish_msg).await?;
            }
        }
        "null" => {
            // Create and send a null chunk
            let mut chunk = Chunk::new_null(agent_name);
            
            // Add any additional annotations from config
            for (key, value) in initial_chunk_annotations {
                chunk = chunk.with_annotation(key, value);
            }
            
            let publish_msg = ClientMessage::Publish {
                session_id: agent_name.to_string(),
                chunk,
            };
            send_once(socket_path, &publish_msg).await?;
        }
        "binary" => {
            if let Some(data) = chunk_data {
                let mime_type = chunk_mime_type.unwrap_or_else(|| "application/octet-stream".to_string());
                let mut chunk = Chunk::new_binary(data, mime_type, agent_name);
                
                // Add any additional annotations from config
                for (key, value) in initial_chunk_annotations {
                    chunk = chunk.with_annotation(key, value);
                }
                
                let publish_msg = ClientMessage::Publish {
                    session_id: agent_name.to_string(),
                    chunk,
                };
                send_once(socket_path, &publish_msg).await?;
            }
        }
        _ => {
            tracing::warn!("Unknown initial chunk type: {}", chunk_type);
        }
    }
    
    Ok(())
}

/// Send a flow.signal: reset to an agent's session.
#[allow(dead_code)]
pub async fn reset_agent_session(socket_path: &str, agent_name: &str) -> Result<()> {
    let chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
        .with_annotation(keys::FLOW_SIGNAL, "reset")
        .with_annotation(keys::FLOW_AGENT_ID, agent_name);
    let msg = ClientMessage::Publish {
        session_id: agent_name.to_string(),
        chunk,
    };
    send_once(socket_path, &msg).await
}

/// Publish a cron tick to an agent's session.
pub async fn tick_agent(socket_path: &str, agent_name: &str) -> Result<()> {
    let chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
        .with_annotation(keys::FLOW_SIGNAL, "tick");
    let msg = ClientMessage::Publish {
        session_id: agent_name.to_string(),
        chunk,
    };
    send_once(socket_path, &msg).await
}
