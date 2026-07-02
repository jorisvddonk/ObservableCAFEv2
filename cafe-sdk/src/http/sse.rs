use cafe_types::Chunk;

/// Parse one SSE `data: {...}\n` chunk from a byte buffer.
///
/// Removes the consumed bytes on success. Returns `None` if no complete SSE
/// chunk is available.
pub fn try_parse_sse_chunk(buffer: &mut String) -> Option<Chunk> {
    let data_prefix = "data: ";
    if let Some(start) = buffer.find(data_prefix) {
        let rest = &buffer[start + data_prefix.len()..];
        if let Some(end) = rest.find('\n') {
            let json_str = &rest[..end];
            let consumed = start + data_prefix.len() + end + 1;
            if let Ok(chunk) = serde_json::from_str::<Chunk>(json_str) {
                buffer.drain(..consumed);
                return Some(chunk);
            }
            buffer.drain(..consumed);
        }
    }
    None
}
