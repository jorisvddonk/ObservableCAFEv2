use crate::binary_ref::serialize_chunk;
use axum::response::sse::Event;
use cafe_sdk::{Chunk, ServerMessage};
use std::convert::Infallible;

/// Convert a ServerMessage into an SSE Event (full binary, no substitution).
#[allow(dead_code)]
pub fn message_to_event(msg: &ServerMessage) -> Option<Result<Event, Infallible>> {
    message_to_event_with_refs(msg, false)
}

/// Convert a ServerMessage into an SSE Event, optionally substituting binary
/// chunks with lightweight binary-ref objects.
pub fn message_to_event_with_refs(
    msg: &ServerMessage,
    binary_refs: bool,
) -> Option<Result<Event, Infallible>> {
    match msg {
        ServerMessage::Chunk { chunk, .. } => {
            let data = serde_json::to_string(&serialize_chunk(chunk, binary_refs)).ok()?;
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
