use serde::{Deserialize, Serialize};

/// A parsed step from an agent TOML `[[steps]]` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDef {
    /// Unique identifier within the agent; used in trigger references.
    pub id: String,
    /// Evaluator type name: `llm`, `tts`, `comfy`, `trust-filter`, etc.
    #[serde(rename = "type")]
    pub step_type: String,
    /// When this step fires: `user_message`, `llm_complete`,
    /// `scheduler_tick`, or `step_complete:<id>`.
    pub trigger: String,
    /// Optional config annotation key; step is skipped unless the resolved
    /// config value is `true`.
    pub enabled_if: Option<String>,
}
