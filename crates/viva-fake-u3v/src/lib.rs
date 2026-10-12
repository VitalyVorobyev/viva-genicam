//! In-process fake USB3 Vision camera for testing.
//!
//! [`FakeU3vTransport`] implements [`UsbTransfer`]
//! with an in-memory register map and GenCP command handling, both laid out
//! from the standards rather than from `viva-u3v` ([`registers`]). It can be used
//! with `U3vDevice::open()` to create a fully functional `Camera` without any
//! USB hardware.
//!
//! # Example
//!
//! ```rust,ignore
//! use viva_fake_u3v::FakeU3vCamera;
//! use viva_genicam::open_u3v_device;
//!
//! let fake = FakeU3vCamera::new(640, 480);
//! let device = fake.open_device()?;
//! let (camera, xml) = open_u3v_device(device)?;
//! let width = camera.get("Width")?;
//! assert_eq!(width, "640");
//! ```

pub mod registers;
mod transport;

pub use registers::{REG_TRIGGER_SOFTWARE, WRITE_ONLY_REGISTERS};
pub use transport::FakeU3vTransport;

use std::sync::Arc;

use viva_u3v::{U3vDevice, U3vError, UsbTransfer};

/// The control interface's bulk IN endpoint: acknowledges.
pub const CONTROL_EP_IN: u8 = 0x81;
/// The control interface's bulk OUT endpoint: commands.
pub const CONTROL_EP_OUT: u8 = 0x01;
/// The streaming interface's bulk IN endpoint.
pub const STREAM_EP_IN: u8 = 0x82;

/// Open a [`U3vDevice`] on a fake's endpoints, over `transport`: a
/// [`FakeU3vTransport`], or a caller's type that delegates to one (a binding
/// that serves real and fake cameras through one type).
pub fn open<T: UsbTransfer + 'static>(transport: Arc<T>) -> Result<U3vDevice<T>, U3vError> {
    U3vDevice::open(
        transport,
        CONTROL_EP_IN,
        CONTROL_EP_OUT,
        Some(STREAM_EP_IN),
        None,
    )
}

/// Builder for a fake USB3 Vision camera.
///
/// Creates an in-process fake camera with configurable image dimensions
/// and pixel format. No USB hardware is involved.
pub struct FakeU3vCamera {
    width: u32,
    height: u32,
    pixel_format: u32,
}

impl FakeU3vCamera {
    /// Create a fake camera with the given dimensions (Mono8 format).
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixel_format: 0x0108_0001, // Mono8
        }
    }

    /// Set the pixel format (PFNC code).
    pub fn pixel_format(mut self, pfnc: u32) -> Self {
        self.pixel_format = pfnc;
        self
    }

    /// Create an opened `U3vDevice` backed by this fake camera.
    ///
    /// The device has ABRM, SBRM, and SIRM registers pre-loaded,
    /// and serves a GenApi XML describing Width, Height, PixelFormat,
    /// and acquisition features.
    pub fn open_device(self) -> Result<U3vDevice<FakeU3vTransport>, U3vError> {
        open(Arc::new(FakeU3vTransport::new(
            self.width,
            self.height,
            self.pixel_format,
        )))
    }
}
