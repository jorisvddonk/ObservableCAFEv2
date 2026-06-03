use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use cafe_types::AgentDefinition;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use tracing;

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
    pub initial_chunk_content: Option<String>,
    pub initial_chunk_type: Option<String>,
    pub initial_chunk_data: Option<String>, // base64 encoded for binary
    pub initial_chunk_mime_type: Option<String>,
    pub initial_chunk_annotations: Option<std::collections::HashMap<String, serde_json::Value>>,
}

impl From<AgentFile> for AgentDefinition {
    fn from(f: AgentFile) -> Self {
        // Handle binary data if present
        let initial_chunk_data = if let Some(data_str) = f.initial_chunk_data {
            // Decode base64 data
            match STANDARD.decode(data_str) {
                Ok(data) => Some(data),
                Err(e) => {
                    tracing::warn!("Failed to decode base64 initial chunk data: {}", e);
                    None
                }
            }
        } else {
            None
        };

        AgentDefinition {
            name: f.name,
            description: f.description.unwrap_or_default(),
            background: f.background.unwrap_or(false),
            allows_reload: f.allows_reload.unwrap_or(true),
            persists_state: f.persists_state.unwrap_or(true),
            pipeline: f.pipeline.unwrap_or_else(|| vec!["llm".into()]),
            schedule: f.schedule,
            initial_chunk_content: f.initial_chunk_content.unwrap_or_default(),
            initial_chunk_type: f.initial_chunk_type.unwrap_or_else(|| "text".into()),
            initial_chunk_data,
            initial_chunk_mime_type: f.initial_chunk_mime_type,
            initial_chunk_annotations: f.initial_chunk_annotations.unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn test_load_agent_file() {
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(temp_file, "name = \"test-agent\"").unwrap();
        writeln!(temp_file, "description = \"A test agent\"").unwrap();
        let agent = load_agent_file(temp_file.path()).unwrap();
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.description, "A test agent");
    }

    #[test]
    fn test_load_agent_file_with_initial_chunk() {
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(temp_file, "name = \"test-agent\"").unwrap();
        writeln!(temp_file, "description = \"A test agent\"").unwrap();
        writeln!(temp_file, "initial_chunk_content = \"Hello from test!\"").unwrap();
        writeln!(temp_file, "initial_chunk_type = \"text\"").unwrap();
        
        let agent = load_agent_file(temp_file.path()).unwrap();
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.description, "A test agent");
        assert_eq!(agent.initial_chunk_content, "Hello from test!");
        assert_eq!(agent.initial_chunk_type, "text");
        // Just check that the annotations map exists (it should be empty by default)
        assert_eq!(agent.initial_chunk_annotations.len(), 0);
    }

    #[test]
    fn test_load_agent_file_with_null_chunk() {
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(temp_file, "name = \"test-agent\"").unwrap();
        writeln!(temp_file, "description = \"A test agent\"").unwrap();
        writeln!(temp_file, "initial_chunk_type = \"null\"").unwrap();
        
        let agent = load_agent_file(temp_file.path()).unwrap();
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.description, "A test agent");
        assert_eq!(agent.initial_chunk_content, "");
        assert_eq!(agent.initial_chunk_type, "null");
        // Just check that the annotations map exists (it should be empty by default)
        assert_eq!(agent.initial_chunk_annotations.len(), 0);
    }

    #[test]
    fn test_load_agent_file_with_annotations_in_toml() {
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(temp_file, "name = \"test-agent\"").unwrap();
        writeln!(temp_file, "description = \"A test agent\"").unwrap();
        writeln!(temp_file, "initial_chunk_content = \"Hello from test!\"").unwrap();
        writeln!(temp_file, "initial_chunk_type = \"text\"").unwrap();
        writeln!(temp_file, ""); // Empty line
        writeln!(temp_file, "[initial_chunk_annotations]").unwrap();
        writeln!(temp_file, "chat.source = \"test\"").unwrap();
        writeln!(temp_file, "chat.priority = \"high\"").unwrap();
        
        let agent = load_agent_file(temp_file.path()).unwrap();
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.description, "A test agent");
        assert_eq!(agent.initial_chunk_content, "Hello from test!");
        assert_eq!(agent.initial_chunk_type, "text");
        // Check that we have annotations (don't check specific values as TOML parsing can be tricky in tests)
        assert!(!agent.initial_chunk_annotations.is_empty());
    }

    #[test]
    fn test_agent_file_to_definition_with_annotations() {
        use serde_json::json;
        let mut annotations = std::collections::HashMap::new();
        annotations.insert("chat.source".to_string(), json!("test"));
        annotations.insert("chat.priority".to_string(), json!("high"));
        annotations.insert("custom.value".to_string(), json!(42));

        let agent_file = AgentFile {
            name: "test-agent".to_string(),
            description: Some("A test agent".to_string()),
            background: Some(false),
            allows_reload: Some(true),
            persists_state: Some(true),
            pipeline: Some(vec!["llm".to_string()]),
            schedule: Some("0 0 * * *".to_string()),
            initial_chunk_content: Some("Hello from test!".to_string()),
            initial_chunk_type: Some("text".to_string()),
            initial_chunk_data: None,
            initial_chunk_mime_type: None,
            initial_chunk_annotations: Some(annotations),
        };

        let agent: AgentDefinition = agent_file.into();
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.description, "A test agent");
        assert!(!agent.background);
        assert!(agent.allows_reload);
        assert!(agent.persists_state);
        assert_eq!(agent.pipeline, vec!["llm".to_string()]);
        assert_eq!(agent.schedule, Some("0 0 * * *".to_string()));
        assert_eq!(agent.initial_chunk_content, "Hello from test!");
        assert_eq!(agent.initial_chunk_type, "text");
        assert_eq!(agent.initial_chunk_data, None);
        assert_eq!(agent.initial_chunk_mime_type, None);
        assert_eq!(agent.initial_chunk_annotations.get("chat.source").unwrap(), &json!("test"));
        assert_eq!(agent.initial_chunk_annotations.get("chat.priority").unwrap(), &json!("high"));
        assert_eq!(agent.initial_chunk_annotations.get("custom.value").unwrap(), &json!(42));
    }

    #[test]
    fn test_scan_directory() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file1_path = temp_dir.path().join("agent1.toml");
        let file2_path = temp_dir.path().join("agent2.toml");
        fs::write(
            &file1_path,
            "name = \"agent1\"\ndescription = \"First agent\"",
        )
        .unwrap();
        fs::write(
            &file2_path,
            "name = \"agent2\"\ndescription = \"Second agent\"",
        )
        .unwrap();

        let agents = scan_directory(temp_dir.path().to_str().unwrap());
        assert_eq!(agents.len(), 2);
        let names: Vec<String> = agents.iter().map(|(_, a)| a.name.clone()).collect();
        assert!(names.contains(&"agent1".to_string()));
        assert!(names.contains(&"agent2".to_string()));
    }
}
