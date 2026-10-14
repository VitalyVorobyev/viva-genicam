//! Acquisition control queryable and frame streaming over Zenoh, for every
//! [`StreamSource`]: GigE Vision and USB3 Vision publish the same way.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{debug, error, info, warn};
use viva_genicam::pfnc;
use viva_genicam::{EvsBlock, EvsFormat, Frame, StreamBlock};
use viva_zenoh_api::frame_header::FrameHeader;
use viva_zenoh_api::{
    AcquisitionCommand, AcquisitionControlRequest, AcquisitionStatus, EvsFormat as ZenohEvsFormat,
    EvsHeader, ImageMeta, NodeOpResponse, StreamCounters, keys,
};
use zenoh::Session;

use crate::device::DeviceOps;
use crate::pixel_format::expected_payload_len;
use crate::source::{BlockStream, StreamSource};

/// Run the acquisition control queryable and frame streaming loop.
pub(crate) async fn run<D: StreamSource>(
    session: Arc<Session>,
    device: Arc<D>,
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
                                        // A stream that ended by itself has
                                        // published its final, inactive
                                        // status; a client that then starts
                                        // again gets the old one stopped first.
                                        AcquisitionCommand::Start
                                            if frame_task
                                                .as_ref()
                                                .is_none_or(|task| task.is_finished()) =>
                                        {
                                            if frame_task.is_some() {
                                                handle_stop(
                                                    &session,
                                                    device.as_ref(),
                                                    &stop_tx,
                                                    &mut frame_task,
                                                )
                                                .await;
                                            }
                                            handle_start(
                                                &session,
                                                device.as_ref(),
                                                &stop_tx,
                                                &mut frame_task,
                                                req.packet_size,
                                            )
                                            .await
                                        }
                                        AcquisitionCommand::Start => {
                                            NodeOpResponse::err("acquisition already active")
                                        }
                                        AcquisitionCommand::Stop if frame_task.is_some() => {
                                            handle_stop(
                                                &session,
                                                device.as_ref(),
                                                &stop_tx,
                                                &mut frame_task,
                                            )
                                            .await
                                        }
                                        AcquisitionCommand::Stop => {
                                            NodeOpResponse::err("acquisition not active")
                                        }
                                    },
                                    Err(e) => NodeOpResponse::err(format!("invalid payload: {e}")),
                                }
                            }
                            None => NodeOpResponse::err("missing payload"),
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
            _ = crate::serve::stopped(&mut shutdown) => {
                // Stop acquisition on shutdown, and when the camera is let
                // go of: the frame loop is awaited, never left running.
                if frame_task.is_some() {
                    handle_stop(&session, device.as_ref(), &stop_tx, &mut frame_task).await;
                }
                break;
            }
        }
    }
}

async fn handle_start<D: StreamSource>(
    session: &Arc<Session>,
    device: &D,
    stop_tx: &watch::Sender<bool>,
    frame_task: &mut Option<tokio::task::JoinHandle<()>>,
    packet_size: Option<u32>,
) -> NodeOpResponse {
    let device_id = device.device_id();

    // 1. Open the stream, configured on the camera, before AcquisitionStart:
    //    a GigE camera must know where to send GVSP first.
    let blocks = match device.open_stream(packet_size).await {
        Ok(blocks) => blocks,
        Err(e) => return NodeOpResponse::err(format!("opening the stream failed: {e}")),
    };

    // 2. Publish metadata before starting acquisition.
    publish_image_meta(session, device, device_id).await;

    let _ = stop_tx.send(false);
    let stop_rx = stop_tx.subscribe();
    let handle = tokio::spawn(frame_loop(
        session.clone(),
        device_id.to_string(),
        blocks,
        stop_rx,
    ));

    // Let the spawned task reach its first `next_block()` poll before the
    // camera starts sending. Without this yield, `tokio::spawn` only
    // schedules the task; on macOS loopback the fake camera can otherwise
    // outrun the receiver and the stream may never recover.
    tokio::task::yield_now().await;

    // 3. Start acquisition only after the frame loop is armed, so the
    //    stream drains from the first packet onward.
    if let Err(e) = device.acquisition_start().await {
        let _ = stop_tx.send(true);
        let _ = handle.await;
        return NodeOpResponse::err(format!("AcquisitionStart failed: {e}"));
    }

    // 4. Publish active status now that the device is streaming.
    publish_status(session, device_id, true).await;
    info!(device_id, "acquisition started, frame loop armed");

    *frame_task = Some(handle);
    NodeOpResponse::ok()
}

async fn handle_stop<D: StreamSource>(
    session: &Arc<Session>,
    device: &D,
    stop_tx: &watch::Sender<bool>,
    frame_task: &mut Option<tokio::task::JoinHandle<()>>,
) -> NodeOpResponse {
    let device_id = device.device_id();
    // Signal the frame loop to stop, and wait for it to let go of the stream.
    let _ = stop_tx.send(true);
    if let Some(task) = frame_task.take() {
        let _ = task.await;
    }

    // Stop acquisition on the camera.
    if let Err(e) = device.acquisition_stop().await {
        warn!(device_id, error = %e, "AcquisitionStop failed");
    }

    publish_status(session, device_id, false).await;
    info!(device_id, "acquisition stopped");

    NodeOpResponse::ok()
}

/// How often the frame loop reports the stream's rate and counters.
const STATUS_INTERVAL: Duration = Duration::from_secs(1);

/// Main frame reading loop: reads blocks from the stream and publishes them
/// to Zenoh — images on [`keys::image`], event-vision blocks on
/// [`keys::evs`]. Every [`STATUS_INTERVAL`] it publishes the frame rate and
/// the stream's counters on [`keys::acquisition_status`], whether or not
/// frames arrive, and when it stops, the final counters as
/// [`AcquisitionStatus::stopped`]. Closes the stream when it stops.
async fn frame_loop(
    session: Arc<Session>,
    device_id: String,
    mut blocks: Box<dyn BlockStream>,
    mut stop: watch::Receiver<bool>,
) {
    let status_key = keys::acquisition_status(&device_id);
    let mut publisher = BlockPublisher::new(&device_id);
    let mut frames_acquired: u64 = 0;
    let mut frames_since_status: u64 = 0;
    let mut last_status = Instant::now();
    let mut status_tick = tokio::time::interval_at(last_status + STATUS_INTERVAL, STATUS_INTERVAL);
    status_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            result = blocks.next_block() => {
                match result {
                    Ok(Some(block)) => {
                        let Some((key, payload)) = publisher.frame(block) else {
                            continue;
                        };
                        publisher.put(&session, &key, payload).await;
                        frames_acquired += 1;
                        frames_since_status += 1;
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
            _ = status_tick.tick() => {
                let now = Instant::now();
                let fps = frames_since_status as f32 / (now - last_status).as_secs_f32();
                let stats = blocks.stats().map(|stats| StreamCounters::from(&stats));
                put_status(&session, &status_key, &AcquisitionStatus::active(Some(fps), stats))
                    .await;
                last_status = now;
                frames_since_status = 0;
                debug!(device_id, fps, frames_acquired, "streaming");
            }
            _ = crate::serve::stopped(&mut stop) => {
                debug!(device_id, "frame loop stop signal received");
                break;
            }
        }
    }

    // The final counters stay readable after the stream is gone.
    let stats = blocks.stats();
    blocks.close().await;
    if let Some(stats) = stats {
        put_status(
            &session,
            &status_key,
            &AcquisitionStatus::stopped(StreamCounters::from(&stats)),
        )
        .await;
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
    fn frame(&mut self, block: StreamBlock) -> Option<(String, Vec<u8>)> {
        match block {
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
                "first frame received"
            );
            self.logged_first_gvsp_frame = true;
        }

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
            pixel_format: frame.pixel_format,
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

/// Publish that acquisition has started or stopped. Neither carries
/// counters: the frame loop sends those.
pub(crate) async fn publish_status(session: &Session, device_id: &str, active: bool) {
    let status = if active {
        AcquisitionStatus::active(None, None)
    } else {
        AcquisitionStatus::inactive()
    };
    put_status(session, &keys::acquisition_status(device_id), &status).await;
}

async fn put_status(session: &Session, key: &str, status: &AcquisitionStatus) {
    if let Ok(payload) = serde_json::to_vec(status) {
        let _ = session.put(key, payload).await;
    }
}

/// Publish image metadata (width, height, pixel format) to Zenoh.
pub(crate) async fn publish_image_meta<D: DeviceOps>(
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

    let payload_size = device
        .get_feature("PayloadSize")
        .await
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let meta = image_meta(width, height, &pf_str, payload_size);

    let key = keys::image_meta(device_id);
    if let Ok(payload) = serde_json::to_vec(&meta) {
        if let Err(e) = session.put(&key, payload).await {
            warn!(device_id, error = %e, "failed to publish image meta");
        } else {
            info!(device_id, width, height, "published image metadata");
        }
    }
}

/// The metadata for a `width × height` stream in the camera's `PixelFormat`
/// (its enumeration entry name), sized as the camera will send it.
///
/// The size is the camera's `PayloadSize` when it has one, since that counts
/// padding and chunk data; otherwise [`pfnc::PixelFormat::image_bytes`],
/// exact for packed formats. The name is read by
/// [`pfnc::PixelFormat::from_name`], which knows the older spellings cameras
/// use (`YUV422Packed`, `RGB8Packed`); an unknown name is published as
/// `Unknown`, sized one byte per pixel.
pub(crate) fn image_meta(
    width: u32,
    height: u32,
    pixel_format: &str,
    payload_size: Option<u64>,
) -> ImageMeta {
    let pfnc = pfnc::PixelFormat::from_name(pixel_format);
    let payload_size = payload_size.filter(|&size| size > 0).unwrap_or_else(|| {
        pfnc.image_bytes(width, height)
            .unwrap_or(width as usize * height as usize) as u64
    });
    ImageMeta::new(pfnc, width, height, payload_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `PixelFormat` entry names cameras report (vendor corpus:
    /// `Mono12Packed` in 28 of 38 documents, `YUV422Packed` and `RGB8Packed`
    /// in 23), sized from their PFNC code.
    #[test]
    fn image_meta_names_and_sizes_the_cameras_format() {
        use pfnc::PixelFormat as P;
        for (name, format, bytes) in [
            ("Mono8", P::Mono8, 307_200),
            ("Mono12", P::Mono12, 614_400),
            ("Mono12Packed", P::Mono12Packed, 460_800),
            ("Mono10p", P::Mono10p, 384_000),
            ("BayerRG12p", P::BayerRG12p, 460_800),
            ("YUV422Packed", P::YUV422_8_UYVY, 614_400),
            ("RGB8Packed", P::RGB8Packed, 921_600),
        ] {
            let meta = image_meta(640, 480, name, None);
            assert_eq!(meta.pixel_format, format, "{name}");
            assert_eq!(meta.payload_size, bytes, "{name}");
        }
        // A name nobody knows is not passed off as Mono8.
        assert_eq!(
            image_meta(2, 2, "Fancy42", None).pixel_format,
            P::Unknown(0)
        );
    }

    /// A stream with nothing to deliver still reports every interval: its
    /// counters matter most when frames stop arriving. Stopping sends the
    /// final ones.
    #[tokio::test(flavor = "multi_thread")]
    async fn status_is_published_while_no_frames_arrive_and_on_stop() {
        struct Stalled(viva_genicam::StreamStats);

        #[async_trait::async_trait]
        impl BlockStream for Stalled {
            async fn next_block(
                &mut self,
            ) -> Result<Option<StreamBlock>, viva_genicam::GenicamError> {
                std::future::pending().await
            }

            fn stats(&self) -> Option<viva_genicam::StreamStats> {
                Some(self.0)
            }
        }

        let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
        let id = format!("status-test-{}", std::process::id());
        let subscriber = session
            .declare_subscriber(keys::acquisition_status(&id))
            .await
            .unwrap();
        let next = || async {
            let sample = tokio::time::timeout(Duration::from_secs(3), subscriber.recv_async())
                .await
                .expect("no status within 3 s")
                .expect("subscriber closed");
            serde_json::from_slice::<AcquisitionStatus>(&sample.payload().to_bytes()).unwrap()
        };

        let mut stats = viva_genicam::StreamStats::default();
        stats.frames_complete = 7;
        stats.frames_missed = 4;
        let (stop_tx, stop_rx) = watch::channel(false);
        let task = tokio::spawn(frame_loop(
            session.clone(),
            id.clone(),
            Box::new(Stalled(stats)),
            stop_rx,
        ));

        let status = next().await;
        assert!(status.active);
        assert_eq!(status.fps, Some(0.0));
        assert_eq!(status.stats, Some(StreamCounters::from(&stats)));

        let _ = stop_tx.send(true);
        task.await.unwrap();
        let mut status = next().await;
        // A tick may have gone out just before the stop.
        while status.active {
            status = next().await;
        }
        assert_eq!(
            status,
            AcquisitionStatus::stopped(StreamCounters::from(&stats))
        );
    }

    /// The camera's `PayloadSize` is what a frame carries — padding and chunk
    /// data included — so it wins over the computed size (backlog SVC-05).
    #[test]
    fn image_meta_takes_the_cameras_payload_size() {
        let meta = image_meta(640, 480, "Mono8", Some(307_456));
        assert_eq!(meta.payload_size, 307_456);
        // Zero is not a size; fall back to the computed one.
        assert_eq!(image_meta(640, 480, "Mono8", Some(0)).payload_size, 307_200);
    }
}
