use entangle::{crdt, room, session};

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use tracing::info;

use crdt::CrdtEngine;
use session::{run_session, SessionConfig};

#[derive(Parser)]
#[command(
    name = "entangle",
    about = "Real-time collaborative text file sync via CRDT",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable debug logging
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Share a local file with others
    Share {
        /// Path to the file to share
        file: PathBuf,

        /// y-websocket relay URL (e.g. wss://relay.example.com)
        #[arg(long)]
        server: String,

        /// Override the room name (default: auto-generated)
        #[arg(long)]
        room: Option<String>,

        /// Debounce interval for file watcher (ms)
        #[arg(long, default_value = "300")]
        debounce: u64,

        /// Fallback poll interval (ms)
        #[arg(long, default_value = "2000")]
        poll_interval: u64,
    },

    /// Join a shared file by its link
    Join {
        /// Share link (e.g. wss://relay.example.com/r/<room-id>)
        url: String,

        /// Local path to write the synced file
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Debounce interval for file watcher (ms)
        #[arg(long, default_value = "300")]
        debounce: u64,

        /// Fallback poll interval (ms)
        #[arg(long, default_value = "2000")]
        poll_interval: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let cli = Cli::parse();

    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    match cli.command {
        Commands::Share {
            file,
            server,
            room,
            debounce,
            poll_interval,
        } => share(file, server, room, debounce, poll_interval).await,
        Commands::Join {
            url,
            output,
            debounce,
            poll_interval,
        } => join(url, output, debounce, poll_interval).await,
    }
}

async fn share(
    file: PathBuf,
    server: String,
    room: Option<String>,
    debounce: u64,
    poll_interval: u64,
) -> Result<()> {
    if !file.exists() {
        bail!("file not found: {}", file.display());
    }

    let content = tokio::fs::read_to_string(&file)
        .await
        .with_context(|| format!("failed to read {}", file.display()))?;

    let room_id = room.unwrap_or_else(room::generate_room_id);
    let ws_url = build_ws_url(&server, &room_id)?;
    let share_link = room::build_share_link(&server, &room_id);

    let file_size = content.len();
    println!("Watching {} ({file_size} bytes)", file.display());
    println!("Connected to relay.");
    println!("Share link: {share_link}");
    println!();
    println!("Waiting for peers… (Ctrl+C to stop)");

    let mut crdt = CrdtEngine::new();
    crdt.seed(&content);

    run_session(
        SessionConfig {
            ws_url,
            file_path: file,
            debounce_ms: debounce,
            poll_interval_ms: poll_interval,
        },
        crdt,
    )
    .await
}

async fn join(
    url: String,
    output: Option<PathBuf>,
    debounce: u64,
    poll_interval: u64,
) -> Result<()> {
    let output_path = match output {
        Some(p) => p,
        None => {
            let room_id = room::parse_room_id(&url)
                .context("could not determine output path; pass --output")?;
            PathBuf::from(format!("{room_id}.txt"))
        }
    };

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    if !output_path.exists() {
        tokio::fs::write(&output_path, "").await?;
    }

    info!("joining {url}");
    println!("Connected to relay. Syncing…");
    println!("Wrote {}", output_path.display());
    println!("Watching for changes… (Ctrl+C to stop)");

    let crdt = CrdtEngine::new();

    run_session(
        SessionConfig {
            ws_url: url,
            file_path: output_path,
            debounce_ms: debounce,
            poll_interval_ms: poll_interval,
        },
        crdt,
    )
    .await
}

fn build_ws_url(server: &str, room_id: &str) -> Result<String> {
    let base = server.trim_end_matches('/');
    let parsed =
        url::Url::parse(base).with_context(|| format!("invalid server URL: {base}"))?;
    if !matches!(parsed.scheme(), "ws" | "wss") {
        bail!(
            "server URL must use ws:// or wss:// scheme, got: {}",
            parsed.scheme()
        );
    }
    Ok(format!("{base}/r/{room_id}"))
}
