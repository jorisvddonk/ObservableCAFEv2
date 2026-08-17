use anyhow::Context;
use reqwest::Client;
use serde_json::Value;

/// Philips Hue CLIP v2 API client.
///
/// All requests go to `https://<bridge>/clip/v2/resource/...` with the
/// `hue-application-key` header. Hue bridges use self-signed certificates,
/// so cert verification is disabled (local-network device only).
pub struct HueClient {
    http: Client,
    bridge_url: String,
    api_key: String,
}

impl HueClient {
    pub fn new(bridge_url: &str, api_key: &str) -> anyhow::Result<Self> {
        let http = Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?;
        Ok(Self {
            http,
            bridge_url: bridge_url.to_string(),
            api_key: api_key.to_string(),
        })
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let url = format!("{}{}", self.bridge_url, path);
        let resp = self
            .http
            .get(&url)
            .header("hue-application-key", &self.api_key)
            .send()
            .await
            .context("Hue bridge GET request failed")?;
        let body: Value = resp.json().await.context("Hue bridge response parse failed")?;
        check_clip_errors(&body)?;
        Ok(body)
    }

    async fn put(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let url = format!("{}{}", self.bridge_url, path);
        let resp = self
            .http
            .put(&url)
            .header("hue-application-key", &self.api_key)
            .json(body)
            .send()
            .await
            .context("Hue bridge PUT request failed")?;
        let body: Value = resp.json().await.context("Hue bridge response parse failed")?;
        check_clip_errors(&body)?;
        Ok(body)
    }

    pub async fn list_lights(&self) -> anyhow::Result<Value> {
        self.get("/clip/v2/resource/light").await
    }

    pub async fn get_light(&self, light_id: &str) -> anyhow::Result<Value> {
        self.get(&format!("/clip/v2/resource/light/{}", light_id)).await
    }

    pub async fn set_light(&self, light_id: &str, state: &Value) -> anyhow::Result<Value> {
        self.put(&format!("/clip/v2/resource/light/{}", light_id), state).await
    }

    pub async fn list_grouped_lights(&self) -> anyhow::Result<Value> {
        self.get("/clip/v2/resource/grouped_light").await
    }

    pub async fn get_grouped_light(&self, group_id: &str) -> anyhow::Result<Value> {
        self.get(&format!("/clip/v2/resource/grouped_light/{}", group_id)).await
    }

    pub async fn set_grouped_light(&self, group_id: &str, state: &Value) -> anyhow::Result<Value> {
        self.put(&format!("/clip/v2/resource/grouped_light/{}", group_id), state).await
    }

    pub async fn list_scenes(&self) -> anyhow::Result<Value> {
        self.get("/clip/v2/resource/scene").await
    }

    pub async fn activate_scene(&self, scene_id: &str) -> anyhow::Result<Value> {
        let body = serde_json::json!({"recall": {"action": "active"}});
        self.put(&format!("/clip/v2/resource/scene/{}", scene_id), &body).await
    }
}

fn check_clip_errors(body: &Value) -> anyhow::Result<()> {
    if let Some(errors) = body.get("errors").and_then(|e| e.as_array()) {
        if !errors.is_empty() {
            let msg = errors
                .iter()
                .filter_map(|e| e.get("description").and_then(|d| d.as_str()))
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!("Hue bridge error: {}", msg);
        }
    }
    Ok(())
}

/// Discover the bridge IP via the meethue.com discovery service.
/// Returns a full `https://<ip>` URL.
pub async fn discover_bridge() -> anyhow::Result<String> {
    let http = Client::builder().danger_accept_invalid_certs(true).build()?;
    let resp = http
        .get("https://discovery.meethue.com/")
        .send()
        .await
        .context("Failed to contact meethue.com discovery service")?;
    let bridges: Vec<Value> = resp.json().await.context("Failed to parse discovery response")?;

    if let Some(bridge) = bridges.first() {
        let ip = bridge["internalipaddress"]
            .as_str()
            .context("No internalipaddress in discovery response")?;
        Ok(format!("https://{}", ip))
    } else {
        anyhow::bail!("No Hue bridge found on the network");
    }
}

/// Register a new application key with the bridge.
/// Requires the physical link button to be pressed; returns an error
/// instructing the caller to retry after pressing it.
pub async fn register(bridge_url: &str) -> anyhow::Result<String> {
    let http = Client::builder().danger_accept_invalid_certs(true).build()?;
    let body = serde_json::json!({"devicetype": "cafe-hue#observablecafe"});
    let url = format!("{}/api", bridge_url);

    let resp = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Failed to register with Hue bridge")?;
    let result: Vec<Value> = resp.json().await.context("Failed to parse registration response")?;

    if let Some(entry) = result.first() {
        if let Some(error) = entry.get("error") {
            let err_type = error["type"].as_u64().unwrap_or(0);
            let description = error["description"].as_str().unwrap_or("unknown error");
            if err_type == 101 {
                anyhow::bail!(
                    "Link button not pressed. Press the button on your Hue bridge, then call hue.register again."
                );
            }
            anyhow::bail!("Hue bridge registration error (type {}): {}", err_type, description);
        }
        if let Some(success) = entry.get("success") {
            let username = success["username"]
                .as_str()
                .context("No username in registration success response")?;
            return Ok(username.to_string());
        }
    }

    anyhow::bail!(
        "Unexpected registration response: {}",
        serde_json::to_string(&result)?
    )
}
