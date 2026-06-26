use anyhow::{bail, Context};
use serde_json::Value;

pub struct SheetbotClient {
    pub base_url: String,
    api_key: String,
    jwt_token: Option<String>,
    http: reqwest::Client,
}

impl SheetbotClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            jwt_token: None,
            http: reqwest::Client::new(),
        }
    }

    /// If the configured key is a SheetBot API key (uuid.secret format),
    /// exchange it for a JWT via POST /login.
    pub async fn login(&mut self) -> anyhow::Result<()> {
        if self.api_key.is_empty() {
            return Ok(());
        }

        // If it contains a dot, it's an API key that needs exchanging
        if self.api_key.contains('.') {
            let url = format!("{}/login", self.base_url);
            let body = serde_json::json!({ "apiKey": self.api_key });
            let resp = self
                .http
                .post(&url)
                .json(&body)
                .send()
                .await
                .context("POST /login failed")?
                .error_for_status()
                .context("POST /login returned error")?;
            let result: Value = resp.json().await.context("failed to parse /login response")?;
            let token = result["token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("login response missing 'token'"))?;
            self.jwt_token = Some(token.to_string());
        } else {
            // Assume it's already a JWT
            self.jwt_token = Some(self.api_key.clone());
        }

        Ok(())
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = &self.jwt_token {
            if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token)) {
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        headers
    }

    // ── Tasks ────────────────────────────────────────────────────────────────

    pub async fn create_task(&self, params: &Value) -> anyhow::Result<Value> {
        self.post("/tasks", params).await
    }

    pub async fn list_tasks(&self) -> anyhow::Result<Value> {
        self.get("/tasks").await
    }

    pub async fn get_task(&self, id: &str) -> anyhow::Result<Value> {
        self.get(&format!("/tasks/{}", id)).await
    }

    pub async fn update_task(&self, id: &str, params: &Value) -> anyhow::Result<Value> {
        self.patch(&format!("/tasks/{}", id), params).await
    }

    pub async fn delete_task(&self, id: &str) -> anyhow::Result<Value> {
        self.delete(&format!("/tasks/{}", id)).await
    }

    pub async fn accept_task(&self, id: &str) -> anyhow::Result<Value> {
        self.post(&format!("/tasks/{}/accept", id), &Value::Null).await
    }

    pub async fn complete_task(&self, id: &str, params: &Value) -> anyhow::Result<Value> {
        self.post(&format!("/tasks/{}/complete", id), params).await
    }

    pub async fn fail_task(&self, id: &str, params: &Value) -> anyhow::Result<Value> {
        self.post(&format!("/tasks/{}/failed", id), params).await
    }

    pub async fn update_task_data(&self, id: &str, params: &Value) -> anyhow::Result<Value> {
        self.post(&format!("/tasks/{}/data", id), params).await
    }

    pub async fn clone_task(&self, id: &str) -> anyhow::Result<Value> {
        self.post(&format!("/tasks/{}/clone", id), &Value::Null).await
    }

    pub async fn get_next_task(&self) -> anyhow::Result<Value> {
        self.post("/tasks/get", &serde_json::json!({})).await
    }

    // ── Sheets ───────────────────────────────────────────────────────────────

    pub async fn list_sheets(&self) -> anyhow::Result<Value> {
        self.get("/sheets").await
    }

    pub async fn get_sheet(&self, id: &str) -> anyhow::Result<Value> {
        self.get(&format!("/sheets/{}", id)).await
    }

    pub async fn upsert_sheet_data(&self, id: &str, params: &Value) -> anyhow::Result<Value> {
        self.post(&format!("/sheets/{}/data", id), params).await
    }

    pub async fn delete_sheet_row(&self, id: &str, key: &str) -> anyhow::Result<Value> {
        self.delete(&format!("/sheets/{}/data/{}", id, key)).await
    }

    // ── Artefacts ────────────────────────────────────────────────────────────

    pub async fn upload_artefact(&self, task_id: &str, filename: &str, data: &Value) -> anyhow::Result<Value> {
        self.post(
            &format!("/tasks/{}/artefacts", task_id),
            &serde_json::json!({ "filename": filename, "data": data }),
        ).await
    }

    pub async fn get_artefact(&self, task_id: &str, filename: &str) -> anyhow::Result<Value> {
        self.get(&format!("/tasks/{}/artefacts/{}", task_id, filename)).await
    }

    pub async fn delete_artefact(&self, task_id: &str, filename: &str) -> anyhow::Result<Value> {
        self.delete(&format!("/tasks/{}/artefacts/{}", task_id, filename)).await
    }

    // ── Library ──────────────────────────────────────────────────────────────

    pub async fn list_library(&self) -> anyhow::Result<Value> {
        self.get("/library").await
    }

    // ── Generic HTTP helpers ─────────────────────────────────────────────────

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .context(format!("GET {} failed", path))?
            .error_for_status()
            .context(format!("GET {} returned error", path))?;
        Ok(resp.json().await.context(format!("failed to parse GET {} response", path))?)
    }

    async fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .post(&url)
            .headers(self.auth_headers())
            .json(body)
            .send()
            .await
            .context(format!("POST {} failed", path))?
            .error_for_status()
            .context(format!("POST {} returned error", path))?;
        Ok(resp.json().await.context(format!("failed to parse POST {} response", path))?)
    }

    async fn patch(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .patch(&url)
            .headers(self.auth_headers())
            .json(body)
            .send()
            .await
            .context(format!("PATCH {} failed", path))?
            .error_for_status()
            .context(format!("PATCH {} returned error", path))?;
        Ok(resp.json().await.context(format!("failed to parse PATCH {} response", path))?)
    }

    async fn delete(&self, path: &str) -> anyhow::Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        let resp = self
            .http
            .delete(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .context(format!("DELETE {} failed", path))?
            .error_for_status()
            .context(format!("DELETE {} returned error", path))?;
        let body = resp.text().await.context(format!("failed to read DELETE {} response", path))?;
        if body.is_empty() {
            Ok(serde_json::json!({ "deleted": true }))
        } else {
            serde_json::from_str(&body).context(format!("failed to parse DELETE {} response", path))
        }
    }

    /// Dispatch a method call from an RPC request.
    /// The `method` should be like "create_task" (without the "sheetbot." prefix).
    pub async fn dispatch(&self, method: &str, params: &Value) -> anyhow::Result<Value> {
        match method {
            "create_task" => self.create_task(params).await,
            "list_tasks" => self.list_tasks().await,
            "get_task" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("get_task: id is required"); }
                self.get_task(id).await
            }
            "update_task" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("update_task: id is required"); }
                self.update_task(id, params).await
            }
            "delete_task" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("delete_task: id is required"); }
                self.delete_task(id).await
            }
            "accept_task" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("accept_task: id is required"); }
                self.accept_task(id).await
            }
            "complete_task" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("complete_task: id is required"); }
                self.complete_task(id, params).await
            }
            "fail_task" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("fail_task: id is required"); }
                self.fail_task(id, params).await
            }
            "update_task_data" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("update_task_data: id is required"); }
                self.update_task_data(id, params).await
            }
            "clone_task" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("clone_task: id is required"); }
                self.clone_task(id).await
            }
            "get_next_task" => self.get_next_task().await,

            "list_sheets" => self.list_sheets().await,
            "get_sheet" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("get_sheet: id is required"); }
                self.get_sheet(id).await
            }
            "upsert_sheet_data" => {
                let id = params["id"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("upsert_sheet_data: id is required"); }
                self.upsert_sheet_data(id, params).await
            }
            "delete_sheet_row" => {
                let id = params["id"].as_str().unwrap_or_default();
                let key = params["key"].as_str().unwrap_or_default();
                if id.is_empty() { bail!("delete_sheet_row: id is required"); }
                if key.is_empty() { bail!("delete_sheet_row: key is required"); }
                self.delete_sheet_row(id, key).await
            }

            "upload_artefact" => {
                let task_id = params["task_id"].as_str().unwrap_or_default();
                let filename = params["filename"].as_str().unwrap_or_default();
                if task_id.is_empty() { bail!("upload_artefact: task_id is required"); }
                if filename.is_empty() { bail!("upload_artefact: filename is required"); }
                self.upload_artefact(task_id, filename, params).await
            }
            "get_artefact" => {
                let task_id = params["task_id"].as_str().unwrap_or_default();
                let filename = params["filename"].as_str().unwrap_or_default();
                if task_id.is_empty() { bail!("get_artefact: task_id is required"); }
                if filename.is_empty() { bail!("get_artefact: filename is required"); }
                self.get_artefact(task_id, filename).await
            }
            "delete_artefact" => {
                let task_id = params["task_id"].as_str().unwrap_or_default();
                let filename = params["filename"].as_str().unwrap_or_default();
                if task_id.is_empty() { bail!("delete_artefact: task_id is required"); }
                if filename.is_empty() { bail!("delete_artefact: filename is required"); }
                self.delete_artefact(task_id, filename).await
            }

            "list_library" => self.list_library().await,

            _ => bail!("unknown sheetbot method: {}", method),
        }
    }
}
