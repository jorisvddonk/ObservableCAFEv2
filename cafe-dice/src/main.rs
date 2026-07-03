use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, JsonRpcResponse, ServerMessage};
use rand::Rng;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let socket_path = std::env::var("CAFE_BUS_SOCKET")
        .unwrap_or_else(|_| "/tmp/cafe-bus.sock".into());

    cafe_sdk::bus::run_with_reconnect("cafe-dice", move || {
        let sp = socket_path.clone();
        async move { subscribe_all(&sp).await }
    })
    .await;

    Ok(())
}

async fn subscribe_all(socket_path: &str) -> anyhow::Result<()> {
    info!("cafe-dice: starting on {}", socket_path);
    let client = BusClient::new(socket_path);
    let mut rx = client.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let c = client.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, c).await {
                    warn!("cafe-dice: session error: {}", e);
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
        if request.method != "dice.invoke" { continue; }

        info!(
            "cafe-dice: handling {} call_id={} session={}",
            request.method, request.id, session_id
        );

        let call_id = request.id.clone();
        let text = request.params["text"].as_str().unwrap_or("");

        let response = if let Some(result) = parse_and_roll(text) {
            info!("cafe-dice: rolled {} for '{}'", result, text);
            let output = serde_json::json!({ "result": result, "expression": text });
            JsonRpcResponse::ok(&call_id, output)
        } else {
            warn!("cafe-dice: failed to parse '{}'", text);
            JsonRpcResponse::err(&call_id, -1, &format!("Invalid roll expression: {}", text))
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-dice")
            .with_annotation(keys::JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = client.publish(&session_id, resp_chunk).await;
    }

    Ok(())
}

/// Parse expressions like "!roll 1d5", "2d20", "1D6" and return the result.
fn parse_and_roll(text: &str) -> Option<i64> {
    let text = text.trim().strip_prefix("!roll ").or_else(|| text.strip_prefix("!r "))?;
    let text = text.trim();

    // Support "NdM" format
    if let Some(rest) = text.strip_prefix("d").or_else(|| text.strip_prefix("D")) {
        let sides: u64 = rest.parse().ok()?;
        if sides < 1 { return None; }
        return Some(rand::thread_rng().gen_range(1..=sides) as i64);
    }

    // Support "N d M" format (e.g., "1d5", "2d20")
    let (count_str, rest) = text.split_once(|c: char| c == 'd' || c == 'D')?;
    let count: u64 = if count_str.is_empty() { 1 } else { count_str.parse().ok()? };
    let sides_str = rest.trim();
    let sides: u64 = sides_str.parse().ok()?;
    if count < 1 || sides < 1 { return None; }

    let mut total: i64 = 0;
    let mut rng = rand::thread_rng();
    for _ in 0..count {
        total += rng.gen_range(1..=sides) as i64;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_d5() {
        let r = parse_and_roll("!roll d5");
        assert!(r.is_some());
        assert!((1..=5).contains(&r.unwrap()));
    }

    #[test]
    fn parse_single_d20() {
        let r = parse_and_roll("!roll 1d20");
        assert!(r.is_some());
        assert!((1..=20).contains(&r.unwrap()));
    }

    #[test]
    fn parse_multi_dice() {
        let r = parse_and_roll("!roll 2d6");
        assert!(r.is_some());
        let v = r.unwrap();
        assert!((2..=12).contains(&v), "2d6 should be 2..=12, got {}", v);
    }

    #[test]
    fn parse_with_exclamation() {
        let r = parse_and_roll("!r d10");
        assert!(r.is_some());
        assert!((1..=10).contains(&r.unwrap()));
    }

    #[test]
    fn invalid_expression() {
        assert!(parse_and_roll("!roll abc").is_none());
        assert!(parse_and_roll("hello world").is_none());
        assert!(parse_and_roll("").is_none());
    }
}
