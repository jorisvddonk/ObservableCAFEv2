use cafe_http_proxy_sdk::{self as proxy_sdk, ProxyRequest, ProxyResponse};
use cafe_sdk::{keys, Chunk, JsonRpcResponse, ServerMessage};
use tracing::{info, warn};

/// Bus RPC by which agents request a fetch (the `web-fetch` evaluator type).
const INVOKE_METHOD: &str = "web-fetch.invoke";

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

    // Agent-driven fetches: handle `web-fetch.invoke` on session subscriptions.
    let rpc_client = client.clone();
    tokio::spawn(async move {
        if let Err(e) = handle_rpc_sessions(rpc_client).await {
            warn!("cafe-web-fetch: rpc subscriber ended: {}", e);
        }
    });

    // Handle incoming messages
    // Gate dispatch on history replay completion (ADR-123).
    let mut history_complete = false;
    while let Some(msg) = rx.recv().await {
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

/// Subscribe to all sessions and serve `web-fetch.invoke` for each.
async fn handle_rpc_sessions(client: cafe_sdk::bus::BusClient) -> anyhow::Result<()> {
    let mut rx = client.subscribe_all().await?;
    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let c = client.clone();
            tokio::spawn(async move {
                if let Err(e) = run_rpc_session(session_id, c).await {
                    warn!("cafe-web-fetch: session error: {}", e);
                }
            });
        }
    }
    Ok(())
}

/// Per-session loop serving the `web-fetch.invoke` RPC (agent-driven fetch).
async fn run_rpc_session(
    session_id: String,
    client: cafe_sdk::bus::BusClient,
) -> anyhow::Result<()> {
    let mut rx = client.subscribe(&session_id).await?;
    let mut history_complete = false;
    while let Some(msg) = rx.recv().await {
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
        let Some(request) = chunk.as_rpc_request() else {
            continue;
        };
        if request.method != INVOKE_METHOD {
            continue;
        }
        let call_id = request.id.clone();
        let text = request.params["text"].as_str().unwrap_or("");
        let url = request.params["url"]
            .as_str()
            .map(str::to_string)
            .or_else(|| parse_fetch_url(text));

        let response = match url {
            None => JsonRpcResponse::err(
                &call_id,
                cafe_sdk::rpc_errors::INVALID_PARAMS,
                "no URL found (send `!fetch <url>` in text, or a url param)",
            ),
            Some(url) => match fetch_url_text(&url).await {
                Ok((stripped, content_type)) => {
                    let chunk = build_content_chunk(&stripped, &url, &content_type);
                    let chunk_id = chunk.id.clone();
                    let _ = client.publish(&session_id, chunk).await;
                    JsonRpcResponse::ok(
                        &call_id,
                        serde_json::json!({ "chunk_id": chunk_id, "url": url }),
                    )
                }
                Err(e) => JsonRpcResponse::err(
                    &call_id,
                    cafe_sdk::rpc_errors::UPSTREAM_ERROR,
                    e.to_string(),
                ),
            },
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-web-fetch")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = client.publish(&session_id, resp_chunk).await;
    }
    Ok(())
}

/// Extract a URL from a `!fetch <url>` command, or accept a bare URL.
fn parse_fetch_url(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let rest = trimmed
        .strip_prefix("!fetch ")
        .or_else(|| trimmed.strip_prefix("!fetch"))
        .unwrap_or(trimmed)
        .trim();
    if rest.starts_with("http://") || rest.starts_with("https://") {
        Some(rest.split_whitespace().next().unwrap_or(rest).to_string())
    } else {
        None
    }
}

/// Fetch `url`, returning (html-stripped text, content type).
async fn fetch_url_text(url: &str) -> anyhow::Result<(String, String)> {
    let response = reqwest::get(url).await?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_string();
    let text = response.text().await?;
    Ok((strip_html(&text), content_type))
}

/// Build the content chunk for a fetched page (untrusted web content).
fn build_content_chunk(stripped: &str, url: &str, content_type: &str) -> Chunk {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    Chunk::new_text(stripped, "com.nominal.cafe-web-fetch")
        .with_annotation(keys::WEB_SOURCE_URL, url)
        .with_annotation(keys::WEB_CONTENT_TYPE, content_type)
        .with_annotation(keys::WEB_FETCH_TIME, now_ms)
        .with_annotation(
            keys::SECURITY_TRUST_LEVEL,
            serde_json::json!({ "trusted": false, "source": "web" }),
        )
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

    let (stripped, content_type) = fetch_url_text(url).await?;
    let chunk = build_content_chunk(&stripped, url, &content_type);

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

    #[test]
    fn parse_fetch_url_from_command() {
        assert_eq!(
            parse_fetch_url("!fetch https://example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            parse_fetch_url("!fetch http://example.com/a?b=1 trailing").as_deref(),
            Some("http://example.com/a?b=1")
        );
    }

    #[test]
    fn parse_fetch_url_bare_and_invalid() {
        assert_eq!(
            parse_fetch_url("https://example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(parse_fetch_url("!fetch not-a-url"), None);
        assert_eq!(parse_fetch_url("hello world"), None);
        assert_eq!(parse_fetch_url(""), None);
    }

    #[test]
    fn build_content_chunk_marks_untrusted() {
        let chunk = build_content_chunk("body", "https://example.com", "text/html");
        assert_eq!(chunk.content.as_deref(), Some("body"));
        let trust = chunk
            .annotations
            .get(cafe_sdk::keys::SECURITY_TRUST_LEVEL)
            .expect("trust annotation");
        assert_eq!(trust["trusted"], serde_json::json!(false));
    }
}
