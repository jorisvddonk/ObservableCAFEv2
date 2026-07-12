//! Database-backed allowlist of iroh peer IDs for the bus.
//!
//! The bus accepts iroh connections from anyone who knows its `EndpointId`.
//! This module gates incoming connections: a connection's `remote_id()` is
//! looked up in an in-memory cache that is periodically refreshed from a
//! dedicated SQLite table (`iroh_allowlist`). The [`AllowlistHook`] plugs into
//! iroh's `EndpointHooks` to reject unauthorized peers at the handshake stage.

use anyhow::Result;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::endpoint::{AfterHandshakeOutcome, Connection, EndpointHooks, Side};
use sqlx::Row;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// How often the in-memory cache is refreshed from the DB.
const DEFAULT_REFRESH: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Allowlist {
    pool: sqlx::SqlitePool,
    cache: Arc<RwLock<HashSet<String>>>,
}

impl Allowlist {
    /// Open (creating if needed) the allowlist database at `path` and load the
    /// current set of peer IDs into the cache.
    pub async fn connect(path: &str) -> Result<Self> {
        let opts = sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite:{}", path))?
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        let db = Self {
            pool,
            cache: Arc::new(RwLock::new(HashSet::new())),
        };
        db.migrate().await?;
        db.refresh().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS iroh_allowlist (
                peer_id    TEXT PRIMARY KEY,
                label      TEXT,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Reload peer IDs from the DB into the in-memory cache.
    pub async fn refresh(&self) -> Result<()> {
        let rows = sqlx::query("SELECT peer_id FROM iroh_allowlist")
            .fetch_all(&self.pool)
            .await?;
        let mut set = HashSet::new();
        for r in rows {
            set.insert(r.get::<String, _>("peer_id"));
        }
        *self.cache.write().await = set;
        Ok(())
    }

    /// Spawn a background task that periodically refreshes the cache so CLI
    /// edits take effect without restarting the bus.
    pub fn spawn_refresh_task(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(DEFAULT_REFRESH);
            loop {
                interval.tick().await;
                if let Err(e) = this.refresh().await {
                    warn!("iroh allowlist refresh failed: {}", e);
                }
            }
        });
    }

    pub async fn contains(&self, peer_id: &str) -> bool {
        self.cache.read().await.contains(peer_id)
    }

    pub async fn add(&self, peer_id: &str, label: Option<&str>) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        sqlx::query(
            "INSERT OR REPLACE INTO iroh_allowlist (peer_id, label, created_at) VALUES (?, ?, ?)",
        )
        .bind(peer_id)
        .bind(label)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.refresh().await?;
        Ok(())
    }

    pub async fn remove(&self, peer_id: &str) -> Result<bool> {
        let res = sqlx::query("DELETE FROM iroh_allowlist WHERE peer_id = ?")
            .bind(peer_id)
            .execute(&self.pool)
            .await?;
        self.refresh().await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn list(&self) -> Result<Vec<(String, Option<String>)>> {
        let rows = sqlx::query("SELECT peer_id, label FROM iroh_allowlist ORDER BY peer_id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<String, _>("peer_id"), r.get::<Option<String>, _>("label")))
            .collect())
    }
}

/// iroh `EndpointHooks` implementation that rejects incoming connections whose
/// `remote_id()` is not present in the allowlist.
#[derive(Debug, Clone)]
pub struct AllowlistHook {
    allowlist: Arc<Allowlist>,
}

impl AllowlistHook {
    pub fn new(allowlist: Arc<Allowlist>) -> Self {
        Self { allowlist }
    }

    /// Decision used by the hook (and unit-tested directly).
    async fn decide(&self, peer: &str) -> AfterHandshakeOutcome {
        if self.allowlist.contains(peer).await {
            AfterHandshakeOutcome::Accept
        } else {
            AfterHandshakeOutcome::Reject {
                error_code: 403u32.into(),
                reason: b"not authorized".to_vec(),
            }
        }
    }
}

impl EndpointHooks for AllowlistHook {
    fn after_handshake<'a>(
        &'a self,
        conn: &'a Connection,
    ) -> impl std::future::Future<Output = AfterHandshakeOutcome> + Send + 'a {
        async move {
            // Only gate incoming connections; the bus does not dial out on this
            // endpoint, but be defensive regardless.
            if conn.side() != Side::Server {
                return AfterHandshakeOutcome::Accept;
            }
            let peer = conn.remote_id().to_string();
            let outcome = self.decide(&peer).await;
            match outcome {
                AfterHandshakeOutcome::Accept => debug!("iroh allowlist: accepted {}", peer),
                AfterHandshakeOutcome::Reject { .. } => {
                    warn!("iroh allowlist: rejecting unauthorized peer {}", peer)
                }
            }
            outcome
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> String {
        let p = std::env::temp_dir().join(format!("cafe-allowlist-test-{}.db", uuid_fast()));
        let _ = std::fs::remove_file(&p);
        p.to_string_lossy().to_string()
    }

    // Cheap unique id without pulling in the uuid crate here.
    fn uuid_fast() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(1);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        t.wrapping_mul(31).wrapping_add(n)
    }

    #[tokio::test]
    async fn add_contains_remove() {
        let db = Allowlist::connect(&temp_db()).await.unwrap();
        assert!(!db.contains("peerA").await);

        db.add("peerA", Some("cli")).await.unwrap();
        assert!(db.contains("peerA").await);

        let listed = db.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, "peerA");
        assert_eq!(listed[0].1.as_deref(), Some("cli"));

        assert!(db.remove("peerA").await.unwrap());
        assert!(!db.contains("peerA").await);
    }

    #[tokio::test]
    async fn hook_rejects_unknown_accepts_known() {
        let db = Arc::new(Allowlist::connect(&temp_db()).await.unwrap());
        db.add("known", None).await.unwrap();
        let hook = AllowlistHook::new(db);

        assert!(matches!(hook.decide("known").await, AfterHandshakeOutcome::Accept));
        assert!(matches!(
            hook.decide("unknown").await,
            AfterHandshakeOutcome::Reject { .. }
        ));
    }

    #[tokio::test]
    async fn empty_list_denies_all() {
        let db = Arc::new(Allowlist::connect(&temp_db()).await.unwrap());
        let hook = AllowlistHook::new(db);
        assert!(matches!(
            hook.decide("anyone").await,
            AfterHandshakeOutcome::Reject { .. }
        ));
    }
}
