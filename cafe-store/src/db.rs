use anyhow::Result;
use cafe_sdk::{Chunk, ContentType, SessionInfo};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::str::FromStr;

pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub async fn connect(path: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite:{}", path))?
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (
                id           TEXT PRIMARY KEY,
                agent_id     TEXT NOT NULL,
                is_background INTEGER NOT NULL DEFAULT 0,
                display_name TEXT,
                ui_mode      TEXT NOT NULL DEFAULT 'chat',
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS chunks (
                id           TEXT PRIMARY KEY,
                session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                content_type TEXT NOT NULL,
                content      TEXT,
                data         BLOB,
                mime_type    TEXT,
                producer     TEXT NOT NULL,
                annotations  TEXT NOT NULL,
                timestamp    INTEGER NOT NULL,
                seq          INTEGER NOT NULL,
                UNIQUE(session_id, seq)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_chunks_session ON chunks(session_id, seq)",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn upsert_session(
        &self,
        session_id: &str,
        agent_id: &str,
        is_background: bool,
    ) -> Result<()> {
        let now = now_ms();
        sqlx::query(
            "INSERT INTO sessions (id, agent_id, is_background, ui_mode, created_at, updated_at)
             VALUES (?, ?, ?, 'chat', ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 updated_at = excluded.updated_at",
        )
        .bind(session_id)
        .bind(agent_id)
        .bind(is_background as i64)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let rows = sqlx::query(
            "SELECT s.id, s.agent_id, s.display_name, s.is_background, s.ui_mode, s.created_at,
                    COUNT(c.id) as message_count
             FROM sessions s
             LEFT JOIN chunks c ON c.session_id = s.id
             GROUP BY s.id
             ORDER BY s.created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| SessionInfo {
                session_id: r.get::<String, _>("id"),
                agent_id: r.get::<String, _>("agent_id"),
                display_name: r.get::<Option<String>, _>("display_name"),
                is_background: r.get::<i64, _>("is_background") != 0,
                ui_mode: r.get::<String, _>("ui_mode"),
                message_count: r.get::<i64, _>("message_count") as usize,
                created_at: r.get::<i64, _>("created_at"),
            })
            .collect())
    }

    pub async fn insert_chunk(&self, session_id: &str, chunk: &Chunk) -> Result<()> {
        let content_type = match chunk.content_type {
            ContentType::Text => "text",
            ContentType::Binary => "binary",
            ContentType::BinaryRef => "binary-ref",
            ContentType::Null => "null",
        };
        let annotations = serde_json::to_string(&chunk.annotations)?;

        // Get next seq number
        let seq: i64 = sqlx::query(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM chunks WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&self.pool)
        .await
        .map(|r| r.get::<i64, _>(0))
        .unwrap_or(0);

        sqlx::query(
            "INSERT OR IGNORE INTO chunks
                (id, session_id, content_type, content, data, mime_type, producer, annotations, timestamp, seq)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&chunk.id)
        .bind(session_id)
        .bind(content_type)
        .bind(&chunk.content)
        .bind(&chunk.data)
        .bind(&chunk.mime_type)
        .bind(&chunk.producer)
        .bind(&annotations)
        .bind(chunk.timestamp)
        .bind(seq)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn load_history(&self, session_id: &str) -> Result<Vec<Chunk>> {
        let rows = sqlx::query(
            "SELECT id, content_type, content, data, mime_type, producer, annotations, timestamp
             FROM chunks
             WHERE session_id = ?
             ORDER BY seq ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        let mut chunks = Vec::with_capacity(rows.len());
        for r in rows {
            let content_type_str: String = r.get("content_type");
            let content_type = match content_type_str.as_str() {
                "text" => ContentType::Text,
                "binary" => ContentType::Binary,
                "binary-ref" => ContentType::BinaryRef,
                _ => ContentType::Null,
            };
            let annotations_str: String = r.get("annotations");
            let annotations = serde_json::from_str(&annotations_str).unwrap_or_default();
            chunks.push(Chunk {
                id: r.get("id"),
                content_type,
                content: r.get("content"),
                data: r.get("data"),
                mime_type: r.get("mime_type"),
                producer: r.get("producer"),
                annotations,
                timestamp: r.get("timestamp"),
            });
        }
        Ok(chunks)
    }

    pub async fn session_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM sessions")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<i64, _>(0))
    }

    pub async fn chunk_count(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM chunks")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<i64, _>(0))
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
