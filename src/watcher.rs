use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// Start a file watcher on the parent directory of `target`, filtered to
/// events for `target` only.
///
/// Returns a channel receiver that emits `()` whenever a relevant change is
/// detected (pre-suppress-flag-check). The caller must check `suppress` before
/// acting on the event (the suppress flag is managed by `writer::write_file_atomic`).
///
/// Also returns the `RecommendedWatcher` — keep it alive for the duration of the
/// session or watching will stop.
pub fn spawn_watcher(
    target: &Path,
    suppress: Arc<AtomicBool>,
) -> Result<(mpsc::Receiver<()>, RecommendedWatcher)> {
    let target = target.to_path_buf();
    let watch_dir = target
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let (std_tx, std_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = RecommendedWatcher::new(std_tx, Config::default())?;
    watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;

    let (tokio_tx, tokio_rx) = mpsc::channel::<()>(32);

    tokio::task::spawn_blocking(move || {
        for res in std_rx {
            match res {
                Ok(event) => {
                    if !is_relevant_event(&event, &target) {
                        continue;
                    }
                    if suppress.load(Ordering::Acquire) {
                        tracing::debug!("fs event suppressed (our own write)");
                        continue;
                    }
                    if tokio_tx.blocking_send(()).is_err() {
                        break; // receiver dropped, session ended
                    }
                }
                Err(e) => {
                    tracing::warn!("watcher error: {e}");
                }
            }
        }
    });

    Ok((tokio_rx, watcher))
}

fn is_relevant_event(event: &notify::Event, target: &PathBuf) -> bool {
    match &event.kind {
        EventKind::Modify(_) | EventKind::Create(_) => {}
        _ => return false,
    }
    event.paths.iter().any(|p| p == target)
}
