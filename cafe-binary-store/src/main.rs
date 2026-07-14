mod bus;
mod config;
mod db;
mod gc;
mod jwt;
mod storage;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use cafe_sdk::{bus::BusClient, keys};
use clap::Parser;
use serde::Deserialize;
use std::sync::Arc;
use tracing::{error, info, warn};

#[derive(Clone)]
struct AppState {
    storage: Arc<storage::Storage>,
    db: Arc<db::Db>,
    jwt_key: Arc<Vec<u8>>,
    config: config::Config,
    #[allow(dead_code)]
    _bus: BusClient,
}

#[derive(Deserialize)]
struct TokenQuery {
    token: String,
}

#[derive(Deserialize)]
struct WriteQuery {
    token: String,
    offset: Option<u64>,
    session_id: Option<String>,
    /// Optional explicit finalize marker for multi-part uploads.
    /// `true` finalizes now, `false` keeps the upload open for more parts.
    /// When absent, a single-shot write (offset=0 with a non-empty body) is
    /// finalized immediately for backward compatibility.
    finalize: Option<bool>,
}

/// Decide whether an upload should be finalized (`.writing` removed) after this
/// part. Extracted from `write_handler` so the decision is unit-testable.
///
/// - An empty-body request is a terminator and always finalizes.
/// - An explicit `finalize` flag is honored as-is.
/// - Otherwise (no marker): a single-shot write (offset==0 with data) finalizes,
///   but a multi-part upload's first part (offset==0 with data) must NOT — the
///   client must pass `finalize=false` on every non-final part.
pub(crate) fn should_finalize(offset: u64, body_empty: bool, explicit: Option<bool>) -> bool {
    if body_empty {
        true
    } else {
        match explicit {
            Some(v) => v,
            None => offset == 0,
        }
    }
}

/// Build the `Content-Range` header for a partial read, or `None` for a full
/// read (offset==0). Extracted from `read_handler` so the byte math is
/// unit-testable and cannot underflow on empty/EOF reads.
pub(crate) fn content_range_header(offset: u64, data_len: u64, file_size: u64) -> Option<String> {
    if offset > 0 {
        let data_end = offset + data_len.saturating_sub(1);
        Some(format!("bytes {}-{}/{}", offset, data_end, file_size))
    } else {
        None
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cfg = config::Config::parse();

    // Ensure data dir exists
    std::fs::create_dir_all(&cfg.data_dir)?;

    // Load JWT signing key
    let jwt_key = Arc::new(jwt::load_key(&cfg.data_dir)?);

    // Initialize storage
    let storage = Arc::new(storage::Storage::new(cfg.data_dir.clone()));

    // Initialize GC tracking DB
    let db = Arc::new(db::Db::connect(&cfg.data_dir).await?);

    // Create bus client for HTTP handlers (bus subscriber creates its own)
    let bus = BusClient::unix(&cfg.bus_socket);

    // Spawn bus subscriber (BinaryRef chunks + session events)
    let bus_cfg = cfg.clone();
    let bus_storage = storage.clone();
    let bus_db = db.clone();
    let bus_key = jwt_key.clone();
    tokio::spawn(async move {
        bus::run(bus_cfg, bus_storage, bus_db, bus_key).await;
    });

    // Spawn GC loop
    let gc_storage = storage.clone();
    let gc_db = db.clone();
    let gc_interval = cfg.gc_interval;
    let gc_ttl = cfg.gc_ttl;
    tokio::spawn(async move {
        gc::run_gc_loop(gc_storage, gc_db, gc_interval, gc_ttl).await;
    });

    // Build Axum app
    let state = AppState {
        storage,
        db,
        jwt_key,
        config: cfg.clone(),
        _bus: bus,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/binary/:chunk_id", get(read_handler).post(write_handler).delete(delete_handler))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", cfg.port);
    info!("cafe-binary-store: listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

/// POST /api/binary/{chunk_id}?token=<write_jwt>[&offset=N]
///
/// Stream write with optional resume. The body is read as raw bytes.
/// On success, publishes a broadcast mutation with read credentials.
async fn write_handler(
    State(state): State<AppState>,
    Path(chunk_id): Path<String>,
    Query(query): Query<WriteQuery>,
    body: axum::body::Bytes,
) -> Response {
    // Verify write JWT
    let claims = match jwt::verify(&query.token, &state.jwt_key) {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": e.to_string()}))).into_response();
        }
    };
    if claims.purpose != "write" {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "token purpose must be 'write'"}))).into_response();
    }
    if claims.chunk_id != chunk_id {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "token chunk_id mismatch"}))).into_response();
    }

    let offset = query.offset.unwrap_or(0);
    let is_new = offset == 0;

    // Start or resume write
    if is_new {
        if let Err(e) = state.storage.start_write(&chunk_id).await {
            let msg = e.to_string();
            return if msg.starts_with("CONFLICT") {
                (StatusCode::CONFLICT, Json(serde_json::json!({"error": msg}))).into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": msg}))).into_response()
            };
        }
    } else {
        if let Err(e) = state.storage.resume_write(&chunk_id, offset).await {
            let msg = e.to_string();
            return if msg.starts_with("NOT_FOUND") {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": msg}))).into_response()
            } else {
                (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": msg}))).into_response()
            };
        }
    }

    // Write bytes
    if let Err(e) = state
        .storage
        .append(&chunk_id, offset, &body, state.config.max_chunk_bytes)
        .await
    {
        let msg = e.to_string();
        return if msg.starts_with("PAYLOAD_TOO_LARGE") {
            (StatusCode::PAYLOAD_TOO_LARGE, Json(serde_json::json!({"error": msg}))).into_response()
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": msg}))).into_response()
        };
    }

    // Publish read credentials mutation on first byte (so consumers can start streaming)
    if is_new {
        // The producer must include session_id query param so we can publish the read mutation
        if let Some(ref sid) = query.session_id {
            let read_token = match jwt::sign_read(&chunk_id, &state.jwt_key) {
                Ok(t) => t,
                Err(e) => {
                    error!("failed to sign read JWT: {e}");
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "internal error"}))).into_response();
                }
            };
            let host = state.config.public_host.clone().unwrap_or_else(|| "localhost".into());
            let read_url = format!(
                "http://{}:{}/api/binary/{}",
                host, state.config.port, chunk_id
            );
            let mut mutation = cafe_sdk::Chunk::mutation(&chunk_id, "com.nominal.cafe-binary-store");
            mutation = mutation
                .with_annotation(keys::CAFE_BINARY_READ_URL, &read_url)
                .with_annotation(keys::CAFE_BINARY_READ_TOKEN, &read_token);
            // Publish broadcast mutation (no direct_to — all subscribers see it)
            if let Err(e) = state._bus.publish(sid, mutation).await {
                warn!("cafe-binary-store: failed to publish read credentials: {e}");
            }
        }
    }

    // Decide whether to finalize now. A single-shot write (offset=0 with data,
    // no explicit marker) finalizes immediately for backward compatibility. A
    // multi-part upload must pass `finalize=false` on non-final parts so the
    // upload stays open; the final part passes `finalize=true` (or an empty
    // body as a terminator). Previously any offset==0 non-empty body finalized,
    // which broke multi-part uploads after the first part.
    let should_finalize = should_finalize(offset, body.is_empty(), query.finalize);
    if should_finalize {
        let _ = state.storage.finalize(&chunk_id).await;
        let _ = state.db.update_file_size(&chunk_id, offset + body.len() as u64).await;
        publish_completion(&state._bus, &state.db, &chunk_id, &query.session_id).await;

        let read_token = jwt::sign_read(&chunk_id, &state.jwt_key).unwrap_or_default();
        return (StatusCode::OK, Json(serde_json::json!({"read_token": read_token}))).into_response();
    }

    Json(serde_json::json!({"status": "ok"})).into_response()
}

/// GET /api/binary/{chunk_id}?token=<read_jwt>
///
/// Stream read. Supports Range header. Returns partial content while .writing exists.
async fn read_handler(
    State(state): State<AppState>,
    Path(chunk_id): Path<String>,
    Query(query): Query<TokenQuery>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    // Verify read JWT
    let claims = match jwt::verify(&query.token, &state.jwt_key) {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": e.to_string()}))).into_response();
        }
    };
    if claims.purpose != "read" {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "token purpose must be 'read'"}))).into_response();
    }
    if claims.chunk_id != chunk_id {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "token chunk_id mismatch"}))).into_response();
    }

    // Parse Range header
    let offset = req
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split('-').next())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    match state.storage.read(&chunk_id, offset, 1024 * 1024 * 8).await {
        Ok((data, file_size, _done)) => {
            let data_len = data.len() as u64;
            let response = axum::response::Response::builder()
                .header("Content-Type", "audio/wav")
                .header("Content-Length", data_len.to_string())
                .header("Accept-Ranges", "bytes")
                .header("Access-Control-Allow-Origin", "*");

            match content_range_header(offset, data_len, file_size) {
                Some(range_header) => response
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header("Content-Range", &range_header)
                    .body(axum::body::Body::from(data))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
                None => response
                    .body(axum::body::Body::from(data))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
            }
        }
        Err(e) => {
            (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e.to_string()}))).into_response()
        }
    }
}

/// DELETE /api/binary/{chunk_id}?token=<write_jwt>
async fn delete_handler(
    State(state): State<AppState>,
    Path(chunk_id): Path<String>,
    Query(query): Query<TokenQuery>,
) -> Response {
    // Verify write JWT (delete requires write permission)
    let claims = match jwt::verify(&query.token, &state.jwt_key) {
        Ok(c) => c,
        Err(e) => {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": e.to_string()}))).into_response();
        }
    };
    if claims.purpose != "write" {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "token purpose must be 'write'"}))).into_response();
    }
    if claims.chunk_id != chunk_id {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "token chunk_id mismatch"}))).into_response();
    }

    let _ = state.storage.delete(&chunk_id).await;
    let _ = state.db.delete_asset(&chunk_id).await;
    StatusCode::NO_CONTENT.into_response()
}

/// Publish a completion event after the upload finalizes.
/// cafe-stt watches for this to know the audio is ready to transcribe.
/// Uses `session_id` from the query param if provided, otherwise falls back
/// to the DB (stored when the BinaryRef chunk was received via the bus).
async fn publish_completion(
    bus: &BusClient,
    db: &crate::db::Db,
    chunk_id: &str,
    session_id: &Option<String>,
) {
    let sid = match session_id {
        Some(s) => Some(s.clone()),
        None => db.get_session_for_chunk(chunk_id).await.unwrap_or(None),
    };
    if let Some(ref sid) = sid {
        let mutation = cafe_sdk::Chunk::mutation(chunk_id, "com.nominal.cafe-binary-store")
            .with_annotation("cafe.binary.completed", true);
        if let Err(e) = bus.publish(sid, mutation).await {
            warn!("cafe-binary-store: failed to publish completion event: {e}");
        }
    }
}

#[cfg(test)]
mod handler_tests {
    use super::*;
    use tempfile::TempDir;

    fn new_storage() -> (storage::Storage, TempDir) {
        let dir = TempDir::new().unwrap();
        let storage = storage::Storage::new(dir.path().to_path_buf());
        (storage, dir)
    }

    // ----- Bug A: empty/EOF read must not underflow (was a u64 subtraction bug) -----

    #[test]
    fn content_range_header_full_read_is_none() {
        // offset==0 => full read, no Content-Range header.
        assert_eq!(content_range_header(0, 10, 10), None);
    }

    #[test]
    fn content_range_header_empty_read_no_underflow() {
        // offset==0 with empty data must NOT panic. Before the fix this was
        // `offset + data.len() as u64 - 1` which underflowed (panic in debug,
        // wrap in release) on empty/EOF reads.
        assert_eq!(content_range_header(0, 0, 0), None);
    }

    #[test]
    fn content_range_header_partial_read_correct() {
        assert_eq!(content_range_header(5, 3, 100), Some("bytes 5-7/100".into()));
        // offset>0 but empty data (past EOF) must not underflow either.
        assert_eq!(content_range_header(10, 0, 100), Some("bytes 10-10/100".into()));
    }

    #[tokio::test]
    async fn read_empty_file_does_not_panic() {
        let (storage, _dir) = new_storage();
        let chunk = "empty-chunk";
        storage.start_write(chunk).await.unwrap();
        storage.finalize(chunk).await.unwrap();
        // An empty file: read returns 0 bytes at offset 0. The handler's
        // content-range math must not underflow.
        let (data, size, done) = storage.read(chunk, 0, 1024).await.unwrap();
        assert_eq!(data.len(), 0);
        assert_eq!(size, 0);
        assert!(done);
        assert_eq!(content_range_header(0, data.len() as u64, size), None);
    }

    // ----- Bug B: multi-part upload must not finalize after the first part -----

    #[test]
    fn should_finalize_single_shot_default_finalizes() {
        // Backward-compatible single-shot write: offset==0 with data, no marker.
        assert!(should_finalize(0, false, None));
    }

    #[test]
    fn should_finalize_multipart_first_part_stays_open() {
        // Multi-part first part: offset==0 with data, finalize=false => NOT finalized.
        assert!(!should_finalize(0, false, Some(false)));
    }

    #[test]
    fn should_finalize_multipart_final_part_finalizes() {
        let off = 6u64;
        assert!(should_finalize(off, false, Some(true)));
    }

    #[test]
    fn should_finalize_empty_body_is_terminator() {
        // An empty-body request is always a terminator, regardless of offset/marker.
        assert!(should_finalize(0, true, None));
        assert!(should_finalize(10, true, Some(false)));
    }

    #[tokio::test]
    async fn multipart_upload_two_parts_succeeds() {
        // Simulate the handler's finalize decision across a 2-part upload.
        let (storage, _dir) = new_storage();
        let chunk = "mp-chunk";
        let part1 = b"hello ";
        let part2 = b"world";

        // Part 1: POST offset=0, finalize=false (handler must NOT finalize).
        storage.start_write(chunk).await.unwrap();
        storage.append(chunk, 0, part1, 1024).await.unwrap();
        assert!(
            !should_finalize(0, false, Some(false)),
            "first part of a multi-part upload must not finalize"
        );
        // The .writing marker must still exist so the next part can resume.
        assert!(tokio::fs::metadata(storage.writing_path(chunk)).await.is_ok());

        // Part 2: POST offset=<len(part1)>, finalize=true => finalize.
        let off = part1.len() as u64;
        storage
            .resume_write(chunk, off)
            .await
            .expect("resume must succeed because the upload was NOT finalized after part 1");
        storage.append(chunk, off, part2, 1024).await.unwrap();
        assert!(should_finalize(off, false, Some(true)));
        storage.finalize(chunk).await.unwrap();

        let (data, _size, done) = storage.read(chunk, 0, 1024).await.unwrap();
        assert_eq!(data, b"hello world");
        assert!(done);
    }
}
