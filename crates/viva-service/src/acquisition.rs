//! Acquisition control queryable and frame streaming over Zenoh.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use viva_genicam::FrameStream;
use viva_genicam::gige::nic::Iface;
use viva_zenoh_api::frame_header::FrameHeader;
use viva_zenoh_api::{
    AcquisitionCommand, AcquisitionControlRequest, AcquisitionStatus, ImageMeta, NodeOpResponse,
    keys,
};
use zenoh::Session;

use crate::device::DeviceHandle;
use crate::pixel_format::pfnc_to_zenoh;

/// Run the acquisition control queryable and frame streaming loop.
pub async fn run(
    session: Arc<Session>,
    device: Arc<DeviceHandle>,
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
    info!(device_id, key, "acquisition control queryable ready");

    // Channel to signal the frame streaming task to stop.
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
                                                &session,
                                                &device,
                                                &device_id,
                                                &stop_tx,
                                                &mut frame_task,
                                            )
                                            .await
                                        }
                                        AcquisitionCommand::Start => {
                                            NodeOpResponse {
                                                ok: false,
                                                error: Some("acquisition already active".to_string()),
                                            }
                                        }
                                        AcquisitionCommand::Stop if frame_task.is_some() => {
                                            handle_stop(
                                                &session,
                                                &device,
                                                &device_id,
                                                &stop_tx,
                                                &mut frame_task,
                                            )
                                            .await
                                        }
                                        AcquisitionCommand::Stop => {
                                            NodeOpResponse {
                                                ok: false,
                                                error: Some("acquisition not active".to_string()),
                                            }
                                        }
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
                            tracing::error!("failed to serialize acquisition response");
                            continue;
                        };
                        let _ = query.reply(&key, payload).await;
                    }
                    Err(_) => break,
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    // Stop acquisition on shutdown.
                    if frame_task.is_some() {
                        handle_stop(&session, &device, &device_id, &stop_tx, &mut frame_task).await;
                    }
                    break;
                }
            }
        }
    }
}

async fn handle_start(
    session: &Arc<Session>,
    device: &Arc<DeviceHandle>,
    device_id: &str,
    stop_tx: &watch::Sender<bool>,
    frame_task: &mut Option<tokio::task::JoinHandle<()>>,
) -> NodeOpResponse {
    // Best-effort refresh before stream setup. On macOS loopback the fake
    // Aravis camera can sit discovered/idle for a while and then fail to
    // deliver GVSP until the control channel is reopened. Immediate-start
    // cases can still work with the existing control channel, so a refresh
    // timeout falls back to the current connection instead of failing start.
    if cfg!(target_os = "macos") {
        if let Err(e) = device.refresh_connection().await {
            warn!(
                device_id,
                error = %e,
                "camera reconnect before acquisition failed; continuing with existing control connection"
            );
        }
    }

    // 1. Resolve the network interface for GVSP reception.
    let iface = match resolve_iface(device) {
        Ok(i) => i,
        Err(e) => {
            return NodeOpResponse {
                ok: false,
                error: Some(format!("interface resolution failed: {e}")),
            };
        }
    };

    // 2. Configure stream registers (SCDA/SCPH/SCPS) and bind socket using
    //    the CCP-holding device. This must happen BEFORE AcquisitionStart so
    //    the camera knows where to send GVSP packets.
    let mut frame_stream = match device.build_stream(iface).await {
        Ok(fs) => fs,
        Err(e) => {
            return NodeOpResponse {
                ok: false,
                error: Some(format!("stream build failed: {e}")),
            };
        }
    };

    // 2b. Immediate heartbeat to refresh the CCP timer after the potentially
    //     long stream-setup phase, before the 3 sequential get_feature calls
    //     in publish_image_meta.
    if let Err(e) = device.heartbeat_ping().await {
        warn!(device_id, error = %e, "heartbeat after build_stream failed (non-fatal)");
    }

    // 3. Publish metadata before starting acquisition.
    publish_image_meta(session, device.as_ref(), device_id).await;

    let _ = stop_tx.send(false);
    let stop_rx = stop_tx.subscribe();

    let session_clone = session.clone();
    let device_id_owned = device_id.to_string();

    let handle = tokio::spawn(async move {
        frame_loop(session_clone, device_id_owned, &mut frame_stream, stop_rx).await;
    });

    // Let the spawned task reach its first `next_frame()` poll before the
    // camera starts emitting GVSP. Without this yield, `tokio::spawn` only
    // schedules the task; on macOS loopback the fake camera can otherwise
    // outrun the receiver and the stream may never recover.
    tokio::task::yield_now().await;

    // 4. Start acquisition only after the frame loop is armed.
    //
    // On macOS loopback, the fake Aravis camera can start emitting GVSP
    // immediately after AcquisitionStart. If the reader task is spawned only
    // afterwards, the initial leader/payload packets can be missed and the
    // stream may never recover in practice. Arming the reader first keeps the
    // UDP socket draining from the first packet onward.
    if let Err(e) = device.exec_command("AcquisitionStart").await {
        handle.abort();
        let _ = handle.await;
        return NodeOpResponse {
            ok: false,
            error: Some(format!("AcquisitionStart failed: {e}")),
        };
    }

    // 5. Publish active status now that the device is streaming.
    publish_status(session, device_id, true).await;

    info!(device_id, "acquisition started, frame loop armed");

    *frame_task = Some(handle);

    NodeOpResponse {
        ok: true,
        error: None,
    }
}

async fn handle_stop(
    session: &Arc<Session>,
    device: &Arc<DeviceHandle>,
    device_id: &str,
    stop_tx: &watch::Sender<bool>,
    frame_task: &mut Option<tokio::task::JoinHandle<()>>,
) -> NodeOpResponse {
    // Signal the frame loop to stop.
    let _ = stop_tx.send(true);

    // Wait for the frame task to finish.
    if let Some(task) = frame_task.take() {
        let _ = task.await;
    }

    // Stop acquisition on the camera.
    if let Err(e) = device.exec_command("AcquisitionStop").await {
        warn!(device_id, error = %e, "AcquisitionStop failed");
    }

    publish_status(session, device_id, false).await;
    info!(device_id, "acquisition stopped");

    NodeOpResponse {
        ok: true,
        error: None,
    }
}

/// Main frame reading loop: reads frames from GigE stream and publishes to Zenoh.
async fn frame_loop(
    session: Arc<Session>,
    device_id: String,
    frame_stream: &mut FrameStream,
    mut stop: watch::Receiver<bool>,
) {
    let image_key = keys::image(&device_id);
    let status_key = keys::acquisition_status(&device_id);
    let mut seq: u32 = 0;
    let mut frames_acquired: u64 = 0;
    let mut fps_start = tokio::time::Instant::now();
    let mut fps_frame_count: u64 = 0;
    let fps_interval = std::time::Duration::from_secs(1);
    let mut logged_first_gvsp_frame = false;
    let mut logged_first_image_publish = false;
    let mut logged_payload_trim = false;

    loop {
        tokio::select! {
            result = frame_stream.next_frame() => {
                match result {
                    Ok(Some(frame)) => {
                        if !logged_first_gvsp_frame {
                            info!(
                                device_id,
                                width = frame.width,
                                height = frame.height,
                                pixel_format = ?frame.pixel_format,
                                payload = frame.payload.len(),
                                "first GVSP frame received"
                            );
                            logged_first_gvsp_frame = true;
                        }

                        let zenoh_pf = pfnc_to_zenoh(frame.pixel_format);
                        let expected_payload_len =
                            ((frame.width as f32 * frame.height as f32) * zenoh_pf.bytes_per_pixel())
                                as usize;
                        let image_bytes = if frame.payload.len() < expected_payload_len {
                            warn!(
                                device_id,
                                seq,
                                actual = frame.payload.len(),
                                expected = expected_payload_len,
                                "dropping undersized frame payload"
                            );
                            continue;
                        } else if frame.payload.len() > expected_payload_len {
                            if !logged_payload_trim {
                                warn!(
                                    device_id,
                                    actual = frame.payload.len(),
                                    expected = expected_payload_len,
                                    "trimming trailing bytes from frame payload before Zenoh publish"
                                );
                                logged_payload_trim = true;
                            }
                            &frame.payload[..expected_payload_len]
                        } else {
                            frame.payload.as_ref()
                        };

                        let header = FrameHeader {
                            pixel_format: zenoh_pf,
                            width: frame.width,
                            height: frame.height,
                            seq,
                        };
                        let encoded_header = header.encode();

                        let mut payload = Vec::with_capacity(
                            encoded_header.len() + image_bytes.len(),
                        );
                        payload.extend_from_slice(&encoded_header);
                        payload.extend_from_slice(image_bytes);
                        let payload_len = payload.len();

                        if let Err(e) = session.put(&image_key, payload).await {
                            warn!(device_id, error = %e, "failed to publish frame");
                        } else if !logged_first_image_publish {
                            info!(
                                device_id,
                                seq,
                                bytes = payload_len,
                                "published first image frame to Zenoh"
                            );
                            logged_first_image_publish = true;
                        }

                        seq = seq.wrapping_add(1);
                        frames_acquired += 1;
                        fps_frame_count += 1;

                        // Publish FPS periodically.
                        let elapsed = fps_start.elapsed();
                        if elapsed >= fps_interval {
                            let fps = fps_frame_count as f32 / elapsed.as_secs_f32();
                            let status = AcquisitionStatus {
                                active: true,
                                fps: Some(fps),
                                dropped: 0,
                            };
                            if let Ok(payload) = serde_json::to_vec(&status) {
                                let _ = session.put(&status_key, payload).await;
                            }
                            fps_start = tokio::time::Instant::now();
                            fps_frame_count = 0;
                            debug!(device_id, fps, frames_acquired, "streaming");
                        }
                    }
                    Ok(None) => {
                        info!(device_id, "frame stream ended");
                        break;
                    }
                    Err(e) => {
                        error!(device_id, error = %e, "frame stream error");
                        break;
                    }
                }
            }
            _ = stop.changed() => {
                if *stop.borrow() {
                    debug!(device_id, "frame loop stop signal received");
                    break;
                }
            }
        }
    }

    info!(device_id, frames_acquired, "frame loop exited");
}

fn resolve_iface(device: &DeviceHandle) -> Result<Iface, String> {
    match device.iface_name() {
        Some(name) => Iface::from_system(name).map_err(|e| e.to_string()),
        None => {
            // Try to resolve the interface from the camera's IP.
            Iface::from_ipv4(device.info().ip).map_err(|e| e.to_string())
        }
    }
}

/// Publish acquisition status to Zenoh.
pub async fn publish_status(session: &Session, device_id: &str, active: bool) {
    let status = AcquisitionStatus {
        active,
        fps: None,
        dropped: 0,
    };
    let key = keys::acquisition_status(device_id);
    if let Ok(payload) = serde_json::to_vec(&status) {
        let _ = session.put(&key, payload).await;
    }
}

/// Publish image metadata (width, height, pixel format) to Zenoh.
pub async fn publish_image_meta<D: crate::device::DeviceOps>(
    session: &Session,
    device: &D,
    device_id: &str,
) {
    let width = device
        .get_feature("Width")
        .await
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(640);
    let height = device
        .get_feature("Height")
        .await
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(480);
    let pf_str = device
        .get_feature("PixelFormat")
        .await
        .unwrap_or_else(|_| "Mono8".to_string());

    let pixel_format: viva_zenoh_api::PixelFormat =
        serde_json::from_value(serde_json::Value::String(pf_str))
            .unwrap_or(viva_zenoh_api::PixelFormat::Mono8);

    let payload_size = (width as u64) * (height as u64) * (pixel_format.bytes_per_pixel() as u64);

    let meta = ImageMeta {
        pixel_format,
        width,
        height,
        payload_size,
    };

    let key = keys::image_meta(device_id);
    if let Ok(payload) = serde_json::to_vec(&meta) {
        if let Err(e) = session.put(&key, payload).await {
            warn!(device_id, error = %e, "failed to publish image meta");
        } else {
            info!(device_id, width, height, "published image metadata");
        }
    }
}
