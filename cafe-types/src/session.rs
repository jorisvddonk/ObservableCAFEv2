use serde::{Deserialize, Serialize};

use crate::step::StepDef;

/// Metadata about a session, returned by list_sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub agent_id: String,
    pub display_name: Option<String>,
    pub is_background: bool,
    pub ui_mode: String,
    pub message_count: usize,
    pub created_at: i64,
}

/// Agent definition loaded from a TOML file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub background: bool,
    pub allows_reload: bool,
    pub persists_state: bool,
    /// Ordered list of pipeline steps ([[steps]] in TOML).
    pub steps: Vec<StepDef>,
    pub schedule: Option<String>,
    pub initial_chunk_content: String,
    pub initial_chunk_type: String,
    pub initial_chunk_data: Option<Vec<u8>>,
    pub initial_chunk_mime_type: Option<String>,
    pub initial_chunk_annotations: std::collections::HashMap<String, serde_json::Value>,
    /// Per-agent RPC timeout in seconds (default 60).
    pub rpc_timeout_secs: u64,
    /// Maximum pipeline recursion depth via step_complete chaining (default 10).
    pub max_pipeline_depth: u32,
    /// Ephemeral session keepalive in seconds (None = persistent).
    /// When set, the session auto-deletes after all subscribers disconnect.
    pub ephemeral_keepalive_secs: Option<u64>,
    /// Role to count for ephemeral lifecycle. Only subscribers with this role
    /// are counted toward session lifetime. None = count all subscribers.
    pub ephemeral_count_role: Option<String>,
}

impl Default for AgentDefinition {
    fn default() -> Self {
        Self {
            name: "default".into(),
            description: "Standard chat agent".into(),
            background: false,
            allows_reload: true,
            persists_state: true,
            steps: vec![
                StepDef {
                    id: "trust-filter".into(),
                    step_type: "trust-filter".into(),
                    trigger: "user_message".into(),
                    enabled_if: None,
                },
                StepDef {
                    id: "llm".into(),
                    step_type: "llm".into(),
                    trigger: "user_message".into(),
                    enabled_if: None,
                },
            ],
            schedule: None,
            initial_chunk_content: String::new(),
            initial_chunk_type: "text".into(),
            initial_chunk_data: None,
            initial_chunk_mime_type: None,
            initial_chunk_annotations: std::collections::HashMap::new(),
            rpc_timeout_secs: 60,
            max_pipeline_depth: 10,
            ephemeral_keepalive_secs: None,
            ephemeral_count_role: None,
        }
    }
}
