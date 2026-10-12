//! The USB3 Vision service against in-process fake cameras, over a real
//! Zenoh session: what a client sees.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use viva_fake_u3v::FakeU3vCamera;
use viva_genicam::open_u3v_device;
use viva_service::{run_discovery, serve_device};
use viva_zenoh_api::frame_header::FrameHeader;
use viva_zenoh_api::{
    AcquisitionCommand, AcquisitionControlRequest, DeviceStatus, DeviceXmlResponse, NodeOpResponse,
    keys,
};

use crate::device::U3vDeviceHandle;
use crate::discovery::{FakeDiscovery, FakeSpec};

const MONO8: u32 = 0x0108_0001;

async fn session() -> Arc<zenoh::Session> {
    Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap())
}

/// Ask `key` until something answers, up to `within`.
async fn ask(
    session: &zenoh::Session,
    key: &str,
    payload: Vec<u8>,
    within: Duration,
) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let replies = session
            .get(key)
            .payload(payload.clone())
            .timeout(Duration::from_millis(500))
            .await
            .unwrap();
        if let Ok(reply) = replies.recv_async().await
            && let Ok(sample) = reply.result()
        {
            return Some(sample.payload().to_bytes().to_vec());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

/// Whether the camera published as `id` answers an XML query.
async fn serves(session: &zenoh::Session, id: &str, within: Duration) -> bool {
    ask(session, &keys::xml(id), Vec::new(), within)
        .await
        .and_then(|bytes| serde_json::from_slice::<DeviceXmlResponse>(&bytes).ok())
        .is_some()
}

fn fake(id: &str) -> FakeSpec {
    FakeSpec {
        id: id.to_string(),
        width: 64,
        height: 48,
        pixel_format: MONO8,
    }
}

/// The service streams what the camera sends, packed formats included.
/// It sized frames as width × height × one byte for a format without a
/// whole-byte pixel, and opened the stream without configuring the SIRM
/// (backlog SVC-03); it now opens it as the library does.
#[tokio::test(flavor = "multi_thread")]
async fn a_packed_format_streams_at_its_payload_size() {
    let session = session().await;
    let device = FakeU3vCamera::new(64, 48).open_device().unwrap();
    let (mut camera, xml) = open_u3v_device(device).unwrap();
    camera.set("PixelFormat", "Mono12Packed").unwrap();
    let device_id = format!("u3v-acquisition-test-{}", std::process::id());
    let announce = viva_zenoh_api::DeviceAnnounce::new(device_id.clone(), "Fake", "Fake", "");
    let handle = Arc::new(U3vDeviceHandle::new(camera, xml, announce));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let tasks = serve_device(session.clone(), handle, shutdown_rx).await;
    let images = session
        .declare_subscriber(keys::image(&device_id))
        .await
        .unwrap();

    let start =
        serde_json::to_vec(&AcquisitionControlRequest::new(AcquisitionCommand::Start)).unwrap();
    let reply = ask(
        &session,
        &keys::acquisition_control(&device_id),
        start,
        Duration::from_secs(10),
    )
    .await
    .expect("no answer to Start");
    let response: NodeOpResponse = serde_json::from_slice(&reply).unwrap();
    assert!(response.ok, "{response:?}");

    let sample = tokio::time::timeout(Duration::from_secs(5), images.recv_async())
        .await
        .expect("no frame within 5 s")
        .unwrap();
    let bytes = sample.payload().to_bytes();
    let (header, pixels) = FrameHeader::decode(&bytes).unwrap();
    assert_eq!((header.width, header.height), (64, 48));
    assert_eq!(pixels.len(), 64 * 48 * 3 / 2, "64 × 48 × 1.5 bytes");

    let _ = shutdown_tx.send(true);
    for task in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
    }
}

/// SVC-04: the USB3 Vision service serves every camera it finds, finds one
/// plugged in later, and publishes a camera disconnected once it is gone —
/// as the GigE Vision service always has. It used to open `devices[0]` once
/// and never look again.
#[tokio::test(flavor = "multi_thread")]
async fn cameras_come_and_go() {
    let session = session().await;
    let tag = std::process::id();
    let (first, second, later) = (
        format!("svc04-a-{tag}"),
        format!("svc04-b-{tag}"),
        format!("svc04-c-{tag}"),
    );
    let discovery = FakeDiscovery::default();
    discovery.plug(fake(&first));
    discovery.plug(fake(&second));
    let status = session
        .declare_subscriber(keys::status(&first))
        .await
        .unwrap();

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let service = tokio::spawn(run_discovery(
        session.clone(),
        discovery.clone(),
        Duration::from_millis(200),
        shutdown_rx,
    ));

    // Both cameras found on the bus are served, not only the first.
    assert!(
        serves(&session, &first, Duration::from_secs(10)).await,
        "{first} not served"
    );
    assert!(
        serves(&session, &second, Duration::from_secs(10)).await,
        "{second} not served"
    );

    // One plugged in later is found on a rescan.
    discovery.plug(fake(&later));
    assert!(
        serves(&session, &later, Duration::from_secs(10)).await,
        "{later} not found on a rescan"
    );

    // One unplugged is published disconnected, and no longer served.
    discovery.unplug(&first);
    let disconnected = async {
        loop {
            let sample = status.recv_async().await.unwrap();
            let status: DeviceStatus =
                serde_json::from_slice(&sample.payload().to_bytes()).unwrap();
            if !status.connected {
                return status;
            }
        }
    };
    let status = tokio::time::timeout(Duration::from_secs(5), disconnected)
        .await
        .expect("no disconnect published for an unplugged camera");
    assert_eq!(status.error.as_deref(), Some("device lost"));
    assert!(
        !serves(&session, &first, Duration::from_millis(1500)).await,
        "an unplugged camera is still served"
    );
    assert!(
        serves(&session, &second, Duration::from_secs(2)).await,
        "{second} stopped"
    );

    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(10), service).await;
}
