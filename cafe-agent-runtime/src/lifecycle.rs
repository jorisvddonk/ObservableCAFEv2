use anyhow::Result;
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, EphemeralConfig, SessionConfig};
use std::collections::HashMap;

/// Create a background session for an agent, optionally sending an initial chunk.
pub async fn create_agent_session(
    socket_path: &str,
    agent_name: &str,
    initial_chunk_content: Option<String>,
    initial_chunk_type: Option<String>,
    initial_chunk_data: Option<Vec<u8>>,
    initial_chunk_mime_type: Option<String>,
    initial_chunk_annotations: HashMap<String, serde_json::Value>,
    ephemeral_keepalive_secs: Option<u64>,
    ephemeral_count_role: Option<String>,
) -> Result<()> {
    let client = BusClient::unix(socket_path);

    let config = if let Some(keepalive) = ephemeral_keepalive_secs {
        SessionConfig {
            ephemeral: Some(EphemeralConfig {
                keepalive_secs: keepalive,
                count_role: ephemeral_count_role,
            }),
            ..Default::default()
        }
    } else {
        SessionConfig::default()
    };
    client.create_session(agent_name, agent_name, config).await?;

    let chunk_type = initial_chunk_type.as_deref().unwrap_or("text");
    let chunk_content = initial_chunk_content.unwrap_or_default();
    let chunk_data = initial_chunk_data;
    let chunk_mime_type = initial_chunk_mime_type;

    match chunk_type {
        "text" => {
            if !chunk_content.trim().is_empty() {
                let mut chunk = Chunk::new_text(chunk_content, agent_name)
                    .with_annotation(keys::CHAT_ROLE, "user");
                for (key, value) in initial_chunk_annotations {
                    chunk = chunk.with_annotation(key, value);
                }
                client.publish(agent_name, chunk).await?;
            }
        }
        "null" => {
            let mut chunk = Chunk::new_null(agent_name);
            for (key, value) in initial_chunk_annotations {
                chunk = chunk.with_annotation(key, value);
            }
            client.publish(agent_name, chunk).await?;
        }
        "binary" => {
            if let Some(data) = chunk_data {
                let mime_type = chunk_mime_type.unwrap_or_else(|| "application/octet-stream".to_string());
                let mut chunk = Chunk::new_binary(data, mime_type, agent_name);
                for (key, value) in initial_chunk_annotations {
                    chunk = chunk.with_annotation(key, value);
                }
                client.publish(agent_name, chunk).await?;
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
    let client = BusClient::unix(socket_path);
    let chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
        .with_annotation(keys::CAFE_FLOW_SIGNAL, "reset")
        .with_annotation(keys::CAFE_FLOW_AGENT_ID, agent_name);
    client.publish(agent_name, chunk).await?;
    Ok(())
}

/// Publish a cron tick to an agent's session.
pub async fn tick_agent(socket_path: &str, agent_name: &str) -> Result<()> {
    let client = BusClient::unix(socket_path);
    let chunk = Chunk::new_null("com.nominal.cafe-agent-runtime")
        .with_annotation(keys::CAFE_FLOW_SIGNAL, "tick");
    client.publish(agent_name, chunk).await?;
    Ok(())
}
