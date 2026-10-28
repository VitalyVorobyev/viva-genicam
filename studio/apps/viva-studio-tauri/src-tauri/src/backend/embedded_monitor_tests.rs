use super::*;
use std::net::SocketAddr;
use viva_fake_gige::{FakeCamera, GvcpCommand};
use viva_genicam::{Access, StreamOptions};

async fn peer(backend: &EmbeddedBackend) -> SocketAddr {
    backend
        .with_camera(|c| {
            Ok(c.camera
                .gige()
                .expect("GigE")
                .transport()
                .lock_device()
                .expect("device")
                .local_addr()
                .expect("address"))
        })
        .await
        .expect("camera")
}

fn assert_read_only(fake: &FakeCamera, peer: SocketAddr) {
    let sent = fake.commands().from(peer);
    assert!(
        !sent
            .iter()
            .any(|&(command, _)| matches!(command, GvcpCommand::WriteReg | GvcpCommand::WriteMem)),
        "monitor wrote: {sent:?}"
    );
    assert!(
        !sent
            .iter()
            .any(|&(_, addr)| addr == viva_genicam::gige::consts::CONTROL_CHANNEL_PRIVILEGE),
        "monitor touched control privilege: {sent:?}"
    );
}

async fn multicast_available(iface: &viva_genicam::gige::Iface) -> bool {
    let group = Ipv4Addr::new(239, 192, 1, 98);
    let socket = match viva_genicam::gige::bind_multicast(
        iface,
        group,
        0,
        &viva_genicam::gige::McOptions {
            loopback: true,
            ..Default::default()
        },
    )
    .await
    {
        Ok(socket) => socket,
        Err(_) => return false,
    };
    let port = socket.local_addr().expect("bound").port();
    if socket.send_to(&[0x51], (group, port)).await.is_err() {
        return false;
    }
    let mut buffer = [0; 4];
    matches!(
        tokio::time::timeout(Duration::from_millis(300), socket.recv_from(&mut buffer)).await,
        Ok(Ok((1, _)))
    ) && buffer[0] == 0x51
}

#[test]
fn monitor_refuses_writes_and_unicast_without_disturbing_the_controller() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let info = viva_genicam::gige::DeviceInfo::from_ip(Ipv4Addr::LOCALHOST);
        let (controller, _) = viva_genicam::connect_gige_as(&info, Access::Control)
            .await
            .expect("controller");
        let owner = fake.controller().await;
        let backend = EmbeddedBackend::new(Default::default());
        backend
            .connect_as("127.0.0.1", ConnectionAccess::Monitor)
            .await
            .expect("monitor alongside owner");
        let addr = peer(&backend).await;
        assert!(matches!(
            backend.connection.get().await,
            ConnectionState::Connected {
                access: Some(ConnectionAccess::Monitor),
                ..
            }
        ));
        assert_eq!(
            backend
                .get_feature_state("Width")
                .await
                .expect("read")
                .access_mode,
            "RO"
        );
        assert!(
            backend
                .set_feature("Width", &serde_json::json!(32))
                .await
                .is_err()
        );
        assert!(backend.exec_command("AcquisitionStart").await.is_err());
        assert!(
            backend
                .set_persistent_ip(
                    Ipv4Addr::LOCALHOST,
                    Ipv4Addr::BROADCAST,
                    Ipv4Addr::UNSPECIFIED
                )
                .await
                .is_err()
        );
        let error = backend
            .start_acquisition(Default::default())
            .await
            .expect_err("unicast cannot be watched");
        assert!(error.to_lowercase().contains("multicast"), "{error}");
        backend.stop_acquisition().await.expect("local stop");
        backend.disconnect().await.expect("disconnect");
        backend.shutdown().await;
        assert_read_only(&fake, addr);
        assert_eq!(fake.resends().requests, 0);
        assert_eq!(fake.controller().await, owner);
        drop(controller);
        fake.stop().await;
    });
}

#[test]
fn monitor_multicast_stop_shutdown_and_recovery_never_write_or_stop_the_owner() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let iface = viva_genicam::gige::Iface::from_ipv4(Ipv4Addr::LOCALHOST).expect("loopback");
        if !multicast_available(&iface).await {
            eprintln!("unsupported: this host does not deliver loopback multicast");
            return;
        }
        let fake = FakeCamera::builder()
            .bind_ip(Ipv4Addr::LOCALHOST)
            .width(64)
            .height(48)
            .fps(30)
            .packet_script(viva_fake_gige::PacketScript::new().step(
                viva_fake_gige::ScriptStep::Drop {
                    block: 1,
                    packet: 2,
                },
            ))
            .build()
            .await
            .expect("scripted multicast camera");
        let info = viva_genicam::gige::DeviceInfo::from_ip(Ipv4Addr::LOCALHOST);
        let (mut controller, _) = viva_genicam::connect_gige_as(&info, Access::Control)
            .await
            .expect("controller");
        let owner = fake.controller().await;
        let own_stream = controller
            .open_stream(
                StreamOptions::new()
                    .iface(iface.clone())
                    .packet_size(1400)
                    .multicast(Ipv4Addr::new(239, 192, 1, 24)),
            )
            .expect("configured multicast");
        let (events, recorder) = crate::state::events::Recorder::events();
        let mut backend = EmbeddedBackend::new(events);
        backend.reopen_interface = Some(iface.name().to_string());
        backend
            .connect_as("127.0.0.1", ConnectionAccess::Monitor)
            .await
            .expect("monitor");
        let before = peer(&backend).await;
        let stream = backend
            .start_acquisition(StartOptions {
                packet_size: Some(1000),
            })
            .await
            .expect("watch existing multicast");
        controller
            .acquisition_start()
            .expect("only the owner starts acquisition");
        fake_camera::first_frame(&stream.ws_url).await;
        let loss = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(status) = recorder.of("acquisition-status").into_iter().find(|s| {
                    s["stats"]["packets_missing"]
                        .as_u64()
                        .is_some_and(|n| n > 0)
                }) {
                    break status;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the scripted missing packet reaches monitor counters");
        assert_eq!(loss["stats"]["resend_requested"], 0);
        backend.stop_acquisition().await.expect("stop watching");
        let blocks = fake.blocks_sent();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(
            fake.blocks_sent() > blocks,
            "owner acquisition must continue"
        );
        fake.unplug();
        let lost = tokio::time::timeout(Duration::from_secs(5), async {
            while matches!(
                backend.connection.get().await,
                ConnectionState::Connected { .. }
            ) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        fake.plug_in();
        lost.expect("idle monitor noticed link loss");
        tokio::time::timeout(Duration::from_secs(8), async {
            while !matches!(
                backend.connection.get().await,
                ConnectionState::Connected { .. }
            ) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("monitor reopens after link returns");
        assert!(matches!(
            backend.connection.get().await,
            ConnectionState::Connected {
                access: Some(ConnectionAccess::Monitor),
                ..
            }
        ));
        let after = peer(&backend).await;
        let stream = backend
            .start_acquisition(Default::default())
            .await
            .expect("watch again after readonly replacement");
        fake_camera::first_frame(&stream.ws_url).await;
        backend.shutdown().await;
        assert_eq!(fake.controller().await, owner);
        let blocks = fake.blocks_sent();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(
            fake.blocks_sent() > blocks,
            "monitor shutdown must not stop owner acquisition"
        );
        assert_read_only(&fake, before);
        assert_read_only(&fake, after);
        assert_eq!(fake.resends().requests, 0);
        controller.acquisition_stop().expect("owner stops");
        drop(own_stream);
        drop(controller);
        fake.stop().await;
    });
}

#[test]
fn discovery_reports_subnet_status_and_requests_broadcast_acknowledgements() {
    let _port = fake_camera::lock();
    tauri::async_runtime::block_on(async {
        let fake = fake_camera::start().await;
        let iface = viva_genicam::gige::Iface::from_ipv4(Ipv4Addr::LOCALHOST).expect("loopback");
        let options = studio_discovery_options().interface(iface.name());
        viva_genicam::gige::discover_with(Duration::from_millis(150), &options)
            .await
            .expect("discovery");
        let requests = fake.discovery_requests();
        assert!(!requests.is_empty());
        assert!(requests.iter().all(|r| r.flags & 0x10 != 0));
        fake.stop().await;
    });
    let mut found = viva_genicam::gige::DeviceInfo::from_ip(Ipv4Addr::new(192, 0, 2, 20));
    found.subnet = Some(Ipv4Addr::new(255, 255, 255, 0));
    found.local_ip = Some(Ipv4Addr::new(198, 51, 100, 10));
    found.local_netmask = Some(Ipv4Addr::new(255, 255, 255, 0));
    let (devices, _) = discovered_gige(vec![found.clone()]);
    assert_eq!(devices[0].shares_subnet_with_host, Some(false));
    found.local_ip = Some(Ipv4Addr::new(192, 0, 2, 10));
    let (devices, _) = discovered_gige(vec![found]);
    assert_eq!(devices[0].shares_subnet_with_host, Some(true));
}

#[test]
fn identity_fallback_requires_both_the_expected_mac_and_address() {
    let mut previous = viva_genicam::gige::DeviceInfo::from_ip(Ipv4Addr::new(192, 0, 2, 20));
    previous.mac = [2, 0, 0, 0, 0, 1];
    assert!(verified_discovery(&previous, vec![previous.clone()]).is_ok());
    let mut wrong = previous.clone();
    wrong.mac = [2, 0, 0, 0, 0, 2];
    assert!(verified_discovery(&previous, vec![wrong]).is_err());
    let mut moved = previous.clone();
    moved.ip = Ipv4Addr::new(192, 0, 2, 21);
    assert!(verified_discovery(&previous, vec![moved]).is_err());
    assert!(verified_discovery(&previous, Vec::new()).is_err());
}

#[test]
fn remote_mode_refuses_direct_monitor_before_opening_a_service() {
    tauri::async_runtime::block_on(async {
        let events = Arc::new(Events::default());
        let connection = Arc::new(Connection::new(events.clone()));
        let backend =
            crate::backend::remote::RemoteBackend::new(events, connection, Default::default());
        let error = backend
            .connect_as("service-camera", ConnectionAccess::Monitor)
            .await
            .err()
            .expect("not supported");
        assert!(error.contains("remote"), "{error}");
    });
}

#[test]
fn unsupported_monitor_does_not_disconnect_the_current_usb_camera() {
    tauri::async_runtime::block_on(async {
        let device = viva_fake_u3v::FakeU3vCamera::new(64, 48)
            .open_device()
            .expect("fake USB");
        let (camera, _) = viva_genicam::open_u3v_device(device).expect("camera");
        let backend = EmbeddedBackend::new(Default::default());
        backend.attach(Box::new(camera), "usb-test", None).await;
        backend.usb.write().await.insert(
            "usb-test".into(),
            U3vDeviceInfo {
                bus: "fake".into(),
                address: 1,
                vendor_id: 1,
                product_id: 2,
                serial: Some("fake".into()),
                manufacturer: None,
                model: None,
                speed: None,
            },
        );
        let ticket = backend.lifecycle.ticket();
        assert!(
            backend
                .connect_as("usb-test", ConnectionAccess::Monitor)
                .await
                .is_err()
        );
        assert_eq!(backend.lifecycle.ticket(), ticket);
        assert_eq!(
            backend
                .get_feature_state("Width")
                .await
                .expect("still connected")
                .value,
            serde_json::json!(64)
        );
        backend.shutdown().await;
    });
}

#[test]
fn remote_monitor_selection_preserves_the_current_service_connection() {
    tauri::async_runtime::block_on(async {
        let events = Arc::new(Events::default());
        let connection = Arc::new(Connection::new(events.clone()));
        connection
            .set(ConnectionState::Connected {
                device_id: "current".into(),
                device_name: "Service".into(),
                model: "Fake".into(),
                access: None,
            })
            .await;
        let model: SharedModelState = Default::default();
        let backend =
            crate::backend::remote::RemoteBackend::new(events, connection.clone(), model.clone());
        assert!(
            backend
                .connect_model("other", ConnectionAccess::Monitor, &connection, &model)
                .await
                .is_err()
        );
        assert!(
            matches!(connection.get().await, ConnectionState::Connected { device_id, .. } if device_id == "current")
        );
    });
}
