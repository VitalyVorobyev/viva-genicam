//! Long ACK waits retain privilege without repeating the active operation.

#[path = "support/heartbeat_sensor.rs"]
mod sensor;

use std::time::Duration;
use tokio::time;
use viva_gige::GigeDevice;
use viva_gige::gvcp::GigeError;

#[tokio::test]
async fn long_read_wait_sends_heartbeats_and_drains_their_late_replies() {
    let (address, task, _) = sensor::start(false).await;
    let mut device = GigeDevice::open(address).await.unwrap();
    device.claim_control().await.unwrap();
    device.set_control_timeout(Duration::from_secs(2));
    device.set_inflight_keepalive_period(Some(Duration::from_millis(75)));
    assert_eq!(
        device.read_register(sensor::SLOW_REGISTER).await.unwrap(),
        sensor::VALUE
    );
    device
        .write_register(sensor::NEXT_REGISTER, 1)
        .await
        .unwrap();
    let (observed, _socket) = task.await.unwrap();
    assert_eq!(observed.commands, 1);
    assert!(observed.heartbeat_opcodes.len() >= 3);
    assert!(!observed.expired);
}

#[tokio::test]
async fn without_inflight_heartbeats_the_same_wait_expires_control() {
    let (address, task, _) = sensor::start(false).await;
    let mut device = GigeDevice::open(address).await.unwrap();
    device.claim_control().await.unwrap();
    device.set_control_timeout(Duration::from_secs(2));
    assert_eq!(
        device.read_register(sensor::SLOW_REGISTER).await.unwrap(),
        sensor::VALUE
    );
    assert!(
        device
            .write_register(sensor::NEXT_REGISTER, 1)
            .await
            .is_err()
    );
    let (observed, _socket) = task.await.unwrap();
    assert!(observed.expired);
    assert!(observed.heartbeat_opcodes.is_empty());
}

#[tokio::test]
async fn pending_write_is_not_repeated_to_refresh_the_heartbeat() {
    let (address, task, _) = sensor::start(true).await;
    let mut device = GigeDevice::open(address).await.unwrap();
    device.claim_control().await.unwrap();
    device.set_inflight_keepalive_period(Some(Duration::from_millis(75)));
    device
        .write_register(sensor::SLOW_REGISTER, 1)
        .await
        .unwrap();
    device
        .write_register(sensor::NEXT_REGISTER, 1)
        .await
        .unwrap();
    let (observed, _socket) = task.await.unwrap();
    assert_eq!(observed.commands, 1);
    assert!(observed.heartbeat_opcodes.len() >= 3);
    assert!(!observed.expired);
}

#[tokio::test]
async fn heartbeat_replies_do_not_extend_an_unanswered_requests_deadline() {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut buffer = [0u8; 1024];
        loop {
            let (len, peer) = socket.recv_from(&mut buffer).await.unwrap();
            assert!(len >= 12);
            let address = u32::from_be_bytes(buffer[8..12].try_into().unwrap());
            if address == 0x0a00 {
                let ack = [0, 0, 0, 0x81, 0, 4, buffer[6], buffer[7], 0, 0, 0, 2];
                socket.send_to(&ack, peer).await.unwrap();
            }
            // Deliberately never acknowledge the ordinary read.
        }
    });
    let mut device = GigeDevice::open(address).await.unwrap();
    device.set_control_timeout(Duration::from_millis(100));
    device.set_inflight_keepalive_period(Some(Duration::from_millis(20)));
    let result = time::timeout(
        Duration::from_secs(2),
        device.read_register(sensor::SLOW_REGISTER),
    )
    .await;
    server.abort();
    assert!(matches!(result, Ok(Err(GigeError::Timeout))), "{result:?}");
}
