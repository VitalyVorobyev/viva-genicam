//! Shared test helpers for integration tests with the in-process fake camera.

use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, OwnedMutexGuard};
use viva_fake_gige::FakeCamera;

/// Global mutex ensuring only one fake camera uses port 3956 at a time.
static CAMERA_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

fn camera_lock() -> Arc<Mutex<()>> {
    CAMERA_LOCK.get_or_init(|| Arc::new(Mutex::new(()))).clone()
}

/// Guard that starts a fake GigE Vision camera and stops it on drop.
///
/// Holds a global mutex to prevent port conflicts when tests run in parallel.
pub struct TestCamera {
    camera: Option<FakeCamera>,
    _guard: OwnedMutexGuard<()>,
}

impl TestCamera {
    /// Start a fake GigE Vision camera on the loopback interface.
    ///
    /// Acquires a global lock to ensure only one camera runs at a time.
    pub async fn start() -> Self {
        let guard = camera_lock().lock_owned().await;
        let camera = loop {
            match FakeCamera::builder()
                .bind_ip([127, 0, 0, 1].into())
                .port(3956)
                .width(640)
                .height(480)
                .fps(30)
                .build()
                .await
            {
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
}

impl Drop for TestCamera {
    fn drop(&mut self) {
        // `FakeCamera::stop` is async; from a sync Drop we rely on
        // `FakeCamera`'s own Drop impl to abort the tasks best-effort.
        self.camera.take();
    }
}
