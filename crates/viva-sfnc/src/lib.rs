#![cfg_attr(docsrs, feature(doc_cfg))]
//! Standard Feature Naming Convention (SFNC) helpers.

#![allow(dead_code)]

/// Exposure time feature name (`ExposureTime`).
pub const EXPOSURE_TIME: &str = "ExposureTime";
/// Gain feature name (`Gain`).
pub const GAIN: &str = "Gain";
/// Gain selector feature name (`GainSelector`).
pub const GAIN_SELECTOR: &str = "GainSelector";
/// Pixel format feature name (`PixelFormat`).
pub const PIXEL_FORMAT: &str = "PixelFormat";
/// Chunk mode enable feature name (`ChunkModeActive`).
pub const CHUNK_MODE_ACTIVE: &str = "ChunkModeActive";
/// Chunk selector enumeration feature name (`ChunkSelector`).
pub const CHUNK_SELECTOR: &str = "ChunkSelector";
/// Chunk enable boolean feature name (`ChunkEnable`).
pub const CHUNK_ENABLE: &str = "ChunkEnable";
/// Acquisition start command feature name (`AcquisitionStart`).
pub const ACQUISITION_START: &str = "AcquisitionStart";
/// Acquisition stop command feature name (`AcquisitionStop`).
pub const ACQUISITION_STOP: &str = "AcquisitionStop";
/// Acquisition mode enumeration feature name (`AcquisitionMode`).
pub const ACQUISITION_MODE: &str = "AcquisitionMode";
/// Device temperature float feature name (`DeviceTemperature`).
pub const DEVICE_TEMPERATURE: &str = "DeviceTemperature";

/// Event selector enumeration feature name (`EventSelector`).
pub const EVENT_SELECTOR: &str = "EventSelector";
/// Event notification mode enumeration feature (`EventNotification`).
pub const EVENT_NOTIFICATION: &str = "EventNotification";
/// Event notification value used to enable delivery (`On`).
pub const EVENT_NOTIF_ON: &str = "On";

/// Message channel selector aliases ordered by preference.
pub const MSG_SEL: &[&str] = &["GevMessageChannelSelector", "MessageChannelSelector"];
/// Message channel IP address aliases ordered by preference.
pub const MSG_IP: &[&str] = &["GevMessageChannelIPAddress", "MessageChannelIPAddress"];
/// Message channel UDP port aliases ordered by preference.
pub const MSG_PORT: &[&str] = &["GevMessageChannelPort", "MessageChannelPort"];
/// Message channel enable aliases ordered by preference.
pub const MSG_EN: &[&str] = &["GevMessageChannelEnable", "MessageChannelEnable"];

/// Stream channel selector feature name (`GevStreamChannelSelector`).
pub const STREAM_CH_SELECTOR: &str = "GevStreamChannelSelector";
/// Stream channel destination UDP port aliases ordered by preference.
pub const SCP_HOST_PORT: &[&str] = &["GevSCPHostPort", "SCPHostPort"];
/// Stream channel destination address aliases ordered by preference.
pub const SCP_DEST_ADDR: &[&str] = &["GevSCPDA", "SCPDestinationAddress"];
/// Multicast enable aliases ordered by preference.
pub const MULTICAST_ENABLE: &[&str] = &["GevSCPSDoNotFragment", "GevMulticastEnable"];

/// Timestamp latch commands ordered by preference.
///
/// Different vendors expose the SFNC timestamp latch using slightly different
/// identifiers. Consumers should iterate the list and execute the first command
/// present in the nodemap.
pub const TS_LATCH_CMDS: &[&str] = &[
    "GevTimestampControlLatch",
    "TimestampControlLatch",
    "TimestampLatch",
];

/// Timestamp value nodes ordered by preference.
///
/// Reading the node immediately after executing the latch command returns the
/// device tick counter captured at the time of the latch.
pub const TS_VALUE_NODES: &[&str] = &["GevTimestampValue", "TimestampValue", "TimestampLatchValue"];

/// Timestamp frequency nodes ordered by preference.
///
/// Returns the number of device ticks per second when present. Some cameras
/// omit the SFNC name in favour of a shortened alias, hence the list of
/// fallbacks.
pub const TS_FREQ_NODES: &[&str] = &["GevTimestampTickFrequency", "TimestampTickFrequency"];

/// Timestamp reset commands ordered by preference.
///
/// Executing any of these commands restarts the device tick counter when the
/// camera supports it. Not every device exposes a reset capability.
pub const TS_RESET_CMDS: &[&str] = &[
    "GevTimestampControlReset",
    "TimestampControlReset",
    "TimestampReset",
];
