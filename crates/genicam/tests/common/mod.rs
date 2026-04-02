//! Shared test helpers for integration tests with `arv-fake-gv-camera-0.8`.

use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const FAKE_CAMERA_BIN: &str = "arv-fake-gv-camera-0.8";
const GVCP_PORT: u16 = 3956;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Guard that starts `arv-fake-gv-camera-0.8` and kills it on drop.
pub struct FakeCamera {
    child: Child,
}

impl FakeCamera {
    /// Start a fake GigE Vision camera on the loopback interface.
    ///
    /// Blocks until the camera is responding on GVCP port 3956, or panics
    /// after a timeout.
    pub fn start() -> Self {
        Self::start_with_args(&["-i", "127.0.0.1"])
    }

    /// Start with custom arguments.
    pub fn start_with_args(args: &[&str]) -> Self {
        let child = Command::new(FAKE_CAMERA_BIN)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("Failed to start {FAKE_CAMERA_BIN}: {e}"));

        let cam = FakeCamera { child };
        cam.wait_for_ready();
        cam
    }

    /// Poll GVCP port until the camera is responding.
    fn wait_for_ready(&self) {
        let start = Instant::now();
        let addr: SocketAddr = ([127, 0, 0, 1], GVCP_PORT).into();
        while start.elapsed() < STARTUP_TIMEOUT {
            // Try sending a minimal GVCP discovery packet and see if we get a response.
            if let Ok(sock) = UdpSocket::bind("127.0.0.1:0") {
                sock.set_read_timeout(Some(Duration::from_millis(200)))
                    .ok();
                // GVCP discovery command: flags=0x11, cmd=0x0002, len=0, req_id=0x0001
                let discovery_pkt = [0x42, 0x11, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01];
                if sock.send_to(&discovery_pkt, addr).is_ok() {
                    let mut buf = [0u8; 512];
                    if sock.recv_from(&mut buf).is_ok() {
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("Fake camera did not start within {STARTUP_TIMEOUT:?}");
    }

    /// Read any stderr output from the process (useful for debugging).
    #[allow(dead_code)]
    pub fn stderr_output(&mut self) -> Vec<String> {
        let stderr = self.child.stderr.take().unwrap();
        BufReader::new(stderr)
            .lines()
            .take(20)
            .map(|l| l.unwrap_or_default())
            .collect()
    }
}

impl Drop for FakeCamera {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Returns `true` if `arv-fake-gv-camera-0.8` is available on PATH.
pub fn aravis_available() -> bool {
    Command::new(FAKE_CAMERA_BIN)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Macro to skip a test if aravis is not installed.
#[macro_export]
macro_rules! skip_if_no_aravis {
    () => {
        if !common::aravis_available() {
            eprintln!("SKIPPED: {} not found on PATH", "arv-fake-gv-camera-0.8");
            return;
        }
    };
}
