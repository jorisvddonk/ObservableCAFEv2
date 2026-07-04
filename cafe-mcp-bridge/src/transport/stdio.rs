use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{handle_mcp_request, AppState};

pub async fn run(state: Arc<AppState>, tool_patterns: Vec<String>) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = tokio::io::stdout();
    let patterns = if tool_patterns.iter().any(|p| p == "*") {
        None
    } else {
        Some(tool_patterns)
    };

    while let Ok(Some(line)) = reader.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("invalid JSON-RPC: {}", e);
                continue;
            }
        };

        if let Some(resp) = handle_mcp_request(&req, &state, patterns.as_deref()).await {
            let mut buf = serde_json::to_string(&resp)?;
            buf.push('\n');
            writer.write_all(buf.as_bytes()).await?;
            writer.flush().await?;
        }
    }

    Ok(())
}
