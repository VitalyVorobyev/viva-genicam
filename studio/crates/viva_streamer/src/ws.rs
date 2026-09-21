//! WebSocket server that fans out the latest BMP frame to all clients.

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use bytes::Bytes;
use serde::Serialize;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::error::StreamerError;

#[derive(Clone, Debug, Serialize)]
pub struct StreamInfo {
    pub width: u32,
    pub height: u32,
    pub pixel_format: String,
    pub encoding: &'static str,
    #[serde(rename = "type")]
    pub frame_type: &'static str,
}

impl StreamInfo {
    pub fn from_image_meta(meta: &viva_zenoh_api::ImageMeta) -> Self {
        let pixel_format = serde_json::to_string(&meta.pixel_format)
            .unwrap_or_else(|_| "\"Unknown\"".to_owned())
            .trim_matches('"')
            .to_owned();
        Self {
            width: meta.width,
            height: meta.height,
            pixel_format,
            encoding: "BMP",
            frame_type: "info",
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                r#"{{"type":"info","width":{},"height":{},"pixel_format":"{}","encoding":"{}"}}"#,
                self.width, self.height, self.pixel_format, self.encoding
            )
        })
    }
}

#[derive(Clone)]
pub struct AppState {
    pub frame_tx: watch::Sender<Bytes>,
    pub info_tx: watch::Sender<StreamInfo>,
}

pub async fn run_server(
    bind: SocketAddr,
    path: String,
    state: AppState,
    shutdown: watch::Receiver<bool>,
) -> Result<(), StreamerError> {
    let listener = TcpListener::bind(bind).await?;
    info!("WebSocket server listening on ws://{bind}{path}");
    run_server_with_listener(listener, path, state, shutdown).await
}

/// Run the WebSocket server with a pre-bound listener (for embedding).
pub async fn run_server_with_listener(
    listener: TcpListener,
    path: String,
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), StreamerError> {
    let app = Router::new()
        .route(&path, get(ws_handler))
        .with_state(state);

    let shutdown_signal = async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            if shutdown.changed().await.is_err() {
                break;
            }
        }
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await
        .map_err(|err| StreamerError::Server(err.to_string()))?;

    Ok(())
}

async fn ws_handler(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut frame_rx = state.frame_tx.subscribe();
    let mut info_rx = state.info_tx.subscribe();
    let mut logged_first_bmp_send = false;

    info!("WebSocket client connected");

    // Send the current info frame immediately on connect.
    let info_json = info_rx.borrow().to_json();
    if socket.send(Message::Text(info_json.into())).await.is_err() {
        return;
    }

    // If a frame is already available, send it immediately.
    let initial = frame_rx.borrow().clone();
    if !initial.is_empty() && socket.send(Message::Binary(initial.clone())).await.is_err() {
        return;
    } else if !initial.is_empty() {
        info!(
            bytes = initial.len(),
            "Sent initial BMP frame to WebSocket client"
        );
        logged_first_bmp_send = true;
    }

    loop {
        tokio::select! {
            result = frame_rx.changed() => {
                if result.is_err() {
                    break;
                }
                let frame = frame_rx.borrow_and_update().clone();
                if frame.is_empty() {
                    continue;
                }
                if socket.send(Message::Binary(frame.clone())).await.is_err() {
                    warn!("WebSocket client disconnected");
                    break;
                } else if !logged_first_bmp_send {
                    info!(
                        bytes = frame.len(),
                        "Sent first live BMP frame to WebSocket client"
                    );
                    logged_first_bmp_send = true;
                }
            }

            result = info_rx.changed() => {
                if result.is_err() {
                    break;
                }
                let info_json = info_rx.borrow_and_update().to_json();
                if socket.send(Message::Text(info_json.into())).await.is_err() {
                    warn!("WebSocket client disconnected while sending info frame");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viva_zenoh_api::{ImageMeta, PixelFormat};

    fn make_meta(pixel_format: PixelFormat, width: u32, height: u32) -> ImageMeta {
        ImageMeta {
            pixel_format,
            width,
            height,
            payload_size: (width as u64) * (height as u64),
        }
    }

    #[test]
    fn test_stream_info_to_json_mono8() {
        let meta = make_meta(PixelFormat::Mono8, 640, 480);
        let info = StreamInfo::from_image_meta(&meta);
        let json = info.to_json();
        assert!(
            json.contains("\"type\":\"info\""),
            "type field missing: {json}"
        );
        assert!(
            json.contains("\"pixel_format\":\"Mono8\""),
            "pixel_format wrong: {json}"
        );
        assert!(json.contains("\"width\":640"), "width wrong: {json}");
        assert!(json.contains("\"height\":480"), "height wrong: {json}");
        assert!(
            json.contains("\"encoding\":\"BMP\""),
            "encoding wrong: {json}"
        );
    }

    #[test]
    fn test_stream_info_to_json_rgb8() {
        let meta = make_meta(PixelFormat::RGB8, 1920, 1080);
        let info = StreamInfo::from_image_meta(&meta);
        let json = info.to_json();
        assert!(
            json.contains("\"pixel_format\":\"RGB8\""),
            "pixel_format wrong: {json}"
        );
        assert!(json.contains("\"width\":1920"), "width wrong: {json}");
        assert!(json.contains("\"height\":1080"), "height wrong: {json}");
    }

    #[test]
    fn test_stream_info_to_json_unknown() {
        let meta = make_meta(PixelFormat::Unknown, 320, 240);
        // Must not panic — serialisation of Unknown variant falls back gracefully.
        let info = StreamInfo::from_image_meta(&meta);
        let json = info.to_json();
        // The pixel_format field should be present and non-empty.
        assert!(
            json.contains("\"pixel_format\":"),
            "pixel_format field missing: {json}"
        );
        assert!(
            json.contains("\"type\":\"info\""),
            "type field missing: {json}"
        );
    }
}
