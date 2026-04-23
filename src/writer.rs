use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::fs;

/// Write `content` to `path` atomically (write to a temp file, then rename).
/// Sets `suppress` to `true` before writing so the file watcher ignores the
/// resulting fs events, then clears it after 50 ms.
pub async fn write_file_atomic(
    path: &Path,
    content: &str,
    suppress: &Arc<AtomicBool>,
) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp_path = tmp_path(dir, path);

    suppress.store(true, Ordering::Release);

    fs::write(&tmp_path, content).await?;
    fs::rename(&tmp_path, path).await?;

    let suppress_clone = Arc::clone(suppress);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        suppress_clone.store(false, Ordering::Release);
    });

    tracing::debug!("wrote {} bytes to {}", content.len(), path.display());
    Ok(())
}

fn tmp_path(dir: &Path, target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    dir.join(format!(".entangle-{name}.tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn atomic_write_creates_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let suppress = Arc::new(AtomicBool::new(false));

        write_file_atomic(&path, "hello", &suppress).await.unwrap();

        let content = fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn suppress_is_set_then_cleared() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let suppress = Arc::new(AtomicBool::new(false));

        // suppress is cleared asynchronously after 50ms
        write_file_atomic(&path, "data", &suppress).await.unwrap();

        // Immediately after: suppress should be true (write just finished)
        // but the spawn-clear hasn't fired yet
        assert!(suppress.load(Ordering::Acquire));

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!suppress.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let suppress = Arc::new(AtomicBool::new(false));

        write_file_atomic(&path, "first", &suppress).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        write_file_atomic(&path, "second", &suppress).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;

        let content = fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "second");
    }

    #[tokio::test]
    async fn no_tmp_file_remains() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.txt");
        let suppress = Arc::new(AtomicBool::new(false));

        write_file_atomic(&path, "content", &suppress).await.unwrap();

        let tmp = dir.path().join(".entangle-data.txt.tmp");
        assert!(!tmp.exists(), "temp file should have been renamed away");
    }
}
