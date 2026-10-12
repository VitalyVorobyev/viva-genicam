//! The GVSP receive thread and the queue it delivers into (ADR-0025).
//!
//! One `std::thread` per stream, on every OS, reads the socket with a short
//! timeout. Each wake-up — a datagram or the timeout — feeds the
//! [`Assembler`] and lets its timers fire, so a block closes on time whether
//! or not the application is asking for frames. Finished blocks go into a
//! [`DeliveryQueue`] of [`DELIVERY_CAPACITY`]: when the application falls
//! behind, the oldest waiting frame is dropped and counted as backpressure,
//! rather than the kernel dropping packets where no counter can see them.
//!
//! The thread also enforces the camera as the only source, and sends the
//! resend requests the assembler finds due — from this socket, to the
//! camera's GVCP port, without waiting for an answer (SR-04).

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::resend::{ResendConfig, ResendRequest};
use super::stats::{IgnoredPacket, StreamStatsAccumulator};
use tracing::{debug, warn};
use viva_gige::{DEFAULT_RCVBUF_BYTES, RecvBuffer};

use super::assembly::{
    AssembledBlock, Assembler, AssemblyConfig, DEFAULT_BLOCK_TIMEOUT, payload_stride,
};
use super::{GenericStreamBlock, classify_completed_block};
use viva_gige::GVSP_HEADER_SIZE;

/// How long one socket read waits. It is the resolution of every timer the
/// thread runs: block timeouts, the silence warning, and how soon it sees a
/// request to stop.
pub(crate) const TICK: Duration = Duration::from_millis(5);

/// Frames waiting for the application before the oldest is dropped.
pub(crate) const DELIVERY_CAPACITY: usize = 4;

/// Largest UDP payload, so no datagram is ever truncated by our buffer.
const DATAGRAM_MAX: usize = 65_536;

/// The end of a stream as the queue reports it.
#[derive(Debug)]
enum End {
    /// The receive thread failed, doing what, with which socket error; reported
    /// once.
    Failed(&'static str, std::io::Error),
    /// The receive thread stopped.
    Finished,
}

/// What a pop found.
#[derive(Debug)]
pub(crate) enum Popped<T> {
    Item(T),
    Failed(&'static str, std::io::Error),
    Finished,
    /// Nothing within the time allowed.
    Empty,
}

#[derive(Debug)]
struct QueueState<T> {
    items: VecDeque<T>,
    end: Option<End>,
}

/// A bounded, drop-oldest hand-off from the receive thread to one consumer,
/// with a blocking and an async side.
#[derive(Debug)]
pub(crate) struct DeliveryQueue<T> {
    state: Mutex<QueueState<T>>,
    ready: Condvar,
    notify: tokio::sync::Notify,
    capacity: usize,
}

impl<T> DeliveryQueue<T> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                items: VecDeque::with_capacity(capacity),
                end: None,
            }),
            ready: Condvar::new(),
            notify: tokio::sync::Notify::new(),
            capacity: capacity.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState<T>> {
        // A poisoned lock means a thread panicked while holding it; the
        // state is a queue of values, still coherent, so carry on.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wake(&self) {
        self.ready.notify_all();
        self.notify.notify_one();
    }

    /// Add an item; returns the oldest one if the queue was full.
    pub(crate) fn push(&self, item: T) -> Option<T> {
        let evicted = {
            let mut state = self.lock();
            let evicted = if state.items.len() >= self.capacity {
                state.items.pop_front()
            } else {
                None
            };
            state.items.push_back(item);
            evicted
        };
        self.wake();
        evicted
    }

    /// Record that the stream failed. Items already queued are still handed
    /// out first.
    pub(crate) fn fail(&self, during: &'static str, err: std::io::Error) {
        {
            let mut state = self.lock();
            if state.end.is_none() {
                state.end = Some(End::Failed(during, err));
            }
        }
        self.wake();
    }

    /// Record that the stream ended, unless it already failed.
    pub(crate) fn finish(&self) {
        {
            let mut state = self.lock();
            if state.end.is_none() {
                state.end = Some(End::Finished);
            }
        }
        self.wake();
    }

    fn try_pop_locked(state: &mut QueueState<T>) -> Popped<T> {
        if let Some(item) = state.items.pop_front() {
            return Popped::Item(item);
        }
        match state.end.take() {
            Some(End::Failed(during, err)) => {
                // Report the failure once; after that the stream has ended.
                state.end = Some(End::Finished);
                Popped::Failed(during, err)
            }
            Some(End::Finished) => {
                state.end = Some(End::Finished);
                Popped::Finished
            }
            None => Popped::Empty,
        }
    }

    /// Take the next item without waiting.
    pub(crate) fn try_pop(&self) -> Popped<T> {
        Self::try_pop_locked(&mut self.lock())
    }

    /// Wait up to `timeout` for the next item.
    pub(crate) fn pop_blocking(&self, timeout: Duration) -> Popped<T> {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = self.lock();
        loop {
            match Self::try_pop_locked(&mut state) {
                Popped::Empty => {}
                other => return other,
            }
            let wait = match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Popped::Empty;
                    }
                    deadline - now
                }
                // A timeout too large to add to now: wait in long slices.
                None => Duration::from_secs(3600),
            };
            state = self
                .ready
                .wait_timeout(state, wait)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// Wait for the next item. Cancel-safe: an item is taken only by the poll
    /// that returns it.
    pub(crate) async fn pop(&self) -> Popped<T> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Register before checking, so a push between the check and the
            // await still wakes us.
            notified.as_mut().enable();
            match self.try_pop() {
                Popped::Empty => notified.await,
                other => return other,
            }
        }
    }
}

/// Where resend requests go: the camera's GVCP address and the stream
/// channel, packed as `ip << 32 | port << 16 | channel`. 0 is "do not ask",
/// since 0.0.0.0 is never a camera.
fn pack_target(camera: SocketAddrV4, channel: u16) -> u64 {
    (u64::from(u32::from(*camera.ip())) << 32)
        | (u64::from(camera.port()) << 16)
        | u64::from(channel)
}

fn unpack_target(packed: u64) -> Option<(SocketAddrV4, u16)> {
    (packed != 0).then(|| {
        let ip = Ipv4Addr::from((packed >> 32) as u32);
        let port = (packed >> 16) as u16;
        (SocketAddrV4::new(ip, port), packed as u16)
    })
}

/// State shared between a `FrameStream` and its receive thread.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) queue: DeliveryQueue<GenericStreamBlock>,
    pub(crate) stop: AtomicBool,
    block_timeout_ns: AtomicU64,
    /// `PayloadSize`; 0 when unknown.
    payload_size: AtomicU64,
    deliver_incomplete: AtomicBool,
    /// The only source accepted, as a `u32`; 0 accepts any.
    source_filter: AtomicU32,
    /// See [`pack_target`].
    resend_target: AtomicU64,
    resend_delay_ns: AtomicU64,
    resend_retry_ns: AtomicU64,
    resend_max_requests: AtomicU32,
    /// `f64` bits.
    resend_budget: AtomicU64,
}

impl Shared {
    pub(crate) fn new() -> Self {
        let resend = ResendConfig::default();
        Self {
            queue: DeliveryQueue::new(DELIVERY_CAPACITY),
            stop: AtomicBool::new(false),
            block_timeout_ns: AtomicU64::new(DEFAULT_BLOCK_TIMEOUT.as_nanos() as u64),
            payload_size: AtomicU64::new(0),
            deliver_incomplete: AtomicBool::new(false),
            source_filter: AtomicU32::new(0),
            resend_target: AtomicU64::new(0),
            resend_delay_ns: AtomicU64::new(nanos(resend.delay)),
            resend_retry_ns: AtomicU64::new(nanos(resend.retry_interval)),
            resend_max_requests: AtomicU32::new(resend.max_requests),
            resend_budget: AtomicU64::new(resend.budget.to_bits()),
        }
    }

    pub(crate) fn set_source_filter(&self, camera: Option<Ipv4Addr>) {
        self.source_filter
            .store(camera.map_or(0, u32::from), Ordering::Relaxed);
    }

    pub(crate) fn source_filter(&self) -> Option<Ipv4Addr> {
        match self.source_filter.load(Ordering::Relaxed) {
            0 => None,
            ip => Some(Ipv4Addr::from(ip)),
        }
    }

    pub(crate) fn set_resend_target(&self, target: Option<(SocketAddrV4, u16)>) {
        let packed = target.map_or(0, |(camera, channel)| pack_target(camera, channel));
        self.resend_target.store(packed, Ordering::Relaxed);
    }

    pub(crate) fn resend_target(&self) -> Option<(SocketAddrV4, u16)> {
        unpack_target(self.resend_target.load(Ordering::Relaxed))
    }

    pub(crate) fn set_resend_delay(&self, delay: Duration) {
        self.resend_delay_ns.store(nanos(delay), Ordering::Relaxed);
    }

    pub(crate) fn set_resend_retry_interval(&self, interval: Duration) {
        self.resend_retry_ns
            .store(nanos(interval), Ordering::Relaxed);
    }

    pub(crate) fn set_resend_max_requests(&self, requests: u32) {
        self.resend_max_requests.store(requests, Ordering::Relaxed);
    }

    pub(crate) fn set_resend_budget(&self, budget: f64) {
        self.resend_budget
            .store(budget.to_bits(), Ordering::Relaxed);
    }

    fn resend_config(&self) -> ResendConfig {
        ResendConfig {
            enabled: self.resend_target.load(Ordering::Relaxed) != 0,
            delay: Duration::from_nanos(self.resend_delay_ns.load(Ordering::Relaxed)),
            retry_interval: Duration::from_nanos(self.resend_retry_ns.load(Ordering::Relaxed)),
            max_requests: self.resend_max_requests.load(Ordering::Relaxed),
            budget: f64::from_bits(self.resend_budget.load(Ordering::Relaxed)),
        }
    }

    pub(crate) fn set_block_timeout(&self, timeout: Duration) {
        self.block_timeout_ns
            .store(nanos(timeout), Ordering::Relaxed);
    }

    pub(crate) fn block_timeout(&self) -> Duration {
        Duration::from_nanos(self.block_timeout_ns.load(Ordering::Relaxed))
    }

    pub(crate) fn set_payload_size(&self, size: Option<usize>) {
        self.payload_size
            .store(size.map_or(0, |s| s as u64), Ordering::Relaxed);
    }

    fn payload_size(&self) -> Option<usize> {
        match self.payload_size.load(Ordering::Relaxed) {
            0 => None,
            size => usize::try_from(size).ok(),
        }
    }

    pub(crate) fn set_deliver_incomplete(&self, deliver: bool) {
        self.deliver_incomplete.store(deliver, Ordering::Relaxed);
    }

    fn deliver_incomplete(&self) -> bool {
        self.deliver_incomplete.load(Ordering::Relaxed)
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// Marks the queue finished however the thread exits, a panic included, so
/// a waiting consumer is never left waiting for a thread that is gone.
struct FinishOnExit(Arc<Shared>);

impl Drop for FinishOnExit {
    fn drop(&mut self) {
        self.0.queue.finish();
    }
}

/// Sizes the socket's receive buffer to the frame and warns once when the
/// kernel grants less than half of it.
struct RecvBufferSizer {
    /// Bytes the caller asked for explicitly; the engine does not override it.
    explicit: Option<usize>,
    requested: usize,
    warned: bool,
}

impl RecvBufferSizer {
    fn new(explicit: Option<usize>) -> Self {
        Self {
            explicit,
            requested: 0,
            warned: false,
        }
    }

    /// Request `max(4 MiB, 2 × frame)`, or the explicit size, if that is more
    /// than already requested.
    fn ensure(&mut self, socket: &UdpSocket, frame: Option<usize>) {
        let target = match self.explicit {
            Some(explicit) => explicit,
            None => DEFAULT_RCVBUF_BYTES.max(frame.unwrap_or(0).saturating_mul(2)),
        };
        if target <= self.requested {
            return;
        }
        self.requested = target;
        match viva_gige::set_recv_buffer(socket, target) {
            Ok(granted) => self.report(granted),
            Err(err) => {
                debug!(error = %err, requested = target, "could not size the GVSP receive buffer")
            }
        }
    }

    fn report(&mut self, granted: RecvBuffer) {
        debug!(
            requested = granted.requested,
            granted = granted.granted,
            "GVSP receive buffer"
        );
        if granted.is_short() && !self.warned {
            self.warned = true;
            warn!(
                requested = granted.requested,
                granted = granted.granted,
                "the kernel granted a GVSP receive buffer under half the size requested; a \
                 burst larger than it is dropped by the kernel before the stream sees it. {}",
                RecvBuffer::limit_hint()
            );
        }
    }
}

/// Start the receive thread for `socket`.
///
/// `packet_size` is the `GevSCPSPacketSize` in force; `explicit_rcvbuf` an
/// `SO_RCVBUF` the caller chose, which the engine leaves alone.
pub(crate) fn spawn(
    socket: UdpSocket,
    packet_size: u32,
    stats: StreamStatsAccumulator,
    shared: Arc<Shared>,
    explicit_rcvbuf: Option<usize>,
) -> std::io::Result<JoinHandle<()>> {
    // Wait for readiness, then read without blocking: not a receive timeout,
    // whose expiry Windows documents as leaving the socket indeterminate —
    // a datagram arriving as it fires can be lost (`viva_gige::wait_readable`).
    socket.set_nonblocking(true)?;
    thread::Builder::new()
        .name("viva-gvsp-rx".into())
        .spawn(move || run(socket, packet_size, stats, shared, explicit_rcvbuf))
}

fn run(
    socket: UdpSocket,
    packet_size: u32,
    stats: StreamStatsAccumulator,
    shared: Arc<Shared>,
    explicit_rcvbuf: Option<usize>,
) {
    let _finish = FinishOnExit(shared.clone());
    let mut config = AssemblyConfig::new(packet_size);
    config.block_timeout = shared.block_timeout();
    config.payload_size = shared.payload_size();
    let mut assembler = Assembler::new(config, stats.clone());
    let mut silence = SilenceWatch::new(packet_size);
    let mut rcvbuf = RecvBufferSizer::new(explicit_rcvbuf);
    rcvbuf.ensure(&socket, assembler.frame_size_hint());
    let mut buf = vec![0u8; DATAGRAM_MAX];
    let mut done: Vec<AssembledBlock> = Vec::new();
    let mut requests: Vec<ResendRequest> = Vec::new();
    let mut sender = ResendSender::default();
    let mut reset_logged = false;
    let mut chunk_layout_warned = false;

    while !shared.stop.load(Ordering::Acquire) {
        let received = match viva_gige::wait_readable(&socket, TICK) {
            Ok(true) => socket.recv_from(&mut buf),
            Ok(false) => Err(ErrorKind::WouldBlock.into()),
            Err(err) => Err(err),
        };
        // Settings are read after the wait, so one changed while the thread
        // was blocked in the read applies to the datagram that ended it.
        let wanted = AssemblyConfig {
            packet_size,
            payload_size: shared.payload_size(),
            block_timeout: shared.block_timeout(),
            resend: shared.resend_config(),
        };
        if wanted != assembler.config() {
            assembler.set_config(wanted);
        }
        let source_filter = shared.source_filter();

        match received {
            Ok((len, from)) => {
                stats.record_packet_received();
                // SR-03: only the camera's datagrams are the stream's. A port
                // another sender knows, or a second camera aimed at this one,
                // must not fill or split its frames.
                if source_filter.is_some_and(|camera| from.ip() != IpAddr::V4(camera)) {
                    stats.record_ignored_packet(IgnoredPacket::Foreign);
                    silence.record_foreign(from);
                } else {
                    silence.record_packet();
                    assembler.ingest(&buf[..len], Instant::now(), &mut done);
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            // An ICMP port-unreachable for something this socket sent comes
            // back as WSAECONNRESET on Windows (ECONNREFUSED elsewhere). It
            // says nothing about the stream, so it must not end it (SR-22).
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused
                ) =>
            {
                if !reset_logged {
                    reset_logged = true;
                    debug!(error = %err, "ignoring a connection reset on the GVSP socket");
                }
            }
            Err(err) => {
                warn!(error = %err, "GVSP socket receive failed; the stream has ended");
                shared.queue.fail("receive from the stream socket", err);
                return;
            }
        }

        let now = Instant::now();
        assembler.poll(now, &mut done);
        if let Some((camera, channel)) = shared.resend_target() {
            assembler.resend_due(now, &mut requests);
            for request in requests.drain(..) {
                sender.send(&socket, camera, channel, &request);
            }
        }
        let deliver_incomplete = shared.deliver_incomplete();
        for block in done.drain(..) {
            deliver(
                block,
                deliver_incomplete,
                &shared,
                &stats,
                &mut silence,
                &mut chunk_layout_warned,
            );
        }
        silence.tick();
        rcvbuf.ensure(&socket, assembler.frame_size_hint());
    }
}

/// Sends `PACKETRESEND_CMD`s from the stream socket.
#[derive(Debug)]
struct ResendSender {
    request_id: u16,
    reset_logged: bool,
    failure_logged: bool,
}

impl Default for ResendSender {
    fn default() -> Self {
        Self {
            request_id: 1,
            reset_logged: false,
            failure_logged: false,
        }
    }
}

impl ResendSender {
    /// Send one request. A failed send never ends the stream: the request is
    /// lost like any datagram, and the frame closes incomplete if nothing else
    /// brings the packets (SR-22).
    fn send(
        &mut self,
        socket: &UdpSocket,
        camera: SocketAddrV4,
        channel: u16,
        request: &ResendRequest,
    ) {
        let id = self.request_id;
        // Request id 0 is not used on GVCP.
        self.request_id = self.request_id.checked_add(1).unwrap_or(1);
        let bytes = viva_gige::encode_packet_resend(
            id,
            channel,
            request.block_id,
            request.first,
            request.last,
            request.extended_ids,
        );
        debug!(
            block_id = request.block_id,
            first = request.first,
            last = request.last,
            %camera,
            "asking for lost packets"
        );
        match socket.send_to(&bytes, SocketAddr::V4(camera)) {
            Ok(_) => {}
            // Windows reports an earlier ICMP port-unreachable on the next
            // call on the socket, a send included.
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused
                ) =>
            {
                if !self.reset_logged {
                    self.reset_logged = true;
                    debug!(error = %err, "ignoring a connection reset while sending a resend request");
                }
            }
            Err(err) => {
                if !self.failure_logged {
                    self.failure_logged = true;
                    warn!(
                        error = %err,
                        %camera,
                        "could not send a packet resend request; lost packets will not be \
                         recovered while this persists"
                    );
                }
            }
        }
    }
}

fn deliver(
    block: AssembledBlock,
    deliver_incomplete: bool,
    shared: &Shared,
    stats: &StreamStatsAccumulator,
    silence: &mut SilenceWatch,
    chunk_layout_warned: &mut bool,
) {
    let complete = block.status.is_complete();
    if complete {
        silence.record_frame();
    } else if !deliver_incomplete {
        stats.record_incomplete_frame();
        return;
    }
    let block_id = block.block_id;
    let (item, chunk_error) = classify_completed_block(block);
    if let Some(err) = chunk_error {
        if !*chunk_layout_warned {
            *chunk_layout_warned = true;
            warn!(
                block_id,
                error = %err,
                "a frame's chunk data does not parse; it is delivered with its whole payload \
                 and no chunks. Further failures are logged at debug"
            );
        } else {
            debug!(block_id, error = %err, "chunk data does not parse");
        }
    }
    if shared.queue.push(item).is_some() {
        stats.record_backpressure_drop();
    }
}

/// How long a stream may produce nothing before [`SilenceWatch`] speaks up.
const SILENCE_GRACE: Duration = Duration::from_secs(3);

/// What a [`SilenceWatch`] has concluded so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Silence {
    /// Nothing to say: still inside the grace period, or already spoken, or the
    /// stream is delivering frames.
    Quiet,
    /// Not one datagram has arrived.
    NoPacket,
    /// Datagrams are arriving and no frame has completed.
    NoFrame,
    /// Datagrams are arriving, but only from addresses that are not the
    /// camera's.
    OnlyForeign(SocketAddr),
}

/// Warns once when a stream produces nothing, and names what to check.
///
/// Backlog DX-09. A silent stream reports `frames=0 drops=0`, and that line is
/// identical for a firewall block, a packet size the path cannot carry, a
/// control privilege held by another application, and a camera that is simply
/// not triggering. [#70](https://github.com/VitalyVorobyev/viva-genicam/issues/70)'s
/// reporter worked the third of those out unaided;
/// [#112](https://github.com/VitalyVorobyev/viva-genicam/issues/112)'s needed a
/// custom instrumented build to find the second.
///
/// The distinction the watch adds over "no frames" is whether *datagrams* are
/// arriving, which the receiver already knows and never reported. Nothing
/// arriving is a path or privilege problem; datagrams arriving with no frame
/// completing is a packet-size disagreement, which is exactly SR-02.
struct SilenceWatch {
    since: Instant,
    packet_size: u32,
    saw_packet: bool,
    saw_frame: bool,
    /// The first datagram from an address that is not the camera's.
    saw_foreign: Option<SocketAddr>,
    warned: bool,
}

impl SilenceWatch {
    fn new(packet_size: u32) -> Self {
        Self {
            since: Instant::now(),
            packet_size,
            saw_packet: false,
            saw_frame: false,
            saw_foreign: None,
            warned: false,
        }
    }

    /// A datagram arrived — parsed or not. Malformed still means the path works.
    fn record_packet(&mut self) {
        self.saw_packet = true;
    }

    /// A datagram arrived from an address that is not the camera's.
    fn record_foreign(&mut self, from: SocketAddr) {
        self.saw_foreign.get_or_insert(from);
    }

    /// A frame completed, so there is nothing left to diagnose.
    fn record_frame(&mut self) {
        self.saw_frame = true;
    }

    /// Decide what to say at `elapsed`, marking the verdict as spoken.
    ///
    /// Split from [`SilenceWatch::tick`] so the decision is testable without a
    /// clock.
    fn assess(&mut self, elapsed: Duration) -> Silence {
        if self.warned || self.saw_frame || elapsed < SILENCE_GRACE {
            return Silence::Quiet;
        }
        self.warned = true;
        match (self.saw_packet, self.saw_foreign) {
            (true, _) => Silence::NoFrame,
            (false, Some(from)) => Silence::OnlyForeign(from),
            (false, None) => Silence::NoPacket,
        }
    }

    /// Emit the warning if one is due. Cheap enough to call on every tick.
    fn tick(&mut self) {
        let seconds = SILENCE_GRACE.as_secs();
        match self.assess(self.since.elapsed()) {
            Silence::Quiet => {}
            Silence::NoPacket => warn!(
                packet_size = self.packet_size,
                seconds,
                "no GVSP packet has arrived since the stream opened. Check, roughly in order of \
                 how often each is the cause: a host firewall blocking inbound UDP on the stream \
                 port; another application holding control privilege, so AcquisitionStart never \
                 reached the camera; a camera waiting for a trigger; or a network path that \
                 cannot carry packets this large — retry with an explicit packet size of 1500 to \
                 rule that out"
            ),
            Silence::OnlyForeign(from) => warn!(
                %from,
                seconds,
                "GVSP datagrams are arriving, but only from {from}, which is not the camera; \
                 they are ignored and counted in packets_ignored_foreign. A camera that sends \
                 its stream from another address than the one it answers control commands on \
                 needs FrameStream::set_source_filter(None)"
            ),
            Silence::NoFrame => warn!(
                packet_size = self.packet_size,
                payload_stride = payload_stride(self.packet_size, GVSP_HEADER_SIZE),
                seconds,
                "GVSP packets are arriving but no frame has completed. Reassembly places each \
                 packet at a stride derived from the negotiated packet size, so the usual cause \
                 is the two ends disagreeing about it — retry with an explicit packet size of \
                 1500"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_watch_stays_quiet_inside_the_grace_period() {
        let mut watch = SilenceWatch::new(1500);
        assert_eq!(
            watch.assess(SILENCE_GRACE - Duration::from_millis(1)),
            Silence::Quiet
        );
        // Still unspoken, so the verdict is available once the grace expires.
        assert_eq!(watch.assess(SILENCE_GRACE), Silence::NoPacket);
    }

    #[test]
    fn silence_watch_distinguishes_no_packet_from_no_frame() {
        let mut nothing = SilenceWatch::new(1500);
        assert_eq!(nothing.assess(SILENCE_GRACE), Silence::NoPacket);

        // The distinction is the whole point of DX-09: nothing arriving is a
        // path or privilege problem, while packets arriving with no frame
        // completing is a packet-size disagreement (SR-02).
        let mut packets = SilenceWatch::new(1500);
        packets.record_packet();
        assert_eq!(packets.assess(SILENCE_GRACE), Silence::NoFrame);
    }

    #[test]
    fn silence_watch_names_a_foreign_sender() {
        let from: SocketAddr = "10.0.0.7:5000".parse().unwrap();
        let mut watch = SilenceWatch::new(1500);
        watch.record_foreign(from);
        assert_eq!(watch.assess(SILENCE_GRACE), Silence::OnlyForeign(from));
    }

    #[test]
    fn a_resend_target_round_trips_through_its_packing() {
        let camera = SocketAddrV4::new(Ipv4Addr::new(169, 254, 1, 2), 3956);
        assert_eq!(unpack_target(pack_target(camera, 7)), Some((camera, 7)));
        assert_eq!(unpack_target(0), None);
    }

    #[test]
    fn silence_watch_speaks_once_and_never_after_a_frame() {
        let mut watch = SilenceWatch::new(1500);
        assert_eq!(watch.assess(SILENCE_GRACE), Silence::NoPacket);
        // A stream that stays silent must not warn on every poll.
        assert_eq!(watch.assess(SILENCE_GRACE * 10), Silence::Quiet);

        let mut healthy = SilenceWatch::new(1500);
        healthy.record_packet();
        healthy.record_frame();
        assert_eq!(healthy.assess(SILENCE_GRACE * 10), Silence::Quiet);
    }

    #[test]
    fn the_queue_drops_the_oldest_when_full() {
        let queue = DeliveryQueue::new(2);
        assert!(queue.push(1).is_none());
        assert!(queue.push(2).is_none());
        assert_eq!(queue.push(3), Some(1));
        assert!(matches!(queue.try_pop(), Popped::Item(2)));
        assert!(matches!(queue.try_pop(), Popped::Item(3)));
        assert!(matches!(queue.try_pop(), Popped::Empty));
    }

    #[test]
    fn a_failure_is_reported_once_after_the_queued_items() {
        let queue = DeliveryQueue::new(4);
        queue.push(7);
        queue.fail("boom", std::io::Error::other("boom"));
        queue.finish();
        assert!(matches!(queue.try_pop(), Popped::Item(7)));
        assert!(matches!(queue.try_pop(), Popped::Failed("boom", _)));
        assert!(matches!(queue.try_pop(), Popped::Finished));
        assert!(matches!(queue.try_pop(), Popped::Finished));
    }

    #[test]
    fn a_blocking_pop_times_out_then_sees_a_push_from_another_thread() {
        let queue = Arc::new(DeliveryQueue::new(4));
        let started = Instant::now();
        assert!(matches!(
            queue.pop_blocking(Duration::from_millis(20)),
            Popped::Empty
        ));
        assert!(started.elapsed() >= Duration::from_millis(20));

        let pusher = {
            let queue = queue.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                queue.push(42);
            })
        };
        assert!(matches!(
            queue.pop_blocking(Duration::from_secs(5)),
            Popped::Item(42)
        ));
        pusher.join().unwrap();
    }

    #[tokio::test]
    async fn an_async_pop_wakes_on_a_push_from_another_thread() {
        let queue = Arc::new(DeliveryQueue::new(4));
        let pusher = {
            let queue = queue.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                queue.push(5);
            })
        };
        let popped = tokio::time::timeout(Duration::from_secs(5), queue.pop()).await;
        assert!(matches!(popped, Ok(Popped::Item(5))));
        pusher.join().unwrap();
    }
}
