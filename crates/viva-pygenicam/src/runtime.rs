//! Managed multi-threaded Tokio runtime shared by all async entry points.

use once_cell::sync::Lazy;
use tokio::runtime::{Builder, Runtime};

static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    Builder::new_multi_thread()
        .enable_all()
        .thread_name("viva-pygenicam-rt")
        .build()
        .expect("failed to build viva-pygenicam tokio runtime")
});

pub(crate) fn runtime() -> &'static Runtime {
    &RUNTIME
}
