use crate::config::TtsBackend;
use crate::voicebox::VoiceboxClient;
use anyhow::Context;
use tracing::warn;

/// Max chars per /speak request. The speech-server process crashes on long
/// inputs (observed: 377 chars OK, 410 chars kills the server, which the
/// proxy then surfaces as 502 "Remote end closed connection without
/// response"). Longer texts are split into sentence chunks and the resulting
/// PCM is concatenated into a single WAV. See ADR-126.
pub const MAX_CHUNK_CHARS: usize = 300;

/// Split text into chunks of at most `max_chars` (char count), keeping
/// sentences whole when possible. A single sentence longer than the budget
/// is hard-split, preferring a word boundary.
pub fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut sentences: Vec<&str> = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'.' || b == b'!' || b == b'?' {
            let mut end = i + 1;
            // Keep closing quotes/brackets with the sentence.
            while end < bytes.len() && matches!(bytes[end], b'"' | b'\'' | b')' | b']') {
                end += 1;
            }
            if end >= bytes.len() || bytes[end].is_ascii_whitespace() {
                sentences.push(text[start..end].trim());
                start = end;
                i = end;
                continue;
            }
        }
        // Step by char to stay on UTF-8 boundaries.
        i += text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        sentences.push(tail);
    }
    sentences.retain(|s| !s.is_empty());

    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    let flush = |current: &mut String, chunks: &mut Vec<String>| {
        if !current.is_empty() {
            chunks.push(std::mem::take(current));
        }
    };
    for s in sentences {
        let s_len = s.chars().count();
        if s_len > max_chars {
            flush(&mut current, &mut chunks);
            current_len = 0;
            // Hard-split the overlong sentence, preferring word boundaries.
            let mut rest = s;
            while rest.chars().count() > max_chars {
                let split_at = rest
                    .char_indices()
                    .take(max_chars + 1)
                    .last()
                    .map(|(i, _)| i)
                    .unwrap_or(rest.len());
                let mut at = split_at;
                if let Some(space) = rest[..split_at].rfind(' ') {
                    if space > 0 {
                        at = space;
                    }
                }
                chunks.push(rest[..at].trim().to_string());
                rest = rest[at..].trim_start();
            }
            if !rest.is_empty() {
                current.push_str(rest);
                current_len = rest.chars().count();
            }
            continue;
        }
        if current_len > 0 && current_len + 1 + s_len > max_chars {
            flush(&mut current, &mut chunks);
            current_len = 0;
        }
        if !current.is_empty() {
            current.push(' ');
            current_len += 1;
        }
        current.push_str(s);
        current_len += s_len;
    }
    flush(&mut current, &mut chunks);
    chunks
}

/// Parsed PCM WAV: raw `fmt ` payload plus sample data.
struct PcmWav {
    fmt: Vec<u8>,
    data: Vec<u8>,
}

/// Split a WAV response into its `fmt ` payload and sample data.
/// Only plain PCM (`audio_format == 1`) with a standard layout
/// (`RIFF....WAVEfmt <16>...data....`) is accepted.
fn split_wav(bytes: &[u8]) -> anyhow::Result<PcmWav> {
    if bytes.len() < 44
        || &bytes[0..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
        || &bytes[12..16] != b"fmt "
    {
        anyhow::bail!("unexpected WAV layout (not RIFF/WAVE/fmt)");
    }
    let fmt_len = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
    if fmt_len < 16 || bytes.len() < 20 + fmt_len + 8 {
        anyhow::bail!("unexpected WAV fmt chunk");
    }
    if u16::from_le_bytes(bytes[20..22].try_into().unwrap()) != 1 {
        anyhow::bail!("only PCM WAV concatenation is supported");
    }
    let data_at = 20 + fmt_len;
    if &bytes[data_at..data_at + 4] != b"data" {
        anyhow::bail!("unexpected WAV layout (data chunk not where expected)");
    }
    let data_len = u32::from_le_bytes(bytes[data_at + 4..data_at + 8].try_into().unwrap()) as usize;
    let data_start = data_at + 8;
    if bytes.len() < data_start + data_len {
        anyhow::bail!("truncated WAV data");
    }
    Ok(PcmWav {
        fmt: bytes[20..20 + fmt_len].to_vec(),
        data: bytes[data_start..data_start + data_len].to_vec(),
    })
}

/// Reassemble one WAV from a shared `fmt ` payload and concatenated data.
fn join_wav(fmt: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(20 + fmt.len() + 8 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + fmt.len() as u32 - 16 + data.len() as u32).to_le_bytes()));
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    out.extend_from_slice(fmt);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// HTTP client for speech-server's POST /speak endpoint (routed through proxy).
#[derive(Clone)]
pub struct SpeechServerClient {
    pub base_url: String,
}

impl SpeechServerClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    pub async fn synthesize(
        &self,
        text: &str,
        engine: Option<&str>,
        _language: Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        if text.is_empty() {
            anyhow::bail!("synthesize: text is empty");
        }

        if text.chars().count() <= MAX_CHUNK_CHARS {
            return self.synthesize_single(text, engine, _language).await;
        }

        // Long input: synthesize sentence chunks separately (the server
        // crashes past ~390 chars) and concatenate the PCM into one WAV.
        let chunks = chunk_text(text, MAX_CHUNK_CHARS);
        tracing::info!(
            "cafe-tts: chunking {}-char speak into {} requests",
            text.chars().count(),
            chunks.len()
        );
        let mut fmt: Option<Vec<u8>> = None;
        let mut data = Vec::new();
        for chunk in &chunks {
            let (wav, _) = self.synthesize_single(chunk, engine, _language).await?;
            let part = split_wav(&wav)?;
            match &fmt {
                Some(f) if *f == part.fmt => {}
                Some(_) => anyhow::bail!("chunked speak returned mismatched WAV formats"),
                None => fmt = Some(part.fmt),
            }
            data.extend_from_slice(&part.data);
        }
        Ok((join_wav(&fmt.unwrap_or_default(), &data), "audio/wav".to_string()))
    }

    async fn synthesize_single(
        &self,
        text: &str,
        engine: Option<&str>,
        _language: Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        let url = format!("{}/speak", self.base_url.trim_end_matches('/'));

        let mut body = serde_json::json!({ "text": text });
        if let Some(e) = engine {
            body["engine"] = serde_json::Value::String(e.to_string());
        } else {
            body["model"] = serde_json::Value::String("cosyvoice-3".into());
        }
        if let Some(l) = _language {
            body["language"] = serde_json::Value::String(l.to_string());
        }

        // Fresh connection per request: the proxy closes client-facing
        // keep-alive connections after proxying a response, so a pooled
        // connection fails the next sequential POST with a reset (ADR-126).
        // Localhost setup cost is negligible next to synthesis latency.
        let response = reqwest::Client::new()
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("POST /speak request failed")?
            .error_for_status()
            .context("POST /speak returned error status")?;

        let mime = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("audio/wav")
            .to_string();

        let audio_bytes = response
            .bytes()
            .await
            .context("Failed to read response body")?
            .to_vec();

        if audio_bytes.is_empty() {
            anyhow::bail!("POST /speak returned an empty body");
        }

        Ok((audio_bytes, mime))
    }
}

/// Owns both TTS backends and dispatches per request.
///
/// The backend is selected per session from `config.tts.backend`
/// (with an optional `config.tts.endpoint` URL override), falling back to
/// the process-level default when a session does not specify one.
pub struct TtsService {
    voicebox: VoiceboxClient,
    speech_server: SpeechServerClient,
    default_backend: TtsBackend,
}

impl TtsService {
    pub fn new(
        voicebox: VoiceboxClient,
        speech_server: SpeechServerClient,
        default_backend: TtsBackend,
    ) -> Self {
        Self {
            voicebox,
            speech_server,
            default_backend,
        }
    }

    pub fn default_backend(&self) -> TtsBackend {
        self.default_backend
    }

    pub async fn synthesize(
        &self,
        backend: Option<&str>,
        endpoint: Option<&str>,
        text: &str,
        profile: &str,
        engine: Option<&str>,
        language: Option<&str>,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        let backend = match backend {
            Some(raw) if !raw.trim().is_empty() => match TtsBackend::parse(raw) {
                Some(b) => b,
                None => {
                    warn!(
                        "cafe-tts: unknown backend {:?}, using default {:?}",
                        raw, self.default_backend
                    );
                    self.default_backend
                }
            },
            _ => self.default_backend,
        };

        match backend {
            TtsBackend::Voicebox => {
                let mut client = self.voicebox.clone();
                if let Some(e) = endpoint {
                    if !e.trim().is_empty() {
                        client.base_url = e.trim().to_string();
                    }
                }
                client.synthesize(text, profile, engine).await
            }
            TtsBackend::SpeechServer => {
                let mut client = self.speech_server.clone();
                if let Some(e) = endpoint {
                    if !e.trim().is_empty() {
                        client.base_url = e.trim().to_string();
                    }
                }
                client.synthesize(text, engine, language).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_wav(samples: &[i16]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + samples.len() as u32 * 2).to_le_bytes()));
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        // PCM mono 24kHz 16-bit
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24000u32.to_le_bytes());
        out.extend_from_slice(&48000u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&((samples.len() as u32 * 2).to_le_bytes()));
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn chunk_short_text_is_single_chunk() {
        let out = chunk_text("Hello world.", 300);
        assert_eq!(out, vec!["Hello world."]);
    }

    #[test]
    fn chunk_empty_text_is_empty() {
        assert!(chunk_text("", 300).is_empty());
        assert!(chunk_text("   ", 300).is_empty());
    }

    #[test]
    fn chunk_long_text_splits_on_sentences_within_budget() {
        let text = "First sentence here. Second sentence there! Third one? Fourth follows. Fifth ends.";
        let out = chunk_text(text, 40);
        assert!(out.len() >= 2, "got {out:?}");
        for c in &out {
            assert!(c.chars().count() <= 40, "chunk too long: {c:?}");
        }
        // No words lost.
        let joined = out.join(" ");
        for w in text.split_whitespace() {
            assert!(joined.contains(w), "lost word {w:?}");
        }
    }

    #[test]
    fn chunk_overlong_sentence_is_hard_split() {
        let text = "word ".repeat(100);
        let out = chunk_text(&text, 50);
        assert!(out.len() > 1);
        for c in &out {
            assert!(c.chars().count() <= 50, "chunk too long: {c:?}");
        }
    }

    #[test]
    fn chunk_realistic_length_text_stays_within_speak_budget() {
        // Regression: ~440-char assistant replies crashed speech-server.
        let text = "This is a sample passage used only to exercise the text chunking logic. It contains several complete sentences so that the splitter has natural boundaries to work with. The content itself is plainly written and carries no meaning beyond its length and punctuation. By keeping the wording ordinary we make the test easy to read and easy to maintain over time. Reviewers can adjust or extend this passage whenever the chunking rules change.";
        assert!(text.chars().count() > MAX_CHUNK_CHARS);
        let out = chunk_text(text, MAX_CHUNK_CHARS);
        assert!(out.len() >= 2);
        for c in &out {
            assert!(c.chars().count() <= MAX_CHUNK_CHARS, "chunk too long: {c:?}");
        }
    }

    #[test]
    fn wav_split_join_roundtrip() {
        let wav = make_wav(&[1, -2, 300, 4000]);
        let part = split_wav(&wav).unwrap();
        assert_eq!(part.data.len(), 8);
        let joined = join_wav(&part.fmt, &[part.data.clone(), part.data.clone()].concat());
        let back = split_wav(&joined).unwrap();
        assert_eq!(back.data.len(), 16);
        assert_eq!(back.fmt, part.fmt);
        // RIFF size field must equal file_len - 8.
        let size = u32::from_le_bytes(joined[4..8].try_into().unwrap()) as usize;
        assert_eq!(size, joined.len() - 8);
    }

    #[test]
    fn wav_split_rejects_garbage() {
        assert!(split_wav(b"not a wav file at all, way too short.....").is_err());
        assert!(split_wav(&vec![0u8; 100]).is_err());
        // Non-PCM format tag must be rejected.
        let mut wav = make_wav(&[1, 2]);
        wav[20] = 3;
        assert!(split_wav(&wav).is_err());
    }
}
