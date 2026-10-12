//! The acquisition engine (GenTL 1.6 §5): announcing, queueing, delivering
//! through `EVENT_NEW_BUFFER`, flushing, and what a delivered buffer says.

mod common;

use std::ffi::c_void;
use std::ptr::null_mut;

use common::*;
use viva_fake_gige::{PacketScript, ScriptStep};
use viva_gentl::abi::*;
use viva_gentl::*;

const FRAME_BYTES: usize = (WIDTH * HEIGHT * 2) as usize;

/// Register `EVENT_NEW_BUFFER` on `stream`.
#[track_caller]
fn new_buffer_event(stream: Handle) -> Handle {
    let mut event = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointer.
    check(unsafe { GCRegisterEvent(stream, EVENT_NEW_BUFFER, &mut event) });
    event
}

/// Check a delivered buffer against the fake's Mono16 ramp, and return its
/// block id.
#[track_caller]
fn check_frame(stream: Handle, buffer: Handle, memory: &[u8]) -> u64 {
    let frame_id = query_u64(buffer_info(stream, buffer, BUFFER_INFO_FRAMEID));
    assert!(!query_bool(buffer_info(
        stream,
        buffer,
        BUFFER_INFO_IS_INCOMPLETE
    )));
    assert!(query_bool(buffer_info(
        stream,
        buffer,
        BUFFER_INFO_NEW_DATA
    )));
    assert!(!query_bool(buffer_info(
        stream,
        buffer,
        BUFFER_INFO_IS_QUEUED
    )));
    assert_eq!(
        query_size(buffer_info(stream, buffer, BUFFER_INFO_WIDTH)),
        WIDTH as usize
    );
    assert_eq!(
        query_size(buffer_info(stream, buffer, BUFFER_INFO_HEIGHT)),
        HEIGHT as usize
    );
    assert_eq!(
        query_u64(buffer_info(stream, buffer, BUFFER_INFO_PIXELFORMAT)),
        u64::from(MONO16)
    );
    assert_eq!(
        query_u64(buffer_info(
            stream,
            buffer,
            BUFFER_INFO_PIXELFORMAT_NAMESPACE
        )),
        PIXELFORMAT_NAMESPACE_PFNC_32BIT
    );
    assert_eq!(
        query_size(buffer_info(stream, buffer, BUFFER_INFO_PAYLOADTYPE)),
        PAYLOAD_TYPE_IMAGE
    );
    assert!(query_bool(buffer_info(
        stream,
        buffer,
        BUFFER_INFO_IMAGEPRESENT
    )));
    assert_eq!(
        query_size(buffer_info(stream, buffer, BUFFER_INFO_SIZE_FILLED)),
        FRAME_BYTES
    );
    assert_eq!(
        query_size(buffer_info(stream, buffer, BUFFER_INFO_DATA_SIZE)),
        FRAME_BYTES
    );
    assert!(query_u64(buffer_info(stream, buffer, BUFFER_INFO_TIMESTAMP)) > 0);
    for (index, &byte) in memory[..FRAME_BYTES].iter().enumerate() {
        assert_eq!(
            byte,
            ramp(index, frame_id),
            "byte {index} of block {frame_id}"
        );
    }
    frame_id
}

/// Consumer memory announced with `DSAnnounceBuffer`: the producer fills
/// each buffer it took from the input pool with one frame, delivers it
/// through `EVENT_NEW_BUFFER` with the private pointer it was announced with,
/// and the pixels are the camera's.
#[test]
fn frames_arrive_in_consumer_buffers() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let remote = remote_port(opened.device);
    let stream = open_stream(opened.device);
    assert!(
        !query_bool(|t, b, s| unsafe {
            DSGetInfo(stream, STREAM_INFO_DEFINES_PAYLOADSIZE, t, b, s)
        }),
        "PayloadSize comes from the remote device (ADR-0031)"
    );
    let mut memory: Vec<Vec<u8>> = (0..4).map(|_| vec![0u8; FRAME_BYTES]).collect();
    let mut buffers = Vec::new();
    for (index, block) in memory.iter_mut().enumerate() {
        let mut buffer = GENTL_INVALID_HANDLE;
        // SAFETY: `block` outlives the buffer: revoked before it is dropped.
        check(unsafe {
            DSAnnounceBuffer(
                stream,
                block.as_mut_ptr().cast(),
                block.len(),
                (index + 1) as *mut c_void,
                &mut buffer,
            )
        });
        // SAFETY: valid handles.
        check(unsafe { DSQueueBuffer(stream, buffer) });
        buffers.push(buffer);
    }
    // The same memory twice is refused (§6.3.5.2).
    let mut twice = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    let rc = unsafe {
        DSAnnounceBuffer(
            stream,
            memory[0].as_mut_ptr().cast(),
            FRAME_BYTES,
            null_mut(),
            &mut twice,
        )
    };
    assert_eq!(rc, GC_ERR_RESOURCE_IN_USE);
    let event = new_buffer_event(stream);
    // SAFETY: valid handle.
    check(unsafe { DSStartAcquisition(stream, ACQ_START_FLAGS_DEFAULT, GENTL_INFINITE) });
    assert!(query_bool(|t, b, s| unsafe {
        DSGetInfo(stream, STREAM_INFO_IS_GRABBING, t, b, s)
    }));
    acquisition(remote, true);

    let mut last = 0;
    for _ in 0..8 {
        let delivered = next_buffer(event, 3000)
            .unwrap_or_else(|rc| panic!("EventGetData: {rc} ({:?})", last_error()));
        let index = buffers
            .iter()
            .position(|&b| b == delivered.buffer_handle)
            .expect("one of ours");
        assert_eq!(
            delivered.user_pointer as usize,
            index + 1,
            "the private pointer"
        );
        assert_eq!(
            query_ptr(buffer_info(
                stream,
                delivered.buffer_handle,
                BUFFER_INFO_BASE
            )),
            memory[index].as_ptr() as usize
        );
        let frame_id = check_frame(stream, delivered.buffer_handle, &memory[index]);
        assert!(frame_id > last, "oldest first: {frame_id} after {last}");
        last = frame_id;
        // SAFETY: valid handles.
        check(unsafe { DSQueueBuffer(stream, delivered.buffer_handle) });
    }
    assert!(
        query_u64(|t, b, s| unsafe { DSGetInfo(stream, STREAM_INFO_NUM_DELIVERED, t, b, s) }) >= 8
    );

    acquisition(remote, false);
    // SAFETY: valid handles.
    unsafe {
        check(DSStopAcquisition(stream, ACQ_STOP_FLAGS_DEFAULT));
        assert_eq!(
            DSStopAcquisition(stream, ACQ_STOP_FLAGS_DEFAULT),
            GC_ERR_RESOURCE_IN_USE,
            "already stopped"
        );
        check(DSFlushQueue(stream, ACQ_QUEUE_ALL_DISCARD));
        for buffer in buffers {
            let mut base = null_mut();
            let mut private = null_mut();
            check(DSRevokeBuffer(stream, buffer, &mut base, &mut private));
            assert!(!base.is_null(), "consumer memory comes back");
        }
        check(GCUnregisterEvent(stream, EVENT_NEW_BUFFER));
        check(DSClose(stream));
    }
}

/// `DSAllocAndAnnounceBuffer`: the producer's own memory, filled the same
/// way, and handed back as null on revocation (§6.3.5.10).
#[test]
fn frames_arrive_in_producer_buffers() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let remote = remote_port(opened.device);
    let stream = open_stream(opened.device);
    let mut buffers = Vec::new();
    for _ in 0..3 {
        let mut buffer = GENTL_INVALID_HANDLE;
        // SAFETY: valid pointers.
        unsafe {
            check(DSAllocAndAnnounceBuffer(
                stream,
                FRAME_BYTES,
                null_mut(),
                &mut buffer,
            ));
            check(DSQueueBuffer(stream, buffer));
        }
        buffers.push(buffer);
    }
    // §3.6: the two ways must not be mixed on one stream.
    let mut memory = vec![0u8; FRAME_BYTES];
    let mut mixed = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    let rc = unsafe {
        DSAnnounceBuffer(
            stream,
            memory.as_mut_ptr().cast(),
            memory.len(),
            null_mut(),
            &mut mixed,
        )
    };
    assert_ne!(rc, GC_ERR_SUCCESS, "mixing announcement kinds is refused");
    let event = new_buffer_event(stream);
    // SAFETY: valid handle.
    check(unsafe { DSStartAcquisition(stream, ACQ_START_FLAGS_DEFAULT, 5) });
    acquisition(remote, true);
    for _ in 0..5 {
        let delivered = next_buffer(event, 3000)
            .unwrap_or_else(|rc| panic!("EventGetData: {rc} ({:?})", last_error()));
        let base = query_ptr(buffer_info(
            stream,
            delivered.buffer_handle,
            BUFFER_INFO_BASE,
        ));
        let size = query_size(buffer_info(
            stream,
            delivered.buffer_handle,
            BUFFER_INFO_SIZE,
        ));
        assert_eq!(size, FRAME_BYTES);
        // SAFETY: the producer's buffer is ours to read while delivered.
        let memory = unsafe { std::slice::from_raw_parts(base as *const u8, size) };
        check_frame(stream, delivered.buffer_handle, memory);
        // SAFETY: valid handles.
        check(unsafe { DSQueueBuffer(stream, delivered.buffer_handle) });
    }
    // §6.3.5.11: the engine stops by itself after `iNumToAcquire` buffers.
    assert_eq!(next_buffer(event, 500).err(), Some(GC_ERR_TIMEOUT));
    acquisition(remote, false);
    // SAFETY: valid handles.
    unsafe {
        check(DSStopAcquisition(stream, ACQ_STOP_FLAGS_DEFAULT));
        check(DSFlushQueue(stream, ACQ_QUEUE_ALL_DISCARD));
        let mut base = std::ptr::dangling_mut::<c_void>();
        check(DSRevokeBuffer(stream, buffers[0], &mut base, null_mut()));
        assert!(base.is_null(), "the producer's memory is not handed out");
    }
}

/// `DSFlushQueue`'s five operations (§6.4.4.1) move buffers between the
/// input pool, the output queue and neither, without an acquisition.
#[test]
fn flushing_moves_buffers_between_queues() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let stream = open_stream(opened.device);
    let count = |cmd| query_size(move |t, b, s| unsafe { DSGetInfo(stream, cmd, t, b, s) });
    let mut buffers = Vec::new();
    for _ in 0..3 {
        let mut buffer = GENTL_INVALID_HANDLE;
        // SAFETY: valid pointers.
        check(unsafe { DSAllocAndAnnounceBuffer(stream, 64, null_mut(), &mut buffer) });
        buffers.push(buffer);
    }
    let event = new_buffer_event(stream);
    // SAFETY: valid handles throughout.
    unsafe {
        assert_eq!(count(STREAM_INFO_NUM_ANNOUNCED), 3);
        check(DSFlushQueue(stream, ACQ_QUEUE_UNQUEUED_TO_INPUT));
        assert_eq!(count(STREAM_INFO_NUM_QUEUED), 3);
        assert_eq!(
            DSQueueBuffer(stream, buffers[0]),
            GC_ERR_RESOURCE_IN_USE,
            "queued"
        );
        assert_eq!(
            DSRevokeBuffer(stream, buffers[0], null_mut(), null_mut()),
            GC_ERR_BUSY,
            "a queued buffer cannot be revoked"
        );

        // Input pool to output queue: delivered, but with no new data.
        check(DSFlushQueue(stream, ACQ_QUEUE_INPUT_TO_OUTPUT));
        assert_eq!(count(STREAM_INFO_NUM_QUEUED), 0);
        assert_eq!(count(STREAM_INFO_NUM_AWAIT_DELIVERY), 3);
        let delivered = next_buffer(event, 0).expect("flushed buffer");
        assert!(!query_bool(buffer_info(
            stream,
            delivered.buffer_handle,
            BUFFER_INFO_NEW_DATA
        )));
        assert_eq!(count(STREAM_INFO_NUM_AWAIT_DELIVERY), 2);

        // Output queue discarded: neither queued nor deliverable.
        check(DSFlushQueue(stream, ACQ_QUEUE_OUTPUT_DISCARD));
        assert_eq!(count(STREAM_INFO_NUM_AWAIT_DELIVERY), 0);
        assert_eq!(next_buffer(event, 0).err(), Some(GC_ERR_TIMEOUT));

        // Everything back to the input pool.
        check(DSQueueBuffer(stream, delivered.buffer_handle));
        check(DSFlushQueue(stream, ACQ_QUEUE_ALL_TO_INPUT));
        assert_eq!(count(STREAM_INFO_NUM_QUEUED), 3);

        // And everything discarded: all revocable.
        check(DSFlushQueue(stream, ACQ_QUEUE_ALL_DISCARD));
        assert_eq!(count(STREAM_INFO_NUM_QUEUED), 0);
        for buffer in &buffers {
            assert!(!query_bool(buffer_info(
                stream,
                *buffer,
                BUFFER_INFO_IS_QUEUED
            )));
            check(DSRevokeBuffer(stream, *buffer, null_mut(), null_mut()));
        }
        assert_eq!(count(STREAM_INFO_NUM_ANNOUNCED), 0);
        assert_eq!(
            DSFlushQueue(stream, 77),
            GC_ERR_NOT_IMPLEMENTED,
            "unknown operation"
        );
    }
}

/// `EventKill` ends one wait with `GC_ERR_ABORT`, even when called before
/// the wait (§4.2.3.2), and `EventFlush` empties the output queue.
#[test]
fn kill_and_flush_the_new_buffer_event() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let stream = open_stream(opened.device);
    let mut buffer = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers throughout.
    unsafe {
        check(DSAllocAndAnnounceBuffer(
            stream,
            64,
            null_mut(),
            &mut buffer,
        ));
        check(DSQueueBuffer(stream, buffer));
        let event = new_buffer_event(stream);
        let mut again = GENTL_INVALID_HANDLE;
        assert_eq!(
            GCRegisterEvent(stream, EVENT_NEW_BUFFER, &mut again),
            GC_ERR_RESOURCE_IN_USE,
            "one registration per module and type"
        );
        check(DSFlushQueue(stream, ACQ_QUEUE_INPUT_TO_OUTPUT));
        check(EventKill(event));
        assert_eq!(
            next_buffer(event, 0).err(),
            Some(GC_ERR_ABORT),
            "kill wins over data"
        );
        let in_queue = query_size(|t, b, s| EventGetInfo(event, EVENT_NUM_IN_QUEUE, t, b, s));
        assert_eq!(in_queue, 1, "the abort removed nothing");
        check(EventFlush(event));
        assert_eq!(next_buffer(event, 0).err(), Some(GC_ERR_TIMEOUT));
        assert!(!query_bool(buffer_info(
            stream,
            buffer,
            BUFFER_INFO_IS_QUEUED
        )));
        let fired = query_u64(|t, b, s| EventGetInfo(event, EVENT_NUM_FIRED, t, b, s));
        assert_eq!(fired, 1);
        let size_max = query_size(|t, b, s| EventGetInfo(event, EVENT_SIZE_MAX, t, b, s));
        assert_eq!(size_max, std::mem::size_of::<EventNewBufferData>());

        // A wait in another thread ends with GC_ERR_ABORT on EventKill.
        let waiter = event as usize;
        let handle =
            std::thread::spawn(move || next_buffer(waiter as Handle, GENTL_INFINITE).err());
        std::thread::sleep(std::time::Duration::from_millis(100));
        check(EventKill(event));
        assert_eq!(handle.join().expect("waiter"), Some(GC_ERR_ABORT));

        check(GCUnregisterEvent(stream, EVENT_NEW_BUFFER));
        assert_eq!(EventKill(event), GC_ERR_INVALID_HANDLE, "unregistered");
    }
}

/// A block that lost a packet the camera cannot send again is delivered
/// with `BUFFER_INFO_IS_INCOMPLETE`, its lost bytes zero (ADR-0031).
#[test]
fn a_frame_with_a_lost_packet_is_delivered_incomplete() {
    let _serial = serial();
    let _fake = Fake::start_with(|builder| {
        builder
            .packet_resend(false)
            .packet_script(PacketScript::new().step(ScriptStep::Drop {
                block: 2,
                packet: 2,
            }))
    });
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let remote = remote_port(opened.device);
    let stream = open_stream(opened.device);
    for _ in 0..4 {
        let mut buffer = GENTL_INVALID_HANDLE;
        // SAFETY: valid pointers.
        unsafe {
            check(DSAllocAndAnnounceBuffer(
                stream,
                FRAME_BYTES,
                null_mut(),
                &mut buffer,
            ));
            check(DSQueueBuffer(stream, buffer));
        }
    }
    let event = new_buffer_event(stream);
    // SAFETY: valid handle.
    check(unsafe { DSStartAcquisition(stream, ACQ_START_FLAGS_DEFAULT, GENTL_INFINITE) });
    acquisition(remote, true);
    let mut seen_incomplete = false;
    for _ in 0..4 {
        let delivered = next_buffer(event, 3000).expect("a buffer");
        let frame_id = query_u64(buffer_info(
            stream,
            delivered.buffer_handle,
            BUFFER_INFO_FRAMEID,
        ));
        let incomplete = query_bool(buffer_info(
            stream,
            delivered.buffer_handle,
            BUFFER_INFO_IS_INCOMPLETE,
        ));
        assert_eq!(incomplete, frame_id == 2, "block {frame_id}");
        seen_incomplete |= incomplete;
        // SAFETY: valid handles.
        check(unsafe { DSQueueBuffer(stream, delivered.buffer_handle) });
    }
    assert!(seen_incomplete, "block 2 arrived incomplete");
    acquisition(remote, false);
    // SAFETY: valid handle.
    check(unsafe { DSStopAcquisition(stream, ACQ_STOP_FLAGS_DEFAULT) });
}

/// With chunk mode on, the buffer holds the whole payload and
/// `DSGetBufferChunkData` locates each chunk (§5.4).
#[test]
fn chunks_are_located_in_the_buffer() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let remote = remote_port(opened.device);
    let set =
        |register, value: u32| write_port(remote, register, &value.to_be_bytes()).expect("write");
    set(
        viva_fake_gige::REG_CHUNK_SELECTOR,
        viva_fake_gige::CHUNK_SELECTOR_FRAME_ID,
    );
    set(viva_fake_gige::REG_CHUNK_ENABLE, 1);
    set(viva_fake_gige::REG_CHUNK_MODE_ACTIVE, 1);
    let stream = open_stream(opened.device);
    for _ in 0..2 {
        let mut buffer = GENTL_INVALID_HANDLE;
        // SAFETY: valid pointers.
        unsafe {
            check(DSAllocAndAnnounceBuffer(
                stream,
                FRAME_BYTES + 64,
                null_mut(),
                &mut buffer,
            ));
            check(DSQueueBuffer(stream, buffer));
        }
    }
    let event = new_buffer_event(stream);
    // SAFETY: valid handle.
    check(unsafe { DSStartAcquisition(stream, ACQ_START_FLAGS_DEFAULT, 1) });
    acquisition(remote, true);
    let delivered = next_buffer(event, 3000).expect("a buffer");
    acquisition(remote, false);
    let buffer = delivered.buffer_handle;
    assert!(query_bool(buffer_info(
        stream,
        buffer,
        BUFFER_INFO_CONTAINS_CHUNKDATA
    )));
    let frame_id = query_u64(buffer_info(stream, buffer, BUFFER_INFO_FRAMEID));
    let base = query_ptr(buffer_info(stream, buffer, BUFFER_INFO_BASE));
    let filled = query_size(buffer_info(stream, buffer, BUFFER_INFO_SIZE_FILLED));
    assert_eq!(
        filled,
        FRAME_BYTES + 8 + 8 + 8,
        "image, its tag, the frame id and its tag"
    );
    let mut count = 0usize;
    // SAFETY: valid pointers.
    check(unsafe { DSGetBufferChunkData(stream, buffer, null_mut(), &mut count) });
    assert_eq!(count, 2);
    let mut chunks = [SingleChunkData {
        chunk_id: 0,
        chunk_offset: 0,
        chunk_length: 0,
    }; 2];
    // SAFETY: `chunks` holds `count` entries.
    check(unsafe { DSGetBufferChunkData(stream, buffer, chunks.as_mut_ptr(), &mut count) });
    let (image, frame) = (chunks[0], chunks[1]);
    assert_eq!(
        ({ image.chunk_id }, { image.chunk_offset }, {
            image.chunk_length
        }),
        (u64::from(viva_fake_gige::CHUNK_ID_IMAGE), 0, FRAME_BYTES)
    );
    assert_eq!(
        { frame.chunk_id },
        u64::from(viva_fake_gige::CHUNK_ID_FRAME_ID)
    );
    assert_eq!({ frame.chunk_length }, 8);
    // SAFETY: the delivered buffer is ours to read.
    let memory = unsafe { std::slice::from_raw_parts(base as *const u8, filled) };
    let offset = frame.chunk_offset as usize;
    let value = u64::from_le_bytes(memory[offset..offset + 8].try_into().unwrap());
    assert_eq!(value, frame_id, "the chunk holds the block id");
    // SAFETY: valid handle.
    check(unsafe { DSStopAcquisition(stream, ACQ_STOP_FLAGS_DEFAULT) });
}

/// A frame larger than its buffer is truncated, and says so (§5.2.1).
#[test]
fn a_frame_larger_than_its_buffer_is_truncated() {
    let _serial = serial();
    let _fake = Fake::start();
    let _lib = Library::init();
    let opened = open_fake(DEVICE_ACCESS_CONTROL);
    let remote = remote_port(opened.device);
    let stream = open_stream(opened.device);
    let mut buffer = GENTL_INVALID_HANDLE;
    // SAFETY: valid pointers.
    unsafe {
        check(DSAllocAndAnnounceBuffer(
            stream,
            1000,
            null_mut(),
            &mut buffer,
        ));
        check(DSQueueBuffer(stream, buffer));
    }
    let event = new_buffer_event(stream);
    // SAFETY: valid handle.
    check(unsafe { DSStartAcquisition(stream, ACQ_START_FLAGS_DEFAULT, 1) });
    acquisition(remote, true);
    let delivered = next_buffer(event, 3000).expect("a buffer");
    acquisition(remote, false);
    let buffer = delivered.buffer_handle;
    assert!(query_bool(buffer_info(
        stream,
        buffer,
        BUFFER_INFO_IS_INCOMPLETE
    )));
    assert!(query_bool(buffer_info(
        stream,
        buffer,
        BUFFER_INFO_DATA_LARGER_THAN_BUFFER
    )));
    assert_eq!(
        query_size(buffer_info(stream, buffer, BUFFER_INFO_SIZE_FILLED)),
        1000
    );
    assert_eq!(
        query_size(buffer_info(stream, buffer, BUFFER_INFO_DATA_SIZE)),
        FRAME_BYTES
    );
    // SAFETY: valid handle.
    check(unsafe { DSStopAcquisition(stream, ACQ_STOP_FLAGS_DEFAULT) });
}
