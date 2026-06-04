use crate::{auth::AuthUser, AppState};
use axum::{extract::State, response::IntoResponse, Json};
use serde::Serialize;

#[derive(Serialize)]
pub struct AgentInfo {
    pub id: String,
    pub description: String,
    pub background: bool,
}

/// GET /api/agents — list available agent definitions by scanning agent TOML files.
/// Uses ObservableCAFE_AGENT_SEARCH_PATHS (falls back to CAFE_AGENT_PATHS) env var.
/// ./agents is always included as a search path.
pub async fn list_agents(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    let mut dirs: Vec<String> = vec!["./agents".to_string()];
    if let Ok(paths_str) = std::env::var("ObservableCAFE_AGENT_SEARCH_PATHS")
        .or_else(|_| std::env::var("CAFE_AGENT_PATHS"))
    {
        dirs.extend(paths_str.split(':').map(String::from));
    }

    let mut agents: Vec<AgentInfo> = Vec::new();

    for dir in dirs {
        let pattern = format!("{}/*.toml", dir);
        if let Ok(paths) = glob::glob(&pattern) {
            for entry in paths.flatten() {
                match std::fs::read_to_string(&entry) {
                    Ok(content) => match toml::from_str::<toml::Value>(&content) {
                        Ok(val) => {
                            let id = val
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let description = val
                                .get("description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let background = val
                                .get("background")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            if !id.is_empty() {
                                agents.push(AgentInfo { id, description, background });
                            }
                        }
                        Err(e) => {
                            tracing::warn!("agents: failed to parse {:?}: {}", entry, e);
                        }
                    },
                    Err(e) => {
                        tracing::warn!("agents: failed to read {:?}: {}", entry, e);
                    }
                }
            }
        }
    }

    // Sort: foreground agents first, then alphabetically
    agents.sort_by(|a, b| {
        a.background
            .cmp(&b.background)
            .then(a.id.cmp(&b.id))
    });

    Json(agents)
}
