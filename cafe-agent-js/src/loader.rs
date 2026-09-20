//! Agent registry over `agents-js/*.js`.
//!
//! [`load_all`] scans every agent dir at startup; [`reload_file`] refreshes a
//! single file for hot-reload. The registry maps agent name → [`JsAgent`]
//! behind a lock so session loops always run the current source.

use cafe_js_manifest::{JsManifest, LoadedJsFile};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// A file-loaded agent: manifest + raw source + provenance.
#[derive(Clone)]
pub struct JsAgent {
    pub manifest: JsManifest,
    pub source: String,
    pub path: PathBuf,
    pub file_hash: String,
}

/// Shared registry: agent name → agent. Short lock holds only.
pub type Registry = Arc<RwLock<HashMap<String, JsAgent>>>;

pub fn new_registry() -> Registry {
    Arc::new(RwLock::new(HashMap::new()))
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn hash_file(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => hash_bytes(&bytes),
        Err(_) => String::new(),
    }
}

/// Insert one scanned file. Both modes are served: `stateless` runs a fresh
/// runtime per event, `stateful` keeps one runtime per session.
fn insert_file(
    registry: &mut HashMap<String, JsAgent>,
    loaded: LoadedJsFile,
    warnings: &mut Vec<String>,
) {
    let name = loaded.manifest.name.clone();
    if let Some(prev) = registry.get(&name) {
        warnings.push(format!(
            "agent '{}' redefined by {} (was {}); last file wins",
            name,
            loaded.path.display(),
            prev.path.display()
        ));
    }
    let hash = hash_file(&loaded.path);
    registry.insert(
        name.clone(),
        JsAgent {
            manifest: loaded.manifest,
            source: loaded.source,
            path: loaded.path,
            file_hash: hash,
        },
    );
}

/// Scan `dirs`, returning the registry plus human-readable warnings
/// (invalid manifests, stateful agents, name collisions). Callers log every
/// warning — skips must be visible, never silent.
pub fn load_all(dirs: &[String]) -> (Registry, Vec<String>) {
    let mut registry = HashMap::new();
    let mut warnings = Vec::new();
    for dir in dirs {
        let res = cafe_js_manifest::scan_directory(dir);
        for loaded in res.loaded {
            insert_file(&mut registry, loaded, &mut warnings);
        }
        for (path, reason) in res.skipped {
            warnings.push(format!("skipping {}: {reason}", path.display()));
        }
    }
    (Arc::new(RwLock::new(registry)), warnings)
}

/// Reload one file after a watcher event. Returns the agent name.
/// Honors `allows_reload = false` and renames (an entry whose path matches
/// but whose name changed is removed).
pub fn reload_file(registry: &Registry, path: &Path) -> anyhow::Result<String> {
    let loaded = cafe_js_manifest::load_file(path)?;
    let name = loaded.manifest.name.clone();
    let mut reg = registry.write().unwrap_or_else(|e| e.into_inner());
    if !loaded.manifest.allows_reload {
        anyhow::bail!("agent '{name}' sets allows_reload = false; change ignored");
    }
    // Drop a stale entry if the file was renamed to a new agent name.
    let stale: Vec<String> = reg
        .iter()
        .filter(|(_, a)| a.path == path && a.manifest.name != name)
        .map(|(n, _)| n.clone())
        .collect();
    for n in stale {
        reg.remove(&n);
    }
    let hash = hash_file(path);
    reg.insert(
        name.clone(),
        JsAgent {
            manifest: loaded.manifest,
            source: loaded.source,
            path: path.to_path_buf(),
            file_hash: hash,
        },
    );
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    const GOOD: &str = r#"
const manifest = { name: "good", background: true };
async function main(cafe) {}
"#;

    #[test]
    fn load_all_registers_valid_agents() {
        let dir = tempfile::TempDir::new().unwrap();
        write(&dir, "good.js", GOOD);
        let (reg, warnings) = load_all(&[dir.path().to_str().unwrap().into()]);
        assert!(warnings.is_empty(), "{warnings:?}");
        let reg = reg.read().unwrap();
        assert!(reg.contains_key("good"));
        assert!(reg["good"].manifest.background);
    }

    #[test]
    fn load_all_warns_on_invalid_but_serves_stateful() {
        let dir = tempfile::TempDir::new().unwrap();
        write(&dir, "good.js", GOOD);
        write(&dir, "bad.js", "const manifest = 42;");
        write(
            &dir,
            "keeper.js",
            "const manifest = { name: 'keeper', mode: 'stateful' };\nasync function main(cafe) {}",
        );
        let (reg, warnings) = load_all(&[dir.path().to_str().unwrap().into()]);
        let reg = reg.read().unwrap();
        assert!(reg.contains_key("good"));
        assert_eq!(reg["keeper"].manifest.mode, "stateful");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("bad.js")));
    }

    #[test]
    fn duplicate_name_last_wins_with_warning() {
        let dir = tempfile::TempDir::new().unwrap();
        write(&dir, "a.js", GOOD);
        write(&dir, "b.js", GOOD);
        let (reg, warnings) = load_all(&[dir.path().to_str().unwrap().into()]);
        assert_eq!(reg.read().unwrap().len(), 1);
        assert!(warnings.iter().any(|w| w.contains("redefined")), "{warnings:?}");
    }

    #[test]
    fn reload_file_updates_source() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write(&dir, "good.js", GOOD);
        let (reg, _) = load_all(&[dir.path().to_str().unwrap().into()]);
        std::fs::write(&path, GOOD.replace("background: true", "background: false")).unwrap();
        let name = reload_file(&reg, &path).unwrap();
        assert_eq!(name, "good");
        assert!(!reg.read().unwrap()["good"].manifest.background);
    }

    #[test]
    fn reload_file_accepts_mode_change_to_stateful() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write(&dir, "good.js", GOOD);
        let (reg, _) = load_all(&[dir.path().to_str().unwrap().into()]);
        std::fs::write(
            &path,
            "const manifest = { name: 'good', mode: 'stateful' };\nasync function main(cafe) {}",
        )
        .unwrap();
        let name = reload_file(&reg, &path).unwrap();
        assert_eq!(name, "good");
        assert_eq!(reg.read().unwrap()["good"].manifest.mode, "stateful");
    }

    #[test]
    fn reload_file_honors_allows_reload_false() {
        let dir = tempfile::TempDir::new().unwrap();
        let locked = r#"
const manifest = { name: "locked", allows_reload: false };
async function main(cafe) {}
"#;
        let path = write(&dir, "locked.js", locked);
        let (reg, _) = load_all(&[dir.path().to_str().unwrap().into()]);
        std::fs::write(&path, locked.replace("locked", "locked2")).unwrap();
        assert!(reload_file(&reg, &path).is_err());
    }

    #[test]
    fn hash_bytes_is_stable_hex() {
        let h1 = hash_bytes(b"hello");
        assert_eq!(h1, hash_bytes(b"hello"));
        assert_ne!(h1, hash_bytes(b"world"));
        assert_eq!(h1.len(), 64);
    }
}
