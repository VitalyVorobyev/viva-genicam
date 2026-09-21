use std::sync::Arc;

use clap::Parser;
use std::net::SocketAddr;
use tokio::sync::{RwLock, watch};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use viva_streamer::error::StreamerError;
use viva_streamer::ws::{AppState, StreamInfo};
use viva_streamer::zenoh_source::ZenohSourceConfig;
use viva_streamer::{meta, ws, zenoh_source};

#[derive(Parser, Debug)]
#[command(
    name = "viva-ws-streamer",
    about = "Zenoh -> BMP -> WebSocket streamer"
)]
struct Cli {
    /// Zenoh key expression to subscribe to (Mono8 bytes)
    #[arg(long, value_name = "KEYEXPR")]
    image_key: String,

    /// Initial image width hint in pixels (overridden by image/meta subscription)
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..), default_value_t = 640)]
    width: u32,

    /// Initial image height hint in pixels (overridden by image/meta subscription)
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..), default_value_t = 480)]
    height: u32,

    /// Bind address for the WebSocket server
    #[arg(long, default_value = "127.0.0.1:8081")]
    bind: String,

    /// WebSocket path
    #[arg(long, default_value = "/ws")]
    path: String,

    /// Optional FPS limit (drop frames above this rate)
    #[arg(long)]
    fps_limit: Option<u32>,

    /// Zenoh configuration (JSON5 string or file path)
    #[arg(long)]
    zenoh_config: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), StreamerError> {
    init_tracing();
    let cli = Cli::parse();

    let bind: SocketAddr = cli
        .bind
        .parse()
        .map_err(|err| StreamerError::Config(format!("Invalid bind address: {err}")))?;
    let path = normalize_path(cli.path);

    let config = load_zenoh_config(cli.zenoh_config.as_deref())?;

    let shared_meta = Arc::new(RwLock::new(meta::default_image_meta(cli.width, cli.height)));
    let meta_key = meta::derive_meta_key(&cli.image_key);

    let initial_info = StreamInfo {
        width: cli.width,
        height: cli.height,
        pixel_format: "Mono8".to_owned(),
        encoding: "BMP",
        frame_type: "info",
    };

    let (frame_tx, _frame_rx) = watch::channel(bytes::Bytes::new());
    let (info_tx, _info_rx) = watch::channel(initial_info);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    info!(
        "Starting streamer: initial {}x{} Mono8 -> ws://{}{} (meta key: {})",
        cli.width, cli.height, bind, path, meta_key
    );

    let ws_state = AppState {
        frame_tx: frame_tx.clone(),
        info_tx: info_tx.clone(),
    };
    let ws_shutdown = shutdown_rx.clone();
    let ws_task = tokio::spawn(async move {
        if let Err(err) = ws::run_server(bind, path, ws_state, ws_shutdown).await {
            error!("WebSocket server error: {err}");
        }
    });

    let source_config = ZenohSourceConfig {
        key_expr: cli.image_key,
        meta_key,
        fps_limit: cli.fps_limit,
    };
    let zenoh_shutdown = shutdown_rx.clone();
    let zenoh_meta = Arc::clone(&shared_meta);
    let zenoh_task = tokio::spawn(async move {
        if let Err(err) = zenoh_source::run(
            config,
            source_config,
            zenoh_meta,
            frame_tx,
            info_tx,
            zenoh_shutdown,
        )
        .await
        {
            error!("Zenoh source error: {err}");
        }
    });

    tokio::signal::ctrl_c().await?;
    info!("Shutdown requested (CTRL+C)");
    let _ = shutdown_tx.send(true);

    let _ = ws_task.await;
    let _ = zenoh_task.await;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn normalize_path(path: String) -> String {
    if path.starts_with('/') {
        path
    } else {
        format!("/{path}")
    }
}

fn load_zenoh_config(input: Option<&str>) -> Result<zenoh::Config, StreamerError> {
    match input {
        None => Ok(zenoh::Config::default()),
        Some(value) => {
            let path = std::path::Path::new(value);
            if path.exists() {
                Ok(zenoh::Config::from_file(path)?)
            } else {
                Ok(zenoh::Config::from_json5(value)?)
            }
        }
    }
}
