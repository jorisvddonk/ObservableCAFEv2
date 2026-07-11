use anyhow::Result;
use cafe_sdk::{keys, roles, Chunk, JsonRpcResponse, ServerMessage};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let socket_path = std::env::var("CAFE_BUS_SOCKET")
        .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());

    cafe_sdk::bus::run_with_reconnect("cafe-rot13", move || {
        let sp = socket_path.clone();
        async move { subscribe_all(&sp).await }
    })
    .await;

    Ok(())
}

async fn subscribe_all(socket_path: &str) -> Result<()> {
    let client = cafe_sdk::bus::BusClient::unix(socket_path);
    let mut rx = client.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let c = client.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, c).await {
                    warn!("cafe-rot13: session error: {}", e);
                }
            });
        }
    }
    Ok(())
}

async fn run_session(
    session_id: String,
    client: cafe_sdk::bus::BusClient,
) -> Result<()> {
    let mut rx = client.subscribe(&session_id).await?;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if request.method != "rot13.invoke" {
            continue;
        }
        let call_id = request.id.clone();

        info!("cafe-rot13: handling rot13.invoke call_id={}", call_id);

        let text = request.params["text"]
            .as_str()
            .unwrap_or("");
        let rot13d = rot13(text);

        let response = JsonRpcResponse::ok(
            &call_id,
            serde_json::json!({ "text": rot13d }),
        );

        let resp_chunk = Chunk::new_null("com.nominal.cafe-rot13")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = client.publish(&session_id, resp_chunk).await;

        // Also publish as assistant text chunk for visibility
        let text_chunk = Chunk::new_text(&rot13d, "com.nominal.cafe-rot13")
            .with_annotation(keys::CHAT_ROLE, roles::ASSISTANT);
        let _ = client.publish(&session_id, text_chunk).await;
    }

    Ok(())
}

fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => char::from_u32((c as u32 - b'a' as u32 + 13) % 26 + b'a' as u32).unwrap(),
            'A'..='Z' => char::from_u32((c as u32 - b'A' as u32 + 13) % 26 + b'A' as u32).unwrap(),
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rot13_roundtrip() {
        let original = "Hello, World!";
        let encoded = rot13(original);
        assert_eq!(encoded, "Uryyb, Jbeyq!");
        assert_eq!(rot13(&encoded), original);
    }

    #[test]
    fn rot13_empty() {
        assert_eq!(rot13(""), "");
    }

    #[test]
    fn rot13_non_alpha() {
        assert_eq!(rot13("123!@#"), "123!@#");
    }
}
