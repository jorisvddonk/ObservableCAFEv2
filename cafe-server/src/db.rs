use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TokenRow {
    pub id: String,
    pub token: String,
    pub description: Option<String>,
    pub is_admin: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Quickie {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub emoji: Option<String>,
    pub agent_id: String,
    pub starter_message: Option<String>,
    pub config_json: Option<String>,
    pub ui_mode: String,
    pub display_order: i64,
    pub created_at: i64,
}

pub struct Db {
    pub pool: SqlitePool,
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
            "CREATE TABLE IF NOT EXISTS tokens (
                id          TEXT PRIMARY KEY,
                token       TEXT NOT NULL UNIQUE,
                description TEXT,
                is_admin    INTEGER NOT NULL DEFAULT 0,
                created_at  INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS quickies (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                name            TEXT NOT NULL,
                description     TEXT,
                emoji           TEXT,
                agent_id        TEXT NOT NULL,
                starter_message TEXT,
                config_json     TEXT,
                ui_mode         TEXT DEFAULT 'chat',
                display_order   INTEGER DEFAULT 0,
                created_at      INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Returns the admin token, creating one if the table is empty.
    pub async fn ensure_admin_token(&self, seed: Option<&str>) -> Result<String> {
        let count: i64 = sqlx::query("SELECT COUNT(*) FROM tokens")
            .fetch_one(&self.pool)
            .await
            .map(|r| r.get::<i64, _>(0))
            .unwrap_or(0);

        if count > 0 {
            // Return existing admin token
            let row = sqlx::query("SELECT token FROM tokens WHERE is_admin = 1 LIMIT 1")
                .fetch_one(&self.pool)
                .await?;
            return Ok(row.get::<String, _>("token"));
        }

        // Generate or use seed token
        let token = seed
            .map(String::from)
            .unwrap_or_else(|| format!("cafe_adm_{}", Uuid::new_v4().simple()));

        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO tokens (id, token, description, is_admin, created_at)
             VALUES (?, ?, 'Initial admin token', 1, ?)",
        )
        .bind(&id)
        .bind(&token)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(token)
    }

    pub async fn lookup_token(&self, token: &str) -> Result<Option<TokenRow>> {
        let row = sqlx::query(
            "SELECT id, token, description, is_admin FROM tokens WHERE token = ?",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| TokenRow {
            id: r.get("id"),
            token: r.get("token"),
            description: r.get("description"),
            is_admin: r.get::<i64, _>("is_admin") != 0,
        }))
    }

    pub async fn list_tokens(&self) -> Result<Vec<TokenRow>> {
        let rows = sqlx::query("SELECT id, token, description, is_admin FROM tokens")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| TokenRow {
                id: r.get("id"),
                token: r.get("token"),
                description: r.get("description"),
                is_admin: r.get::<i64, _>("is_admin") != 0,
            })
            .collect())
    }

    pub async fn create_token(
        &self,
        description: Option<&str>,
        is_admin: bool,
    ) -> Result<TokenRow> {
        let id = Uuid::new_v4().to_string();
        let token = format!("cafe_{}", Uuid::new_v4().simple());
        let now = now_ms();
        sqlx::query(
            "INSERT INTO tokens (id, token, description, is_admin, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&token)
        .bind(description)
        .bind(is_admin as i64)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(TokenRow {
            id,
            token,
            description: description.map(String::from),
            is_admin,
        })
    }

    pub async fn delete_token(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM tokens WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_quickies(&self) -> Result<Vec<Quickie>> {
        let rows = sqlx::query(
            "SELECT id, name, description, emoji, agent_id, starter_message,
                    config_json, ui_mode, display_order, created_at
             FROM quickies ORDER BY display_order ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| Quickie {
                id: r.get("id"),
                name: r.get("name"),
                description: r.get("description"),
                emoji: r.get("emoji"),
                agent_id: r.get("agent_id"),
                starter_message: r.get("starter_message"),
                config_json: r.get("config_json"),
                ui_mode: r.get("ui_mode"),
                display_order: r.get("display_order"),
                created_at: r.get("created_at"),
            })
            .collect())
    }

    pub async fn create_quickie(
        &self,
        name: &str,
        description: Option<&str>,
        emoji: Option<&str>,
        agent_id: &str,
        starter_message: Option<&str>,
        config_json: Option<&str>,
        ui_mode: &str,
        display_order: i64,
    ) -> Result<Quickie> {
        let now = now_ms();
        let result = sqlx::query(
            "INSERT INTO quickies
                (name, description, emoji, agent_id, starter_message, config_json, ui_mode, display_order, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(description)
        .bind(emoji)
        .bind(agent_id)
        .bind(starter_message)
        .bind(config_json)
        .bind(ui_mode)
        .bind(display_order)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(Quickie {
            id: result.last_insert_rowid(),
            name: name.to_string(),
            description: description.map(String::from),
            emoji: emoji.map(String::from),
            agent_id: agent_id.to_string(),
            starter_message: starter_message.map(String::from),
            config_json: config_json.map(String::from),
            ui_mode: ui_mode.to_string(),
            display_order,
            created_at: now,
        })
    }

    pub async fn delete_quickie(&self, id: i64) -> Result<bool> {
        let result = sqlx::query("DELETE FROM quickies WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
