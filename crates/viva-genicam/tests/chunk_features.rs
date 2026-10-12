//! Chunk and event features read through GenApi, from bytes the fake camera's
//! own encoder lays out (ADR-0026). No socket at all: the fake writes the
//! payload, `viva-genicam` parses it, and the nodemap built from the fake's
//! XML reads the values. The fake and the client are written independently
//! (ADR-0019), so a disagreement about the layout, an offset or a byte order
//! fails here.

use bytes::Bytes;
use viva_fake_gige::chunk_payload;
use viva_fake_gige::{
    CHUNK_ID_EXPOSURE_TIME, CHUNK_ID_FRAME_ID, CHUNK_ID_IMAGE, CHUNK_ID_IMAGE_INFO,
    CHUNK_ID_TIMESTAMP, EVENT_EXPOSURE_END, FAKE_XML,
};
use viva_genicam::genapi::{GenApiError, NodeMap, NullIo, PortKind};
use viva_genicam::{Camera, Event, Frame, GenicamError, chunk_data};
use viva_pfnc::PixelFormat;

fn camera() -> Camera<NullIo> {
    let model = viva_genapi_xml::parse(FAKE_XML).expect("parse the fake's XML");
    let nodemap = NodeMap::try_from_xml(model).expect("build");
    assert!(nodemap.skipped().is_empty(), "{:?}", nodemap.skipped());
    Camera::new(NullIo, nodemap)
}

/// A frame as the stream delivers one with chunk mode on: the image and,
/// separately, its chunks.
fn frame_with_chunks() -> Frame {
    let image = vec![0x5Au8; 4 * 2];
    let info: Vec<u8> = [4u32, 2, 0, 8]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    let payload = Bytes::from(chunk_payload(&[
        (CHUNK_ID_IMAGE, &image),
        (CHUNK_ID_TIMESTAMP, &0x0000_0001_2345_6789u64.to_le_bytes()),
        (CHUNK_ID_EXPOSURE_TIME, &1250.5f64.to_le_bytes()),
        (CHUNK_ID_FRAME_ID, &77u64.to_le_bytes()),
        (CHUNK_ID_IMAGE_INFO, &info),
    ]));
    let mut frame = Frame::new(Bytes::from(image), 4, 2, PixelFormat::Mono8);
    frame.chunks = Some(chunk_data(&payload).expect("the fake's layout parses"));
    frame
}

/// The fake declares its ports as its XML says: five chunk ports, the event
/// port, and IDs above 0xFFFF that a 16-bit ID could not carry.
#[test]
fn the_fake_declares_its_chunk_and_event_ports() {
    let camera = camera();
    let ports = camera.nodemap().ports();
    for id in [
        CHUNK_ID_IMAGE,
        CHUNK_ID_TIMESTAMP,
        CHUNK_ID_EXPOSURE_TIME,
        CHUNK_ID_FRAME_ID,
        CHUNK_ID_IMAGE_INFO,
    ] {
        assert!(
            ports.iter().any(|(_, kind)| *kind
                == PortKind::Chunk {
                    chunk_id: id.into()
                }),
            "no port for chunk {id:#x}: {ports:?}"
        );
    }
    assert!(ports.iter().any(|(_, kind)| {
        *kind
            == PortKind::Event {
                event_id: EVENT_EXPOSURE_END.into(),
            }
    }));
}

/// Every chunk feature reads its own chunk, at the offset and in the byte
/// order the XML declares — including `ChunkImageLength` at address `-4`.
#[test]
fn chunk_features_read_through_genapi() {
    let mut camera = camera();
    let frame = frame_with_chunks();
    let values: Vec<(String, String)> = camera
        .chunk_features(&frame)
        .into_iter()
        .map(|(name, value)| (name, value.expect("readable")))
        .collect();
    let expect = |name: &str, value: &str| (name.to_string(), value.to_string());
    assert_eq!(
        values,
        vec![
            expect("ChunkExposureTime", "1250.5"),
            expect("ChunkFrameID", "77"),
            expect("ChunkHeight", "2"),
            expect("ChunkImageLength", "8"),
            expect("ChunkTimestamp", &0x0000_0001_2345_6789u64.to_string()),
            expect("ChunkWidth", "4"),
        ]
    );
    // Detached again afterwards: nothing outlives the frame.
    assert!(matches!(
        camera.get("ChunkFrameID"),
        Err(GenicamError::GenApi(GenApiError::NotAttached { .. }))
    ));
}

/// The scoped form, and a frame without chunks.
#[test]
fn with_chunks_reads_one_feature_and_a_plain_frame_has_none() {
    let mut camera = camera();
    let frame = frame_with_chunks();
    let id = camera.with_chunks(&frame, |cam| cam.get("ChunkFrameID"));
    assert_eq!(id.expect("frame id"), "77");

    let plain = Frame::new(Bytes::from_static(&[0; 8]), 4, 2, PixelFormat::Mono8);
    assert!(camera.chunk_features(&plain).is_empty());
    assert!(matches!(
        camera.with_chunks(&plain, |cam| cam.get("ChunkFrameID")),
        Err(GenicamError::GenApi(GenApiError::NotAttached { .. }))
    ));
}

/// An `EVENTDATA` ExposureEnd as the GigE Vision field table lays it out:
/// reserved | event id | stream channel | block id | timestamp | data (the
/// frame id, little-endian). The event features read it through the port
/// whose `<EventID>` matches.
#[test]
fn event_features_read_the_event_through_its_port() {
    let mut camera = camera();
    let mut raw = vec![0x00, 0x00];
    raw.extend_from_slice(&EVENT_EXPOSURE_END.to_be_bytes());
    raw.extend_from_slice(&[0x00, 0x00, 0x00, 0x2A]); // channel 0, block 42
    raw.extend_from_slice(&0x0000_0003_0000_0004u64.to_be_bytes());
    raw.extend_from_slice(&42u64.to_le_bytes());
    let raw = Bytes::from(raw);
    let event = Event {
        id: EVENT_EXPOSURE_END,
        stream_channel: 0,
        block_id: 42,
        ts_dev: 0x0000_0003_0000_0004,
        ts_host: None,
        data: raw.slice(16..),
        raw,
    };
    let values: Vec<(String, String)> = camera
        .event_features(&event)
        .into_iter()
        .map(|(name, value)| (name, value.expect("readable")))
        .collect();
    assert_eq!(
        values,
        vec![
            (
                "EventExposureEnd".to_string(),
                EVENT_EXPOSURE_END.to_string()
            ),
            ("EventExposureEndFrameID".to_string(), "42".to_string()),
            (
                "EventExposureEndTimestamp".to_string(),
                0x0000_0003_0000_0004u64.to_string()
            ),
        ]
    );
    let ts = camera.with_event(&event, |cam| cam.get("EventExposureEndTimestamp"));
    assert_eq!(ts.expect("timestamp"), 0x0000_0003_0000_0004u64.to_string());
}
