//! The facade's device mutex cannot starve heartbeats during register I/O.

#[path = "../../viva-gige/tests/support/heartbeat_sensor.rs"]
mod sensor;

use std::time::Duration;
use viva_genicam::GigeRegisterIo;
use viva_genicam::genapi::RegisterIo;
use viva_genicam::gige::GigeDevice;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_register_read_keeps_control_while_holding_the_device_mutex() {
    let (address, task, reported) = sensor::start(false).await;
    let mut device = GigeDevice::open(address).await.unwrap();
    device.claim_control().await.unwrap();
    device.set_control_timeout(Duration::from_secs(2));
    let transport = GigeRegisterIo::new(tokio::runtime::Handle::current(), device);
    // The initial keepalive holds the mutex until it has read this value and
    // configured in-flight probes. The slow read waits for that mutex, giving
    // a deterministic handshake instead of a sleep to allow initialization.
    reported.await.unwrap();
    let transport = tokio::task::spawn_blocking(move || {
        assert_eq!(
            transport.read(u64::from(sensor::SLOW_REGISTER), 4).unwrap(),
            sensor::VALUE.to_be_bytes()
        );
        transport
            .write(u64::from(sensor::NEXT_REGISTER), &1u32.to_be_bytes())
            .unwrap();
        transport
    })
    .await
    .unwrap();
    let (observed, socket) = task.await.unwrap();
    assert_eq!(observed.commands, 1);
    assert!(observed.heartbeat_opcodes.len() >= 3);
    assert!(!observed.expired);
    // Keep answering idle probes while the existing background task retires.
    let responder = tokio::spawn(async move {
        let mut buffer = [0u8; 1024];
        loop {
            let (_, peer) = socket.recv_from(&mut buffer).await.unwrap();
            let ack = [0, 0, 0, 0x81, 0, 4, buffer[6], buffer[7], 0, 0, 0, 2];
            socket.send_to(&ack, peer).await.unwrap();
        }
    });
    drop(transport);
    tokio::time::sleep(Duration::from_millis(150)).await;
    responder.abort();
}
