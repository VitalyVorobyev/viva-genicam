//! The Data Stream module and its acquisition engine (GenTL 1.6 §2.2.4,
//! §3.5, §5), the Buffer module (§3.6), and the `EVENT_NEW_BUFFER` event
//! (§4.2).
//!
//! The receive engine assembles each block into memory of its own; a
//! delivery thread takes the finished block and, under this stream's lock,
//! copies it once into a buffer from the input pool and appends that buffer
//! to the output queue (ADR-0031). The output queue *is* the new-buffer
//! event's data queue (§5.1.3: "When the event data is retrieved the
//! associated buffer is removed from the output buffer queue").

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use viva_genicam::{Frame, GenicamError, StreamOptions};

use crate::abi::*;
use crate::device::{Device, STREAM_ID};
use crate::error::{
    Direction, Failure, Result, from_genicam, invalid_parameter, not_available, not_implemented,
    poisoned,
};
use crate::library::{Library, Object};
use crate::out::Info;
use crate::regmap::{Feature, Features, Kind, Layout, Register, Repr, TL_TYPE_ENTRIES, Value};

/// How long the delivery thread waits for a frame before checking whether
/// it should stop.
const POLL: Duration = Duration::from_millis(50);

/// `STREAM_INFO_BUF_ANNOUNCE_MIN`: one buffer is enough to stream.
const ANNOUNCE_MIN: usize = 1;

/// Where a buffer is (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    /// Announced only: the consumer's.
    Unqueued,
    /// In the input pool, waiting to be filled.
    Input,
    /// In the output queue, waiting to be delivered.
    Output,
}

/// Which way the stream's buffers were announced; §3.6: the two "must not
/// be mixed on a single Data Stream module".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Consumer,
    Producer,
}

/// Memory `DSAllocAndAnnounceBuffer` allocated: owned through a raw pointer,
/// never through a reference, because the consumer reads it through one too.
struct Allocation {
    ptr: *mut [u8],
}

// SAFETY: plain memory, freed once, by whoever drops the allocation.
unsafe impl Send for Allocation {}

impl Allocation {
    fn new(size: usize) -> Result<Self> {
        let mut memory = Vec::new();
        memory.try_reserve_exact(size).map_err(|_| {
            Failure::new(
                GC_ERR_OUT_OF_MEMORY,
                format!("cannot allocate a {size}-byte buffer"),
            )
        })?;
        memory.resize(size, 0u8);
        Ok(Self {
            ptr: Box::into_raw(memory.into_boxed_slice()),
        })
    }

    fn base(&self) -> usize {
        self.ptr.cast::<u8>() as usize
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from `Box::into_raw` and is freed only here.
        drop(unsafe { Box::from_raw(self.ptr) });
    }
}

/// What the last delivery wrote into a buffer.
#[derive(Debug, Clone, Default)]
struct Fill {
    /// Filled by the acquisition engine, rather than flushed to the output
    /// queue (`BUFFER_INFO_NEW_DATA`).
    new_data: bool,
    incomplete: bool,
    larger_than_buffer: bool,
    size_filled: usize,
    /// The block's whole size; `None` when packets were lost from its end.
    data_size: Option<usize>,
    frame: Option<FrameInfo>,
}

/// The block's description, from its leader and trailer.
#[derive(Debug, Clone)]
struct FrameInfo {
    block_id: u64,
    width: u32,
    height: u32,
    padding_x: u16,
    pixel_format: u32,
    payload_type: u16,
    timestamp: Option<u64>,
    /// Each chunk's ID, offset and length in the buffer.
    chunks: Vec<(u64, usize, usize)>,
}

struct BufferEntry {
    key: usize,
    /// The memory's address, as an integer: the stream's state must be
    /// `Send`, and this is only ever turned back into a pointer to copy.
    base: usize,
    size: usize,
    private: usize,
    owned: Option<Allocation>,
    location: Location,
    fill: Option<Fill>,
}

/// The registered new-buffer event.
struct EventState {
    key: usize,
    /// `EventKill`s not yet consumed by a wait (§4.2.3.2).
    kills: usize,
    fired: u64,
}

struct State {
    buffers: Vec<BufferEntry>,
    input: VecDeque<usize>,
    output: VecDeque<usize>,
    origin: Option<Origin>,
    /// `DSStartAcquisition` called, and not yet `DSStopAcquisition`.
    started: bool,
    /// The engine is delivering (`STREAM_INFO_IS_GRABBING`).
    running: bool,
    /// Buffers still to deliver before the engine stops by itself.
    remaining: Option<u64>,
    delivered: u64,
    underruns: u64,
    begun: u64,
    event: Option<EventState>,
    closed: bool,
}

/// The acquisition: the delivery thread, and the flag that stops it.
struct Engine {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

/// The Data Stream module.
pub(crate) struct Stream {
    pub(crate) key: usize,
    device: Arc<Device>,
    state: Mutex<State>,
    /// Signalled whenever the output queue grows, an event is killed or
    /// unregistered, or the stream closes.
    wake: Condvar,
    engine: Mutex<Option<Engine>>,
}

impl Stream {
    pub(crate) fn new(key: usize, device: Arc<Device>) -> Self {
        Self {
            key,
            device,
            state: Mutex::new(State {
                buffers: Vec::new(),
                input: VecDeque::new(),
                output: VecDeque::new(),
                origin: None,
                started: false,
                running: false,
                remaining: None,
                delivered: 0,
                underruns: 0,
                begun: 0,
                event: None,
                closed: false,
            }),
            wake: Condvar::new(),
            engine: Mutex::new(None),
        }
    }

    pub(crate) fn device_key(&self) -> usize {
        self.device.key
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state.lock().map_err(poisoned)
    }

    // -----------------------------------------------------------------------
    // Buffers
    // -----------------------------------------------------------------------

    /// `DSAnnounceBuffer` (`Some(memory)`) and `DSAllocAndAnnounceBuffer`
    /// (`None`).
    pub(crate) fn announce(
        self: &Arc<Self>,
        lib: &Library,
        memory: Option<usize>,
        size: usize,
        private: usize,
    ) -> Result<usize> {
        if size == 0 {
            return Err(invalid_parameter("a buffer of 0 bytes"));
        }
        let origin = match memory {
            Some(_) => Origin::Consumer,
            None => Origin::Producer,
        };
        // Allocate before taking the lock: it may be large.
        let owned = match memory {
            Some(_) => None,
            None => Some(Allocation::new(size)?),
        };
        let base = memory.unwrap_or_else(|| owned.as_ref().map_or(0, Allocation::base));
        let mut state = self.lock()?;
        if state.origin.is_some_and(|o| o != origin) {
            return Err(Failure::new(
                GC_ERR_ACCESS_DENIED,
                "consumer and producer buffers must not be mixed on one stream (§3.6)",
            ));
        }
        if origin == Origin::Consumer && state.buffers.iter().any(|b| b.base == base) {
            return Err(Failure::new(
                GC_ERR_RESOURCE_IN_USE,
                "this memory is already announced to the stream",
            ));
        }
        let key = lib.insert(Some(self.key), Object::Buffer(self.clone()))?;
        state.origin = Some(origin);
        state.buffers.push(BufferEntry {
            key,
            base,
            size,
            private,
            owned,
            location: Location::Unqueued,
            fill: None,
        });
        Ok(key)
    }

    fn entry_mut(state: &mut State, key: usize) -> Result<&mut BufferEntry> {
        state
            .buffers
            .iter_mut()
            .find(|b| b.key == key)
            .ok_or_else(|| Failure::new(GC_ERR_INVALID_HANDLE, "not a buffer of this stream"))
    }

    /// `DSRevokeBuffer`: the consumer's memory (or null for the producer's)
    /// and the private pointer.
    pub(crate) fn revoke(&self, lib: &Library, key: usize) -> Result<(usize, usize)> {
        let mut state = self.lock()?;
        let index = state
            .buffers
            .iter()
            .position(|b| b.key == key)
            .ok_or_else(|| Failure::new(GC_ERR_INVALID_HANDLE, "not a buffer of this stream"))?;
        if state.buffers[index].location != Location::Unqueued {
            return Err(Failure::new(
                GC_ERR_BUSY,
                "the buffer is queued; flush or deliver it first",
            ));
        }
        let entry = state.buffers.remove(index);
        if state.buffers.is_empty() {
            state.origin = None;
        }
        drop(state);
        lib.forget(key)?;
        let memory = if entry.owned.is_some() { 0 } else { entry.base };
        Ok((memory, entry.private))
    }

    /// `DSQueueBuffer`.
    pub(crate) fn queue(&self, key: usize) -> Result<()> {
        let mut state = self.lock()?;
        let entry = Self::entry_mut(&mut state, key)?;
        if entry.location != Location::Unqueued {
            return Err(Failure::new(
                GC_ERR_RESOURCE_IN_USE,
                "the buffer is already queued or awaiting delivery",
            ));
        }
        entry.location = Location::Input;
        // "This value is reset to 0 when the buffer is placed into the Input
        // Buffer Pool" (BUFFER_INFO_SIZE_FILLED).
        entry.fill = None;
        state.input.push_back(key);
        Ok(())
    }

    /// `DSGetBufferID`.
    pub(crate) fn buffer_at(&self, index: u32) -> Result<usize> {
        let state = self.lock()?;
        state
            .buffers
            .get(index as usize)
            .map(|b| b.key)
            .ok_or_else(|| {
                Failure::new(
                    GC_ERR_INVALID_INDEX,
                    format!("buffer {index} of {}", state.buffers.len()),
                )
            })
    }

    /// `DSFlushQueue` (§6.4.4.1).
    pub(crate) fn flush(&self, operation: AcqQueueType) -> Result<()> {
        let mut state = self.lock()?;
        let mut moved = 0u64;
        match operation {
            ACQ_QUEUE_INPUT_TO_OUTPUT => {
                while let Some(key) = state.input.pop_front() {
                    let entry = Self::entry_mut(&mut state, key)?;
                    entry.location = Location::Output;
                    entry.fill = Some(Fill::default());
                    state.output.push_back(key);
                    moved += 1;
                }
            }
            ACQ_QUEUE_OUTPUT_DISCARD => Self::discard_output(&mut state),
            ACQ_QUEUE_ALL_TO_INPUT => {
                // "Puts all buffers in the input pool. This is including those
                // in the output buffer queue": every announced buffer, and the
                // event queue's entries go with the output queue.
                state.output.clear();
                let keys: Vec<usize> = state
                    .buffers
                    .iter_mut()
                    .filter(|b| b.location != Location::Input)
                    .map(|b| {
                        b.location = Location::Input;
                        b.fill = None;
                        b.key
                    })
                    .collect();
                state.input.extend(keys);
            }
            ACQ_QUEUE_UNQUEUED_TO_INPUT => {
                let keys: Vec<usize> = state
                    .buffers
                    .iter_mut()
                    .filter(|b| b.location == Location::Unqueued)
                    .map(|b| {
                        b.location = Location::Input;
                        b.fill = None;
                        b.key
                    })
                    .collect();
                state.input.extend(keys);
            }
            ACQ_QUEUE_ALL_DISCARD => {
                Self::discard_output(&mut state);
                let keys: Vec<usize> = state.input.drain(..).collect();
                for key in keys {
                    Self::entry_mut(&mut state, key)?.location = Location::Unqueued;
                }
            }
            other => return Err(not_implemented(format!("ACQ_QUEUE_TYPE {other}"))),
        }
        if moved > 0 {
            if let Some(event) = state.event.as_mut() {
                event.fired += moved;
            }
            drop(state);
            self.wake.notify_all();
        }
        Ok(())
    }

    /// Every buffer in the output queue back to the consumer, undelivered.
    fn discard_output(state: &mut State) {
        let keys: Vec<usize> = state.output.drain(..).collect();
        for key in keys {
            if let Ok(entry) = Self::entry_mut(state, key) {
                entry.location = Location::Unqueued;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Acquisition
    // -----------------------------------------------------------------------

    /// `DSStartAcquisition`: open the camera's stream channel to a fresh
    /// socket and start delivering.
    pub(crate) fn start(self: &Arc<Self>, flags: AcqStartFlags, count: u64) -> Result<()> {
        if flags != ACQ_START_FLAGS_DEFAULT {
            return Err(not_implemented(format!("ACQ_START_FLAGS {flags}")));
        }
        if count == 0 {
            return Err(invalid_parameter("iNumToAcquire is 0"));
        }
        let mut engine = self.engine.lock().map_err(poisoned)?;
        {
            let state = self.lock()?;
            if state.started {
                return Err(Failure::new(
                    GC_ERR_RESOURCE_IN_USE,
                    "the acquisition is already started",
                ));
            }
            if state.buffers.len() < ANNOUNCE_MIN {
                return Err(Failure::new(
                    GC_ERR_INVALID_BUFFER,
                    "announce a buffer before starting the acquisition",
                ));
            }
        }
        let io = self.device.io()?;
        let mut frames = io
            .open_stream(StreamOptions::new())
            .map_err(|err| from_genicam(&err, Direction::Write))?;
        frames.set_deliver_incomplete(true);
        {
            let mut state = self.lock()?;
            state.started = true;
            state.running = true;
            state.delivered = 0;
            state.remaining = (count != GENTL_INFINITE).then_some(count);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stream = self.clone();
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("viva-gentl-delivery".into())
            .spawn(move || {
                loop {
                    if stopping.load(Ordering::Acquire) {
                        break;
                    }
                    match frames.next_frame_blocking(POLL) {
                        Ok(Some(frame)) => {
                            if !stream.deliver(&frame) {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(GenicamError::Timeout(_)) => {}
                        Err(err) => {
                            tracing::warn!(error = %err, "the stream failed; delivery stops");
                            break;
                        }
                    }
                }
                // The receive engine and its socket go with `frames`.
            })
            .map_err(|err| {
                Failure::new(
                    GC_ERR_RESOURCE_EXHAUSTED,
                    format!("start the delivery thread: {err}"),
                )
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(failure) => {
                let mut state = self.lock()?;
                state.started = false;
                state.running = false;
                return Err(failure);
            }
        };
        *engine = Some(Engine { stop, thread });
        Ok(())
    }

    /// Copy `frame` into a buffer from the input pool and queue it for
    /// delivery. Whether the engine keeps going.
    fn deliver(&self, frame: &Frame) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if !state.running {
            return false;
        }
        let Some(key) = state.input.pop_front() else {
            // §5.3.1: with no buffer in the input pool the image is lost.
            state.underruns += 1;
            return true;
        };
        let Ok(entry) = Self::entry_mut(&mut state, key) else {
            return true;
        };
        let data = &frame.raw_payload;
        let filled = data.len().min(entry.size);
        // SAFETY: `entry.base` is valid for `entry.size` bytes until the
        // buffer is revoked (§6.3.5.2), and a buffer in the input pool cannot
        // be revoked; while it is there the producer alone may write it.
        // `copy` rather than `copy_nonoverlapping`: nothing stops a consumer
        // from announcing memory we cannot see into.
        unsafe { std::ptr::copy(data.as_ptr(), entry.base as *mut u8, filled) };
        let complete = frame.is_complete();
        let larger = data.len() > entry.size;
        let chunks = if frame.chunks.is_some() {
            chunk_layout(data, filled)
        } else {
            Vec::new()
        };
        entry.fill = Some(Fill {
            new_data: true,
            incomplete: !complete || larger,
            larger_than_buffer: larger,
            size_filled: filled,
            data_size: complete.then_some(data.len()),
            frame: Some(FrameInfo {
                block_id: frame.block_id,
                width: frame.width,
                height: frame.height,
                padding_x: frame.padding_x,
                pixel_format: frame.pixel_format.code(),
                payload_type: frame.payload_type,
                timestamp: frame.ts_dev,
                chunks,
            }),
        });
        entry.location = Location::Output;
        state.output.push_back(key);
        state.begun += 1;
        state.delivered += 1;
        if let Some(event) = state.event.as_mut() {
            event.fired += 1;
        }
        let more = match state.remaining.as_mut() {
            Some(remaining) => {
                *remaining = remaining.saturating_sub(1);
                *remaining > 0
            }
            None => true,
        };
        if !more {
            // §6.3.5.11: "the acquisition engine stops automatically".
            state.running = false;
        }
        drop(state);
        self.wake.notify_all();
        more
    }

    /// `DSStopAcquisition`.
    pub(crate) fn stop(&self, flags: AcqStopFlags) -> Result<()> {
        if flags != ACQ_STOP_FLAGS_DEFAULT && flags != ACQ_STOP_FLAGS_KILL {
            return Err(not_implemented(format!("ACQ_STOP_FLAGS {flags}")));
        }
        let mut engine = self.engine.lock().map_err(poisoned)?;
        if !self.lock()?.started {
            return Err(Failure::new(
                GC_ERR_RESOURCE_IN_USE,
                "the acquisition is not started",
            ));
        }
        // A buffer is filled in one step under the lock, so none is ever
        // partly filled: ACQ_STOP_FLAGS_KILL has nothing more to return.
        Self::join(engine.take());
        let mut state = self.lock()?;
        state.started = false;
        state.running = false;
        Ok(())
    }

    fn join(engine: Option<Engine>) {
        if let Some(Engine { stop, thread }) = engine {
            stop.store(true, Ordering::Release);
            let _ = thread.join();
        }
    }

    /// Close: stop the acquisition, wake every wait, revoke every buffer.
    pub(crate) fn shut_down(&self) {
        if let Ok(mut engine) = self.engine.lock() {
            Self::join(engine.take());
        }
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.started = false;
            state.running = false;
            state.event = None;
            state.input.clear();
            state.output.clear();
            state.buffers.clear();
        }
        self.wake.notify_all();
    }

    // -----------------------------------------------------------------------
    // The new-buffer event
    // -----------------------------------------------------------------------

    /// `GCRegisterEvent(EVENT_NEW_BUFFER)`.
    pub(crate) fn register_event(self: &Arc<Self>, lib: &Library) -> Result<usize> {
        let mut state = self.lock()?;
        if state.event.is_some() {
            return Err(Failure::new(
                GC_ERR_RESOURCE_IN_USE,
                "EVENT_NEW_BUFFER is already registered on this stream",
            ));
        }
        let key = lib.insert(Some(self.key), Object::NewBufferEvent(self.clone()))?;
        state.event = Some(EventState {
            key,
            kills: 0,
            fired: 0,
        });
        Ok(key)
    }

    /// The registered event's key, if any.
    pub(crate) fn event_key(&self) -> Result<Option<usize>> {
        Ok(self.lock()?.event.as_ref().map(|e| e.key))
    }

    /// `GCUnregisterEvent`, or the event's module closing: pending waits end
    /// with `GC_ERR_ABORT`, and the output queue's buffers become the
    /// consumer's (§6.3.7.7).
    pub(crate) fn unregister_event(&self, key: usize) {
        if let Ok(mut state) = self.state.lock()
            && state.event.as_ref().is_some_and(|e| e.key == key)
        {
            state.event = None;
            Self::discard_output(&mut state);
        }
        self.wake.notify_all();
    }

    /// `EventGetData`: the next delivered buffer, as `(buffer, private)`.
    /// The caller has checked that a non-null buffer is large enough, so a
    /// buffer taken here is always handed out.
    pub(crate) fn next_event(&self, key: usize, timeout_ms: u64) -> Result<(usize, usize)> {
        let deadline = (timeout_ms != GENTL_INFINITE)
            .then(|| Instant::now().checked_add(Duration::from_millis(timeout_ms)))
            .flatten();
        let mut state = self.lock()?;
        loop {
            let closed = state.closed;
            let event = match state.event.as_mut() {
                Some(event) if event.key == key && !closed => event,
                _ => {
                    return Err(Failure::new(GC_ERR_ABORT, "the event was unregistered"));
                }
            };
            if event.kills > 0 {
                event.kills -= 1;
                return Err(Failure::new(GC_ERR_ABORT, "EventKill ended the wait"));
            }
            if let Some(buffer) = state.output.pop_front() {
                let entry = Self::entry_mut(&mut state, buffer)?;
                entry.location = Location::Unqueued;
                return Ok((buffer, entry.private));
            }
            let wait = match deadline {
                None => Duration::from_secs(3600),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(Failure::new(
                            GC_ERR_TIMEOUT,
                            "no new buffer within the timeout",
                        ));
                    }
                    deadline - now
                }
            };
            state = self.wake.wait_timeout(state, wait).map_err(poisoned)?.0;
        }
    }

    /// `EventKill`: end one wait, now or the next one (§4.2.3.2).
    pub(crate) fn kill_event(&self, key: usize) -> Result<()> {
        let mut state = self.lock()?;
        match state.event.as_mut() {
            Some(event) if event.key == key => event.kills += 1,
            _ => {
                return Err(Failure::new(
                    GC_ERR_INVALID_HANDLE,
                    "the event is not registered",
                ));
            }
        }
        drop(state);
        self.wake.notify_all();
        Ok(())
    }

    /// `EventFlush`: discard the output queue.
    pub(crate) fn flush_event(&self, key: usize) -> Result<()> {
        let mut state = self.lock()?;
        if !state.event.as_ref().is_some_and(|e| e.key == key) {
            return Err(Failure::new(
                GC_ERR_INVALID_HANDLE,
                "the event is not registered",
            ));
        }
        Self::discard_output(&mut state);
        Ok(())
    }

    /// `EventGetInfo` (§6.4.6.2).
    pub(crate) fn event_info(&self, key: usize, cmd: EventInfoCmd) -> Result<Info> {
        let state = self.lock()?;
        let event = state
            .event
            .as_ref()
            .filter(|e| e.key == key)
            .ok_or_else(|| Failure::new(GC_ERR_INVALID_HANDLE, "the event is not registered"))?;
        Ok(match cmd {
            EVENT_EVENT_TYPE => Info::I32(EVENT_NEW_BUFFER),
            EVENT_NUM_IN_QUEUE => Info::SizeT(state.output.len()),
            EVENT_NUM_FIRED => Info::U64(event.fired),
            EVENT_SIZE_MAX => Info::SizeT(std::mem::size_of::<EventNewBufferData>()),
            EVENT_INFO_DATA_SIZE_MAX => Info::SizeT(std::mem::size_of::<usize>()),
            other => return Err(not_implemented(format!("EVENT_INFO_CMD {other}"))),
        })
    }

    // -----------------------------------------------------------------------
    // Info
    // -----------------------------------------------------------------------

    /// `DSGetInfo` (§6.4.4.8).
    pub(crate) fn info(&self, cmd: StreamInfoCmd) -> Result<Info> {
        let state = self.lock()?;
        let count = |location| {
            state
                .buffers
                .iter()
                .filter(|b| b.location == location)
                .count()
        };
        Ok(match cmd {
            STREAM_INFO_ID => Info::Str(STREAM_ID.into()),
            STREAM_INFO_NUM_DELIVERED => Info::U64(state.delivered),
            STREAM_INFO_NUM_UNDERRUN => Info::U64(state.underruns),
            STREAM_INFO_NUM_ANNOUNCED => Info::SizeT(state.buffers.len()),
            STREAM_INFO_NUM_QUEUED => Info::SizeT(count(Location::Input)),
            STREAM_INFO_NUM_AWAIT_DELIVERY => Info::SizeT(count(Location::Output)),
            STREAM_INFO_NUM_STARTED => Info::U64(state.begun),
            STREAM_INFO_PAYLOAD_SIZE => {
                return Err(not_available(
                    "the stream does not define the payload size: read PayloadSize from the \
                     remote device (STREAM_INFO_DEFINES_PAYLOADSIZE is false)",
                ));
            }
            STREAM_INFO_IS_GRABBING => Info::Bool(state.running),
            STREAM_INFO_DEFINES_PAYLOADSIZE => Info::Bool(false),
            STREAM_INFO_TLTYPE => Info::Str(TL_TYPE_GEV.into()),
            STREAM_INFO_NUM_CHUNKS_MAX => {
                return Err(not_available(
                    "the number of chunks is not known in advance",
                ));
            }
            STREAM_INFO_BUF_ANNOUNCE_MIN => Info::SizeT(ANNOUNCE_MIN),
            STREAM_INFO_BUF_ALIGNMENT => Info::SizeT(1),
            STREAM_INFO_FLOW_TABLE | STREAM_INFO_GENDC_PREFETCH_DESCRIPTOR => {
                return Err(Failure::new(
                    GC_ERR_NO_DATA,
                    "the stream uses no flows and delivers no GenDC",
                ));
            }
            other => return Err(not_implemented(format!("STREAM_INFO_CMD {other}"))),
        })
    }

    /// `DSGetBufferInfo` (§6.4.4.4).
    pub(crate) fn buffer_info(&self, key: usize, cmd: BufferInfoCmd) -> Result<Info> {
        let state = self.lock()?;
        let entry =
            state.buffers.iter().find(|b| b.key == key).ok_or_else(|| {
                Failure::new(GC_ERR_INVALID_HANDLE, "not a buffer of this stream")
            })?;
        let fill = entry.fill.clone().unwrap_or_default();
        let frame = || {
            fill.frame
                .clone()
                .ok_or_else(|| not_available("the buffer holds no frame"))
        };
        let chunked = fill.frame.as_ref().is_some_and(|f| !f.chunks.is_empty());
        Ok(match cmd {
            BUFFER_INFO_BASE => Info::Ptr(entry.base),
            BUFFER_INFO_SIZE => Info::SizeT(entry.size),
            BUFFER_INFO_USER_PTR => Info::Ptr(entry.private),
            BUFFER_INFO_TIMESTAMP => Info::U64(
                frame()?
                    .timestamp
                    .ok_or_else(|| not_available("the block carried no timestamp"))?,
            ),
            BUFFER_INFO_NEW_DATA => Info::Bool(fill.new_data),
            BUFFER_INFO_IS_QUEUED => Info::Bool(entry.location != Location::Unqueued),
            BUFFER_INFO_IS_ACQUIRING => Info::Bool(false),
            BUFFER_INFO_IS_INCOMPLETE => Info::Bool(fill.incomplete),
            BUFFER_INFO_TLTYPE => Info::Str(TL_TYPE_GEV.into()),
            BUFFER_INFO_SIZE_FILLED => Info::SizeT(fill.size_filled),
            BUFFER_INFO_WIDTH => Info::SizeT(frame()?.width as usize),
            BUFFER_INFO_HEIGHT => Info::SizeT(frame()?.height as usize),
            BUFFER_INFO_XPADDING => Info::SizeT(usize::from(frame()?.padding_x)),
            BUFFER_INFO_XOFFSET | BUFFER_INFO_YOFFSET | BUFFER_INFO_YPADDING => {
                // §5.5: not available means "as the remote device says".
                return Err(not_available("read the offsets from the remote device"));
            }
            BUFFER_INFO_FRAMEID => Info::U64(frame()?.block_id),
            BUFFER_INFO_IMAGEPRESENT => Info::Bool(is_image(frame()?.payload_type)),
            BUFFER_INFO_IMAGEOFFSET => Info::SizeT(0),
            BUFFER_INFO_PAYLOADTYPE => Info::SizeT(payload_type(frame()?.payload_type)),
            BUFFER_INFO_PIXELFORMAT => Info::U64(u64::from(frame()?.pixel_format)),
            BUFFER_INFO_PIXELFORMAT_NAMESPACE => {
                frame()?;
                Info::U64(PIXELFORMAT_NAMESPACE_PFNC_32BIT)
            }
            BUFFER_INFO_PIXEL_ENDIANNESS => {
                frame()?;
                Info::I32(PIXELENDIANNESS_LITTLE)
            }
            BUFFER_INFO_DELIVERED_IMAGEHEIGHT => {
                let frame = frame()?;
                if fill.incomplete {
                    return Err(not_available("the block is incomplete"));
                }
                Info::SizeT(frame.height as usize)
            }
            BUFFER_INFO_DELIVERED_CHUNKPAYLOADSIZE => {
                if !chunked {
                    return Err(not_available("the buffer holds no chunks"));
                }
                Info::SizeT(fill.size_filled)
            }
            BUFFER_INFO_CHUNKLAYOUTID | BUFFER_INFO_FILENAME => {
                return Err(not_available("not carried by the receive engine"));
            }
            BUFFER_INFO_DATA_SIZE => match (&fill.frame, fill.data_size) {
                (None, _) => Info::SizeT(0),
                (Some(_), Some(size)) => Info::SizeT(size),
                (Some(_), None) => {
                    return Err(not_available(
                        "packets were lost; the full size is not known",
                    ));
                }
            },
            BUFFER_INFO_TIMESTAMP_NS => {
                let ticks = frame()?
                    .timestamp
                    .ok_or_else(|| not_available("the block carried no timestamp"))?;
                let hz = self
                    .device
                    .tick_frequency()
                    .ok_or_else(|| not_available("the camera's tick frequency is not known"))?;
                let ns = u128::from(ticks) * 1_000_000_000 / u128::from(hz);
                Info::U64(u64::try_from(ns).unwrap_or(u64::MAX))
            }
            BUFFER_INFO_DATA_LARGER_THAN_BUFFER => Info::Bool(fill.larger_than_buffer),
            BUFFER_INFO_CONTAINS_CHUNKDATA => Info::Bool(chunked),
            BUFFER_INFO_IS_COMPOSITE => Info::Bool(false),
            other => return Err(not_implemented(format!("BUFFER_INFO_CMD {other}"))),
        })
    }

    /// `DSGetBufferChunkData`: each chunk's ID, offset and length.
    pub(crate) fn chunks(&self, key: usize) -> Result<Vec<(u64, usize, usize)>> {
        let state = self.lock()?;
        let entry =
            state.buffers.iter().find(|b| b.key == key).ok_or_else(|| {
                Failure::new(GC_ERR_INVALID_HANDLE, "not a buffer of this stream")
            })?;
        entry
            .fill
            .as_ref()
            .and_then(|fill| fill.frame.as_ref())
            .filter(|frame| !frame.chunks.is_empty())
            .map(|frame| frame.chunks.clone())
            .ok_or_else(|| Failure::new(GC_ERR_NO_DATA, "the buffer holds no chunk data"))
    }
}

/// The chunks of a GigE Vision payload that lie within its first `filled`
/// bytes, as (ID, offset, length).
/// In the order they sit in the buffer.
fn chunk_layout(payload: &[u8], filled: usize) -> Vec<(u64, usize, usize)> {
    let mut chunks: Vec<(u64, usize, usize)> = viva_genicam::gige::parse_chunks(payload)
        .map(|chunks| {
            chunks
                .into_iter()
                .map(|chunk| (u64::from(chunk.id), chunk.offset, chunk.data.len()))
                .filter(|&(_, offset, len)| offset + len <= filled)
                .collect()
        })
        .unwrap_or_default();
    chunks.sort_by_key(|&(_, offset, _)| offset);
    chunks
}

/// Whether a GigE Vision payload type carries an image: `0x0001`, with or
/// without the extended-chunk flag.
fn is_image(gev: u16) -> bool {
    gev & 0x3FFF == 0x0001
}

/// GenTL's payload type for a GigE Vision one (§6.4.4.5). The extended-chunk
/// flag (`0x4000`) does not change the type: chunks are reported with
/// `BUFFER_INFO_CONTAINS_CHUNKDATA` since GenTL 1.5.
fn payload_type(gev: u16) -> usize {
    if gev & 0x8000 != 0 {
        return PAYLOAD_TYPE_DEVICE_SPECIFIC;
    }
    match gev & 0x3FFF {
        0x0001 => PAYLOAD_TYPE_IMAGE,
        0x0002 => PAYLOAD_TYPE_RAW_DATA,
        0x0003 => PAYLOAD_TYPE_FILE,
        0x0004 => PAYLOAD_TYPE_CHUNK_ONLY,
        0x0006 => PAYLOAD_TYPE_JPEG,
        0x0007 => PAYLOAD_TYPE_JPEG2000,
        0x0008 => PAYLOAD_TYPE_H264,
        0x000A => PAYLOAD_TYPE_MULTI_PART,
        0x000B => PAYLOAD_TYPE_GENDC,
        _ => PAYLOAD_TYPE_UNKNOWN,
    }
}

static LAYOUT: Layout = Layout {
    file: "viva_gentl_datastream",
    port: "StreamPort",
    guid: "76697661-6765-6e74-6c00-000000000004",
    categories: &[
        ("Root", &["StreamInformation", "BufferHandlingControl"]),
        ("StreamInformation", &["StreamID", "StreamType"]),
        (
            "BufferHandlingControl",
            &[
                "StreamAnnouncedBufferCount",
                "StreamBufferHandlingMode",
                "StreamAnnounceBufferMinimum",
                "StreamDeliveredFrameCount",
                "StreamLostFrameCount",
                "StreamInputBufferCount",
                "StreamOutputBufferCount",
                "StreamStartedFrameCount",
                "StreamIsGrabbing",
            ],
        ),
    ],
    features: &[
        Feature {
            name: "StreamID",
            kind: Kind::Str,
            tip: "Device-unique identifier of this data stream.",
        },
        Feature {
            name: "StreamType",
            kind: Kind::Enum(TL_TYPE_ENTRIES),
            tip: "Transport layer type of this data stream.",
        },
        Feature {
            name: "StreamAnnouncedBufferCount",
            kind: Kind::Int(Repr::Number),
            tip: "Buffers announced to this stream.",
        },
        Feature {
            name: "StreamBufferHandlingMode",
            kind: Kind::Enum(&[("OldestFirst", 0)]),
            tip: "The oldest filled buffer is delivered first; with no buffer queued, a frame is lost.",
        },
        Feature {
            name: "StreamAnnounceBufferMinimum",
            kind: Kind::Int(Repr::Number),
            tip: "Buffers to announce before the acquisition can start.",
        },
        Feature {
            name: "StreamDeliveredFrameCount",
            kind: Kind::Int(Repr::Number),
            tip: "Frames delivered since the acquisition started.",
        },
        Feature {
            name: "StreamLostFrameCount",
            kind: Kind::Int(Repr::Number),
            tip: "Frames lost because no buffer was queued.",
        },
        Feature {
            name: "StreamInputBufferCount",
            kind: Kind::Int(Repr::Number),
            tip: "Buffers in the input pool.",
        },
        Feature {
            name: "StreamOutputBufferCount",
            kind: Kind::Int(Repr::Number),
            tip: "Buffers in the output queue.",
        },
        Feature {
            name: "StreamStartedFrameCount",
            kind: Kind::Int(Repr::Number),
            tip: "Frames the acquisition engine began to fill.",
        },
        Feature {
            name: "StreamIsGrabbing",
            kind: Kind::Bool,
            tip: "Whether the acquisition engine is delivering.",
        },
    ],
    xml: OnceLock::new(),
};

impl Features for Stream {
    fn layout(&self) -> &'static Layout {
        &LAYOUT
    }

    fn read_feature(&self, feature: &Feature, _register: Register) -> Result<Value> {
        let number = |cmd| -> Result<Value> {
            Ok(Value::Int(match self.info(cmd)? {
                Info::SizeT(n) => n as i64,
                Info::U64(n) => i64::try_from(n).unwrap_or(i64::MAX),
                Info::Bool(b) => i64::from(b),
                _ => 0,
            }))
        };
        match feature.name {
            "StreamID" => Ok(Value::Str(STREAM_ID.into())),
            "StreamType" => Ok(Value::Int(crate::regmap::GIGE_VISION)),
            "StreamAnnouncedBufferCount" => number(STREAM_INFO_NUM_ANNOUNCED),
            "StreamBufferHandlingMode" => Ok(Value::Int(0)),
            "StreamAnnounceBufferMinimum" => number(STREAM_INFO_BUF_ANNOUNCE_MIN),
            "StreamDeliveredFrameCount" => number(STREAM_INFO_NUM_DELIVERED),
            "StreamLostFrameCount" => number(STREAM_INFO_NUM_UNDERRUN),
            "StreamInputBufferCount" => number(STREAM_INFO_NUM_QUEUED),
            "StreamOutputBufferCount" => number(STREAM_INFO_NUM_AWAIT_DELIVERY),
            "StreamStartedFrameCount" => number(STREAM_INFO_NUM_STARTED),
            "StreamIsGrabbing" => number(STREAM_INFO_IS_GRABBING),
            name => Err(not_implemented(format!("feature {name}"))),
        }
    }

    fn write_feature(&self, feature: &Feature, _value: i64) -> Result<()> {
        Err(Failure::new(
            GC_ERR_ACCESS_DENIED,
            format!("{} is not writable", feature.name),
        ))
    }
}
