//! Pixel format conversion between viva-genicam (pfnc) and genicam_zenoh_api types.

use viva_genicam::pfnc;
use viva_zenoh_api::PixelFormat as ZenohPixelFormat;

/// Convert a `pfnc::PixelFormat` (32-bit PFNC code) to a `viva_zenoh_api::PixelFormat`.
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
        _ => ZenohPixelFormat::Unknown,
    }
}

/// How many payload bytes a frame of this geometry should carry, if we can say.
///
/// Sized from the **PFNC** code, never from the Zenoh projection.
/// [`pfnc_to_zenoh`] collapses every format the wire enum cannot name down to
/// `Unknown`, and `ZenohPixelFormat::Unknown::bytes_per_pixel()` answers `1.0` —
/// so a `Coord3D_ABC32f` profile used to size at one twelfth of itself and be
/// trimmed to that, published as valid with a single warning and silence
/// afterwards (backlog `DC-01`). The camera's own format is the authority here;
/// what the bridge can name is a separate question, and a narrower one.
///
/// `None` means the frame cannot be length-checked at all — a packed format, or
/// an event stream whose leader fields are not image geometry. Callers must
/// publish such a payload unmodified: an expected length we cannot compute must
/// not become a length we enforce.
pub fn expected_payload_len(pf: pfnc::PixelFormat, width: u32, height: u32) -> Option<usize> {
    if matches!(
        pf,
        pfnc::PixelFormat::EvsEvt30 | pfnc::PixelFormat::EvsEvt21
    ) {
        return None;
    }
    pf.bytes_per_pixel()
        .map(|bpp| width as usize * height as usize * bpp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_3d_profile_is_sized_from_its_pfnc_code_not_the_zenoh_projection() {
        let pf = pfnc::PixelFormat::Coord3DABC32f;
        // The bridge cannot name it...
        assert_eq!(pfnc_to_zenoh(pf), ZenohPixelFormat::Unknown);
        assert_eq!(ZenohPixelFormat::Unknown.bytes_per_pixel(), 1.0);
        // ...but it is still twelve bytes per pixel, not one.
        assert_eq!(expected_payload_len(pf, 1280, 1), Some(15_360));
    }

    #[test]
    fn a_named_format_is_sized_as_before() {
        assert_eq!(
            expected_payload_len(pfnc::PixelFormat::Mono8, 640, 480),
            Some(307_200)
        );
        assert_eq!(
            expected_payload_len(pfnc::PixelFormat::BayerRG16, 640, 480),
            Some(614_400)
        );
    }

    /// `Mono12Packed` is offered by eleven of the 37 vendor-corpus documents.
    /// There is no integer byte size for it, so there is no length to check.
    #[test]
    fn a_packed_format_cannot_be_length_checked() {
        let packed = pfnc::PixelFormat::from_code(0x010C_0006);
        assert!(matches!(packed, pfnc::PixelFormat::Unknown(_)));
        assert_eq!(expected_payload_len(packed, 640, 480), None);
    }

    #[test]
    fn event_blocks_are_not_sized_as_images() {
        // The TRT009S-E reports 1 x 64000 in its leader, while the trailer's
        // Size Y is the variable encoded payload length. Neither event format
        // may be validated as width * height * event-word-size.
        assert_eq!(
            expected_payload_len(pfnc::PixelFormat::EvsEvt30, 1, 64_000),
            None
        );
        assert_eq!(
            expected_payload_len(pfnc::PixelFormat::EvsEvt21, 1, 64_000),
            None
        );
    }
}
