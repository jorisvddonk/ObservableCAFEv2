use crate::{auth::AuthUser, AppState};
use axum::{extract::State, response::IntoResponse, Json};
use serde::Serialize;

#[derive(Serialize, Clone, PartialEq)]
pub struct AgentInfo {
    pub id: String,
    pub description: String,
    pub background: bool,
    /// Which runtime owns the agent: `"toml"` (cafe-agent-runtime) or
    /// `"js"` (cafe-agent-js). Added for the TOML→JS migration; serde
    /// ignores unknown fields, so older clients keep working.
    pub source: String,
}

/// Merge TOML and JS agent lists. Same id in both → the JS agent wins (the
/// migration direction) and the TOML entry is dropped.
pub fn merge_agent_lists(mut toml: Vec<AgentInfo>, mut js: Vec<AgentInfo>) -> Vec<AgentInfo> {
    use std::collections::HashSet;
    let js_ids: HashSet<_> = js.iter().map(|a| a.id.clone()).collect();
    toml.retain(|a| !js_ids.contains(&a.id));
    let mut out = toml;
    out.append(&mut js);
    // Sort: foreground agents first, then alphabetically
    out.sort_by(|a, b| {
        a.background
            .cmp(&b.background)
            .then(a.id.cmp(&b.id))
    });
    out
}

/// GET /api/agents — list available agent definitions by scanning agent TOML
/// files (`./agents`, `ObservableCAFE_AGENT_SEARCH_PATHS`/`CAFE_AGENT_PATHS`)
/// plus JS agents (`./agents-js`, `CAFE_JS_AGENT_PATHS`).
pub async fn list_agents(
    State(_state): State<AppState>,
    _auth: AuthUser,
) -> impl IntoResponse {
    // Each scanner dedupes internally; merge drops TOML entries shadowed
    // by a JS agent of the same name.
    let agents = merge_agent_lists(scan_toml_agents(), scan_js_agents());

    Json(agents)
}

fn toml_dirs() -> Vec<String> {
    let mut dirs: Vec<String> = vec!["./agents".to_string()];
    if let Ok(paths_str) = std::env::var("ObservableCAFE_AGENT_SEARCH_PATHS")
        .or_else(|_| std::env::var("CAFE_AGENT_PATHS"))
    {
        dirs.extend(paths_str.split(':').map(String::from));
    }
    dirs
}

fn scan_toml_agents() -> Vec<AgentInfo> {
    use std::collections::HashSet;
    let seen: HashSet<_> = toml_dirs().into_iter().collect();
    let mut agents: Vec<AgentInfo> = Vec::new();

    for dir in seen {
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
                                agents.push(AgentInfo {
                                    id,
                                    description,
                                    background,
                                    source: "toml".into(),
                                });
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

    let mut seen_ids: HashSet<String> = HashSet::new();
    agents.retain(|a| seen_ids.insert(a.id.clone()));
    agents
}

fn scan_js_agents() -> Vec<AgentInfo> {
    use std::collections::HashSet;
    let mut agents: Vec<AgentInfo> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    for dir in cafe_js_manifest::agent_dirs() {
        let res = cafe_js_manifest::scan_directory(&dir);
        for skipped in &res.skipped {
            // Missing default dir is normal (JS agents optional); anything
            // else is worth a warning so broken agents stay visible.
            if skipped.0.is_dir() || dir != "./agents-js" {
                tracing::warn!("agents: skipping {}: {}", skipped.0.display(), skipped.1);
            }
        }
        for loaded in res.loaded {
            let m = loaded.manifest;
            if seen_ids.insert(m.name.clone()) {
                agents.push(AgentInfo {
                    id: m.name,
                    description: m.description,
                    background: m.background,
                    source: "js".into(),
                });
            }
        }
    }
    agents
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toml_agent(id: &str, background: bool) -> AgentInfo {
        AgentInfo {
            id: id.into(),
            description: format!("{id} toml"),
            background,
            source: "toml".into(),
        }
    }

    fn js_agent(id: &str, background: bool) -> AgentInfo {
        AgentInfo {
            id: id.into(),
            description: format!("{id} js"),
            background,
            source: "js".into(),
        }
    }

    #[test]
    fn merge_keeps_disjoint_agents_sorted() {
        let out = merge_agent_lists(
            vec![toml_agent("zebra", false), toml_agent("bg", true)],
            vec![js_agent("apple", false)],
        );
        let ids: Vec<_> = out.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["apple", "zebra", "bg"]);
    }

    #[test]
    fn merge_js_wins_name_collision() {
        let out = merge_agent_lists(vec![toml_agent("demo", false)], vec![js_agent("demo", false)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, "js");
        assert_eq!(out[0].description, "demo js");
    }

    #[test]
    fn merge_empty_both_sides() {
        assert!(merge_agent_lists(vec![], vec![]).is_empty());
    }
}
