use crate::config::Config;
use crate::hue_client::{self, HueClient};
use cafe_sdk::bus::BusClient;
use cafe_sdk::{keys, rpc_errors, Chunk, EvaluatorSchema, JsonRpcRequest, JsonRpcResponse, ServerMessage};
use std::sync::Arc;
use tracing::{info, warn};

pub async fn run_with_reconnect(socket_path: String, config: Arc<Config>) {
    cafe_sdk::bus::run_with_reconnect("cafe-hue", move || {
        let sp = socket_path.clone();
        let cfg = config.clone();
        async move { subscribe_sessions(&sp, cfg).await }
    })
    .await;
}

async fn subscribe_sessions(socket_path: &str, config: Arc<Config>) -> anyhow::Result<()> {
    info!("cafe-hue: starting on {}", socket_path);
    let client = BusClient::unix(socket_path);

    let schema = EvaluatorSchema {
        name: "hue".into(),
        description: "Philips Hue light controller — list, get, set lights and groups, activate scenes".into(),
        config_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "config.hue.bridge_url": { "type": "string", "description": "Hue bridge URL (https://IP)" },
                "config.hue.api_key": { "type": "string", "description": "Hue application key" }
            }
        }),
        rpc_params_schema: serde_json::json!({ "type": "object", "properties": {} }),
    };
    if let Err(e) = cafe_sdk::schema::announce_schema(&client, schema).await {
        warn!("cafe-hue: failed to announce schema: {}", e);
    }

    let mut rx = client.subscribe_all().await?;

    while let Some(msg) = rx.recv().await {
        if let ServerMessage::SessionCreated { session_id, .. } = msg {
            let c = client.clone();
            let cfg = config.clone();
            tokio::spawn(async move {
                if let Err(e) = run_session(session_id, c, cfg).await {
                    warn!("cafe-hue: session error: {}", e);
                }
            });
        }
    }

    Ok(())
}

async fn run_session(
    session_id: String,
    client: BusClient,
    config: Arc<Config>,
) -> anyhow::Result<()> {
    let mut sub = client.subscribe_session(&session_id).await?;

    // Gate RPC dispatch on history replay completion (ADR-123).
    let mut history_complete = false;

    while let Some(msg) = sub.rx.recv().await {
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

        let Some(request) = chunk.as_rpc_request() else { continue; };
        if !request.method.starts_with("hue.") { continue; }

        let call_id = request.id.clone();
        info!(
            "cafe-hue: handling RPC id={} method={} session={}",
            call_id, request.method, session_id
        );

        let result = handle_hue_request(&config, &request).await;

        let response = match result {
            Ok(data) => JsonRpcResponse::ok(&call_id, data),
            Err(e) => {
                warn!("cafe-hue: error for call {}: {}", call_id, e);
                JsonRpcResponse::err(&call_id, rpc_errors::UPSTREAM_ERROR, e.to_string())
            }
        };

        let resp_chunk = Chunk::new_null("com.nominal.cafe-hue")
            .with_annotation(keys::CAFE_JSONRPC_RESPONSE, &response)
            .as_transient()
            .with_retain(60);
        let _ = sub.publish(resp_chunk).await;
    }

    Ok(())
}

async fn handle_hue_request(
    config: &Config,
    request: &JsonRpcRequest,
) -> anyhow::Result<serde_json::Value> {
    match request.method.as_str() {
        "hue.register" => {
            let bridge_url = match request.params.get("bridge_ip").and_then(|v| v.as_str()) {
                Some(ip) => format!("https://{}", ip),
                None => hue_client::discover_bridge().await?,
            };
            let api_key = hue_client::register(&bridge_url).await?;
            Ok(serde_json::json!({ "bridge_url": bridge_url, "api_key": api_key }))
        }
        method => {
            let hue = HueClient::new(&config.bridge_url, &config.api_key).map_err(|e| {
                anyhow::anyhow!(
                    "{} — set HUE_BRIDGE_URL and HUE_API_KEY, or call hue.register first",
                    e
                )
            })?;

            match method {
                "hue.list_lights" => hue.list_lights().await,
                "hue.get_light" => {
                    let light_id = param_string(&request.params, "light_id")?;
                    hue.get_light(&light_id).await
                }
                "hue.set_light" => {
                    let light_id = param_string(&request.params, "light_id")?;
                    let state = build_state(&request.params);
                    hue.set_light(&light_id, &state).await
                }
                "hue.list_groups" => hue.list_grouped_lights().await,
                "hue.get_group" => {
                    let group_id = param_string(&request.params, "group_id")?;
                    hue.get_grouped_light(&group_id).await
                }
                "hue.set_group" => {
                    let group_id = param_string(&request.params, "group_id")?;
                    let state = build_state(&request.params);
                    hue.set_grouped_light(&group_id, &state).await
                }
                "hue.list_scenes" => hue.list_scenes().await,
                "hue.activate_scene" => {
                    let scene_id = param_string(&request.params, "scene_id")?;
                    hue.activate_scene(&scene_id).await
                }
                m => anyhow::bail!("unknown hue method: {}", m),
            }
        }
    }
}

fn param_string(params: &serde_json::Value, key: &str) -> anyhow::Result<String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("missing required parameter: {}", key))
}

/// Build a CLIP v2 state body from RPC params.
/// Supports: on, brightness (0-100), color_xy ([x,y]), color_temp (mirek), transition_time (ms).
fn build_state(params: &serde_json::Value) -> serde_json::Value {
    let mut state = serde_json::json!({});

    if let Some(on) = params.get("on").and_then(|v| v.as_bool()) {
        state["on"] = serde_json::json!({ "on": on });
    }
    if let Some(brightness) = params.get("brightness").and_then(|v| v.as_f64()) {
        state["dimming"] = serde_json::json!({ "brightness": brightness.clamp(0.0, 100.0) });
    }
    if let Some(color_xy) = params.get("color_xy").and_then(|v| v.as_array()) {
        if color_xy.len() == 2 {
            if let (Some(x), Some(y)) = (color_xy[0].as_f64(), color_xy[1].as_f64()) {
                state["color"] = serde_json::json!({ "xy": { "x": x, "y": y } });
            }
        }
    }
    if let Some(ct) = params.get("color_temp").and_then(|v| v.as_i64()) {
        state["color_temperature"] = serde_json::json!({ "mirek": ct });
    }
    if let Some(duration) = params.get("transition_time").and_then(|v| v.as_i64()) {
        state["dynamics"] = serde_json::json!({ "duration": duration });
    }

    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(json: serde_json::Value) -> serde_json::Value {
        json
    }

    #[test]
    fn build_state_on_only() {
        let state = build_state(&params(serde_json::json!({ "on": false })));
        assert_eq!(state, serde_json::json!({ "on": { "on": false } }));
    }

    #[test]
    fn build_state_brightness_clamped() {
        let state = build_state(&params(serde_json::json!({ "brightness": 150.0 })));
        assert_eq!(state, serde_json::json!({ "dimming": { "brightness": 100.0 } }));
    }

    #[test]
    fn build_state_color_and_temp() {
        let state = build_state(&params(serde_json::json!({
            "color_xy": [0.5, 0.5],
            "color_temp": 250,
            "transition_time": 200
        })));
        assert_eq!(
            state,
            serde_json::json!({
                "color": { "xy": { "x": 0.5, "y": 0.5 } },
                "color_temperature": { "mirek": 250 },
                "dynamics": { "duration": 200 }
            })
        );
    }

    #[test]
    fn build_state_empty() {
        let state = build_state(&params(serde_json::json!({})));
        assert_eq!(state, serde_json::json!({}));
    }

    #[test]
    fn build_state_bad_color_xy_ignored() {
        let state = build_state(&params(serde_json::json!({ "color_xy": [0.5] })));
        assert_eq!(state, serde_json::json!({}));
    }
}
