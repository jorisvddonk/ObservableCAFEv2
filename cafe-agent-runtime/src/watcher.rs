use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Start a file watcher on the given directories.
/// Returns a receiver that yields paths of changed .toml files.
pub fn start_watcher(dirs: &[String]) -> Result<(RecommendedWatcher, mpsc::Receiver<PathBuf>)> {
    let (tx, rx) = mpsc::channel::<PathBuf>(64);

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            match event.kind {
                EventKind::Modify(_) | EventKind::Create(_) => {
                    for path in event.paths {
                        if path.extension().map(|e| e == "toml").unwrap_or(false) {
                            let _ = tx.blocking_send(path);
                        }
                    }
                }
                _ => {}
            }
        }
    })?;

    for dir in dirs {
        let path = std::path::Path::new(dir);
        if path.exists() {
            watcher.watch(path, RecursiveMode::NonRecursive)?;
        }
    }

    Ok((watcher, rx))
}
