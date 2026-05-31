use anyhow::Result;
use cafe_types::AgentDefinition;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Raw TOML representation of an agent file (superset of AgentDefinition).
#[derive(Debug, Clone, Deserialize)]
pub struct AgentFile {
    pub name: String,
    pub description: Option<String>,
    pub background: Option<bool>,
    pub allows_reload: Option<bool>,
    pub persists_state: Option<bool>,
    pub pipeline: Option<Vec<String>>,
    pub schedule: Option<String>,
}

impl From<AgentFile> for AgentDefinition {
    fn from(f: AgentFile) -> Self {
        AgentDefinition {
            name: f.name,
            description: f.description.unwrap_or_default(),
            background: f.background.unwrap_or(false),
            allows_reload: f.allows_reload.unwrap_or(true),
            persists_state: f.persists_state.unwrap_or(true),
            pipeline: f.pipeline.unwrap_or_else(|| vec!["llm".into()]),
            schedule: f.schedule,
        }
    }
}

pub fn load_agent_file(path: &Path) -> Result<AgentDefinition> {
    let content = std::fs::read_to_string(path)?;
    let file: AgentFile = toml::from_str(&content)?;
    Ok(file.into())
}

pub fn hash_file(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            format!("{:x}", hasher.finalize())
        }
        Err(_) => String::new(),
    }
}

/// Scan a directory for *.toml agent files.
pub fn scan_directory(dir: &str) -> Vec<(std::path::PathBuf, AgentDefinition)> {
    let pattern = format!("{}/*.toml", dir);
    let mut agents = Vec::new();
    if let Ok(paths) = glob::glob(&pattern) {
        for entry in paths.flatten() {
            match load_agent_file(&entry) {
                Ok(def) => agents.push((entry, def)),
                Err(e) => tracing::warn!("Failed to load agent {:?}: {}", entry, e),
            }
        }
    }
    agents
}
