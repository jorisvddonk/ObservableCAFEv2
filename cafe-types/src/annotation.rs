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

    // Namespaced runtime config keys (used by resolve_session_config)
    pub const CONFIG_LLM_SYSTEM_PROMPT: &str = "config.llm.system_prompt";
    pub const CONFIG_LLM_TEMPERATURE: &str = "config.llm.temperature";
    pub const CONFIG_LLM_MAX_TOKENS: &str = "config.llm.max_tokens";
    pub const CONFIG_LLM_MODEL: &str = "config.llm.model";
    pub const CONFIG_LLM_BACKEND: &str = "config.llm.backend";

    pub const CONFIG_TTS_PROFILE: &str = "config.tts.profile";
    pub const CONFIG_TTS_ENGINE: &str = "config.tts.engine";
    pub const CONFIG_TTS_ENDPOINT: &str = "config.tts.endpoint";

    pub const CONFIG_COMFY_WORKFLOW_PATH: &str = "config.comfy.workflow_path";
    pub const CONFIG_COMFY_WORKFLOW_INPUT_NODE: &str = "config.comfy.workflow_input_node";
    pub const CONFIG_COMFY_ENDPOINT: &str = "config.comfy.endpoint";

    pub const CONFIG_SHEETBOT_URL: &str = "config.sheetbot.url";
    pub const CONFIG_SHEETBOT_API_KEY: &str = "config.sheetbot.api_key";

    pub const CONFIG_STT_BASE_URL: &str = "config.stt.base_url";
    pub const CONFIG_STT_RESPONSE_FORMAT: &str = "config.stt.response_format";

    pub const CONFIG_RSS_URL: &str = "config.rss.url";

    pub const CONFIG_SESSION_NAME: &str = "config.session.name";

    pub const WEB_SOURCE_URL: &str = "web.source_url";
    pub const WEB_CONTENT_TYPE: &str = "web.content_type";
    pub const WEB_FETCH_TIME: &str = "web.fetch_time";
    pub const WEB_ERROR: &str = "web.error";

    pub const TOOL_CALL: &str = "tool.call";
    pub const TOOL_RESULT: &str = "tool.result";

    pub const FLOW_SIGNAL: &str = "flow.signal";
    pub const FLOW_AGENT_ID: &str = "flow.agent_id";
    pub const FLOW_TOMBSTONE: &str = "flow.tombstone";
    pub const MUTATES_TARGET_ID: &str = "mutates.target_id";

    pub const ERROR_MESSAGE: &str = "error.message";
    pub const ERROR_CODE: &str = "error.code";

    // JSON-RPC over bus
    pub const JSONRPC_REQUEST: &str = "jsonrpc.request";
    pub const JSONRPC_RESPONSE: &str = "jsonrpc.response";

    // Transport properties
    pub const TRANSIENT: &str = "transient";
    pub const TRANSIENT_RETAIN_SECS: &str = "transient.retain_secs";
    pub const SOURCE_CONNECTION: &str = "source.connection";
    pub const DIRECT_TO: &str = "direct_to";
    pub const BINARY_WRITE_URL: &str = "binary.write_url";
    pub const BINARY_WRITE_TOKEN: &str = "binary.write_token";
    pub const BINARY_READ_URL: &str = "binary.read_url";
    pub const BINARY_READ_TOKEN: &str = "binary.read_token";
    pub const BINARY_BYTE_SIZE: &str = "binary.byte_size";

    // ── Cafe-namespaced (preferred) ──
    pub const CAFE_TRANSIENT: &str = "cafe.transient";
    pub const CAFE_TRANSIENT_RETAIN_SECS: &str = "cafe.transient.retain_secs";
    pub const CAFE_SOURCE_CONNECTION: &str = "cafe.source.connection";
    pub const CAFE_DIRECT_TO: &str = "cafe.direct_to";
    pub const CAFE_MUTATES_TARGET_ID: &str = "cafe.mutates.target_id";
    pub const CAFE_JSONRPC_REQUEST: &str = "cafe.jsonrpc.request";
    pub const CAFE_JSONRPC_RESPONSE: &str = "cafe.jsonrpc.response";
    pub const CAFE_BINARY_WRITE_URL: &str = "cafe.binary.write_url";
    pub const CAFE_BINARY_WRITE_TOKEN: &str = "cafe.binary.write_token";
    pub const CAFE_BINARY_READ_URL: &str = "cafe.binary.read_url";
    pub const CAFE_BINARY_READ_TOKEN: &str = "cafe.binary.read_token";
    pub const CAFE_BINARY_BYTE_SIZE: &str = "cafe.binary.byte_size";
    pub const CAFE_FLOW_SIGNAL: &str = "cafe.flow.signal";
    pub const CAFE_FLOW_AGENT_ID: &str = "cafe.flow.agent_id";
    pub const CAFE_FLOW_TOMBSTONE: &str = "cafe.flow.tombstone";
    pub const CAFE_TOOL_CALL: &str = "cafe.tool.call";
    pub const CAFE_TOOL_RESULT: &str = "cafe.tool.result";
    pub const CAFE_ERROR_MESSAGE: &str = "cafe.error.message";
    pub const CAFE_ERROR_CODE: &str = "cafe.error.code";
}

/// Standard chat role values.
pub mod roles {
    pub const USER: &str = "user";
    pub const ASSISTANT: &str = "assistant";
    pub const SYSTEM: &str = "system";
}
