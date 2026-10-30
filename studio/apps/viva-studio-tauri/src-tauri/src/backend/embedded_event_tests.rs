//! Camera events in embedded mode (backlog ST-51): each event reaches the UI
//! with its features, and stopping — explicitly, by disconnecting, or by
//! losing the camera — restores what starting changed or says it could not.
//! Device state is read from the fakes, past the protocol.

use super::*;
use crate::state::events::Recorder;
use viva_fake_gige::EVENT_EXPOSURE_END;
use viva_fake_u3v::FakeU3vTransport;
use viva_fake_u3v::registers::REG_EVENT_NOTIFICATION;
use viva_genicam::gige::consts::MESSAGE_DESTINATION_PORT;

const EXPOSURE_END: &str = "ExposureEnd";

/// The first `camera-event` the recorder receives, within 5 s.
async fn first_event(recorder: &Recorder) -> CameraEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(event) = recorder.of("camera-event").into_iter().next() {
                return serde_json::from_value::<CameraEvent>(event).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("no camera event within 5 s")
}

fn feature(event: &CameraEvent, name: &str) -> String {
    event
        .features
        .iter()
        .find(|f| f.name == name)
        .and_then(|f| f.value.clone())
        .unwrap_or_else(|| panic!("{name} in {:?}", event.features))
}

fn statuses(recorder: &Recorder) -> Vec<EventMonitorStatus> {
    recorder
        .of("camera-events-status")
        .into_iter()
        .map(|status| serde_json::from_value(status).unwrap())
        .collect()
}

fn usb_camera() -> (Arc<FakeU3vTransport>, Box<dyn Device>) {
    let transport = Arc::new(FakeU3vTransport::new(64, 48, 0x0108_0001));
    let device = viva_fake_u3v::open(transport.clone()).unwrap();
    let (camera, _) = viva_genicam::open_u3v_device(device).unwrap();
    (transport, Box::new(camera))
}

#[test]
fn gige_events_reach_the_ui_and_stop_restores_the_camera() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let (events, recorder) = Recorder::events();
        let backend = EmbeddedBackend::new(events);
        backend.connect("127.0.0.1").await.unwrap();
        let port_before = fake.peek(MESSAGE_DESTINATION_PORT, 4).await;

        let refused = backend
            .start_events(vec!["NoSuchEvent".into()])
            .await
            .unwrap_err();
        assert!(refused.contains("NoSuchEvent"), "{refused}");
        assert!(!fake.event_notification_on(EVENT_EXPOSURE_END).await);

        let status = backend
            .start_events(vec![EXPOSURE_END.into()])
            .await
            .unwrap();
        assert_eq!(
            status,
            EventMonitorStatus::active(vec![EXPOSURE_END.into()])
        );
        assert!(fake.event_notification_on(EVENT_EXPOSURE_END).await);
        backend.start_acquisition(Default::default()).await.unwrap();

        let event = first_event(&recorder).await;
        assert_eq!(event.event_id, "0x9001");
        assert_eq!(event.name.as_deref(), Some(EXPOSURE_END));
        assert_eq!(event.stream_channel, Some(0));
        assert_eq!(feature(&event, "EventExposureEndTimestamp"), event.ts_dev);
        assert_eq!(
            Some(feature(&event, "EventExposureEndFrameID")),
            event.block_id
        );
        assert!(event.ts_host_ns.is_none(), "no fitted clock, no host time");

        let status = backend.stop_events().await.unwrap();
        assert_eq!(status, EventMonitorStatus::inactive(Vec::new()));
        assert!(!fake.event_notification_on(EVENT_EXPOSURE_END).await);
        assert_eq!(fake.peek(MESSAGE_DESTINATION_PORT, 4).await, port_before);
        assert_eq!(statuses(&recorder).last(), Some(&status));

        backend.shutdown().await;
        fake.stop().await;
    });
}

/// Disconnecting stops monitoring and restores, as Stop does.
#[test]
fn disconnect_stops_events_and_restores_notification() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let (events, recorder) = Recorder::events();
        let backend = EmbeddedBackend::new(events);
        backend.connect("127.0.0.1").await.unwrap();
        backend
            .start_events(vec![EXPOSURE_END.into()])
            .await
            .unwrap();
        assert!(fake.event_notification_on(EVENT_EXPOSURE_END).await);

        backend.disconnect().await.unwrap();
        assert!(!fake.event_notification_on(EVENT_EXPOSURE_END).await);
        assert_eq!(
            statuses(&recorder).last(),
            Some(&EventMonitorStatus::inactive(Vec::new()))
        );
        assert!(backend.event_monitor.lock().await.is_none());
        fake.stop().await;
    });
}

#[test]
fn usb_events_reach_the_ui_and_stop_restores_the_device() {
    tauri::async_runtime::block_on(async {
        let (transport, camera) = usb_camera();
        let (events, recorder) = Recorder::events();
        let backend = EmbeddedBackend::new(events);
        backend.attach(camera, "usb-events-test", None).await;

        backend
            .start_events(vec![EXPOSURE_END.into()])
            .await
            .unwrap();
        assert_eq!(
            transport.peek(REG_EVENT_NOTIFICATION, 4),
            1u32.to_le_bytes()
        );
        assert!(transport.events_enabled());
        backend.start_acquisition(Default::default()).await.unwrap();

        let event = first_event(&recorder).await;
        assert_eq!(event.name.as_deref(), Some(EXPOSURE_END));
        assert_eq!((event.stream_channel, event.block_id.clone()), (None, None));
        assert_eq!(feature(&event, "EventExposureEndTimestamp"), event.ts_dev);

        let status = backend.stop_events().await.unwrap();
        assert_eq!(status, EventMonitorStatus::inactive(Vec::new()));
        assert_eq!(
            transport.peek(REG_EVENT_NOTIFICATION, 4),
            0u32.to_le_bytes()
        );
        assert!(!transport.events_enabled(), "EventEnable left set");
        backend.shutdown().await;
    });
}

/// An unplugged camera cannot be restored, and the UI is told so rather
/// than shown a clean stop; nothing is written to the missing device.
#[test]
fn losing_a_usb_camera_mid_stream_reports_unrestored_settings() {
    tauri::async_runtime::block_on(async {
        let (transport, camera) = usb_camera();
        let (events, recorder) = Recorder::events();
        let backend = EmbeddedBackend::new(events);
        backend.attach(camera, "usb-events-loss-test", None).await;
        backend
            .start_events(vec![EXPOSURE_END.into()])
            .await
            .unwrap();
        backend.start_acquisition(Default::default()).await.unwrap();
        first_event(&recorder).await;

        transport.unplug();
        let writes = transport.writes().len();
        let stopped = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = statuses(&recorder).into_iter().find(|s| !s.active) {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("monitoring did not stop after the camera was lost");
        assert!(!stopped.warnings.is_empty(), "{stopped:?}");
        assert!(
            stopped.warnings.iter().any(|w| w.contains("not restored")),
            "{stopped:?}"
        );
        assert_eq!(transport.writes().len(), writes, "wrote to a lost device");
        backend.shutdown().await;
    });
}
