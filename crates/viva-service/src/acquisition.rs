//! Acquisition control queryable and frame streaming over Zenoh.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use viva_genicam::gige::nic::Iface;
use viva_genicam::{EvsBlock, EvsFormat, Frame, FrameStream, GenericStreamBlock, StreamBlock};
use viva_zenoh_api::frame_header::FrameHeader;
use viva_zenoh_api::{
    AcquisitionCommand, AcquisitionControlRequest, AcquisitionStatus, EvsFormat as ZenohEvsFormat,
    EvsHeader, ImageMeta, NodeOpResponse, keys,
};
use zenoh::Session;

use crate::device::DeviceHandle;
use crate::pixel_format::{expected_payload_len, pfnc_to_zenoh};

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
    if cfg!(target_os = "macos")
        && let Err(e) = device.refresh_connection().await
    {
        warn!(
            device_id,
            error = %e,
            "camera reconnect before acquisition failed; continuing with existing control connection"
        );
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

    // No explicit CCP refresh after the potentially long stream setup: the
    // transport's keepalive waits on the same device mutex `build_stream` holds
    // and pings as soon as that guard drops.

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

/// Main frame reading loop: reads blocks from the GigE stream and publishes
/// them to Zenoh — images on [`keys::image`], event-vision blocks on
/// [`keys::evs`].
async fn frame_loop(
    session: Arc<Session>,
    device_id: String,
    frame_stream: &mut FrameStream,
    mut stop: watch::Receiver<bool>,
) {
    let status_key = keys::acquisition_status(&device_id);
    let mut publisher = BlockPublisher::new(&device_id);
    let mut frames_acquired: u64 = 0;
    let mut fps_start = tokio::time::Instant::now();
    let mut fps_frame_count: u64 = 0;
    let fps_interval = std::time::Duration::from_secs(1);

    loop {
        tokio::select! {
            result = frame_stream.next_block() => {
                match result {
                    Ok(Some(block)) => {
                        let Some((key, payload)) = publisher.frame(block) else {
                            continue;
                        };
                        publisher.put(&session, &key, payload).await;

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

/// Turns stream blocks into Zenoh `(key, payload)` pairs, one topic per kind
/// of block, and keeps each topic's sequence counter.
///
/// Separated from the loop so the dispatch — which block goes on which topic,
/// framed how — can be tested without a Zenoh session.
struct BlockPublisher {
    device_id: String,
    image_key: String,
    evs_key: String,
    image_seq: u32,
    evs_seq: u32,
    logged_first_gvsp_frame: bool,
    logged_first_publish: bool,
    logged_payload_trim: bool,
    logged_unsized_format: bool,
    logged_first_evs_block: bool,
    logged_unknown_block: bool,
}

impl BlockPublisher {
    fn new(device_id: &str) -> Self {
        Self {
            device_id: device_id.to_string(),
            image_key: keys::image(device_id),
            evs_key: keys::evs(device_id),
            image_seq: 0,
            evs_seq: 0,
            logged_first_gvsp_frame: false,
            logged_first_publish: false,
            logged_payload_trim: false,
            logged_unsized_format: false,
            logged_first_evs_block: false,
            logged_unknown_block: false,
        }
    }

    /// The key and framed payload for one block, or `None` when it is dropped.
    fn frame(&mut self, block: GenericStreamBlock) -> Option<(String, Vec<u8>)> {
        match StreamBlock::from(block) {
            StreamBlock::Image(frame) => self.frame_image(&frame),
            StreamBlock::Evs(events) => self.frame_evs(&events),
            // `StreamBlock` is `#[non_exhaustive]`. A kind this service does
            // not know is not an image, so it must not go on the image topic.
            _ => {
                if !self.logged_unknown_block {
                    warn!(
                        device_id = self.device_id,
                        "dropping a stream block of a kind this service cannot publish"
                    );
                    self.logged_unknown_block = true;
                }
                None
            }
        }
    }

    fn frame_image(&mut self, frame: &Frame) -> Option<(String, Vec<u8>)> {
        let device_id = self.device_id.as_str();
        if !self.logged_first_gvsp_frame {
            info!(
                device_id,
                width = frame.width,
                height = frame.height,
                pixel_format = ?frame.pixel_format,
                payload = frame.payload.len(),
                "first GVSP frame received"
            );
            self.logged_first_gvsp_frame = true;
        }

        let zenoh_pf = pfnc_to_zenoh(frame.pixel_format);
        let expected = expected_payload_len(frame.pixel_format, frame.width, frame.height);

        if expected.is_none() && !self.logged_unsized_format {
            warn!(
                device_id,
                pixel_format = %frame.pixel_format,
                "payload cannot be sized from image geometry; publishing \
                 it unmodified and without a length check"
            );
            self.logged_unsized_format = true;
        }

        let image_bytes = match expected {
            Some(expected) if frame.payload.len() < expected => {
                warn!(
                    device_id,
                    seq = self.image_seq,
                    actual = frame.payload.len(),
                    expected,
                    "dropping undersized frame payload"
                );
                return None;
            }
            Some(expected) if frame.payload.len() > expected => {
                if !self.logged_payload_trim {
                    warn!(
                        device_id,
                        actual = frame.payload.len(),
                        expected,
                        "trimming trailing bytes from frame payload before Zenoh publish"
                    );
                    self.logged_payload_trim = true;
                }
                &frame.payload[..expected]
            }
            // Either the length is exactly right, or we have no
            // business claiming to know it. Publish it whole.
            _ => frame.payload.as_ref(),
        };

        let header = FrameHeader {
            pixel_format: zenoh_pf,
            width: frame.width,
            height: frame.height,
            seq: self.image_seq,
        };
        self.image_seq = self.image_seq.wrapping_add(1);
        Some((
            self.image_key.clone(),
            framed(&header.encode(), image_bytes),
        ))
    }

    /// An event-vision block goes on its own topic, never on `image`
    /// (backlog `DC-05`): it has no image geometry, and the leader's Size X /
    /// Size Y do not describe one.
    fn frame_evs(&mut self, events: &EvsBlock) -> Option<(String, Vec<u8>)> {
        let device_id = self.device_id.as_str();
        let format = evs_format_to_zenoh(events.format);
        let Ok(payload_len) = u32::try_from(events.payload.len()) else {
            warn!(
                device_id,
                bytes = events.payload.len(),
                "dropping an event block too large for its header's length field"
            );
            return None;
        };
        if !self.logged_first_evs_block {
            info!(
                device_id,
                format = format.as_str(),
                bytes = payload_len,
                key = self.evs_key,
                "first event-vision block received; publishing event blocks on their own key"
            );
            self.logged_first_evs_block = true;
        }

        let header = EvsHeader::new(format, self.evs_seq, events.ts_dev, payload_len);
        self.evs_seq = self.evs_seq.wrapping_add(1);
        Some((
            self.evs_key.clone(),
            framed(&header.encode(), &events.payload),
        ))
    }

    async fn put(&mut self, session: &Session, key: &str, payload: Vec<u8>) {
        let bytes = payload.len();
        if let Err(e) = session.put(key, payload).await {
            warn!(device_id = self.device_id, key, error = %e, "failed to publish block");
        } else if !self.logged_first_publish {
            info!(
                device_id = self.device_id,
                key, bytes, "published first stream block to Zenoh"
            );
            self.logged_first_publish = true;
        }
    }
}

/// A header followed by its payload, as one Zenoh message.
fn framed(header: &[u8], body: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(header.len() + body.len());
    payload.extend_from_slice(header);
    payload.extend_from_slice(body);
    payload
}

fn evs_format_to_zenoh(format: EvsFormat) -> ZenohEvsFormat {
    match format {
        EvsFormat::Evt30 => ZenohEvsFormat::Evt30,
        EvsFormat::Evt21 => ZenohEvsFormat::Evt21,
        // `EvsFormat` is `#[non_exhaustive]`; the header's `Unknown` exists
        // for exactly this.
        _ => ZenohEvsFormat::Unknown,
    }
}

/// The host interface that will receive this device's GVSP traffic.
///
/// With `--iface` the operator has already chosen, and the handle carries the
/// interface they chose, resolved once.
///
/// Without it, the OS is asked which local interface routes to the camera.
/// This arm used to call `Iface::from_ipv4(device.info().ip)` — the camera's
/// address handed to the *host*-address lookup, which can only ever fail with
/// `no interface with IPv4 <camera-ip>`. So `viva-service` could not stream
/// without `--iface` at all. That is the same confusion as #70 (`REL-04` in
/// the studio backend, `DX-08` in `viva-camctl`); this was the third copy and
/// the last one left (backlog `SVC-06`).
fn resolve_iface(device: &DeviceHandle) -> Result<Iface, String> {
    receive_iface(device.iface(), device.info().ip)
}

/// The decision behind [`resolve_iface`], separated so it can be tested
/// without a camera — which matters here, because the fake camera cannot
/// reproduce the defect this guards (see the test).
fn receive_iface(chosen: Option<&Iface>, camera_ip: std::net::Ipv4Addr) -> Result<Iface, String> {
    match chosen {
        Some(iface) => Ok(iface.clone()),
        None => Iface::from_remote_ipv4(camera_ip).map_err(|e| {
            format!(
                "probe which local interface routes to {camera_ip}: {e} \
                 (pass --iface <HOST-IP|NAME> to choose one explicitly)"
            )
        }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// Backlog `SVC-06`. Without `--iface` the receive interface must come
    /// from a route probe, not from looking the *camera's* address up among
    /// the host's own — a lookup that can only ever fail.
    ///
    /// `127.0.0.2` is the whole test: it is routable (loopback answers for the
    /// entire `127/8`) but it is not an address any interface reports, so the
    /// two functions give visibly different answers. The camera we test
    /// against cannot show this, because it lives on `127.0.0.1`, which *is* a
    /// host address — so the broken lookup succeeds there by coincidence. That
    /// is why this is a unit test and not an e2e one.
    #[test]
    fn no_iface_probes_the_route_rather_than_looking_up_the_camera_address() {
        let camera_ip = Ipv4Addr::new(127, 0, 0, 2);

        // What the code used to do.
        assert!(
            Iface::from_ipv4(camera_ip).is_err(),
            "a camera address is not a host address; if this ever succeeds \
             the test has stopped proving anything"
        );

        // What it does now.
        let iface = receive_iface(None, camera_ip).expect("route probe should resolve");
        assert_eq!(iface.ipv4(), Some(Ipv4Addr::LOCALHOST));
    }

    /// An explicit `--iface` is honoured verbatim; the camera's address is not
    /// consulted at all.
    #[test]
    fn an_explicit_iface_is_used_as_given() {
        let chosen = Iface::from_ipv4(Ipv4Addr::LOCALHOST).expect("loopback iface");
        let resolved = receive_iface(Some(&chosen), Ipv4Addr::new(192, 0, 2, 1))
            .expect("an explicit interface needs no probe");
        assert_eq!(resolved, chosen);
    }
}
