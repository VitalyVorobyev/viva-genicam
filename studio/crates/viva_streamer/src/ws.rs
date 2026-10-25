//! WebSocket server that fans out the latest BMP frame to all clients.

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
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
        let pixel_format = meta.pixel_format.name().unwrap_or("Unknown").to_owned();
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
    pub native: crate::native::NativeCapture,
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
    let mut app = Router::new().route(&path, get(ws_handler));
    if listener.local_addr()?.ip().is_loopback() {
        app = app.route(&format!("{path}/native"), get(native_handler));
    }
    let app = app.with_state(state);

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

/// Restrict browser origins to the known app/development renderer origins.
/// This is not authentication: native local clients can forge Origin. Keep
/// this route loopback-only and never use wildcard CORS for camera data.
fn allowed_origin(origin: &str) -> bool {
    matches!(
        origin,
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    ) || (cfg!(debug_assertions)
        && matches!(origin, "http://localhost:5183" | "http://127.0.0.1:5183"))
}

async fn native_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> axum::response::Response {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .filter(|origin| allowed_origin(origin))
    else {
        return (
            StatusCode::FORBIDDEN,
            "Native capture requires the Studio renderer origin",
        )
            .into_response();
    };
    let mut response = native_capture(state).await;
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_str(origin).expect("validated origin"),
    );
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Origin"));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn native_capture(state: AppState) -> axum::response::Response {
    let capture = state.native.clone();
    let Ok(permit) = capture.permit.clone().try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "A native capture is already being encoded",
        )
            .into_response();
    };
    let source = match capture.latest() {
        Ok(Some(source)) => source,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "No native source frame is available").into_response();
        }
        Err(error) => return (StatusCode::UNPROCESSABLE_ENTITY, error).into_response(),
    };
    let encoded =
        tokio::task::spawn_blocking(move || (crate::native::encode(&source), permit)).await;
    match encoded {
        Ok((Ok(bytes), permit)) => {
            // Bytes clones and slices retain this owner through body/network
            // consumption: a slow client cannot release the permit early.
            let bytes = Bytes::from_owner(CaptureResponse {
                bytes,
                _permit: permit,
            });
            ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response()
        }
        Ok((Err(error), _permit)) => (StatusCode::UNPROCESSABLE_ENTITY, error).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Native capture could not be encoded",
        )
            .into_response(),
    }
}

struct CaptureResponse {
    bytes: Bytes,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl AsRef<[u8]> for CaptureResponse {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
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
    use viva_pfnc::PixelFormat;
    use viva_zenoh_api::ImageMeta;

    #[test]
    fn native_capture_accepts_only_explicit_renderer_origins() {
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ] {
            assert!(allowed_origin(origin));
        }
        for origin in [
            "null",
            "https://example.com",
            "http://localhost:5183.evil",
            "http://localhost:8080",
            "",
        ] {
            assert!(!allowed_origin(origin));
        }
    }

    #[tokio::test]
    async fn native_capture_requires_a_renderer_and_a_source_and_one_encoder() {
        let (frame_tx, _) = watch::channel(Bytes::new());
        let info_tx = watch::channel(StreamInfo::from_image_meta(&make_meta(
            PixelFormat::Mono8,
            1,
            1,
        )))
        .0;
        let state = AppState {
            frame_tx,
            info_tx,
            native: Default::default(),
        };
        assert_eq!(
            native_handler(State(state.clone()), HeaderMap::new())
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("tauri://localhost"),
        );
        assert_eq!(
            native_handler(State(state.clone()), headers.clone())
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        let _permit = state.native.permit.clone().try_acquire_owned().unwrap();
        assert_eq!(
            native_handler(State(state), headers).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[tokio::test]
    async fn response_bytes_and_their_clones_retain_the_capture_permit() {
        let (frame_tx, _) = watch::channel(Bytes::new());
        let info_tx = watch::channel(StreamInfo::from_image_meta(&make_meta(
            PixelFormat::Mono8,
            1,
            1,
        )))
        .0;
        let state = AppState {
            frame_tx,
            info_tx,
            native: Default::default(),
        };
        let bmp = crate::frame::FrameEncoder::new()
            .encode(PixelFormat::Mono8, 1, 1, None, &[7])
            .unwrap();
        state.native.replace(crate::native::SourceFrame {
            format: PixelFormat::Mono8,
            width: 1,
            height: 1,
            row_stride: None,
            pixels: Bytes::from_static(&[7]),
            bmp,
        });
        let response = native_capture(state.clone()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            native_capture(state.clone()).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let held_by_slow_client = bytes.clone();
        drop(bytes);
        assert_eq!(
            native_capture(state.clone()).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(held_by_slow_client);
        assert_eq!(native_capture(state).await.status(), StatusCode::OK);
    }

    fn make_meta(pixel_format: PixelFormat, width: u32, height: u32) -> ImageMeta {
        ImageMeta::new(
            pixel_format,
            width,
            height,
            (width as u64) * (height as u64),
        )
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
        let meta = make_meta(PixelFormat::RGB8Packed, 1920, 1080);
        let info = StreamInfo::from_image_meta(&meta);
        let json = info.to_json();
        assert!(
            json.contains("\"pixel_format\":\"RGB8Packed\""),
            "pixel_format wrong: {json}"
        );
        assert!(json.contains("\"width\":1920"), "width wrong: {json}");
        assert!(json.contains("\"height\":1080"), "height wrong: {json}");
    }

    #[test]
    fn test_stream_info_to_json_unknown() {
        let meta = make_meta(PixelFormat::Unknown(0), 320, 240);
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
