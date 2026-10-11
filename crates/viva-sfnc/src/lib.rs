#![cfg_attr(docsrs, feature(doc_cfg))]
//! Standard Feature Naming Convention (SFNC) helpers.

#![allow(dead_code)]

/// Exposure time feature name (`ExposureTime`).
pub const EXPOSURE_TIME: &str = "ExposureTime";
/// The names cameras give the exposure time, in the order to try them.
///
/// `ExposureTime` is SFNC 2's `<Float>` in microseconds; `ExposureTimeAbs`,
/// SFNC 1's, is the same in microseconds; `ExposureTimeRaw` is an `<Integer>`
/// in device units. Of the 38 vendor-corpus documents, 21 declare
/// `ExposureTime`, 12 only `ExposureTimeAbs` (one of them as an `<Integer>`),
/// 2 only `ExposureTimeRaw`, and 3 none of them.
pub const EXPOSURE_TIME_NAMES: &[&str] = &["ExposureTime", "ExposureTimeAbs", "ExposureTimeRaw"];
/// Gain feature name (`Gain`).
pub const GAIN: &str = "Gain";
/// The names cameras give the gain, in the order to try them.
///
/// `Gain` is SFNC 2's `<Float>` in dB; `GainRaw` an `<Integer>` in device
/// units. Of the 38 vendor-corpus documents, 25 declare `Gain` (one as an
/// `<Integer>`), 10 only `GainRaw` (one as a `<Float>`) and 3 neither; three
/// documents name a category `Gain`, which is not the feature. `GainAbs`
/// appears only beside `Gain`, so it would never be chosen and is not listed.
pub const GAIN_NAMES: &[&str] = &["Gain", "GainRaw"];
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
/// The names cameras give the acquisition frame rate, in the order to try
/// them: SFNC 2's `AcquisitionFrameRate` (22 corpus documents), then SFNC 1's
/// `AcquisitionFrameRateAbs` (9 more, 7 of them a `<Converter>`). 7 declare
/// neither.
pub const FRAME_RATE_NAMES: &[&str] = &["AcquisitionFrameRate", "AcquisitionFrameRateAbs"];
/// The names cameras give the switch that makes the frame rate apply, in the
/// order to try them: `AcquisitionFrameRateEnable` (14 corpus documents),
/// then `AcquisitionFrameRateEnabled` (5 more, Point Grey and FLIR). 19
/// declare neither: their frame rate applies as written.
pub const FRAME_RATE_ENABLE_NAMES: &[&str] =
    &["AcquisitionFrameRateEnable", "AcquisitionFrameRateEnabled"];
/// Trigger selector enumeration feature name (`TriggerSelector`).
pub const TRIGGER_SELECTOR: &str = "TriggerSelector";
/// Trigger mode enumeration feature name (`TriggerMode`): `On` or `Off` in
/// every corpus document that has one.
pub const TRIGGER_MODE: &str = "TriggerMode";
/// Trigger source enumeration feature name (`TriggerSource`).
pub const TRIGGER_SOURCE: &str = "TriggerSource";
/// Software trigger command feature name (`TriggerSoftware`).
pub const TRIGGER_SOFTWARE: &str = "TriggerSoftware";
/// Image width feature name (`Width`).
pub const WIDTH: &str = "Width";
/// Image height feature name (`Height`).
pub const HEIGHT: &str = "Height";
/// Horizontal offset feature name (`OffsetX`).
pub const OFFSET_X: &str = "OffsetX";
/// Vertical offset feature name (`OffsetY`).
pub const OFFSET_Y: &str = "OffsetY";
/// Horizontal binning feature name (`BinningHorizontal`).
pub const BINNING_HORIZONTAL: &str = "BinningHorizontal";
/// Vertical binning feature name (`BinningVertical`).
pub const BINNING_VERTICAL: &str = "BinningVertical";
/// User set selector enumeration feature name (`UserSetSelector`).
pub const USER_SET_SELECTOR: &str = "UserSetSelector";
/// Command loading the selected user set (`UserSetLoad`).
pub const USER_SET_LOAD: &str = "UserSetLoad";
/// Command saving into the selected user set (`UserSetSave`).
pub const USER_SET_SAVE: &str = "UserSetSave";
/// The names cameras give the power-up user set, in the order to try them:
/// SFNC 2's `UserSetDefault` (13 corpus documents), then the older
/// `UserSetDefaultSelector` (22; 4 documents declare both).
pub const USER_SET_DEFAULT_NAMES: &[&str] = &["UserSetDefault", "UserSetDefaultSelector"];
/// `TLParamsLocked`: set while the host streams, so features that would
/// change the payload are locked. An `<Integer>` in 27 corpus documents and
/// a `<Boolean>` in 6, usually with no register behind it.
pub const TL_PARAMS_LOCKED: &str = "TLParamsLocked";
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
///
/// SFNC names it `GevSCDA` (25 documents in the vendor corpus). A multicast
/// address here is what makes a stream multicast: SFNC has no separate
/// "multicast enable" feature.
pub const SCP_DEST_ADDR: &[&str] = &["GevSCDA"];

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
