//! Camera tools in embedded mode: configuration files (backlog ST-50),
//! `<Register>` nodes (ST-53) and action commands (ST-54). Every check reads
//! the fake camera's state past the protocol, not the backend's own answer.

use super::*;
use viva_fake_gige::{
    FAKE_DEVICE_KEY, FAKE_GROUP_KEY, FAKE_GROUP_MASK, REG_DEVICE_USER_BUFFER, REG_EXPOSURE_TIME,
};
use viva_genicam::gige::ActionParams;

async fn exposure(fake: &viva_fake_gige::FakeCamera) -> f64 {
    f64::from_be_bytes(
        fake.peek(REG_EXPOSURE_TIME, 8)
            .await
            .try_into()
            .expect("eight bytes"),
    )
}

#[test]
fn a_saved_configuration_loaded_back_restores_the_camera() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let backend = EmbeddedBackend::new(Default::default());
        backend.connect("127.0.0.1").await.unwrap();

        backend
            .set_feature("ExposureTime", &serde_json::json!(1234.0))
            .await
            .unwrap();
        let saved = backend.save_configuration().await.expect("save");
        assert!(
            saved.text.contains("ExposureTime\t1234\n"),
            "{}",
            saved.text
        );
        assert!(saved.values > 0);

        backend
            .set_feature("ExposureTime", &serde_json::json!(5000.0))
            .await
            .unwrap();
        assert_eq!(exposure(&fake).await, 5000.0);

        let loaded = backend.load_configuration(saved.text).await.expect("load");
        assert!(loaded.failures.is_empty(), "{:?}", loaded.failures);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(exposure(&fake).await, 1234.0);

        // A file that cannot be read writes nothing.
        let refused = backend
            .load_configuration("Width 640\n".into())
            .await
            .unwrap_err();
        assert!(refused.contains("line 1"), "{refused}");
        assert_eq!(exposure(&fake).await, 1234.0);

        backend.shutdown().await;
        fake.stop().await;
    });
}

#[test]
fn a_register_is_written_exactly_its_length_and_read_back() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let backend = EmbeddedBackend::new(Default::default());
        backend.connect("127.0.0.1").await.unwrap();

        let bytes: Vec<u8> = (0xF0..=0xFF).collect();
        backend
            .write_register("DeviceUserBuffer", bytes.clone())
            .await
            .expect("write");
        assert_eq!(fake.peek(REG_DEVICE_USER_BUFFER, 16).await, bytes);
        assert_eq!(
            backend.read_register("DeviceUserBuffer").await.unwrap(),
            bytes
        );

        let refused = backend
            .write_register("DeviceUserBuffer", vec![0; 15])
            .await
            .unwrap_err();
        assert!(refused.contains("16"), "{refused}");
        assert_eq!(fake.peek(REG_DEVICE_USER_BUFFER, 16).await, bytes);

        backend.shutdown().await;
        fake.stop().await;
    });
}

/// A monitor session reads a register but changes nothing: no register
/// write, no configuration load, and no save, which sets selectors.
#[test]
fn a_read_only_session_reads_registers_but_writes_nothing() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let info = viva_genicam::gige::DeviceInfo::from_ip(Ipv4Addr::LOCALHOST);
        let (_controller, _) = viva_genicam::connect_gige_as(&info, viva_genicam::Access::Control)
            .await
            .expect("controller");
        let backend = EmbeddedBackend::new(Default::default());
        backend
            .connect_as("127.0.0.1", ConnectionAccess::Monitor)
            .await
            .expect("monitor");
        let before = fake.peek(REG_DEVICE_USER_BUFFER, 16).await;

        assert_eq!(
            backend.read_register("DeviceUserBuffer").await.unwrap(),
            before
        );
        assert!(
            backend
                .write_register("DeviceUserBuffer", vec![0x55; 16])
                .await
                .is_err()
        );
        assert_eq!(fake.peek(REG_DEVICE_USER_BUFFER, 16).await, before);
        let refused = backend.save_configuration().await.unwrap_err();
        assert!(refused.contains("read-only"), "{refused}");
        let file = "# {05D8C294-F295-4dfb-9D01-096BD04049F4}\nExposureTime\t4321\n";
        let refused = backend.load_configuration(file.into()).await.unwrap_err();
        assert!(refused.contains("read-only"), "{refused}");
        assert_ne!(exposure(&fake).await, 4321.0);

        backend.disconnect().await.unwrap();
        fake.stop().await;
    });
}

/// The fake acts on an action addressed to its keys, and ignores one for
/// another group. The default destination is the broadcast address of the
/// network the camera was found on.
#[test]
fn an_action_command_reaches_the_camera_whose_keys_match() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let backend = EmbeddedBackend::new(Default::default());
        assert_eq!(
            backend.default_action_destination().await.unwrap(),
            Ipv4Addr::BROADCAST,
            "no camera: the limited broadcast"
        );
        backend.connect("127.0.0.1").await.unwrap();
        assert_eq!(
            backend.default_action_destination().await.unwrap(),
            Ipv4Addr::new(127, 255, 255, 255),
            "loopback is a /8"
        );

        let params = ActionParams {
            device_key: FAKE_DEVICE_KEY,
            group_key: FAKE_GROUP_KEY,
            group_mask: FAKE_GROUP_MASK,
            scheduled_time: None,
        };
        let summary = backend
            .send_action(Ipv4Addr::LOCALHOST, params.clone(), 500)
            .await
            .expect("send");
        assert_eq!((summary.sent, summary.acks), (1, 1), "{summary:?}");
        assert_eq!(fake.actions(), 1);

        let other = ActionParams {
            group_key: FAKE_GROUP_KEY ^ 0xFFFF,
            ..params
        };
        let summary = backend
            .send_action(Ipv4Addr::LOCALHOST, other, 200)
            .await
            .expect("send");
        assert_eq!(summary.acks, 0);
        assert_eq!(fake.actions(), 1, "acted on another group's action");

        backend.shutdown().await;
        fake.stop().await;
    });
}

#[test]
fn a_broadcast_address_is_the_host_networks() {
    let mut info = viva_genicam::gige::DeviceInfo::from_ip(Ipv4Addr::new(192, 168, 2, 10));
    assert_eq!(subnet_broadcast(&info), None);
    info.subnet = Some(Ipv4Addr::new(255, 255, 255, 0));
    assert_eq!(
        subnet_broadcast(&info),
        Some(Ipv4Addr::new(192, 168, 2, 255))
    );
    // The host's side wins: the broadcast leaves from its interface.
    info.local_ip = Some(Ipv4Addr::new(169, 254, 3, 4));
    info.local_netmask = Some(Ipv4Addr::new(255, 255, 0, 0));
    assert_eq!(
        subnet_broadcast(&info),
        Some(Ipv4Addr::new(169, 254, 255, 255))
    );
    // A host route has no broadcast address.
    info.local_netmask = Some(Ipv4Addr::BROADCAST);
    assert_eq!(subnet_broadcast(&info), None);
}
