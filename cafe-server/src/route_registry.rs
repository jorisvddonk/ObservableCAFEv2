use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Notify, RwLock};
use tracing::debug;

const GC_INTERVAL_SECS: u64 = 30;
const STALE_PURGE_SECS: u64 = 60;

/// A registered HTTP route from a bus service.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    pub pattern: String,
    pub methods: Vec<String>,
    pub connection_id: String,
    pub last_seen: Instant,
}

/// Thread-safe route registry with lazy GC.
pub struct RouteRegistryInner {
    routes: RwLock<HashMap<String, RouteEntry>>,
    /// Signaled when a new route is added, so the proxy handler can wake.
    pub changed: Notify,
    max_body_size: usize,
    gc_interval_secs: u64,
    stale_purge_secs: u64,
}

impl RouteRegistryInner {
    pub fn new(max_body_size: usize, gc_interval_secs: u64, stale_purge_secs: u64) -> Self {
        Self {
            routes: RwLock::new(HashMap::new()),
            changed: Notify::new(),
            max_body_size,
            gc_interval_secs,
            stale_purge_secs,
        }
    }

    /// Add or refresh a route registration.
    pub async fn upsert(&self, pattern: &str, methods: Vec<String>, connection_id: &str) {
        let mut routes = self.routes.write().await;
        if let Some(entry) = routes.get_mut(pattern) {
            entry.last_seen = Instant::now();
            if entry.connection_id != connection_id {
                entry.connection_id = connection_id.to_string();
            }
        } else {
            routes.insert(
                pattern.to_string(),
                RouteEntry {
                    pattern: pattern.to_string(),
                    methods,
                    connection_id: connection_id.to_string(),
                    last_seen: Instant::now(),
                },
            );
            self.changed.notify_one();
        }
    }

    /// Remove a route registration.
    pub async fn remove(&self, pattern: &str) {
        self.routes.write().await.remove(pattern);
    }

    /// Remove all routes for a given connection (service disconnected).
    pub async fn remove_by_connection(&self, connection_id: &str) {
        self.routes
            .write()
            .await
            .retain(|_, entry| entry.connection_id != connection_id);
    }

    /// Match a path against registered routes. Returns the entry and extracted params.
    pub async fn match_path(
        &self,
        path: &str,
        method: &str,
    ) -> Option<(RouteEntry, HashMap<String, String>)> {
        let routes = self.routes.read().await;
        for entry in routes.values() {
            if !entry.methods.iter().any(|m| m.eq_ignore_ascii_case(method)) {
                continue;
            }
            if let Some(params) = match_pattern(&entry.pattern, path) {
                return Some((entry.clone(), params));
            }
        }
        None
    }

    pub fn max_body_size(&self) -> usize {
        self.max_body_size
    }
}

/// Simple path pattern matcher. Supports `:param` segments.
/// Returns extracted params if the path matches.
fn match_pattern(pattern: &str, path: &str) -> Option<HashMap<String, String>> {
    let pat_segs: Vec<&str> = pattern.trim_matches('/').split('/').collect();
    let path_segs: Vec<&str> = path.trim_matches('/').split('/').collect();

    if pat_segs.len() != path_segs.len() {
        return None;
    }

    let mut params = HashMap::new();
    for (pat, seg) in pat_segs.iter().zip(path_segs.iter()) {
        if let Some(name) = pat.strip_prefix(':') {
            params.insert(name.to_string(), (*seg).to_string());
        } else if !pat.eq_ignore_ascii_case(seg) {
            return None;
        }
    }
    Some(params)
}

/// Spawn the GC task for stale routes.
pub fn spawn_gc(registry: Arc<RouteRegistryInner>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(
                registry.gc_interval_secs,
            ))
            .await;
            let now = Instant::now();
            let stale = registry.stale_purge_secs;
            let mut removed = 0usize;
            let mut routes = registry.routes.write().await;
            routes.retain(|_, entry| {
                if now.duration_since(entry.last_seen).as_secs() > stale {
                    removed += 1;
                    false
                } else {
                    true
                }
            });
            drop(routes);
            if removed > 0 {
                debug!("RouteRegistry GC: removed {} stale routes", removed);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_simple() {
        let params = match_pattern("/api/ext/sessions/:id/fetch", "/api/ext/sessions/abc/fetch");
        assert!(params.is_some());
        assert_eq!(params.unwrap().get("id").unwrap(), "abc");
    }

    #[test]
    fn match_mismatch_segment_count() {
        assert!(match_pattern("/api/ext/sessions/:id/fetch", "/api/ext/sessions/abc/fetch/extra").is_none());
    }

    #[test]
    fn match_no_params() {
        let params = match_pattern("/api/ext/status", "/api/ext/status");
        assert!(params.is_some());
        assert!(params.unwrap().is_empty());
    }

    #[test]
    fn match_mismatch_literal() {
        assert!(match_pattern("/api/ext/status", "/api/ext/health").is_none());
    }

    #[test]
    fn match_multiple_params() {
        let params = match_pattern("/:a/:b/:c", "/x/y/z");
        assert!(params.is_some());
        let p = params.unwrap();
        assert_eq!(p.get("a").unwrap(), "x");
        assert_eq!(p.get("b").unwrap(), "y");
        assert_eq!(p.get("c").unwrap(), "z");
    }
}
