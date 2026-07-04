use serde_json::{json, Value};
use std::sync::LazyLock;

/// A registered MCP tool that maps to a bus RPC or inline handler.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    /// If Some, dispatch via bus RPC. If None, handled inline.
    pub rpc_method: Option<&'static str>,
}

pub(crate) static TOOLS: LazyLock<Vec<ToolDef>> = LazyLock::new(|| vec![
    // ── Knowledge Base ──
    ToolDef {
        name: "kb_search",
        description: "Search indexed documents by semantic similarity",
        input_schema: json!({
            "type": "object",
            "properties": {
                "namespace": {"type": "string", "description": "Namespace to search"},
                "query": {"type": "string", "description": "Search query"},
                "k": {"type": "integer", "description": "Number of results (default 5)"}
            },
            "required": ["namespace", "query"]
        }),
        rpc_method: Some("knowledgebase.search"),
    },
    ToolDef {
        name: "kb_search_context",
        description: "Search indexed documents and return neighboring chunks for context",
        input_schema: json!({
            "type": "object",
            "properties": {
                "namespace": {"type": "string", "description": "Namespace to search"},
                "query": {"type": "string", "description": "Search query"},
                "k": {"type": "integer", "description": "Number of results (default 5)"},
                "context_chunks": {"type": "integer", "description": "Neighbors per result (default 2)"}
            },
            "required": ["namespace", "query"]
        }),
        rpc_method: Some("knowledgebase.search_with_context"),
    },
    ToolDef {
        name: "kb_index",
        description: "Index a document into the knowledge base (full + chunks automatically)",
        input_schema: json!({
            "type": "object",
            "properties": {
                "namespace": {"type": "string", "description": "Namespace to store in"},
                "text": {"type": "string", "description": "Document text content"},
                "doc_id": {"type": "string", "description": "Optional document ID (auto-generated if omitted)"},
                "metadata": {"type": "string", "description": "Optional JSON metadata string"},
                "chunk_size": {"type": "integer", "description": "Chunk size in chars (default 512)"},
                "chunk_overlap": {"type": "integer", "description": "Chunk overlap in chars (default 64)"}
            },
            "required": ["namespace", "text"]
        }),
        rpc_method: Some("knowledgebase.index"),
    },
    ToolDef {
        name: "kb_list",
        description: "List all documents in a namespace",
        input_schema: json!({
            "type": "object",
            "properties": {
                "namespace": {"type": "string", "description": "Namespace to list"}
            },
            "required": ["namespace"]
        }),
        rpc_method: Some("knowledgebase.list"),
    },
    ToolDef {
        name: "kb_delete",
        description: "Delete a document and its chunks from the knowledge base",
        input_schema: json!({
            "type": "object",
            "properties": {
                "namespace": {"type": "string", "description": "Namespace containing the document"},
                "doc_id": {"type": "string", "description": "Document ID to delete"}
            },
            "required": ["namespace", "doc_id"]
        }),
        rpc_method: Some("knowledgebase.delete"),
    },
    // ── Speech ──
    ToolDef {
        name: "stt_transcribe",
        description: "Transcribe audio to text using voicebox",
        input_schema: json!({
            "type": "object",
            "properties": {
                "audio": {"type": "string", "description": "Base64-encoded WAV audio data"},
                "mime_type": {"type": "string", "description": "MIME type (default audio/wav)"},
                "language": {"type": "string", "description": "Language code (default en)"},
                "model": {"type": "string", "description": "Model name (default whisper-small)"}
            },
            "required": ["audio"]
        }),
        rpc_method: Some("stt.invoke"),
    },
    ToolDef {
        name: "tts_synthesize",
        description: "Synthesize text to speech using voicebox (returns chunk_id of generated audio)",
        input_schema: json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Text to synthesize"},
                "profile": {"type": "string", "description": "Voice profile name (default default)"},
                "engine": {"type": "string", "description": "TTS engine (default qwen)"}
            },
            "required": ["text"]
        }),
        rpc_method: Some("tts.invoke"),
    },
    // ── Dice ──
    ToolDef {
        name: "dice_roll",
        description: "Roll dice and return the total",
        input_schema: json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer", "description": "Number of dice (default 1)"},
                "sides": {"type": "integer", "description": "Sides per die (default 6)"}
            },
            "required": []
        }),
        rpc_method: Some("dice.roll"),
    },
    // ── Web Fetch (inline) ──
    ToolDef {
        name: "web_fetch",
        description: "Fetch a URL and return the text content with HTML stripped. Handled inline by cafe-mcp-bridge.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "URL to fetch"}
            },
            "required": ["url"]
        }),
        rpc_method: None, // inline
    },
    // ── SheetBot ──
    ToolDef {
        name: "sheetbot_list_tasks",
        description: "List all SheetBot tasks",
        input_schema: json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        rpc_method: Some("sheetbot.list_tasks"),
    },
    ToolDef {
        name: "sheetbot_get_task",
        description: "Get a SheetBot task by ID",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task ID"}
            },
            "required": ["id"]
        }),
        rpc_method: Some("sheetbot.get_task"),
    },
    ToolDef {
        name: "sheetbot_create_task",
        description: "Create a new SheetBot task",
        input_schema: json!({
            "type": "object",
            "properties": {
                "script": {"type": "string", "description": "Sheet script content"},
                "name": {"type": "string", "description": "Optional task name"},
                "type": {"type": "string", "description": "Optional task type"},
                "data": {"type": "string", "description": "Optional JSON data string"}
            },
            "required": ["script"]
        }),
        rpc_method: Some("sheetbot.create_task"),
    },
    ToolDef {
        name: "sheetbot_update_task",
        description: "Update an existing SheetBot task",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task ID"},
                "name": {"type": "string", "description": "Optional new name"},
                "data": {"type": "string", "description": "Optional new data JSON"}
            },
            "required": ["id"]
        }),
        rpc_method: Some("sheetbot.update_task"),
    },
    ToolDef {
        name: "sheetbot_complete_task",
        description: "Mark a SheetBot task as complete",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task ID"}
            },
            "required": ["id"]
        }),
        rpc_method: Some("sheetbot.complete_task"),
    },
    ToolDef {
        name: "sheetbot_fail_task",
        description: "Mark a SheetBot task as failed",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task ID"}
            },
            "required": ["id"]
        }),
        rpc_method: Some("sheetbot.fail_task"),
    },
    ToolDef {
        name: "sheetbot_delete_task",
        description: "Delete a SheetBot task",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task ID"}
            },
            "required": ["id"]
        }),
        rpc_method: Some("sheetbot.delete_task"),
    },
    ToolDef {
        name: "sheetbot_accept_task",
        description: "Accept a SheetBot task for processing",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task ID"}
            },
            "required": ["id"]
        }),
        rpc_method: Some("sheetbot.accept_task"),
    },
    ToolDef {
        name: "sheetbot_clone_task",
        description: "Clone an existing SheetBot task",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Task ID to clone"}
            },
            "required": ["id"]
        }),
        rpc_method: Some("sheetbot.clone_task"),
    },
    ToolDef {
        name: "sheetbot_get_next_task",
        description: "Get the next pending SheetBot task",
        input_schema: json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        rpc_method: Some("sheetbot.get_next_task"),
    },
    ToolDef {
        name: "sheetbot_list_sheets",
        description: "List all SheetBot sheets",
        input_schema: json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        rpc_method: Some("sheetbot.list_sheets"),
    },
    ToolDef {
        name: "sheetbot_get_sheet",
        description: "Get a SheetBot sheet by ID",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Sheet ID"}
            },
            "required": ["id"]
        }),
        rpc_method: Some("sheetbot.get_sheet"),
    },
    ToolDef {
        name: "sheetbot_upsert_sheet_data",
        description: "Upsert data into a SheetBot sheet",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Sheet ID"},
                "key": {"type": "string", "description": "Row key"},
                "data": {"type": "string", "description": "JSON data string to store"}
            },
            "required": ["id", "key", "data"]
        }),
        rpc_method: Some("sheetbot.upsert_sheet_data"),
    },
    ToolDef {
        name: "sheetbot_delete_sheet_row",
        description: "Delete a row from a SheetBot sheet by key",
        input_schema: json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Sheet ID"},
                "key": {"type": "string", "description": "Row key to delete"}
            },
            "required": ["id", "key"]
        }),
        rpc_method: Some("sheetbot.delete_sheet_row"),
    },
    ToolDef {
        name: "sheetbot_list_library",
        description: "List all SheetBot library scripts",
        input_schema: json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        rpc_method: Some("sheetbot.list_library"),
    },
    // ── Comfy ──
    ToolDef {
        name: "comfy_generate",
        description: "Generate an image using ComfyUI from a text prompt",
        input_schema: json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "Image generation prompt"},
                "workflow_path": {"type": "string", "description": "Optional workflow path"},
                "input_node": {"type": "string", "description": "Optional input node ID"}
            },
            "required": ["text"]
        }),
        rpc_method: Some("comfy.invoke"),
    },
]);

/// Simple glob match for tool name filtering.
/// Supports `*` (any chars) and `?` (single char).
pub fn matches_pattern(name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let pat_chars: Vec<char> = pattern.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    glob_match(&pat_chars, &name_chars, 0, 0)
}

fn glob_match(pat: &[char], name: &[char], pi: usize, ni: usize) -> bool {
    if pi == pat.len() {
        return ni == name.len();
    }
    if ni == name.len() {
        return pat[pi..].iter().all(|&c| c == '*');
    }
    match pat[pi] {
        '*' => {
            // * matches zero or more chars
            glob_match(pat, name, pi + 1, ni)
                || glob_match(pat, name, pi, ni + 1)
        }
        '?' => glob_match(pat, name, pi + 1, ni + 1),
        c if c == name[ni] => glob_match(pat, name, pi + 1, ni + 1),
        _ => false,
    }
}

/// Filter tools by patterns. Empty patterns = all tools.
pub fn filter_tools(patterns: &[String]) -> Vec<&'static ToolDef> {
    let tools = &*TOOLS;
    if patterns.is_empty() {
        return tools.iter().collect();
    }
    tools
        .iter()
        .filter(|t| patterns.iter().any(|p| matches_pattern(t.name, p)))
        .collect()
}
