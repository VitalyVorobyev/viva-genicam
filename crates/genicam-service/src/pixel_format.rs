//! Pixel format conversion between genicam-rs (pfnc) and genicam_zenoh_api types.

use genicam::pfnc;
use genicam_zenoh_api::PixelFormat as ZenohPixelFormat;

/// Convert a `pfnc::PixelFormat` (32-bit PFNC code) to a `genicam_zenoh_api::PixelFormat`.
pub fn pfnc_to_zenoh(pf: pfnc::PixelFormat) -> ZenohPixelFormat {
    match pf {
        pfnc::PixelFormat::Mono8 => ZenohPixelFormat::Mono8,
        pfnc::PixelFormat::Mono16 => ZenohPixelFormat::Mono16,
        pfnc::PixelFormat::BayerRG8 => ZenohPixelFormat::BayerRG8,
        pfnc::PixelFormat::BayerGR8 => ZenohPixelFormat::BayerGR8,
        pfnc::PixelFormat::BayerBG8 => ZenohPixelFormat::BayerBG8,
        pfnc::PixelFormat::BayerGB8 => ZenohPixelFormat::BayerGB8,
        pfnc::PixelFormat::RGB8Packed => ZenohPixelFormat::RGB8,
        pfnc::PixelFormat::BGR8Packed => ZenohPixelFormat::BGR8,
        pfnc::PixelFormat::Unknown(_) => ZenohPixelFormat::Unknown,
    }
}
