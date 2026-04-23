use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use crate::crdt::CrdtEngine;
use crate::protocol::{self, SyncMessage};
use crate::watcher::spawn_watcher;
use crate::writer::write_file_atomic;

const RECONNECT_BASE_MS: u64 = 1_000;
const RECONNECT_MAX_MS: u64 = 30_000;

pub struct SessionConfig {
    pub ws_url: String,
    pub file_path: PathBuf,
    pub debounce_ms: u64,
    pub poll_interval_ms: u64,
}

/// Run an entangle session. `crdt` should already be seeded with the initial
/// file content for the share command; it should be empty for the join command.
pub async fn run_session(config: SessionConfig, mut crdt: CrdtEngine) -> Result<()> {
    let suppress = Arc::new(AtomicBool::new(false));

    let (mut file_rx, _watcher) = spawn_watcher(&config.file_path, Arc::clone(&suppress))
        .context("failed to start file watcher")?;

    // Poll fallback: re-check file every poll_interval_ms in case notify misses events.
    let poll_tx_file = config.file_path.clone();
    let (poll_tx, mut poll_rx) = tokio::sync::mpsc::channel::<()>(4);
    let poll_suppress = Arc::clone(&suppress);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(config.poll_interval_ms)).await;
            if !poll_suppress.load(Ordering::Relaxed) {
                if tokio::fs::metadata(&poll_tx_file).await.is_ok() {
                    let _ = poll_tx.try_send(());
                }
            }
        }
    });

    let mut delay_ms = RECONNECT_BASE_MS;

    loop {
        match run_connection(&config, &mut crdt, &suppress, &mut file_rx, &mut poll_rx).await {
            Ok(()) => {
                // Ctrl-C or clean close
                break;
            }
            Err(e) => {
                warn!("connection error: {e:#}. Reconnecting in {delay_ms}ms…");
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                delay_ms = (delay_ms * 2).min(RECONNECT_MAX_MS);
            }
        }
    }

    Ok(())
}

async fn run_connection(
    config: &SessionConfig,
    crdt: &mut CrdtEngine,
    suppress: &Arc<AtomicBool>,
    file_rx: &mut tokio::sync::mpsc::Receiver<()>,
    poll_rx: &mut tokio::sync::mpsc::Receiver<()>,
) -> Result<()> {
    info!("connecting to {}", config.ws_url);

    let (ws, _resp) = connect_async(&config.ws_url)
        .await
        .with_context(|| format!("failed to connect to {}", config.ws_url))?;

    let (mut ws_sink, mut ws_stream) = ws.split();

    // Perform SyncStep1 immediately after connecting
    let sv = crdt.state_vector_bytes();
    ws_sink
        .send(Message::Binary(protocol::encode_sync_step1(&sv)))
        .await
        .context("failed to send SyncStep1")?;

    info!("connected. watching {}…", config.file_path.display());

    let debounce_dur = Duration::from_millis(config.debounce_ms);
    let far_future = Duration::from_secs(86_400);

    let mut debounce_pending = false;
    let deadline = tokio::time::sleep(far_future);
    tokio::pin!(deadline);

    // Last content hash for the poll fallback deduplication
    let mut last_poll_hash: u64 = hash_str(crdt.current_text());

    loop {
        tokio::select! {
            biased;

            // Ctrl-C: clean shutdown
            _ = tokio::signal::ctrl_c() => {
                info!("interrupted");
                return Ok(());
            }

            // File watcher event
            Some(()) = file_rx.recv() => {
                debounce_pending = true;
                deadline.as_mut().reset(tokio::time::Instant::now() + debounce_dur);
            }

            // Poll fallback event
            Some(()) = poll_rx.recv() => {
                if !debounce_pending {
                    // Only trigger if file content actually changed
                    if let Ok(content) = tokio::fs::read_to_string(&config.file_path).await {
                        let h = hash_str(&content);
                        if h != last_poll_hash {
                            last_poll_hash = h;
                            debounce_pending = true;
                            deadline.as_mut().reset(
                                tokio::time::Instant::now() + debounce_dur,
                            );
                        }
                    }
                }
            }

            // Debounce timer fired
            () = &mut deadline, if debounce_pending => {
                debounce_pending = false;
                deadline.as_mut().reset(tokio::time::Instant::now() + far_future);

                if suppress.load(Ordering::Acquire) {
                    debug!("debounce fired but suppress is set, skipping");
                    continue;
                }

                match tokio::fs::read_to_string(&config.file_path).await {
                    Ok(content) => {
                        last_poll_hash = hash_str(&content);
                        if let Some(update) = crdt.apply_local_edit(&content) {
                            debug!("local edit: {} update bytes", update.len());
                            ws_sink
                                .send(Message::Binary(protocol::encode_update(&update)))
                                .await
                                .context("failed to send update")?;
                        }
                    }
                    Err(e) => {
                        warn!("could not read {}: {e}", config.file_path.display());
                    }
                }
            }

            // Incoming WebSocket message
            msg = ws_stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        handle_incoming(
                            &data,
                            crdt,
                            &mut ws_sink,
                            suppress,
                            &config.file_path,
                            &mut last_poll_hash,
                        )
                        .await?;
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        ws_sink.send(Message::Pong(payload)).await?;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return Err(anyhow::anyhow!("WebSocket connection closed"));
                    }
                    Some(Err(e)) => {
                        return Err(anyhow::anyhow!("WebSocket error: {e}"));
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn handle_incoming<S>(
    data: &[u8],
    crdt: &mut CrdtEngine,
    ws_sink: &mut S,
    suppress: &Arc<AtomicBool>,
    file_path: &Path,
    last_poll_hash: &mut u64,
) -> Result<()>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    match protocol::decode_message(data) {
        Some(SyncMessage::SyncStep1(peer_sv)) => {
            debug!("received SyncStep1");
            let update = crdt.encode_state_as_update(&peer_sv);
            ws_sink
                .send(Message::Binary(protocol::encode_sync_step2(&update)))
                .await
                .context("failed to send SyncStep2")?;
        }
        Some(SyncMessage::SyncStep2(update) | SyncMessage::Update(update)) => {
            debug!("received update ({} bytes)", update.len());
            if let Some(new_content) = crdt
                .apply_remote_update(&update)
                .context("failed to apply remote update")?
            {
                *last_poll_hash = hash_str(&new_content);
                write_file_atomic(file_path, &new_content, suppress).await?;
                info!("synced {} bytes to {}", new_content.len(), file_path.display());
            }
        }
        None => {
            debug!("ignoring unknown message ({} bytes)", data.len());
        }
    }
    Ok(())
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
