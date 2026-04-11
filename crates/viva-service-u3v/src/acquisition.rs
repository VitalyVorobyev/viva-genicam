//! Acquisition control queryable and frame streaming over Zenoh for U3V.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{error, info, warn};
use viva_service::acquisition::{publish_image_meta, publish_status};
use viva_service::device::DeviceOps;
use viva_service::pixel_format::pfnc_to_zenoh;
use viva_zenoh_api::frame_header::FrameHeader;
use viva_zenoh_api::{AcquisitionCommand, AcquisitionControlRequest, NodeOpResponse, keys};
use zenoh::Session;

use crate::device::U3vDeviceHandle;
use viva_u3v::usb::UsbTransfer;

/// Run the acquisition control queryable for a U3V device.
pub async fn run<T: UsbTransfer + 'static>(
    session: Arc<Session>,
    device: Arc<U3vDeviceHandle<T>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let device_id = device.device_id().to_string();
    let key = keys::acquisition_control(&device_id);
    let queryable = match session.declare_queryable(&key).await {
        Ok(q) => q,
        Err(e) => {
            warn!(device_id, error = %e, "failed to declare acquisition queryable");
            return;
        }
    };
    info!(device_id, key, "U3V acquisition control queryable ready");

    let (stop_tx, _stop_rx) = watch::channel(false);
    let mut frame_task: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        tokio::select! {
            query = queryable.recv_async() => {
                match query {
                    Ok(query) => {
                        let response = match query.payload() {
                            Some(payload) => {
                                match serde_json::from_slice::<AcquisitionControlRequest>(
                                    &payload.to_bytes(),
                                ) {
                                    Ok(req) => match req.command {
                                        AcquisitionCommand::Start if frame_task.is_none() => {
                                            handle_start(
                                                &session, &device, &device_id,
                                                &stop_tx, &mut frame_task,
                                            ).await
                                        }
                                        AcquisitionCommand::Start => NodeOpResponse {
                                            ok: false,
                                            error: Some("acquisition already active".to_string()),
                                        },
                                        AcquisitionCommand::Stop if frame_task.is_some() => {
                                            handle_stop(
                                                &session, &device, &device_id,
                                                &stop_tx, &mut frame_task,
                                            ).await
                                        }
                                        AcquisitionCommand::Stop => NodeOpResponse {
                                            ok: false,
                                            error: Some("acquisition not active".to_string()),
                                        },
                                    },
                                    Err(e) => NodeOpResponse {
                                        ok: false,
                                        error: Some(format!("invalid payload: {e}")),
                                    },
                                }
                            }
                            None => NodeOpResponse {
                                ok: false,
                                error: Some("missing payload".to_string()),
                            },
                        };
                        let Ok(payload) = serde_json::to_vec(&response) else {
                            error!("failed to serialize acquisition response");
                            continue;
                        };
                        let _ = query.reply(&key, payload).await;
                    }
                    Err(_) => break,
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    if frame_task.is_some() {
                        handle_stop(&session, &device, &device_id, &stop_tx, &mut frame_task).await;
                    }
                    break;
                }
            }
        }
    }
}

async fn handle_start<T: UsbTransfer + 'static>(
    session: &Arc<Session>,
    device: &Arc<U3vDeviceHandle<T>>,
    device_id: &str,
    stop_tx: &watch::Sender<bool>,
    frame_task: &mut Option<tokio::task::JoinHandle<()>>,
) -> NodeOpResponse {
    // Resolve payload size from camera features.
    let width: u32 = device
        .get_feature("Width")
        .await
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(640);
    let height: u32 = device
        .get_feature("Height")
        .await
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(480);
    let pf_str = device
        .get_feature("PixelFormat")
        .await
        .unwrap_or_else(|_| "Mono8".to_string());
    let bpp: usize = match pf_str.as_str() {
        "RGB8Packed" | "RGB8" | "BGR8" | "BGR8Packed" => 3,
        _ => 1,
    };
    let payload_size = (width as usize) * (height as usize) * bpp;

    // Open U3V stream.
    let mut stream = match device.open_stream(payload_size) {
        Some(s) => s,
        None => {
            return NodeOpResponse {
                ok: false,
                error: Some("device has no streaming endpoint".to_string()),
            };
        }
    };

    // Publish image metadata.
    publish_image_meta(session, device.as_ref(), device_id).await;

    let _ = stop_tx.send(false);
    let stop_rx = stop_tx.subscribe();

    let session_clone = session.clone();
    let device_id_owned = device_id.to_string();

    // Spawn the frame loop. U3V streaming is synchronous (USB bulk reads),
    // so the inner loop runs in spawn_blocking and sends frames through a
    // channel to the async Zenoh publisher.
    let handle = tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<FrameData>(4);

        // Reader thread: blocking U3V bulk reads.
        // With a real USB camera, bulk_read blocks until a transfer completes,
        // naturally rate-limiting to the camera's frame rate. The fake transport
        // returns instantly, so we throttle to ~30 fps to avoid spinning.
        let reader = tokio::task::spawn_blocking(move || {
            let frame_interval = std::time::Duration::from_millis(33); // ~30 fps
            loop {
                // Check stop flag (non-blocking).
                if *stop_rx.borrow() {
                    break;
                }
                let t0 = std::time::Instant::now();
                match stream.next_frame() {
                    Ok(frame) => {
                        if tx
                            .blocking_send(FrameData {
                                width: frame.leader.width,
                                height: frame.leader.height,
                                pixel_format: frame.leader.pixel_format,
                                payload: frame.payload.to_vec(),
                            })
                            .is_err()
                        {
                            break; // receiver dropped
                        }
                        // Throttle: sleep for remainder of frame interval.
                        let elapsed = t0.elapsed();
                        if elapsed < frame_interval {
                            std::thread::sleep(frame_interval - elapsed);
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "U3V stream error");
                        break;
                    }
                }
            }
        });

        // Publisher loop: encode and publish frames to Zenoh.
        let image_key = keys::image(&device_id_owned);
        let mut seq: u32 = 0;
        let mut logged_first = false;

        while let Some(frame) = rx.recv().await {
            let zenoh_pf = pfnc_to_zenoh(viva_genicam::pfnc::PixelFormat::from_code(
                frame.pixel_format,
            ));

            if !logged_first {
                info!(
                    device_id = device_id_owned,
                    width = frame.width,
                    height = frame.height,
                    payload = frame.payload.len(),
                    "first U3V frame received"
                );
                logged_first = true;
            }

            let header = FrameHeader {
                pixel_format: zenoh_pf,
                width: frame.width,
                height: frame.height,
                seq,
            };
            let encoded_header = header.encode();
            let mut payload = Vec::with_capacity(encoded_header.len() + frame.payload.len());
            payload.extend_from_slice(&encoded_header);
            payload.extend_from_slice(&frame.payload);

            if let Err(e) = session_clone.put(&image_key, payload).await {
                warn!(error = %e, "failed to publish frame");
            }

            seq = seq.wrapping_add(1);
        }

        let _ = reader.await;
        info!(device_id = device_id_owned, seq, "U3V frame loop exited");
    });

    // Execute AcquisitionStart on the camera.
    if let Err(e) = device.exec_command("AcquisitionStart").await {
        handle.abort();
        let _ = handle.await;
        return NodeOpResponse {
            ok: false,
            error: Some(format!("AcquisitionStart failed: {e}")),
        };
    }

    publish_status(session, device_id, true).await;
    info!(device_id, "U3V acquisition started");

    *frame_task = Some(handle);
    NodeOpResponse {
        ok: true,
        error: None,
    }
}

async fn handle_stop<T: UsbTransfer + 'static>(
    session: &Arc<Session>,
    device: &Arc<U3vDeviceHandle<T>>,
    device_id: &str,
    stop_tx: &watch::Sender<bool>,
    frame_task: &mut Option<tokio::task::JoinHandle<()>>,
) -> NodeOpResponse {
    let _ = stop_tx.send(true);

    if let Some(task) = frame_task.take() {
        let _ = task.await;
    }

    if let Err(e) = device.exec_command("AcquisitionStop").await {
        warn!(device_id, error = %e, "AcquisitionStop failed");
    }

    publish_status(session, device_id, false).await;
    info!(device_id, "U3V acquisition stopped");

    NodeOpResponse {
        ok: true,
        error: None,
    }
}

/// Intermediate frame data passed from the blocking reader to the async publisher.
struct FrameData {
    width: u32,
    height: u32,
    pixel_format: u32,
    payload: Vec<u8>,
}
