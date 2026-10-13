//! USB3 Vision frame streams (ADR-0032).
//!
//! `viva-u3v`'s [`U3vStream`](crate::u3v::U3vStream) keeps several transfers
//! queued on the stream endpoint and resynchronises after a failed transfer;
//! here it runs on a thread of its own — a `std::thread`, not a Tokio
//! runtime's blocking pool, so the blocking API (ADR-0027) streams from a
//! USB3 Vision camera too — and hands frames on through the GVSP receive
//! engine's drop-oldest queue, counted in the same [`StreamStats`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use tracing::{debug, warn};
use viva_pfnc::PixelFormat;

use super::receiver::{DELIVERY_CAPACITY, DeliveryQueue, Popped};
use super::stats::{StreamStats, StreamStatsAccumulator};
use crate::frame::{Frame, FrameStatus};
use crate::u3v::{RawFrame, U3vError, U3vStatus, U3vStream, U3vStreamOptions, UsbTransfer};
use crate::{GenicamError, TimeSync, U3vRegisterIo};

/// How long the reader waits for a transfer before checking whether it was
/// told to stop: what dropping or closing a stream waits at most.
const U3V_READ_POLL: Duration = Duration::from_millis(50);

/// Clears a device's StreamEnable. Called with `true` it waits for the
/// device lock; with `false` it gives up if someone holds the lock, and
/// returns whether it ran.
type DisableStream = Arc<dyn Fn(bool) -> bool + Send + Sync>;

/// State shared between a [`U3vFrameStream`] and its reader thread.
struct Shared {
    queue: DeliveryQueue<Frame, U3vError>,
    stop: AtomicBool,
    deliver_incomplete: AtomicBool,
}

/// Frames from a USB3 Vision camera, read on a thread of its own.
///
/// [`next_frame`](Self::next_frame) waits asynchronously,
/// [`next_frame_blocking`](Self::next_frame_blocking) on the calling thread;
/// neither needs the other. A stream from [`U3vStreamBuilder`] or
/// [`Camera::open_stream`](crate::Camera::open_stream) clears the device's
/// StreamEnable when it is closed or dropped.
///
/// A transfer that fails is recovered from — the endpoint reset, the block
/// in progress lost and counted — and the stream goes on; a camera that is
/// unplugged ends it with [`U3vError::Disconnected`] as the source of the
/// error, and marks the camera's transport lost
/// ([`U3vRegisterIo::is_lost`]).
///
/// ```rust,ignore
/// let mut frames = camera.open_stream(U3vStreamOptions::default())?;
/// camera.acquisition_start()?;
/// while let Some(frame) = frames.next_frame().await? {
///     println!("{}x{} frame {}", frame.width, frame.height, frame.block_id);
/// }
/// ```
pub struct U3vFrameStream {
    shared: Arc<Shared>,
    reader: Option<JoinHandle<()>>,
    stats: StreamStatsAccumulator,
    time_sync: Option<TimeSync>,
    disable: Option<DisableStream>,
}

impl U3vFrameStream {
    /// Read frames from `stream`, a stream already configured and enabled on
    /// the device. Nothing clears the device's StreamEnable when it ends;
    /// [`U3vStreamBuilder`] starts streams that do.
    pub fn start(stream: U3vStream) -> Self {
        Self::start_inner(stream, None, None)
    }

    fn start_inner(
        stream: U3vStream,
        disable: Option<DisableStream>,
        lost: Option<Arc<AtomicBool>>,
    ) -> Self {
        let shared = Arc::new(Shared {
            queue: DeliveryQueue::new(DELIVERY_CAPACITY),
            stop: AtomicBool::new(false),
            deliver_incomplete: AtomicBool::new(false),
        });
        let stats = StreamStatsAccumulator::new();
        let reader = {
            let (shared, stats) = (Arc::clone(&shared), stats.clone());
            std::thread::Builder::new()
                .name("viva-u3v-stream".into())
                .spawn(move || read_loop(stream, &shared, &stats, lost.as_deref()))
        };
        let reader = match reader {
            Ok(handle) => Some(handle),
            Err(error) => {
                warn!(%error, "could not start the USB3 Vision reader thread");
                shared.queue.finish();
                None
            }
        };
        Self {
            shared,
            reader,
            stats,
            time_sync: None,
            disable,
        }
    }

    /// Also deliver frames that did not complete — a block given up before
    /// its trailer, or one whose trailer reports an error — with their
    /// [`FrameStatus`] saying how (default: off). Every incomplete frame is
    /// counted in [`StreamStats::frames_incomplete`] either way.
    pub fn set_deliver_incomplete(&mut self, deliver: bool) {
        self.shared
            .deliver_incomplete
            .store(deliver, Ordering::Relaxed);
    }

    /// Map device timestamps to host time with `time_sync`, fitted by
    /// [`Camera::time_calibrate`](crate::Camera::time_calibrate).
    pub fn set_time_sync(&mut self, time_sync: TimeSync) {
        self.time_sync = Some(time_sync);
    }

    /// Snapshot the stream's counters.
    pub fn stats(&self) -> StreamStats {
        self.stats.snapshot()
    }

    /// A clone of the counters' handle, readable from any thread.
    pub fn stats_handle(&self) -> StreamStatsAccumulator {
        self.stats.clone()
    }

    /// Receive the next frame. `Ok(None)` means the stream has ended.
    /// Cancel-safe.
    pub async fn next_frame(&mut self) -> Result<Option<Frame>, GenicamError> {
        let popped = self.shared.queue.pop().await;
        self.take(popped)
    }

    /// Wait up to `timeout` for the next frame on the calling thread:
    /// `Ok(None)` when the stream has ended, [`GenicamError::Timeout`] when
    /// nothing arrived.
    pub fn next_frame_blocking(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<Frame>, GenicamError> {
        match self.shared.queue.pop_blocking(timeout) {
            Popped::Empty => Err(GenicamError::Timeout(timeout)),
            popped => self.take(popped),
        }
    }

    /// Account for a frame leaving the queue and give it its host time.
    fn take(&mut self, popped: Popped<Frame, U3vError>) -> Result<Option<Frame>, GenicamError> {
        match popped {
            Popped::Item(mut frame) => {
                frame.ts_host = frame.ts_dev.and_then(|ticks| {
                    self.time_sync
                        .as_ref()
                        .and_then(|sync| sync.to_host_time(ticks))
                });
                if frame.is_complete() {
                    let latency = frame
                        .ts_host
                        .and_then(|ts| SystemTime::now().duration_since(ts).ok());
                    self.stats.record_frame(frame.payload.len(), latency);
                } else {
                    self.stats.record_incomplete_frame();
                }
                Ok(Some(frame))
            }
            Popped::Failed(during, source) => Err(GenicamError::U3v { during, source }),
            Popped::Finished | Popped::Empty => Ok(None),
        }
    }

    /// Tell the reader to stop. It stops within a read poll; this does not
    /// wait for it, [`close`](Self::close) does.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
    }

    /// Stop the reader, wait for it, and clear the device's StreamEnable:
    /// the endpoint and the device are free once this returns.
    pub fn close(mut self) {
        self.close_inner();
    }

    /// [`close`](Self::close), for async callers: the wait runs on the
    /// runtime's blocking pool.
    pub async fn shutdown(self) {
        let closing = tokio::task::spawn_blocking(move || self.close());
        let _ = closing.await;
    }

    fn close_inner(&mut self) {
        self.stop();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(disable) = self.disable.take() {
            disable(true);
        }
    }
}

impl Drop for U3vFrameStream {
    /// A stream dropped without [`close`](Self::close) still stops its
    /// reader and the device's stream. If the device is busy, a thread of
    /// its own waits for it: the dropping thread must not block on a lock it
    /// may hold, and giving up left StreamEnable set (Codex review of #184).
    fn drop(&mut self) {
        self.stop();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(disable) = self.disable.take()
            && !disable(false)
        {
            std::thread::spawn(move || {
                disable(true);
            });
        }
    }
}

/// The reader thread: blocks from the stream, counted, converted and queued
/// until told to stop or the stream ends.
fn read_loop(
    mut stream: U3vStream,
    shared: &Shared,
    stats: &StreamStatsAccumulator,
    lost: Option<&AtomicBool>,
) {
    let mut counted = stream.counters();
    let mut last_block: Option<u64> = None;
    let mut chunk_layout_warned = false;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        let result = stream.next_frame_within(U3V_READ_POLL);
        let counters = stream.counters();
        stats.record_packets_received(counters.transfers - counted.transfers);
        stats.record_malformed_packets(counters.transfers_skipped - counted.transfers_skipped);
        stats.record_transfer_errors(counters.transfer_errors - counted.transfer_errors);
        counted = counters;
        match result {
            Ok(None) => {}
            Ok(Some(raw)) => {
                // Block ids count up by one; a lower id is the camera
                // starting again, not a gap.
                let id = raw.leader.block_id;
                if let Some(last) = last_block
                    && id > last + 1
                {
                    stats.record_missed_frames(id - last - 1);
                }
                last_block = Some(id);
                deliver(raw, shared, stats, &mut chunk_layout_warned);
            }
            Err(error) => {
                if matches!(error, U3vError::Disconnected) {
                    warn!("the USB3 Vision camera is gone; its stream ends");
                    if let Some(lost) = lost {
                        lost.store(true, Ordering::Release);
                    }
                }
                shared.queue.fail("read a frame", error);
                break;
            }
        }
    }
    shared.queue.finish();
}

/// Queue one block as a frame — or count it, when it is incomplete and the
/// application did not ask for incomplete ones, or not an image or chunks.
fn deliver(raw: RawFrame, shared: &Shared, stats: &StreamStatsAccumulator, warned: &mut bool) {
    let payload_type = raw.leader.payload_type;
    if payload_type & !CHUNK_FLAG != PAYLOAD_TYPE_IMAGE && payload_type != CHUNK_FLAG {
        debug!(
            payload_type,
            "a USB3 Vision payload type that is not assembled"
        );
        stats.record_unsupported_block();
        return;
    }
    let block_id = raw.leader.block_id;
    let (frame, chunk_error) = to_frame(raw);
    if let Some(error) = chunk_error {
        if !*warned {
            *warned = true;
            warn!(
                block_id,
                %error,
                "a frame's chunk data does not parse; it is delivered with its whole payload \
                 and no chunks. Further failures are logged at debug"
            );
        } else {
            debug!(block_id, %error, "chunk data does not parse");
        }
    }
    if !frame.is_complete() && !shared.deliver_incomplete.load(Ordering::Relaxed) {
        stats.record_incomplete_frame();
        return;
    }
    if shared.queue.push(frame).is_some() {
        stats.record_backpressure_drop();
    }
}

/// An image, with or without chunks (USB3 Vision `0x0001`, `0x4001`).
const PAYLOAD_TYPE_IMAGE: u16 = 0x0001;
/// The extended chunk flag; alone, chunk data (`0x4000`).
const CHUNK_FLAG: u16 = 0x4000;

/// The frame a block makes, and why its chunks did not parse, if they did
/// not.
fn to_frame(raw: RawFrame) -> (Frame, Option<U3vError>) {
    let RawFrame {
        leader,
        payload,
        trailer,
        ..
    } = raw;
    let status = match &trailer {
        None => FrameStatus::TrailerMissing,
        Some(t) if t.status & 0x8000 != 0 => {
            FrameStatus::U3vDeviceStatus(U3vStatus::from_raw(t.status))
        }
        Some(t) if (payload.len() as u64) < t.valid_payload_size => {
            FrameStatus::Incomplete { missing_packets: 0 }
        }
        Some(_) => FrameStatus::Complete,
    };
    let raw_payload = payload.clone();
    let (image, chunks) = if status.is_complete() {
        crate::chunks::split_u3v(payload, leader.payload_type)
    } else {
        (payload, None)
    };
    let (chunks, chunk_error) = match chunks {
        Some(Ok(chunks)) => (Some(chunks), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    let mut frame = Frame::new(
        image,
        leader.width,
        leader.height,
        PixelFormat::from_code(leader.pixel_format),
    );
    frame.raw_payload = raw_payload;
    frame.chunks = chunks;
    frame.ts_dev = Some(leader.timestamp);
    frame.padding_x = leader.x_padding;
    frame.block_id = leader.block_id;
    frame.payload_type = leader.payload_type;
    frame.status = status;
    frame.delivered_height = trailer
        .as_ref()
        .and_then(|t| t.size_y)
        .filter(|_| leader.payload_type & !CHUNK_FLAG == PAYLOAD_TYPE_IMAGE);
    frame.expected_size = trailer
        .as_ref()
        .and_then(|t| usize::try_from(t.valid_payload_size).ok());
    (frame, chunk_error)
}

/// Builder for a USB3 Vision [`U3vFrameStream`] on a camera's transport —
/// the device, as GigE Vision's `StreamBuilder` takes the device.
///
/// [`build`](Self::build) is synchronous where GigE Vision's is async:
/// opening a USB3 Vision stream is a handful of blocking register writes,
/// with nothing to wait on a socket for, and a program on the blocking API
/// (ADR-0027) must be able to open one without a runtime (ADR-0032).
pub struct U3vStreamBuilder<'a, T: UsbTransfer + 'static> {
    device: &'a U3vRegisterIo<T>,
    options: U3vStreamOptions,
    time_sync: Option<TimeSync>,
}

impl<'a, T: UsbTransfer + 'static> U3vStreamBuilder<'a, T> {
    /// A builder for the stream of the camera behind `device`, with the
    /// default transfer settings.
    pub fn new(device: &'a U3vRegisterIo<T>) -> Self {
        Self {
            device,
            options: U3vStreamOptions::default(),
            time_sync: None,
        }
    }

    /// The transfer settings and payload size hint.
    pub fn options(mut self, options: U3vStreamOptions) -> Self {
        self.options = options;
        self
    }

    /// Bytes in one frame, used only when the device's SIRM leaves its
    /// Required Payload Size at 0.
    pub fn payload_size(mut self, bytes: u64) -> Self {
        self.options.payload_size_hint = bytes;
        self
    }

    /// The largest payload transfer to ask for.
    pub fn max_transfer_size(mut self, bytes: u32) -> Self {
        self.options.max_transfer_size = bytes;
        self
    }

    /// How many transfers to keep queued on the stream endpoint.
    pub fn transfers_in_flight(mut self, transfers: usize) -> Self {
        self.options.transfers_in_flight = transfers;
        self
    }

    /// Map device timestamps to host time with `time_sync`.
    pub fn time_sync(mut self, time_sync: TimeSync) -> Self {
        self.time_sync = Some(time_sync);
        self
    }

    /// Configure the SIRM, reset the stream endpoint, set StreamEnable, and
    /// start reading. The stream clears StreamEnable again when it is closed
    /// or dropped.
    pub fn build(self) -> Result<U3vFrameStream, GenicamError> {
        let device = self.device.shared_device();
        let stream = device
            .lock()
            .map_err(|_| GenicamError::Panicked("a thread holding the camera's device lock"))?
            .open_stream_with(&self.options)
            .map_err(|source| {
                if matches!(source, U3vError::Disconnected) {
                    self.device.mark_lost();
                }
                GenicamError::U3v {
                    during: "open the stream",
                    source,
                }
            })?;
        let disable: DisableStream = Arc::new(move |wait: bool| {
            let guard = if wait {
                device.lock().ok()
            } else {
                device.try_lock().ok()
            };
            let Some(mut device) = guard else {
                return false;
            };
            if let Err(error) = device.stop_stream() {
                debug!(%error, "clearing StreamEnable");
            }
            true
        });
        let mut frames =
            U3vFrameStream::start_inner(stream, Some(disable), Some(self.device.lost_flag()));
        if let Some(time_sync) = self.time_sync {
            frames.set_time_sync(time_sync);
        }
        Ok(frames)
    }
}

impl<T: UsbTransfer + 'static> crate::Camera<U3vRegisterIo<T>> {
    /// Open the camera's stream, ready for `acquisition_start`: the recipe
    /// [`Camera::open_stream`](crate::Camera::open_stream) is for GigE
    /// Vision, with the camera's `PayloadSize` as the hint when the device's
    /// SIRM gives none, and its time sync.
    ///
    /// Without `PayloadSize`, the hint is computed from Width, Height and
    /// PixelFormat; a camera that has the feature and cannot answer it is
    /// an error, since a computed size misses padding and chunk data.
    pub fn open_stream(
        &mut self,
        options: U3vStreamOptions,
    ) -> Result<U3vFrameStream, GenicamError> {
        let mut options = options;
        if options.payload_size_hint == 0 {
            options.payload_size_hint = frame_size(self)?;
        }
        U3vStreamBuilder::new(self.transport())
            .options(options)
            .time_sync(self.time_sync().clone())
            .build()
    }
}

/// Bytes in one frame. The camera's `PayloadSize` is the authority: it knows
/// its own packing and padding. Without it, the format's `image_bytes` for
/// width × height, exact for a packed format such as `Mono12Packed`; guessing
/// one byte a pixel made the reader take the rest of the image for the
/// trailer and fail (Codex review of #179).
fn frame_size<T: viva_genapi::RegisterIo>(camera: &crate::Camera<T>) -> Result<u64, GenicamError> {
    match camera.get("PayloadSize") {
        Ok(text) => {
            if let Some(size) = text.parse::<u64>().ok().filter(|&size| size > 0) {
                return Ok(size);
            }
        }
        // Only a camera without the feature falls back. One that has it but
        // cannot answer — I/O, access, availability — must not get a
        // computed size that misses its padding or chunk data (Codex review
        // of #180).
        Err(error) if payload_size_is_absent(&error) => {}
        Err(error) => return Err(error),
    }
    let dimension = |name: &str| -> Result<u32, GenicamError> {
        let value = camera.get_integer(name)?;
        u32::try_from(value).map_err(|_| {
            viva_genapi::GenApiError::Range {
                name: name.to_string(),
                value: viva_genapi::swissknife::Value::Int(value),
                violation: viva_genapi::RangeViolation::Unrepresentable,
            }
            .into()
        })
    };
    let pixel_format = camera.get("PixelFormat")?;
    computed_frame_size(&pixel_format, dimension("Width")?, dimension("Height")?)
}

/// The bytes a `width × height` frame in the `pixel_format` named occupies,
/// for a camera without `PayloadSize`.
fn computed_frame_size(pixel_format: &str, width: u32, height: u32) -> Result<u64, GenicamError> {
    PixelFormat::from_name(pixel_format)
        .image_bytes(width, height)
        .map(|bytes| bytes as u64)
        .ok_or_else(|| GenicamError::UnsizedFrame {
            pixel_format: pixel_format.to_string(),
        })
}

/// Whether reading `PayloadSize` failed because the camera has no such
/// feature, the one failure a computed size may stand in for.
fn payload_size_is_absent(error: &GenicamError) -> bool {
    matches!(
        error,
        GenicamError::GenApi(viva_genapi::GenApiError::NodeNotFound(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a missing `PayloadSize` may be replaced by a computed size (Codex
    /// review of #180).
    #[test]
    fn only_a_missing_payload_size_falls_back() {
        use viva_genapi::GenApiError;
        let missing = GenicamError::GenApi(GenApiError::NodeNotFound("PayloadSize".into()));
        let refused = GenicamError::GenApi(GenApiError::Access {
            name: "PayloadSize".into(),
            refused: viva_genapi::AccessDirection::Read,
        });
        let failed = GenicamError::U3v {
            during: "read PayloadSize",
            source: U3vError::Timeout,
        };
        assert!(payload_size_is_absent(&missing));
        assert!(!payload_size_is_absent(&refused));
        assert!(!payload_size_is_absent(&failed));
    }

    /// Without `PayloadSize`, a frame is sized from its PFNC code: a packed
    /// format at its packed size (`Mono12Packed`: 64 × 48 × 1.5, as the fake
    /// U3V camera's own `PayloadSize` says), not refused.
    #[test]
    fn a_camera_without_payload_size_is_sized_from_its_format() {
        assert_eq!(computed_frame_size("Mono8", 64, 48).unwrap(), 3072);
        assert_eq!(computed_frame_size("Mono16", 64, 48).unwrap(), 6144);
        assert_eq!(computed_frame_size("Mono12Packed", 64, 48).unwrap(), 4608);
        assert_eq!(computed_frame_size("Mono10p", 64, 48).unwrap(), 3840);
        assert_eq!(computed_frame_size("YUV422Packed", 64, 48).unwrap(), 6144);
        // A name nobody knows has no size to compute.
        assert!(computed_frame_size("Fancy42", 64, 48).is_err());
    }
}
