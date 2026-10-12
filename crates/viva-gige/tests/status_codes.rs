//! GigE Vision decodes an acknowledgement's status with GigE Vision's own
//! table, not a table shared with GenCP (ADR-0020, backlog TC-16).
//!
//! The device is a socket that answers every command with one literal
//! acknowledgement, written from the GVCP header layout (status, acknowledge
//! id, length, request id, all big-endian) — not from this crate's encoder.

use std::net::SocketAddr;

use tokio::net::UdpSocket;
use viva_gige::GigeDevice;

/// Answer the first command with `status` in a `READREG_ACK` carrying one
/// zero register.
async fn device_answering(status: u16) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
            if len < 8 {
                continue;
            }
            let request_id = [buf[6], buf[7]];
            #[rustfmt::skip]
            let ack = [
                (status >> 8) as u8, status as u8, // status
                0x00, 0x81,                        // READREG_ACK
                0x00, 0x04,                        // payload length
                request_id[0], request_id[1],      // request id, echoed
                0x00, 0x00, 0x00, 0x00,            // register value
            ];
            let _ = socket.send_to(&ack, from).await;
        }
    });
    address
}

async fn read_error(status: u16) -> String {
    let address = device_answering(status).await;
    let mut device = GigeDevice::open(address).await.unwrap();
    device
        .read_register(0x0A00)
        .await
        .expect_err("the device refused the read")
        .to_string()
}

/// `0x800B` is `NO_MSG` in GigE Vision's table; GenCP's `MSG_TIMEOUT` is the
/// other transport's meaning of the same number.
#[tokio::test]
async fn gvcp_names_0x800b_no_msg() {
    let message = read_error(0x800B).await;
    assert!(message.contains("NO_MSG (0x800B)"), "{message}");
}

/// A code only GigE Vision defines is named, not reported as unknown.
#[tokio::test]
async fn gvcp_names_its_own_codes() {
    let message = read_error(0x800C).await;
    assert!(message.contains("PACKET_UNAVAILABLE (0x800C)"), "{message}");
}

/// The shared core still reads as it did.
#[tokio::test]
async fn gvcp_names_the_shared_codes() {
    let message = read_error(0x8006).await;
    assert!(message.contains("ACCESS_DENIED (0x8006)"), "{message}");
}
