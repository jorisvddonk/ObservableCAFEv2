use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::Path;
use std::str::FromStr;

pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub async fn connect(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join("cafe-binary-store.db");
        let opts = SqliteConnectOptions::from_str(&format!(
            "sqlite:{}",
            db_path.display()
        ))?
        .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS assets (
                chunk_id     TEXT PRIMARY KEY,
                session_id   TEXT NOT NULL,
                is_transient INTEGER NOT NULL,
                created_at   INTEGER NOT NULL,
                file_size    INTEGER
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_assets_session ON assets(session_id)",
        )
        .execute(&self.pool)
        .await
        .ok();

        Ok(())
    }

    pub async fn insert_asset(
        &self,
        chunk_id: &str,
        session_id: &str,
        is_transient: bool,
    ) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        sqlx::query(
            "INSERT OR IGNORE INTO assets (chunk_id, session_id, is_transient, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(chunk_id)
        .bind(session_id)
        .bind(is_transient as i64)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_file_size(&self, chunk_id: &str, file_size: u64) -> Result<()> {
        sqlx::query("UPDATE assets SET file_size = ? WHERE chunk_id = ?")
            .bind(file_size as i64)
            .bind(chunk_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_asset(&self, chunk_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM assets WHERE chunk_id = ?")
            .bind(chunk_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete all assets for a session and return their chunk_ids.
    pub async fn delete_session_non_transient(&self, session_id: &str) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT chunk_id FROM assets WHERE session_id = ? AND is_transient = 0",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        sqlx::query("DELETE FROM assets WHERE session_id = ? AND is_transient = 0")
            .bind(session_id)
            .execute(&self.pool)
            .await?;

        Ok(rows)
    }

    /// GC: delete expired transient assets, return their chunk_ids.
    pub async fn gc_transient(&self, gc_ttl_secs: u64) -> Result<Vec<String>> {
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            - gc_ttl_secs as i64;

        let rows = sqlx::query_scalar::<_, String>(
            "SELECT chunk_id FROM assets WHERE is_transient = 1 AND created_at < ?",
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;

        sqlx::query("DELETE FROM assets WHERE is_transient = 1 AND created_at < ?")
            .bind(cutoff)
            .execute(&self.pool)
            .await?;

        Ok(rows)
    }
}
