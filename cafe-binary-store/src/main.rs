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
    let bus = BusClient::new(&cfg.bus_socket);

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
            let read_url = format!(
                "http://0.0.0.0:{}/api/binary/{}",
                state.config.port, chunk_id
            );
            let mut mutation = cafe_sdk::Chunk::mutation(&chunk_id, "com.nominal.cafe-binary-store");
            mutation = mutation
                .with_annotation(keys::BINARY_READ_URL, &read_url)
                .with_annotation(keys::BINARY_READ_TOKEN, &read_token)
                .as_transient();
            // Publish broadcast mutation (no direct_to — all subscribers see it)
            if let Err(e) = state._bus.publish(sid, mutation).await {
                warn!("cafe-binary-store: failed to publish read credentials: {e}");
            }
        }
    }

    // Check if this is the final segment (connection is done — the body has been read)
    let writing_path = {
        let p = state.storage.chunk_path(&chunk_id);
        let name = format!("{}.writing", p.file_name().unwrap().to_string_lossy());
        let mut wpath = p;
        wpath.set_file_name(name);
        wpath
    };
    let still_writing = tokio::fs::metadata(&writing_path).await.is_ok();
    if !still_writing && offset == 0 && !body.is_empty() {
        // Single-shot write (no resume, body already complete) — finalize
        let _ = state.storage.finalize(&chunk_id).await;
        let _ = state.db.update_file_size(&chunk_id, body.len() as u64).await;

        // Generate read JWT for this case too
        let read_token = jwt::sign_read(&chunk_id, &state.jwt_key).unwrap_or_default();
        return (StatusCode::OK, Json(serde_json::json!({"read_token": read_token}))).into_response();
    }

    // Check if the body was empty — final segment marker
    if offset > 0 || body.is_empty() {
        // Final segment
        let _ = state.storage.finalize(&chunk_id).await;
        let _ = state.db.update_file_size(&chunk_id, offset + body.len() as u64).await;
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
        Ok((data, _file_size, _done)) => {
            let response = axum::response::Response::builder()
                .header("Content-Type", "application/octet-stream")
                .header("Content-Length", data.len().to_string())
                .header("Accept-Ranges", "bytes")
                .header("Access-Control-Allow-Origin", "*");

            if offset > 0 {
                let response = response.status(StatusCode::PARTIAL_CONTENT);
                response
                    .body(axum::body::Body::from(data))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            } else {
                response
                    .body(axum::body::Body::from(data))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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
