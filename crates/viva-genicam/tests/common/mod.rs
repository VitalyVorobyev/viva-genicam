//! Shared test helpers for integration tests with the in-process fake camera.

use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, OwnedMutexGuard};
use viva_fake_gige::{FakeCamera, FakeCameraBuilder};

/// Global mutex ensuring only one fake camera uses port 3956 at a time.
static CAMERA_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

fn camera_lock() -> Arc<Mutex<()>> {
    CAMERA_LOCK.get_or_init(|| Arc::new(Mutex::new(()))).clone()
}

/// Guard that starts a fake GigE Vision camera and stops it on drop.
///
/// Holds a global mutex to prevent port conflicts when tests run in parallel.
pub(crate) struct TestCamera {
    camera: Option<FakeCamera>,
    _guard: OwnedMutexGuard<()>,
}

impl TestCamera {
    /// Start a fake GigE Vision camera on the loopback interface.
    ///
    /// Acquires a global lock to ensure only one camera runs at a time.
    // Each integration test is its own crate, so a helper only some of them use
    // reads as dead code in the others.
    #[allow(dead_code)]
    pub(crate) async fn start() -> Self {
        Self::start_with(|builder| builder).await
    }

    /// Start a fake camera with extra builder customization applied on top
    /// of the standard loopback test configuration.
    pub(crate) async fn start_with<F>(customize: F) -> Self
    where
        F: Fn(FakeCameraBuilder) -> FakeCameraBuilder,
    {
        let guard = camera_lock().lock_owned().await;
        let camera = loop {
            let builder = FakeCamera::builder()
                .bind_ip([127, 0, 0, 1].into())
                .port(3956)
                .width(640)
                .height(480)
                .fps(30);
            match customize(builder).build().await {
                Ok(cam) => break cam,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                Err(e) => panic!("failed to start fake camera: {e}"),
            }
        };
        TestCamera {
            camera: Some(camera),
            _guard: guard,
        }
    }

    /// The running fake, for tests that inspect what reached it on the wire.
    #[allow(dead_code)]
    pub(crate) fn fake(&self) -> &FakeCamera {
        // `camera` is only taken in `drop`, so it is always present here.
        self.camera.as_ref().expect("fake camera is running")
    }
}

impl Drop for TestCamera {
    fn drop(&mut self) {
        // `FakeCamera::stop` is async; from a sync Drop we rely on
        // `FakeCamera`'s own Drop impl to abort the tasks best-effort.
        self.camera.take();
    }
}

/// A camera that never answers: a socket bound to the GVCP port on loopback
/// that reads nothing and sends nothing, held under the same lock as
/// [`TestCamera`].
#[allow(dead_code)]
pub(crate) struct SilentCamera {
    _socket: std::net::UdpSocket,
    _guard: OwnedMutexGuard<()>,
}

impl SilentCamera {
    #[allow(dead_code)]
    pub(crate) async fn bind() -> Self {
        let guard = camera_lock().lock_owned().await;
        let socket = loop {
            match std::net::UdpSocket::bind(("127.0.0.1", 3956)) {
                Ok(socket) => break socket,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(e) => panic!("bind the silent camera: {e}"),
            }
        };
        SilentCamera {
            _socket: socket,
            _guard: guard,
        }
    }
}
