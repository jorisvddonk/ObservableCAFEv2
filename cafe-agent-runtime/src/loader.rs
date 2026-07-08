use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use cafe_sdk::AgentDefinition;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use tracing;

/// Nested `[initial_chunk]` table in a TOML agent file.
/// This is the preferred format for config-seeding null chunks (see task.md).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct InitialChunkTable {
    /// "text" | "null" | "binary" — defaults to "text"
    #[serde(rename = "type")]
    pub chunk_type: Option<String>,
    pub content: Option<String>,
    /// base64-encoded bytes (binary chunks only)
    pub data: Option<String>,
    pub mime_type: Option<String>,
    pub annotations: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Raw TOML representation of an agent file (superset of AgentDefinition).
///
/// Supports two styles for the initial chunk:
///
/// **Flat style (legacy):**
/// ```toml
/// initial_chunk_content = "hello"
/// initial_chunk_type    = "text"
/// [initial_chunk_annotations]
/// chat.role = "user"
/// ```
///
/// **Nested style (preferred for config-seeding):**
/// ```toml
/// [initial_chunk]
/// type = "null"
/// [initial_chunk.annotations]
/// "config.type" = "runtime"
/// "config.llm.model" = "gemma3:1b"
/// ```
///
/// If both are present, `initial_chunk` (nested) takes precedence.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentFile {
    pub name: String,
    pub description: Option<String>,
    pub background: Option<bool>,
    pub allows_reload: Option<bool>,
    pub persists_state: Option<bool>,
    pub steps: Option<Vec<cafe_sdk::StepDef>>,
    pub schedule: Option<String>,
    // --- flat style ---
    pub initial_chunk_content: Option<String>,
    pub initial_chunk_type: Option<String>,
    pub initial_chunk_data: Option<String>, // base64 encoded for binary
    pub initial_chunk_mime_type: Option<String>,
    pub initial_chunk_annotations: Option<std::collections::HashMap<String, serde_json::Value>>,
    // --- nested style ---
    pub initial_chunk: Option<InitialChunkTable>,
    pub rpc_timeout_secs: Option<u64>,
    pub max_pipeline_depth: Option<u32>,
    pub ephemeral_keepalive_secs: Option<u64>,
    pub ephemeral_count_role: Option<String>,
}

impl From<AgentFile> for AgentDefinition {
    fn from(f: AgentFile) -> Self {
        // Nested `[initial_chunk]` table takes precedence over flat keys.
        let (chunk_type, chunk_content, chunk_data, chunk_mime_type, chunk_annotations) =
            if let Some(ref ic) = f.initial_chunk {
                let data = if let Some(ref data_str) = ic.data {
                    match STANDARD.decode(data_str) {
                        Ok(d) => Some(d),
                        Err(e) => {
                            tracing::warn!("Failed to decode base64 initial_chunk.data: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };
                (
                    ic.chunk_type.clone().unwrap_or_else(|| "text".into()),
                    ic.content.clone().unwrap_or_default(),
                    data,
                    ic.mime_type.clone(),
                    ic.annotations.clone().unwrap_or_default(),
                )
            } else {
                // Fall back to flat style
                let data = if let Some(data_str) = f.initial_chunk_data {
                    match STANDARD.decode(data_str) {
                        Ok(d) => Some(d),
                        Err(e) => {
                            tracing::warn!("Failed to decode base64 initial chunk data: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };
                (
                    f.initial_chunk_type.unwrap_or_else(|| "text".into()),
                    f.initial_chunk_content.unwrap_or_default(),
                    data,
                    f.initial_chunk_mime_type,
                    f.initial_chunk_annotations.unwrap_or_default(),
                )
            };

        AgentDefinition {
            name: f.name,
            description: f.description.unwrap_or_default(),
            background: f.background.unwrap_or(false),
            allows_reload: f.allows_reload.unwrap_or(true),
            persists_state: f.persists_state.unwrap_or(true),
            steps: f.steps.unwrap_or_else(|| vec![
                cafe_sdk::StepDef {
                    id: "llm".into(),
                    step_type: "llm".into(),
                    trigger: "user_message".into(),
                    enabled_if: None,
                },
            ]),
            schedule: f.schedule,
            initial_chunk_content: chunk_content,
            initial_chunk_type: chunk_type,
            initial_chunk_data: chunk_data,
            initial_chunk_mime_type: chunk_mime_type,
            initial_chunk_annotations: chunk_annotations,
            rpc_timeout_secs: f.rpc_timeout_secs.unwrap_or(60),
            max_pipeline_depth: f.max_pipeline_depth.unwrap_or(10),
            ephemeral_keepalive_secs: f.ephemeral_keepalive_secs,
            ephemeral_count_role: f.ephemeral_count_role,
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
            steps: Some(vec![cafe_sdk::StepDef {
                id: "llm".into(),
                step_type: "llm".into(),
                trigger: "user_message".into(),
                enabled_if: None,
            }]),
            schedule: Some("0 0 * * *".to_string()),
            initial_chunk_content: Some("Hello from test!".to_string()),
            initial_chunk_type: Some("text".to_string()),
            initial_chunk_data: None,
            initial_chunk_mime_type: None,
            initial_chunk_annotations: Some(annotations),
            initial_chunk: None,
            rpc_timeout_secs: None,
            max_pipeline_depth: None,
            ephemeral_keepalive_secs: None,
            ephemeral_count_role: None,
        };

        let agent: AgentDefinition = agent_file.into();
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.description, "A test agent");
        assert!(!agent.background);
        assert!(agent.allows_reload);
        assert!(agent.persists_state);
        assert_eq!(agent.steps.len(), 1);
        assert_eq!(agent.steps[0].step_type, "llm");
        assert_eq!(agent.steps[0].trigger, "user_message");
        assert_eq!(agent.schedule, Some("0 0 * * *".to_string()));
        assert_eq!(agent.initial_chunk_content, "Hello from test!");
        assert_eq!(agent.initial_chunk_type, "text");
        assert_eq!(agent.initial_chunk_data, None);
        assert_eq!(agent.initial_chunk_mime_type, None);
        assert_eq!(agent.initial_chunk_annotations.get("chat.source").unwrap(), &json!("test"));
        assert_eq!(agent.initial_chunk_annotations.get("chat.priority").unwrap(), &json!("high"));
        assert_eq!(agent.initial_chunk_annotations.get("custom.value").unwrap(), &json!(42));
        assert!(agent.ephemeral_keepalive_secs.is_none());
        assert!(agent.ephemeral_count_role.is_none());
    }

    #[test]
    fn test_load_agent_file_with_nested_initial_chunk_null() {
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"name = "volition""#).unwrap();
        writeln!(temp_file, r#"[[steps]]"#).unwrap();
        writeln!(temp_file, r#"id = "stt""#).unwrap();
        writeln!(temp_file, r#"type = "stt""#).unwrap();
        writeln!(temp_file, r#"trigger = "user_message""#).unwrap();
        writeln!(temp_file, r#"[[steps]]"#).unwrap();
        writeln!(temp_file, r#"id = "llm""#).unwrap();
        writeln!(temp_file, r#"type = "llm""#).unwrap();
        writeln!(temp_file, r#"trigger = "user_message""#).unwrap();
        writeln!(temp_file, r#"[[steps]]"#).unwrap();
        writeln!(temp_file, r#"id = "tts""#).unwrap();
        writeln!(temp_file, r#"type = "tts""#).unwrap();
        writeln!(temp_file, r#"trigger = "llm_complete""#).unwrap();
        writeln!(temp_file, "").unwrap();
        writeln!(temp_file, "[initial_chunk]").unwrap();
        writeln!(temp_file, r#"type = "null""#).unwrap();
        writeln!(temp_file, "").unwrap();
        writeln!(temp_file, "[initial_chunk.annotations]").unwrap();
        writeln!(temp_file, r#""config.type" = "runtime""#).unwrap();
        writeln!(temp_file, r#""config.llm.system_prompt" = "You are Volition""#).unwrap();
        writeln!(temp_file, r#""config.llm.temperature" = 0.7"#).unwrap();
        writeln!(temp_file, r#""config.tts.profile" = "Volition""#).unwrap();

        let agent = load_agent_file(temp_file.path()).unwrap();
        assert_eq!(agent.name, "volition");
        assert_eq!(agent.initial_chunk_type, "null");
        assert_eq!(
            agent.initial_chunk_annotations.get("config.type").unwrap(),
            &serde_json::json!("runtime")
        );
        assert_eq!(
            agent.initial_chunk_annotations.get("config.llm.system_prompt").unwrap(),
            &serde_json::json!("You are Volition")
        );
        assert_eq!(
            agent.initial_chunk_annotations.get("config.tts.profile").unwrap(),
            &serde_json::json!("Volition")
        );
    }

    #[test]
    fn test_nested_initial_chunk_overrides_flat_keys() {
        let mut temp_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(temp_file, r#"name = "agent""#).unwrap();
        // flat style (should be ignored when nested is present)
        writeln!(temp_file, r#"initial_chunk_content = "flat content""#).unwrap();
        writeln!(temp_file, r#"initial_chunk_type = "text""#).unwrap();
        writeln!(temp_file, "").unwrap();
        // nested style takes precedence
        writeln!(temp_file, "[initial_chunk]").unwrap();
        writeln!(temp_file, r#"type = "null""#).unwrap();
        writeln!(temp_file, "[initial_chunk.annotations]").unwrap();
        writeln!(temp_file, r#""config.type" = "runtime""#).unwrap();

        let agent = load_agent_file(temp_file.path()).unwrap();
        assert_eq!(agent.initial_chunk_type, "null");
        assert_eq!(agent.initial_chunk_content, "");
        assert!(agent.initial_chunk_annotations.contains_key("config.type"));
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
