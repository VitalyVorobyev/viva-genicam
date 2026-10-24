# ADR-0033: The GVCP message channel acknowledges on arrival

**Status:** Accepted
**Date:** 2026-10-24

## Context

A GigE Vision device sends its events to the controller as `EVENT_CMD` and
`EVENTDATA_CMD` commands on the message channel, and may ask for each to be
acknowledged. Until this decision `viva_gige::EventSocket` read the socket
only inside `recv`, and sent the acknowledgement there: an application that
was busy, or read its events after its frames, left every message
unacknowledged for as long as it did not read.

What an Allied Vision Manta G-125B (firmware 00.01.44.28514, `GevMCTT`
100 ms, `GevMCRC` `0xFFFFFFFF`) did with that, seen in `trace` logs of the
message socket (backlog TC-42, `-vv` evidence in the PR):

- it keeps one message outstanding and sends it again, same request id and
  bytes, every `GevMCTT` until it is acknowledged: a reader a second late
  received one `ExposureEnd` twelve times;
- events that occur meanwhile wait on the camera and leave packed into the
  next `EVENT_CMD` once the outstanding one is acknowledged: 5, 11 and 32
  events in one message;
- left unacknowledged for ten seconds at 60 events a second, it delivered 44
  and an `EventOverflow` (`0xFFFE`), and lost the rest.

#216 recognised the copies and delivered each event once, but the
retransmissions, the delay and the camera's own overflow remained: a slow
reader lost events on the camera, where nothing on the host could count
them.

## Decision

**Binding an `EventSocket` starts a receive thread that acknowledges each
message as it arrives and queues its events for the application.**

- **Thread model.** One `std::thread` per socket, as the GVSP receive engine
  ([ADR-0025](adr0025-gvsp-receive-engine.md)) and the USB3 Vision event
  reader have: it waits for the socket to be readable (`wait_readable`, 20 ms
  ticks, non-blocking reads, for the reason ADR-0025's amendment gives) and
  drains it. It needs no async runtime, so an application blocked on a
  current-thread runtime still acknowledges. Dropping the socket stops the
  thread within one tick.
- **Acknowledgement** goes out from the thread before the events are queued,
  to the datagram's source (the Manta's events left from port 10020, not
  3956).
- **Retransmissions** — the same request id and the same bytes as the
  source's message before — are acknowledged again and not queued.
- **The queue** holds `EVENT_QUEUE_CAPACITY` (1024) events. Past that the
  oldest are dropped: the first time logged as a warning, afterwards at
  debug, always counted. Dropping the oldest matches the frame queue; an
  application that wants every event reads them, and one that falls behind
  sees the newest.
- **Counters.** `EventSocket::stats` (`EventStream::stats` in the facade)
  returns `EventStats`: messages, acknowledged, retransmissions, events
  queued, dropped, malformed.
- `recv` stays async and cancel-safe; concurrent callers share one queue.

**The fake camera's message channel** (`viva-fake-gige`, `events` module) is
built from what the Manta did, with the fake's own numbers: every message
asks for an acknowledgement; one is outstanding at a time and is sent again
every `GevMCTT` (40 ms by default) until acknowledged or `GevMCRC` (25)
retries are spent; events wait in a 16-event queue and leave up to eight to
an `EVENT_CMD`; an event that finds the queue full is lost and counted.
`FakeCamera::message_channel` reports what it sent, so a test asserts the
device's transmissions, not the client's own account.

## Consequences

### Positive

- A late or busy reader costs the camera nothing: on the Manta, a reader a
  second late now sees each event once and the camera sent each once.
- Event loss moves from the camera, where it was invisible, to a bounded
  host queue that counts it.
- Events from one camera keep arriving in order across a slow reader; they
  used to arrive in bursts behind the outstanding message.

### Negative

- A thread per event stream, and up to 1024 events held for an application
  that never reads them.
- An acknowledgement no longer says the application has the event, only that
  the host does. No GigE Vision behaviour we know of depends on the
  difference.
- The facade's event stream changes shape internally; `EventSocket` gains
  `stats` and loses its tokio socket, and its `local_addr` no longer fails.
