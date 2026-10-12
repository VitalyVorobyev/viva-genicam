//! GigE Vision's status codes: the table GVCP acknowledgements and GVSP
//! packets share.
//!
//! The codes GenCP defines too are [`viva_gencp::StatusCode`], wrapped as
//! [`GigeStatus::Common`]; the rest are GigE Vision's own. `0x800B` is the
//! reason the two are not one table: GigE Vision calls it `NO_MSG`, GenCP
//! `MSG_TIMEOUT` (ADR-0020).
//!
//! Values from aravis's `ArvGvspPacketStatus` (`arvgvspprivate.h`), which
//! lists the GigE Vision table and marks what was deprecated or added in
//! which version, cross-checked against Wireshark's `GEV_STATUS_*`
//! (`packet-gvcp.c`). The GigE Vision standard itself is not freely
//! available.

viva_gencp::transport_status! {
    /// A GigE Vision status: a GVCP acknowledgement's, or a GVSP packet's.
    pub enum GigeStatus {
        /// A GVSP packet the device sent again, in answer to a resend
        /// request. Not an error.
        PacketResend = 0x0100, "PACKET_RESEND";
        /// An internal problem in the device (deprecated).
        LocalProblem = 0x8008, "LOCAL_PROBLEM";
        /// The acknowledgement does not match the command (deprecated).
        MsgMismatch = 0x8009, "MSG_MISMATCH";
        /// The command is not of a protocol the device supports
        /// (deprecated).
        InvalidProtocol = 0x800A, "INVALID_PROTOCOL";
        /// No message (deprecated). GenCP's `MSG_TIMEOUT` is the same number.
        NoMsg = 0x800B, "NO_MSG";
        /// The packet asked for again is no longer available.
        PacketUnavailable = 0x800C, "PACKET_UNAVAILABLE";
        /// The device's internal memory overran: data was lost.
        DataOverrun = 0x800D, "DATA_OVERRUN";
        /// The packet asked for again has not been sent yet.
        PacketNotYetAvailable = 0x8010, "PACKET_NOT_YET_AVAILABLE";
        /// The packet asked for again, and every packet before it, has been
        /// removed from the device's memory.
        PacketAndPreviousRemovedFromMemory = 0x8011, "PACKET_AND_PREV_REMOVED_FROM_MEMORY";
        /// The packet asked for again has been removed from the device's
        /// memory.
        PacketRemovedFromMemory = 0x8012, "PACKET_REMOVED_FROM_MEMORY";
        /// The device has no time reference for a scheduled action (GigE
        /// Vision 2.0).
        NoRefTime = 0x8013, "NO_REF_TIME";
        /// The packet asked for again cannot be sent just now (GigE Vision
        /// 2.0).
        PacketTemporarilyUnavailable = 0x8014, "PACKET_TEMPORARILY_UNAVAILABLE";
        /// The device's queue overflowed (GigE Vision 2.0).
        Overflow = 0x8015, "OVERFLOW";
        /// A scheduled action arrived after its time (GigE Vision 2.0).
        ActionLate = 0x8016, "ACTION_LATE";
        /// A leader or trailer did not fit the packet size (GigE Vision 2.1).
        LeaderTrailerOverflow = 0x8017, "LEADER_TRAILER_OVERFLOW";
    }
}

impl GigeStatus {
    /// Whether a GVSP packet with this status is in error: bit 15 set, as
    /// the standard marks every error code.
    pub const fn is_error(self) -> bool {
        self.to_raw() & 0x8000 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viva_gencp::StatusCode;

    /// The GigE Vision table, written as literals from aravis's
    /// `ArvGvspPacketStatus` rather than from `to_raw` (ADR-0019).
    const TABLE: &[(u16, &str)] = &[
        (0x0000, "SUCCESS"),
        (0x0100, "PACKET_RESEND"),
        (0x8001, "NOT_IMPLEMENTED"),
        (0x8002, "INVALID_PARAMETER"),
        (0x8003, "INVALID_ADDRESS"),
        (0x8004, "WRITE_PROTECT"),
        (0x8005, "BAD_ALIGNMENT"),
        (0x8006, "ACCESS_DENIED"),
        (0x8007, "BUSY"),
        (0x8008, "LOCAL_PROBLEM"),
        (0x8009, "MSG_MISMATCH"),
        (0x800A, "INVALID_PROTOCOL"),
        (0x800B, "NO_MSG"),
        (0x800C, "PACKET_UNAVAILABLE"),
        (0x800D, "DATA_OVERRUN"),
        (0x800E, "INVALID_HEADER"),
        (0x800F, "WRONG_CONFIG"),
        (0x8010, "PACKET_NOT_YET_AVAILABLE"),
        (0x8011, "PACKET_AND_PREV_REMOVED_FROM_MEMORY"),
        (0x8012, "PACKET_REMOVED_FROM_MEMORY"),
        (0x8013, "NO_REF_TIME"),
        (0x8014, "PACKET_TEMPORARILY_UNAVAILABLE"),
        (0x8015, "OVERFLOW"),
        (0x8016, "ACTION_LATE"),
        (0x8017, "LEADER_TRAILER_OVERFLOW"),
        (0x8FFF, "ERROR"),
    ];

    #[test]
    fn every_code_in_the_table_is_named_and_round_trips() {
        for &(raw, name) in TABLE {
            let status = GigeStatus::from_raw(raw);
            assert_eq!(status.name(), name, "{raw:#06x}");
            assert_eq!(status.to_raw(), raw, "{name}");
            assert_eq!(status.to_string(), format!("{name} (0x{raw:04X})"));
        }
    }

    #[test]
    fn the_shared_core_is_genicams() {
        assert_eq!(
            GigeStatus::from_raw(0x8006),
            GigeStatus::Common(StatusCode::AccessDenied)
        );
        assert_eq!(GigeStatus::from_raw(0x800B).common(), None);
        assert_eq!(GigeStatus::from_raw(0x8018), GigeStatus::Unknown(0x8018));
        assert_eq!(
            GigeStatus::Unknown(0x8018).to_string(),
            "unknown status (0x8018)"
        );
    }

    #[test]
    fn an_error_has_bit_15_set() {
        assert!(GigeStatus::from_raw(0x800C).is_error());
        assert!(!GigeStatus::PacketResend.is_error());
        assert!(!GigeStatus::from_raw(0).is_error());
    }
}
