use cafe_http_proxy_sdk::{self as proxy_sdk, ProxyRequest, ProxyResponse};
use cafe_sdk::{keys, Chunk, ServerMessage};
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let socket_path = std::env::var("CAFE_BUS_SOCKET")
        .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());

    cafe_sdk::bus::run_with_reconnect("cafe-web-fetch", move || {
        let sp = socket_path.clone();
        async move { run(&sp).await }
    })
    .await;

    Ok(())
}

async fn run(socket_path: &str) -> anyhow::Result<()> {
    info!("cafe-web-fetch: starting on {}", socket_path);
    let client = cafe_sdk::bus::BusClient::unix(socket_path);

    // Subscribe to the proxy session
    let mut rx = client.subscribe(proxy_sdk::PROXY_SESSION).await?;

    // Register our route
    let reg = proxy_sdk::RouteRegistration {
        pattern: "/api/ext/sessions/:id/fetch".into(),
        methods: vec!["POST".into(), "GET".into()],
    };
    proxy_sdk::publish_registration(&client, &reg).await?;
    info!("cafe-web-fetch: registered route {}", reg.pattern);

    // Spawn heartbeat re-registration every 30s
    let hb_client = client.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
            if let Err(e) = proxy_sdk::publish_registration(&hb_client, &reg).await {
                warn!("cafe-web-fetch: heartbeat registration failed: {}", e);
            }
        }
    });

    // Handle incoming messages
    while let Some(msg) = rx.recv().await {
        let chunk = match msg {
            ServerMessage::Chunk { chunk, .. } => chunk,
            _ => continue,
        };

        // Only process RPC requests for our method
        let rpc_req = match chunk.as_rpc_request() {
            Some(r) if r.method == proxy_sdk::HTTP_REQUEST_HANDLE => r,
            _ => continue,
        };
        let call_id = rpc_req.id.clone();

        let Some(req) = proxy_sdk::parse_request(&chunk) else {
            continue;
        };

        // Fetch the session ID from the path
        let session_id = extract_session_id(&req.path);

        let result = handle_fetch(&req, &client, session_id.as_deref()).await;

        let response = match result {
            Ok(chunk_id) => ProxyResponse {
                status: 200,
                headers: [("content-type".into(), "application/json".into())]
                    .into_iter()
                    .collect(),
                body: proxy_sdk::encode_body(
                    serde_json::json!({ "chunk_id": chunk_id }).to_string().as_bytes(),
                ),
            },
            Err(e) => ProxyResponse {
                status: 502,
                headers: [("content-type".into(), "application/json".into())]
                    .into_iter()
                    .collect(),
                body: proxy_sdk::encode_body(
                    serde_json::json!({ "error": e.to_string() }).to_string().as_bytes(),
                ),
            },
        };

        // Publish response via direct_to (the chunk has source.connection from the publisher)
        let conn_id = chunk
            .annotations
            .get(keys::CAFE_SOURCE_CONNECTION)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !conn_id.is_empty() {
            if let Err(e) = proxy_sdk::publish_response(
                &client,
                conn_id,
                &call_id,
                &response,
            )
            .await
            {
                warn!("cafe-web-fetch: failed to publish response: {}", e);
            }
        }
    }

    Ok(())
}

fn extract_session_id(path: &str) -> Option<String> {
    // Path is like /api/ext/sessions/:id/fetch
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    if segs.len() >= 4 && segs[0] == "api" && segs[1] == "ext" && segs[2] == "sessions" {
        Some(segs[3].to_string())
    } else {
        None
    }
}

/// Fetch the URL from the request body and publish the result as a chunk.
async fn handle_fetch(
    req: &ProxyRequest,
    client: &cafe_sdk::bus::BusClient,
    session_id: Option<&str>,
) -> anyhow::Result<String> {
    let body_str = String::from_utf8(proxy_sdk::decode_body(&req.body)?)?;
    let body_json: serde_json::Value = serde_json::from_str(&body_str)?;
    let url = body_json["url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing url in request body"))?;

    let response = reqwest::get(url).await?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_string();
    let text = response.text().await?;
    let stripped = strip_html(&text);

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let chunk = Chunk::new_text(stripped, "com.nominal.cafe-web-fetch")
        .with_annotation(keys::WEB_SOURCE_URL, url)
        .with_annotation(keys::WEB_CONTENT_TYPE, &content_type)
        .with_annotation(keys::WEB_FETCH_TIME, now_ms)
        .with_annotation(
            keys::SECURITY_TRUST_LEVEL,
            serde_json::json!({ "trusted": false, "source": "web" }),
        );

    let chunk_id = chunk.id.clone();

    if let Some(sid) = session_id {
        client.publish(sid, chunk).await?;
    } else {
        warn!("cafe-web-fetch: no session_id in path, cannot publish result chunk");
    }

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

    #[test]
    fn extract_session_id_ok() {
        assert_eq!(
            extract_session_id("/api/ext/sessions/abc123/fetch"),
            Some("abc123".into())
        );
    }

    #[test]
    fn extract_session_id_wrong_path() {
        assert_eq!(extract_session_id("/api/sessions/abc/fetch"), None);
    }
}
