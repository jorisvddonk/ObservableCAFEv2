use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, JsonRpcResponse, ServerMessage};
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let socket_path = std::env::var("CAFE_BUS_SOCKET")
        .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());

    cafe_sdk::bus::run_with_reconnect("cafe-web-fetch", move || {
        let sp = socket_path.clone();
        async move { subscribe_all(&sp).await }
    })
    .await;

    Ok(())
}

async fn subscribe_all(socket_path: &str) -> anyhow::Result<()> {
    info!("cafe-web-fetch: starting on {}", socket_path);
    let client = BusClient::new(socket_path);
    let mut rx = client.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let c = client.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, c).await {
                    warn!("cafe-web-fetch: session error: {}", e);
                }
            });
        }
    }
    Ok(())
}

async fn run_session(session_id: String, client: BusClient) -> anyhow::Result<()> {
    let mut rx = client.subscribe(&session_id).await?;

    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if request.method != "web-fetch.invoke" { continue; }

        let call_id = request.id.clone();
        let text = request.params["text"].as_str().unwrap_or("");

        info!("cafe-web-fetch: handling web-fetch.invoke call_id={}", call_id);

        let response = match parse_and_fetch(text, &client, &session_id).await {
            Ok(chunk_id) => {
                info!("cafe-web-fetch: fetched, chunk_id={}", chunk_id);
                JsonRpcResponse::ok(&call_id, serde_json::json!({"chunk_id": chunk_id}))
            }
            Err(e) => {
                warn!("cafe-web-fetch: fetch error: {}", e);
                JsonRpcResponse::err(&call_id, -1, &e.to_string())
            }
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-web-fetch")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = client.publish(&session_id, resp_chunk).await;
    }

    Ok(())
}

/// Parse "!fetch <url>", fetch and publish, return the chunk ID.
async fn parse_and_fetch(text: &str, client: &BusClient, session_id: &str) -> anyhow::Result<String> {
    let text = text.trim();
    let url = text
        .strip_prefix("!fetch ")
        .or_else(|| text.strip_prefix("!f "))
        .ok_or_else(|| anyhow::anyhow!("not a fetch command"))?
        .trim();

    let response = reqwest::get(url).await?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_string();
    let body = response.text().await?;
    let stripped = strip_html(&body);

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let chunk = Chunk::new_text(stripped, "com.nominal.cafe-web-fetch")
        .with_annotation(keys::WEB_SOURCE_URL, &url)
        .with_annotation(keys::WEB_CONTENT_TYPE, &content_type)
        .with_annotation(keys::WEB_FETCH_TIME, now_ms)
        .with_annotation(
            keys::SECURITY_TRUST_LEVEL,
            serde_json::json!({ "trusted": false, "source": "web" }),
        );

    let chunk_id = chunk.id.clone();
    client.publish(session_id, chunk).await?;
    Ok(chunk_id)
}

fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_basic() {
        assert_eq!(strip_html("<p>hello</p>"), "hello");
    }

    #[test]
    fn strip_html_nested() {
        assert_eq!(strip_html("<div><p>hi</p></div>"), "hi");
    }

    #[test]
    fn strip_html_no_tags() {
        assert_eq!(strip_html("hello world"), "hello world");
    }

    #[test]
    fn strip_html_empty() {
        assert_eq!(strip_html(""), "");
    }
}
