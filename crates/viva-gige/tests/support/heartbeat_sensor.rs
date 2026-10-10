//! Wire fixture shared by the raw GVCP and facade keepalive regression tests.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{self, Instant};

pub const SLOW_REGISTER: u32 = 0x2000;
pub const NEXT_REGISTER: u32 = 0x2004;
pub const VALUE: u32 = 0xcafe_babe;
pub const HEARTBEAT_MS: u32 = 300;
pub const REPLY_DELAY: Duration = Duration::from_millis(900);

#[derive(Debug)]
pub struct Observations {
    pub commands: usize,
    pub heartbeat_opcodes: Vec<u16>,
    pub expired: bool,
}

async fn send_ack(
    socket: &UdpSocket,
    peer: SocketAddr,
    id: u16,
    opcode: u16,
    status: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut ack = Vec::new();
    ack.extend_from_slice(&status.to_be_bytes());
    ack.extend_from_slice(&(opcode + 1).to_be_bytes());
    ack.extend_from_slice(&u16::try_from(payload.len()).unwrap().to_be_bytes());
    ack.extend_from_slice(&id.to_be_bytes());
    ack.extend_from_slice(payload);
    socket.send_to(&ack, peer).await.unwrap();
    ack
}

/// Acknowledge one file-like operation after several heartbeat windows.
/// Process heartbeat requests while it is pending, enforce expiry, and leave
/// duplicate heartbeat replies queued for the following operation to drain.
pub async fn start(
    pending_ack: bool,
) -> (
    SocketAddr,
    JoinHandle<(Observations, UdpSocket)>,
    oneshot::Receiver<()>,
) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let (ready, reported) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut ready = Some(ready);
        let mut observations = Observations {
            commands: 0,
            heartbeat_opcodes: Vec::new(),
            expired: false,
        };
        let mut buffer = [0u8; 2048];
        let mut owner = None;
        let mut last_command = Instant::now();
        let mut slow_command = None;
        let mut reply_deadline = None;
        let mut last_heartbeat_ack: Option<Vec<u8>> = None;
        loop {
            tokio::select! {
                _ = async {
                    if let Some(deadline) = reply_deadline {
                        time::sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    let (peer, id, opcode) = slow_command.unwrap();
                    let mut payload = Vec::new();
                    if opcode == 0x0084 {
                        payload.extend_from_slice(&SLOW_REGISTER.to_be_bytes());
                    }
                    let value = if opcode == 0x0082 { 0 } else { VALUE };
                    payload.extend_from_slice(&value.to_be_bytes());
                    send_ack(&socket, peer, id, opcode, 0, &payload).await;
                    // Late and duplicated heartbeat ACKs must not steal the
                    // next read/write or packet-resend acknowledgement.
                    if let Some(ref ack) = last_heartbeat_ack {
                        for _ in 0..5 {
                            socket.send_to(ack, peer).await.unwrap();
                        }
                    }
                    reply_deadline = None;
                }
                received = socket.recv_from(&mut buffer) => {
                    let (len, peer) = received.unwrap();
                    assert!(len >= 12);
                    assert_eq!(buffer[0], 0x42);
                    let opcode = u16::from_be_bytes(buffer[2..4].try_into().unwrap());
                    let id = u16::from_be_bytes(buffer[6..8].try_into().unwrap());
                    let address = u32::from_be_bytes(buffer[8..12].try_into().unwrap());
                    if owner.is_some() {
                        assert_eq!(owner, Some(peer), "heartbeat must use the controller's source port");
                        if last_command.elapsed() >= Duration::from_millis(u64::from(HEARTBEAT_MS)) {
                            observations.expired = true;
                        }
                    }
                    last_command = Instant::now();
                    if address == 0x0a00 && opcode == 0x0082 {
                        owner = Some(peer);
                        send_ack(&socket, peer, id, opcode, 0, &0u32.to_be_bytes()).await;
                    } else if address == 0x0938 {
                        send_ack(&socket, peer, id, opcode, 0, &HEARTBEAT_MS.to_be_bytes()).await;
                        if let Some(ready) = ready.take() {
                            let _ = ready.send(());
                        }
                    } else if address == SLOW_REGISTER {
                        observations.commands += 1;
                        if let Some((_, active_id, _)) = slow_command {
                            assert_eq!(active_id, id, "a retry must retain its request ID");
                        } else {
                            slow_command = Some((peer, id, opcode));
                            reply_deadline = Some(Instant::now() + REPLY_DELAY);
                            if pending_ack {
                                // GVCP PENDING_ACK: reserved u16, timeout u16.
                                let pending = [0, 0, 0, 0x89, 0, 4, buffer[6], buffer[7], 0, 0, 0x05, 0xdc];
                                socket.send_to(&pending, peer).await.unwrap();
                            }
                        }
                    } else if address == 0x0a00 {
                        assert!(matches!(opcode, 0x0080 | 0x0084));
                        observations.heartbeat_opcodes.push(opcode);
                        assert_ne!(slow_command.map(|(_, id, _)| id), Some(id));
                        let mut payload = Vec::new();
                        if opcode == 0x0084 {
                            assert_eq!(len, 16);
                            assert_eq!(&buffer[12..16], &[0, 0, 0, 4]);
                            payload.extend_from_slice(&address.to_be_bytes());
                        }
                        let privilege: u32 = if observations.expired { 0 } else { 2 };
                        payload.extend_from_slice(&privilege.to_be_bytes());
                        last_heartbeat_ack = Some(send_ack(&socket, peer, id, opcode, 0, &payload).await);
                    } else if address == NEXT_REGISTER || opcode == 0x0040 {
                        let status = if observations.expired { 0x8004 } else { 0 };
                        let payload = if opcode == 0x0040 { Vec::new() } else { 0u32.to_be_bytes().to_vec() };
                        send_ack(&socket, peer, id, opcode, status, &payload).await;
                        // The facade test keeps servicing this socket during
                        // teardown so an idle ping cannot be stranded.
                        return (observations, socket);
                    } else {
                        panic!("unexpected request: opcode {opcode:#x}, address {address:#x}");
                    }
                }
            }
        }
    });
    (address, task, reported)
}
