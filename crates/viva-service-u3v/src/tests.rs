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
    let metadata = header.metadata.as_ref().expect("USB same-frame metadata");
    assert!(metadata.complete);
    assert!(metadata.block_id.parse::<u64>().is_ok());
    assert!(metadata.ts_dev.is_some());
    assert!(metadata.features.is_empty());
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

/// Codex review of #203: a camera a rescan no longer finds while it streams
/// is let go of in order — its frame loop stopped and awaited, the stream
/// closed and acquisition stopped on the device — before it is published
/// disconnected. Aborting the acquisition task used to detach the frame loop,
/// which kept the stream and published frames under the lost id.
#[tokio::test(flavor = "multi_thread")]
async fn a_camera_lost_mid_acquisition_is_stopped_before_it_is_published_lost() {
    let session = session().await;
    let id = format!("lost-mid-acq-{}", std::process::id());
    let discovery = FakeDiscovery::default();
    discovery.plug(fake(&id));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let service = tokio::spawn(run_discovery(
        session.clone(),
        discovery.clone(),
        Duration::from_millis(200),
        shutdown_rx,
    ));
    assert!(
        serves(&session, &id, Duration::from_secs(10)).await,
        "{id} not served"
    );
    // Drained as they come: a subscriber left to fill blocks the publisher,
    // which would hide a frame loop that never stops.
    let images = session.declare_subscriber(keys::image(&id)).await.unwrap();
    let received = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter = received.clone();
    let drain = tokio::spawn(async move {
        while images.recv_async().await.is_ok() {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    });
    let status = session.declare_subscriber(keys::status(&id)).await.unwrap();

    let start =
        serde_json::to_vec(&AcquisitionControlRequest::new(AcquisitionCommand::Start)).unwrap();
    let reply = ask(
        &session,
        &keys::acquisition_control(&id),
        start,
        Duration::from_secs(10),
    )
    .await
    .expect("no answer to Start");
    let response: NodeOpResponse = serde_json::from_slice(&reply).unwrap();
    assert!(response.ok, "{response:?}");
    let count = || received.load(std::sync::atomic::Ordering::SeqCst);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while count() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no frame before the camera is lost"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let device = discovery.device(&id).expect("connected");
    assert!(device.acquiring() && device.stream_enabled());

    discovery.unplug(&id);
    let disconnected = async {
        loop {
            let sample = status.recv_async().await.unwrap();
            let status: DeviceStatus =
                serde_json::from_slice(&sample.payload().to_bytes()).unwrap();
            if !status.connected {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), disconnected)
        .await
        .expect("no disconnect published");
    // Frames published before the disconnect may still be on their way.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let at_disconnect = count();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        count() - at_disconnect,
        0,
        "frames published after the camera was lost"
    );
    assert!(!device.stream_enabled(), "the stream was left enabled");
    assert!(!device.acquiring(), "acquisition was left running");
    drain.abort();

    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(10), service).await;
}

/// ST-51: USB3 Vision events reach Zenoh with their features and no stream
/// channel or block, and stopping restores both what the device held before:
/// `EventNotification` off and the EIRM's EventEnable clear — read from the
/// fake's registers, not through the service.
#[tokio::test(flavor = "multi_thread")]
async fn events_are_published_and_stop_restores_the_device() {
    use viva_fake_u3v::registers::REG_EVENT_NOTIFICATION;
    use viva_zenoh_api::{CameraEvent, EventControlRequest, EventMonitorStatus};

    let session = session().await;
    let transport = Arc::new(viva_fake_u3v::FakeU3vTransport::new(64, 48, MONO8));
    let device = viva_fake_u3v::open(transport.clone()).unwrap();
    let (camera, xml) = open_u3v_device(device).unwrap();
    let device_id = format!("u3v-events-test-{}", std::process::id());
    let announce = viva_zenoh_api::DeviceAnnounce::new(device_id.clone(), "Fake", "Fake", "");
    let handle = Arc::new(U3vDeviceHandle::new(camera, xml, announce));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let tasks = serve_device(session.clone(), handle, shutdown_rx).await;
    let events = session
        .declare_subscriber(keys::events(&device_id))
        .await
        .unwrap();
    let control = |request: EventControlRequest| {
        let session = session.clone();
        let key = keys::events_control(&device_id);
        async move {
            let reply = ask(
                &session,
                &key,
                serde_json::to_vec(&request).unwrap(),
                Duration::from_secs(10),
            )
            .await
            .expect("no answer to the event control");
            serde_json::from_slice::<EventMonitorStatus>(&reply).unwrap()
        }
    };

    let status = control(EventControlRequest::start(vec!["ExposureEnd".into()])).await;
    assert!(status.active, "{status:?}");
    assert_eq!(
        transport.peek(REG_EVENT_NOTIFICATION, 4),
        1u32.to_le_bytes()
    );
    assert!(transport.events_enabled());

    let start =
        serde_json::to_vec(&AcquisitionControlRequest::new(AcquisitionCommand::Start)).unwrap();
    ask(
        &session,
        &keys::acquisition_control(&device_id),
        start,
        Duration::from_secs(10),
    )
    .await
    .expect("no answer to Start");
    let sample = tokio::time::timeout(Duration::from_secs(5), events.recv_async())
        .await
        .expect("no event within 5 s")
        .unwrap();
    let event: CameraEvent = serde_json::from_slice(&sample.payload().to_bytes()).unwrap();
    assert_eq!(event.event_id, "0x9001");
    assert_eq!(event.name.as_deref(), Some("ExposureEnd"));
    assert_eq!((event.stream_channel, event.block_id), (None, None));
    let timestamp = event
        .features
        .iter()
        .find(|f| f.name == "EventExposureEndTimestamp")
        .and_then(|f| f.value.clone());
    assert_eq!(timestamp, Some(event.ts_dev.clone()));

    let status = control(EventControlRequest::stop()).await;
    assert_eq!(status, EventMonitorStatus::inactive(Vec::new()));
    assert_eq!(
        transport.peek(REG_EVENT_NOTIFICATION, 4),
        0u32.to_le_bytes()
    );
    assert!(!transport.events_enabled(), "EventEnable left set");

    let _ = shutdown_tx.send(true);
    for task in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
    }
}
