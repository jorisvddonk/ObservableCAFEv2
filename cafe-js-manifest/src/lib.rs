//! Manifest extraction for pure-JS agents (`agents-js/*.js`).
//!
//! A JS agent is a plain script (no modules) that defines two globals:
//! - `manifest` — a JSON-compatible object literal (see [`JsManifest`]).
//! - `main(cafe)` — an async function driving the agent via the `cafe`
//!   bridge (`for await (const event of cafe.events()) …`).
//!
//! Top-level code in agent files must be side-effect free: extraction
//! evaluates the whole file in a throwaway QuickJS runtime and reads back
//! `manifest`. Shared by `cafe-agent-js` (execution) and `cafe-server`
//! (`GET /api/agents` listing) so there is exactly one parser.

use anyhow::{Context as _, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn default_mode() -> String {
    "stateless".into()
}

/// Agent metadata. Mirrors the TOML top-level fields (`AgentDefinition`)
/// minus the pipeline: orchestration lives in `main(cafe)`, not `[[steps]]`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct JsManifest {
    /// Unique id. Used as session id for background agents.
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub background: bool,
    #[serde(default = "default_true")]
    pub allows_reload: bool,
    /// Informational in Phase 2 (the store persists everything the bus
    /// carries); kept for TOML parity and future use.
    #[serde(default = "default_true")]
    pub persists_state: bool,
    /// `stateless` (fresh runtime per event) or `stateful` (one runtime per
    /// session; execution lands in Phase 3).
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Cron expression for background agents (`tick` events). Must be
    /// 7-field with seconds (e.g. `"*/10 * * * * * *"`); 5-field is
    /// rejected by tokio-cron-scheduler 0.10.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Per-agent RPC timeout in seconds (default 30).
    #[serde(default)]
    pub rpc_timeout_secs: Option<u64>,
    /// Annotations for the config-seeding null chunk published at session
    /// creation. `config.type = "runtime"` is injected when absent.
    #[serde(default)]
    pub initial_config: HashMap<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

impl JsManifest {
    /// Structural validation shared by all consumers.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            anyhow::bail!("manifest.name must be a non-empty string");
        }
        match self.mode.as_str() {
            "stateless" | "stateful" => Ok(()),
            other => anyhow::bail!(
                "manifest.mode must be \"stateless\" or \"stateful\", got {other:?}"
            ),
        }
    }
}

/// Extract and validate the `manifest` global from JS `source`.
///
/// Evaluates the file as a plain script in a synchronous throwaway runtime
/// (no bridge, no I/O) and reads back `JSON.stringify(manifest)`.
pub fn extract_manifest(source: &str) -> Result<JsManifest> {
    let rt = rquickjs::Runtime::new().context("failed to create QuickJS runtime")?;
    let ctx = rquickjs::Context::full(&rt).context("failed to create QuickJS context")?;
    ctx.with(|ctx| {
        ctx.eval::<(), _>(source)
            .map_err(|e| anyhow::anyhow!("agent source failed to evaluate: {e}"))?;
        let json: String = ctx
            .eval("JSON.stringify(typeof manifest === 'undefined' ? null : manifest)")
            .map_err(|e| anyhow::anyhow!("failed to read manifest global: {e}"))?;
        let value: serde_json::Value =
            serde_json::from_str(&json).context("manifest is not JSON-serializable")?;
        if value.is_null() {
            anyhow::bail!("agent file must define a top-level `manifest` object");
        }
        let manifest: JsManifest =
            serde_json::from_value(value).context("manifest has invalid fields")?;
        manifest.validate()?;
        Ok(manifest)
    })
}

/// A successfully scanned agent file: path, raw source, parsed manifest.
pub struct LoadedJsFile {
    pub path: PathBuf,
    pub source: String,
    pub manifest: JsManifest,
}

/// Result of scanning one directory: loaded files plus per-file skip reasons.
/// Skips are data (logged by callers), never silent: an invalid agent must be
/// visible in logs, not vanish.
pub struct ScanResult {
    pub loaded: Vec<LoadedJsFile>,
    pub skipped: Vec<(PathBuf, String)>,
}

/// Scan `dir/*.js`, returning loaded agents and skip reasons.
pub fn scan_directory(dir: &str) -> ScanResult {
    let mut out = ScanResult {
        loaded: Vec::new(),
        skipped: Vec::new(),
    };
    let pattern = format!("{dir}/*.js");
    let paths = match glob::glob(&pattern) {
        Ok(p) => p,
        Err(e) => {
            out.skipped.push((PathBuf::from(dir), e.to_string()));
            return out;
        }
    };
    for entry in paths {
        let path = match entry {
            Ok(p) => p,
            Err(e) => {
                out.skipped.push((PathBuf::from(dir), e.to_string()));
                continue;
            }
        };
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                out.skipped.push((path, format!("unreadable: {e}")));
                continue;
            }
        };
        match extract_manifest(&source) {
            Ok(manifest) => out.loaded.push(LoadedJsFile {
                path,
                source,
                manifest,
            }),
            Err(e) => out.skipped.push((path, format!("invalid manifest: {e:#}"))),
        }
    }
    out.loaded.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Directories to scan: `./agents-js` plus `CAFE_JS_AGENT_PATHS`
/// (colon-separated extras). Mirrors the TOML `CAFE_AGENT_PATHS` pattern
/// without sharing the variable. Empty segments are dropped and duplicates
/// removed (order-preserving), so a trailing colon or a repeated `./agents-js`
/// is harmless instead of a noisy double scan.
pub fn agent_dirs() -> Vec<String> {
    let mut dirs = vec!["./agents-js".to_string()];
    if let Ok(extra) = std::env::var("CAFE_JS_AGENT_PATHS") {
        dirs.extend(split_paths(&extra));
    }
    dedupe_dirs(dirs)
}

/// Split a colon-separated path list, dropping empty segments.
fn split_paths(extra: &str) -> Vec<String> {
    extra
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Remove duplicate directories, preserving first-seen order.
fn dedupe_dirs(dirs: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    dirs.into_iter()
        .filter(|d| seen.insert(d.clone()))
        .collect()
}

/// Read a single agent file (used by hot-reload).
pub fn load_file(path: &Path) -> Result<LoadedJsFile> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("unreadable: {}", path.display()))?;
    let manifest = extract_manifest(&source)?;
    Ok(LoadedJsFile {
        path: path.to_path_buf(),
        source,
        manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
const manifest = { name: "demo", description: "d" };
async function main(cafe) { return "ok"; }
"#;

    #[test]
    fn minimal_manifest_defaults() {
        let m = extract_manifest(MINIMAL).unwrap();
        assert_eq!(m.name, "demo");
        assert!(!m.background);
        assert!(m.allows_reload);
        assert!(m.persists_state);
        assert_eq!(m.mode, "stateless");
        assert!(m.schedule.is_none());
        assert!(m.rpc_timeout_secs.is_none());
        assert!(m.initial_config.is_empty());
    }

    #[test]
    fn full_manifest_roundtrip() {
        let m = extract_manifest(
            r#"
const manifest = {
  name: "ticker",
  description: "ticks",
  background: true,
  allows_reload: false,
  persists_state: false,
  mode: "stateless",
  schedule: "*/10 * * * * * *",
  rpc_timeout_secs: 15,
  initial_config: { "config.js.tag": "tick", "config.tts.enabled": false }
};
async function main(cafe) {}
"#,
        )
        .unwrap();
        assert_eq!(m.name, "ticker");
        assert!(m.background);
        assert!(!m.allows_reload);
        assert!(!m.persists_state);
        assert_eq!(m.schedule.as_deref(), Some("*/10 * * * * * *"));
        assert_eq!(m.rpc_timeout_secs, Some(15));
        assert_eq!(m.initial_config["config.js.tag"], "tick");
    }

    #[test]
    fn missing_manifest_is_an_error() {
        let err = extract_manifest("async function main(cafe) {}").unwrap_err();
        assert!(err.to_string().contains("must define"), "{err}");
    }

    #[test]
    fn empty_name_is_an_error() {
        let err = extract_manifest("const manifest = { name: '  ' };").unwrap_err();
        assert!(err.to_string().contains("non-empty"), "{err}");
    }

    #[test]
    fn unknown_mode_is_an_error() {
        let err =
            extract_manifest("const manifest = { name: 'x', mode: 'quantum' };").unwrap_err();
        assert!(err.to_string().contains("stateless"), "{err}");
    }

    #[test]
    fn syntax_error_is_an_error() {
        assert!(extract_manifest("const manifest = {{{;").is_err());
    }

    #[test]
    fn scan_directory_loads_and_skips() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("good.js"), MINIMAL).unwrap();
        std::fs::write(dir.path().join("bad.js"), "const manifest = 42;").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        let res = scan_directory(dir.path().to_str().unwrap());
        assert_eq!(res.loaded.len(), 1);
        assert_eq!(res.loaded[0].manifest.name, "demo");
        assert_eq!(res.skipped.len(), 1);
        assert!(res.skipped[0].0.ends_with("bad.js"));
    }

    #[test]
    fn scan_missing_dir_reports_skip() {
        let res = scan_directory("/tmp/definitely-not-a-cafe-dir-12345");
        assert!(res.loaded.is_empty());
    }

    #[test]
    fn split_paths_drops_empties() {
        assert_eq!(split_paths("/a:/b"), vec!["/a".to_string(), "/b".to_string()]);
        assert_eq!(split_paths("/a:"), vec!["/a".to_string()]);
        assert_eq!(split_paths(":/a::/b:"), vec!["/a".to_string(), "/b".to_string()]);
        assert!(split_paths("").is_empty());
        assert!(split_paths(":::").is_empty());
    }

    #[test]
    fn dedupe_dirs_keeps_first_seen_order() {
        assert_eq!(
            dedupe_dirs(vec![
                "./agents-js".into(),
                "/extra".into(),
                "./agents-js".into(),
                "/extra".into(),
                "/other".into()
            ]),
            vec![
                "./agents-js".to_string(),
                "/extra".to_string(),
                "/other".to_string()
            ]
        );
    }
}
