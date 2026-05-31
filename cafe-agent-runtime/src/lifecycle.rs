use anyhow::Result;
use cafe_types::{keys, Chunk, ClientMessage, SessionConfig};
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

/// Create a background session for an agent.
pub async fn create_agent_session(
    socket_path: &str,
    agent_name: &str,
) -> Result<()> {
    let msg = ClientMessage::CreateSession {
        session_id: agent_name.to_string(),
        agent_id: agent_name.to_string(),
        config: SessionConfig::default(),
    };
    send_once(socket_path, &msg).await
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
