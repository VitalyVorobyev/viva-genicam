//! The status codes a USB3 Vision device answers with: GenCP's table, and
//! USB3 Vision's own codes.
//!
//! The codes GigE Vision defines too are [`viva_gencp::StatusCode`], wrapped
//! as [`U3vStatus::Common`]. `0x800B` is the reason the two transports have
//! a table each: GenCP calls it `MSG_TIMEOUT`, GigE Vision `NO_MSG`
//! (ADR-0020).
//!
//! GenCP 1.2 Table 6 is free; the USB3 Vision codes (`0xA001`–`0xA101`) are
//! from aravis's `ArvUvcpStatus` (`arvuvcpprivate.h`), since the USB3 Vision
//! standard is not freely available.

viva_gencp::transport_status! {
    /// A USB3 Vision acknowledgement's status.
    pub enum U3vStatus {
        /// GenCP: a timeout waiting for an acknowledge. GigE Vision's
        /// `NO_MSG` is the same number.
        MsgTimeout = 0x800B, "MSG_TIMEOUT";
        /// USB3 Vision: the device cannot resend.
        ResendNotSupported = 0xA001, "RESEND_NOT_SUPPORTED";
        /// USB3 Vision: the stream endpoint is halted.
        DsiEndpointHalted = 0xA002, "DSI_ENDPOINT_HALTED";
        /// USB3 Vision: the payload size is not a multiple of the transfer
        /// alignment the device requires.
        SiPayloadSizeNotAligned = 0xA003, "SI_PAYLOAD_SIZE_NOT_ALIGNED";
        /// USB3 Vision: the streaming interface registers contradict each
        /// other.
        SiRegistersInconsistent = 0xA004, "SI_REGISTERS_INCONSISTENT";
        /// USB3 Vision: data was discarded.
        DataDiscarded = 0xA100, "DATA_DISCARDED";
        /// USB3 Vision: the device's buffer overran.
        DataOverrun = 0xA101, "DATA_OVERRUN";
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viva_gencp::StatusCode;

    /// GenCP 1.2 Table 6 and aravis's `ArvUvcpStatus`, as literals rather
    /// than from `to_raw` (ADR-0019).
    const TABLE: &[(u16, &str)] = &[
        (0x0000, "SUCCESS"),
        (0x8001, "NOT_IMPLEMENTED"),
        (0x8002, "INVALID_PARAMETER"),
        (0x8003, "INVALID_ADDRESS"),
        (0x8004, "WRITE_PROTECT"),
        (0x8005, "BAD_ALIGNMENT"),
        (0x8006, "ACCESS_DENIED"),
        (0x8007, "BUSY"),
        (0x800B, "MSG_TIMEOUT"),
        (0x800E, "INVALID_HEADER"),
        (0x800F, "WRONG_CONFIG"),
        (0x8FFF, "ERROR"),
        (0xA001, "RESEND_NOT_SUPPORTED"),
        (0xA002, "DSI_ENDPOINT_HALTED"),
        (0xA003, "SI_PAYLOAD_SIZE_NOT_ALIGNED"),
        (0xA004, "SI_REGISTERS_INCONSISTENT"),
        (0xA100, "DATA_DISCARDED"),
        (0xA101, "DATA_OVERRUN"),
    ];

    #[test]
    fn every_code_in_the_table_is_named_and_round_trips() {
        for &(raw, name) in TABLE {
            let status = U3vStatus::from_raw(raw);
            assert_eq!(status.name(), name, "{raw:#06x}");
            assert_eq!(status.to_raw(), raw, "{name}");
        }
    }

    /// GigE Vision's packet-resend family is not GenCP's.
    #[test]
    fn gige_codes_are_unknown_here() {
        assert_eq!(U3vStatus::from_raw(0x800C), U3vStatus::Unknown(0x800C));
        assert_eq!(
            U3vStatus::from_raw(0x8004),
            U3vStatus::Common(StatusCode::WriteProtect)
        );
    }
}
