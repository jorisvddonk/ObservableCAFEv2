use crate::backends::LlmMessage;
use cafe_types::{Chunk, ContentType};

/// Build an LLM message list from session history.
pub fn build_messages(history: &[Chunk], system_prompt: Option<&str>) -> Vec<LlmMessage> {
    let mut messages = Vec::new();

    if let Some(prompt) = system_prompt {
        messages.push(LlmMessage {
            role: "system".into(),
            content: prompt.into(),
        });
    }

    for chunk in history {
        if chunk.content_type != ContentType::Text {
            continue;
        }

        // Skip untrusted chunks
        if let Some(trust) = chunk.get_annotation::<serde_json::Value>("security.trust-level") {
            if trust["trusted"] == serde_json::Value::Bool(false) {
                continue;
            }
        }

        match chunk.role() {
            Some("user") | Some("assistant") => {
                messages.push(LlmMessage {
                    role: chunk.role().unwrap().into(),
                    content: chunk.content.clone().unwrap_or_default(),
                });
            }
            _ => {}
        }
    }

    messages
}

/// Extract the most recent runtime config from session history.
pub struct SessionConfig {
    pub backend: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

pub fn extract_config(history: &[Chunk]) -> SessionConfig {
    let mut cfg = SessionConfig {
        backend: None,
        model: None,
        system_prompt: None,
        temperature: None,
        max_tokens: None,
    };

    // Scan in reverse to find the most recent config chunk
    for chunk in history.iter().rev() {
        if chunk.is_runtime_config() {
            cfg.backend = chunk.get_annotation("config.backend");
            cfg.model = chunk.get_annotation("config.model");
            cfg.system_prompt = chunk.get_annotation("config.system_prompt");
            cfg.temperature = chunk.get_annotation("config.temperature");
            cfg.max_tokens = chunk.get_annotation("config.max_tokens");
            break;
        }
    }

    cfg
}
