use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, Chunk, JsonRpcResponse, rpc_errors, ServerMessage, ToolCall};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tracing::{info, warn};

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
    let client = BusClient::unix(socket_path);
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
        let call_id = request.id.clone();

        match request.method.as_str() {
            "dice-detector.invoke" => {
                let text = request.params["text"].as_str().unwrap_or("");
                info!("cafe-dice: detecting '{}'", text);

                let response = if let Some((count, sides)) = parse_roll(text) {
                    // Publish tool.call chunk so the pipeline's tool-executor can dispatch it
                    let tool_call = ToolCall {
                        name: "dice.roll".into(),
                        parameters: serde_json::json!({ "count": count, "sides": sides }),
                        provider: None,
                    };
                    let tc_chunk = Chunk::new_null("com.nominal.cafe-dice")
                        .with_annotation(keys::CAFE_TOOL_CALL, &tool_call);
                    let _ = client.publish(&session_id, tc_chunk).await;

                    info!("cafe-dice: detected !roll {}d{}", count, sides);
                    JsonRpcResponse::ok(&call_id, serde_json::json!({"detected": true, "count": count, "sides": sides}))
                } else {
                    JsonRpcResponse::ok(&call_id, serde_json::json!({"detected": false}))
                };

                let resp_chunk = Chunk::new_null("com.nominal.cafe-dice")
                    .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
                    .as_transient()
                    .with_retain(60);
                let _ = client.publish(&session_id, resp_chunk).await;
            }

            "dice.roll" => {
                let count = request.params["count"].as_u64().unwrap_or(1);
                let sides = request.params["sides"].as_u64().unwrap_or(6);
                let rolls = match roll_dice(count, sides) {
                    Ok(rolls) => rolls,
                    Err(e) => {
                        let response = JsonRpcResponse::err(
                            &call_id,
                            rpc_errors::INVALID_PARAMS,
                            format!("invalid dice parameters: {}", e),
                        );
                        let resp_chunk = Chunk::new_null("com.nominal.cafe-dice")
                            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
                            .as_transient()
                            .with_retain(60);
                        let _ = client.publish(&session_id, resp_chunk).await;
                        continue;
                    }
                };
                let total: i64 = rolls.iter().map(|v| *v as i64).sum();
                info!("cafe-dice: rolled {}d{} = {}", count, sides, total);

                let response = JsonRpcResponse::ok(&call_id, serde_json::json!({"result": total}));
                let resp_chunk = Chunk::new_null("com.nominal.cafe-dice")
                    .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
                    .as_transient()
                    .with_retain(60);
                let _ = client.publish(&session_id, resp_chunk).await;
            }

            _ => continue,
        }
    }

    Ok(())
}

const MAX_DICE_COUNT: u64 = 10_000;

/// Roll `count` dice each with `sides` sides. Returns the individual rolls.
///
/// Validates inputs so it can be called directly from the `dice.roll` RPC
/// handler (which receives raw, untrusted params) without panicking on
/// `sides == 0` or stalling on an unbounded `count`.
pub(crate) fn roll_dice(count: u64, sides: u64) -> Result<Vec<u64>, String> {
    if sides < 1 {
        return Err(format!("sides must be >= 1, got {}", sides));
    }
    if count == 0 {
        return Err(format!("count must be >= 1, got {}", count));
    }
    if count > MAX_DICE_COUNT {
        return Err(format!("count must be <= {}, got {}", MAX_DICE_COUNT, count));
    }

    let mut rng = StdRng::from_entropy();
    let mut rolls = Vec::with_capacity(count as usize);
    for _ in 0..count {
        rolls.push(rng.gen_range(1..=sides));
    }
    Ok(rolls)
}

/// Parse "!roll 2d6" or "!r 1d20" into (count, sides). Returns None if not a roll.
fn parse_roll(text: &str) -> Option<(u64, u64)> {
    let text = text.trim().strip_prefix("!roll ").or_else(|| text.strip_prefix("!r "))?;
    let text = text.trim();

    // "d20" (single die)
    if let Some(rest) = text.strip_prefix("d").or_else(|| text.strip_prefix("D")) {
        let sides: u64 = rest.parse().ok()?;
        if sides < 1 { return None; }
        return Some((1, sides));
    }

    // "2d6" or "1D20" (count + die)
    let (count_str, rest) = text.split_once(|c: char| c == 'd' || c == 'D')?;
    let count: u64 = if count_str.is_empty() { 1 } else { count_str.parse().ok()? };
    let sides: u64 = rest.trim().parse().ok()?;
    if count < 1 || sides < 1 { return None; }
    Some((count, sides))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_d5() {
        let r = parse_roll("!roll d5");
        assert_eq!(r, Some((1, 5)));
    }

    #[test]
    fn parse_single_d20() {
        let r = parse_roll("!roll 1d20");
        assert_eq!(r, Some((1, 20)));
    }

    #[test]
    fn parse_multi_d6() {
        let r = parse_roll("!roll 2d6");
        assert_eq!(r, Some((2, 6)));
    }

    #[test]
    fn parse_with_shorthand() {
        let r = parse_roll("!r d10");
        assert_eq!(r, Some((1, 10)));
    }

    #[test]
    fn invalid() {
        assert!(parse_roll("!roll abc").is_none());
        assert!(parse_roll("hello").is_none());
    }

    #[test]
    fn roll_zero_sides_is_err() {
        let r = roll_dice(2, 0);
        assert!(r.is_err(), "sides==0 must error, not panic");
    }

    #[test]
    fn roll_zero_count_is_err() {
        let r = roll_dice(0, 6);
        assert!(r.is_err(), "count==0 must error");
    }

    #[test]
    fn roll_huge_count_is_err() {
        let r = roll_dice(MAX_DICE_COUNT + 1, 6);
        assert!(r.is_err(), "excessive count must error, not DoS");
    }

    #[test]
    fn roll_valid_returns_bounded_values() {
        let rolls = roll_dice(2, 6).expect("valid roll should succeed");
        assert_eq!(rolls.len(), 2);
        for v in &rolls {
            assert!((1..=6).contains(v), "roll {} out of range 1..=6", v);
        }
    }
}
