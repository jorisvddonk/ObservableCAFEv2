use axum::response::sse::Event;
use cafe_types::{Chunk, ServerMessage};
use std::convert::Infallible;

/// Convert a ServerMessage into an SSE Event.
pub fn message_to_event(msg: &ServerMessage) -> Option<Result<Event, Infallible>> {
    match msg {
        ServerMessage::Chunk { chunk, .. } => {
            let data = serde_json::to_string(chunk).ok()?;
            Some(Ok(Event::default().data(data)))
        }
        ServerMessage::HistoryComplete { count, .. } => {
            let data = serde_json::to_string(&serde_json::json!({
                "type": "history_complete",
                "count": count
            }))
            .ok()?;
            Some(Ok(Event::default().data(data)))
        }
        ServerMessage::Error { message, .. } => {
            let data = serde_json::to_string(&serde_json::json!({
                "type": "error",
                "message": message
            }))
            .ok()?;
            Some(Ok(Event::default().data(data)))
        }
        _ => None,
    }
}

/// Returns true if this chunk signals the end of a streaming LLM response.
#[allow(dead_code)]
pub fn is_stream_complete(chunk: &Chunk) -> bool {
    chunk
        .get_annotation::<bool>("chat.stream_complete")
        .unwrap_or(false)
}
