//! Acquisition control queryable and frame streaming over Zenoh for U3V.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::{error, info, warn};
use viva_service::acquisition::{publish_image_meta, publish_status};
use viva_service::device::DeviceOps;
use viva_zenoh_api::frame_header::FrameHeader;
use viva_zenoh_api::{AcquisitionCommand, AcquisitionControlRequest, NodeOpResponse, keys};
use zenoh::Session;

use crate::device::U3vDeviceHandle;
use viva_u3v::usb::UsbTransfer;

/// Run the acquisition control queryable for a U3V device.
pub async fn run<T: UsbTransfer + Send + 'static>(
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
                                        AcquisitionCommand::Start => NodeOpResponse::err("acquisition already active"),
                                        AcquisitionCommand::Stop if frame_task.is_some() => {
                                            handle_stop(
                                                &session, &device, &device_id,
                                                &stop_tx, &mut frame_task,
                                            ).await
                                        }
                                        AcquisitionCommand::Stop => NodeOpResponse::err("acquisition not active"),
                                    },
                                    Err(e) => NodeOpResponse::err(format!("invalid payload: {e}")),
                                }
                            }
                            None => NodeOpResponse::err("missing payload"),
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

async fn handle_start<T: UsbTransfer + Send + 'static>(
    session: &Arc<Session>,
    device: &Arc<U3vDeviceHandle<T>>,
    device_id: &str,
    stop_tx: &watch::Sender<bool>,
    frame_task: &mut Option<tokio::task::JoinHandle<()>>,
) -> NodeOpResponse {
    let mut frames = match device.open_frame_stream().await {
        Ok(frames) => frames,
        Err(e) => return NodeOpResponse::err(format!("opening the stream failed: {e}")),
    };

    // Publish image metadata.
    publish_image_meta(session, device.as_ref(), device_id).await;

    let _ = stop_tx.send(false);
    let mut stop_rx = stop_tx.subscribe();
    let publisher = session.clone();
    let device_id_owned = device_id.to_string();

    // Publish every frame as it arrives. Reading is the library's: a reader
    // thread that waits out idle periods (trigger mode) and is joined on
    // stop. The service used to read itself and sleep to hold ~30 fps, which
    // a faster camera overruns (backlog SVC-06); a viewer paces itself.
    let handle = tokio::spawn(async move {
        let image_key = keys::image(&device_id_owned);
        let mut seq: u32 = 0;
        let mut logged_first = false;
        loop {
            let frame = tokio::select! {
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        break;
                    }
                    continue;
                }
                result = frames.next_frame() => match result {
                    Ok(Some(frame)) => frame,
                    Ok(None) => {
                        info!(device_id = device_id_owned, "U3V stream ended");
                        break;
                    }
                    Err(e) => {
                        error!(device_id = device_id_owned, error = %e, "U3V stream error");
                        break;
                    }
                },
            };
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
                pixel_format: frame.pixel_format.into(),
                width: frame.width,
                height: frame.height,
                seq,
            };
            let encoded_header = header.encode();
            let mut payload = Vec::with_capacity(encoded_header.len() + frame.payload.len());
            payload.extend_from_slice(&encoded_header);
            payload.extend_from_slice(&frame.payload);
            if let Err(e) = publisher.put(&image_key, payload).await {
                warn!(error = %e, "failed to publish frame");
            }
            seq = seq.wrapping_add(1);
        }
        // The reader thread lets go of the endpoint before this task ends.
        frames.shutdown().await;
        info!(device_id = device_id_owned, seq, "U3V frame loop exited");
    });

    // Execute AcquisitionStart on the camera.
    if let Err(e) = device.exec_command("AcquisitionStart").await {
        let _ = stop_tx.send(true);
        let _ = handle.await;
        return NodeOpResponse::err(format!("AcquisitionStart failed: {e}"));
    }

    publish_status(session, device_id, true).await;
    info!(device_id, "U3V acquisition started");

    *frame_task = Some(handle);
    NodeOpResponse::ok()
}

async fn handle_stop<T: UsbTransfer + Send + 'static>(
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

    NodeOpResponse::ok()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use viva_fake_u3v::FakeU3vCamera;
    use viva_genicam::open_u3v_device;

    use super::*;

    /// The service streams what the camera sends, packed formats included.
    /// It sized frames as width × height × one byte for a format without a
    /// whole-byte pixel, and opened the stream without configuring the SIRM
    /// (backlog SVC-03); it now opens it as the library does.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_packed_format_streams_at_its_payload_size() {
        let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
        let device = FakeU3vCamera::new(64, 48).open_device().unwrap();
        let (mut camera, xml) = open_u3v_device(device).unwrap();
        camera.set("PixelFormat", "Mono12Packed").unwrap();
        let device_id = format!("u3v-acquisition-test-{}", std::process::id());
        let handle = Arc::new(U3vDeviceHandle::new(camera, xml, device_id.clone()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let service = tokio::spawn(run(session.clone(), handle, shutdown_rx));
        let images = session
            .declare_subscriber(keys::image(&device_id))
            .await
            .unwrap();

        let start =
            serde_json::to_vec(&AcquisitionControlRequest::new(AcquisitionCommand::Start)).unwrap();
        let started = async {
            loop {
                let replies = session
                    .get(keys::acquisition_control(&device_id))
                    .payload(start.clone())
                    .timeout(Duration::from_secs(5))
                    .await
                    .unwrap();
                if let Ok(reply) = replies.recv_async().await
                    && let Ok(sample) = reply.result()
                {
                    let response: NodeOpResponse =
                        serde_json::from_slice(&sample.payload().to_bytes()).unwrap();
                    assert!(response.ok, "{response:?}");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(10), started)
            .await
            .expect("no answer to Start");

        let sample = tokio::time::timeout(Duration::from_secs(5), images.recv_async())
            .await
            .expect("no frame within 5 s")
            .unwrap();
        let bytes = sample.payload().to_bytes();
        let (header, pixels) = FrameHeader::decode(&bytes).unwrap();
        assert_eq!((header.width, header.height), (64, 48));
        assert_eq!(pixels.len(), 64 * 48 * 3 / 2, "64 × 48 × 1.5 bytes");

        let _ = shutdown_tx.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), service).await;
    }
}
