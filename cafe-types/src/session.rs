use serde::{Deserialize, Serialize};

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
    pub pipeline: Vec<String>,
    pub schedule: Option<String>,
}

impl Default for AgentDefinition {
    fn default() -> Self {
        Self {
            name: "default".into(),
            description: "Standard chat agent".into(),
            background: false,
            allows_reload: true,
            persists_state: true,
            pipeline: vec!["trust-filter".into(), "llm".into()],
            schedule: None,
        }
    }
}
