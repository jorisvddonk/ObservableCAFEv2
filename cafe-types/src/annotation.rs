/// Standard annotation key constants.
pub mod keys {
    pub const CHAT_ROLE: &str = "chat.role";
    pub const CHAT_MODEL: &str = "chat.model";
    pub const CHAT_FINISH_REASON: &str = "chat.finish_reason";
    pub const CHAT_TOKEN_COUNT: &str = "chat.token_count";
    pub const CHAT_IS_STREAMING: &str = "chat.is_streaming";
    pub const CHAT_STREAM_COMPLETE: &str = "chat.stream_complete";

    pub const SESSION_ID: &str = "session.id";
    pub const SESSION_NAME: &str = "session.name";

    pub const SECURITY_TRUST_LEVEL: &str = "security.trust-level";
    pub const SECURITY_REQUIRES_REVIEW: &str = "security.requires-review";

    pub const CONFIG_TYPE: &str = "config.type";
    pub const CONFIG_BACKEND: &str = "config.backend";
    pub const CONFIG_MODEL: &str = "config.model";
    pub const CONFIG_SYSTEM_PROMPT: &str = "config.system_prompt";
    pub const CONFIG_TEMPERATURE: &str = "config.temperature";
    pub const CONFIG_MAX_TOKENS: &str = "config.max_tokens";

    pub const WEB_SOURCE_URL: &str = "web.source_url";
    pub const WEB_CONTENT_TYPE: &str = "web.content_type";
    pub const WEB_FETCH_TIME: &str = "web.fetch_time";
    pub const WEB_ERROR: &str = "web.error";

    pub const TOOL_CALL: &str = "tool.call";
    pub const TOOL_RESULT: &str = "tool.result";

    pub const FLOW_SIGNAL: &str = "flow.signal";
    pub const FLOW_AGENT_ID: &str = "flow.agent_id";

    pub const ERROR_MESSAGE: &str = "error.message";
    pub const ERROR_CODE: &str = "error.code";
}

/// Standard chat role values.
pub mod roles {
    pub const USER: &str = "user";
    pub const ASSISTANT: &str = "assistant";
    pub const SYSTEM: &str = "system";
}
