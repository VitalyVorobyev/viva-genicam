//! The GenTL C interface's vocabulary: error codes, info commands, enumeration
//! values, structures and handle types.
//!
//! Transcribed from the EMVA GenICam GenTL Standard v1.6 (§6.2–§6.7) and its
//! `GenTL.h`, which is not vendored (ADR-0031). Every value here is the
//! standard's; the names follow the header's, so a reader can grep one
//! against the other. Enumerations are `int32_t` in the header (§6.2.1.3), so
//! they are `i32` constants here, not Rust enums: a consumer may pass any
//! value, and an unknown one must be answered, not undefined behaviour.

use std::ffi::c_void;

/// Define the header's constants, each documented with its name and the
/// section of the standard that lists it.
macro_rules! constants {
    ($section:literal, $ty:ty; $($name:ident = $value:expr),* $(,)?) => {
        $(
            #[doc = concat!("`", stringify!($name), "` (GenTL 1.6 ", $section, ").")]
            pub const $name: $ty = $value;
        )*
    };
}

// ---------------------------------------------------------------------------
// §6.2 Data types
// ---------------------------------------------------------------------------

/// `GC_ERROR`: every function's return value (§6.2.1.1).
pub type GcError = i32;
/// `bool8_t` (§6.2.1.5): 0 is false, anything else true.
pub type Bool8 = u8;

/// `TL_HANDLE`, `IF_HANDLE`, `DEV_HANDLE`, `DS_HANDLE`, `PORT_HANDLE`,
/// `BUFFER_HANDLE`, `EVENTSRC_HANDLE` and `EVENT_HANDLE` are all `void *`
/// (§6.2.1.2).
pub type Handle = *mut c_void;

/// `GENTL_INVALID_HANDLE` (§6.7).
pub const GENTL_INVALID_HANDLE: Handle = std::ptr::null_mut();
/// `GENTL_INFINITE` (§6.7): an infinite timeout or count.
pub const GENTL_INFINITE: u64 = 0xFFFF_FFFF_FFFF_FFFF;

// ---------------------------------------------------------------------------
// §6.1.5 Error codes (Table 6-4)
// ---------------------------------------------------------------------------

constants! { "§6.1.5", GcError;
    GC_ERR_SUCCESS = 0,
    GC_ERR_ERROR = -1001,
    GC_ERR_NOT_INITIALIZED = -1002,
    GC_ERR_NOT_IMPLEMENTED = -1003,
    GC_ERR_RESOURCE_IN_USE = -1004,
    GC_ERR_ACCESS_DENIED = -1005,
    GC_ERR_INVALID_HANDLE = -1006,
    GC_ERR_INVALID_ID = -1007,
    GC_ERR_NO_DATA = -1008,
    GC_ERR_INVALID_PARAMETER = -1009,
    GC_ERR_IO = -1010,
    GC_ERR_TIMEOUT = -1011,
    GC_ERR_ABORT = -1012,
    GC_ERR_INVALID_BUFFER = -1013,
    GC_ERR_NOT_AVAILABLE = -1014,
    GC_ERR_INVALID_ADDRESS = -1015,
    GC_ERR_BUFFER_TOO_SMALL = -1016,
    GC_ERR_INVALID_INDEX = -1017,
    GC_ERR_PARSING_CHUNK_DATA = -1018,
    GC_ERR_INVALID_VALUE = -1019,
    GC_ERR_RESOURCE_EXHAUSTED = -1020,
    GC_ERR_OUT_OF_MEMORY = -1021,
    GC_ERR_BUSY = -1022,
    GC_ERR_AMBIGUOUS = -1023,
    GC_ERR_CUSTOM_ID = -10000,
}

// ---------------------------------------------------------------------------
// §6.4.1 Library and system enumerations
// ---------------------------------------------------------------------------

/// `INFO_DATATYPE` (§6.4.1.1).
pub type InfoDatatype = i32;
constants! { "§6.4.1", InfoDatatype;
    INFO_DATATYPE_UNKNOWN = 0,
    INFO_DATATYPE_STRING = 1,
    INFO_DATATYPE_STRINGLIST = 2,
    INFO_DATATYPE_INT16 = 3,
    INFO_DATATYPE_UINT16 = 4,
    INFO_DATATYPE_INT32 = 5,
    INFO_DATATYPE_UINT32 = 6,
    INFO_DATATYPE_INT64 = 7,
    INFO_DATATYPE_UINT64 = 8,
    INFO_DATATYPE_FLOAT64 = 9,
    INFO_DATATYPE_PTR = 10,
    INFO_DATATYPE_BOOL8 = 11,
    INFO_DATATYPE_SIZET = 12,
    INFO_DATATYPE_BUFFER = 13,
    INFO_DATATYPE_PTRDIFF = 14,
}

// `TL_CHAR_ENCODING` (§6.4.1.2).
constants! { "§6.4.1", i32;
    TL_CHAR_ENCODING_ASCII = 0,
    TL_CHAR_ENCODING_UTF8 = 1,
}

/// `TL_INFO_CMD` (§6.4.1.3).
pub type TlInfoCmd = i32;
constants! { "§6.4.1", TlInfoCmd;
    TL_INFO_ID = 0,
    TL_INFO_VENDOR = 1,
    TL_INFO_MODEL = 2,
    TL_INFO_VERSION = 3,
    TL_INFO_TLTYPE = 4,
    TL_INFO_NAME = 5,
    TL_INFO_PATHNAME = 6,
    TL_INFO_DISPLAYNAME = 7,
    TL_INFO_CHAR_ENCODING = 8,
    TL_INFO_GENTL_VER_MAJOR = 9,
    TL_INFO_GENTL_VER_MINOR = 10,
}

// ---------------------------------------------------------------------------
// §6.4.2 Interface enumerations
// ---------------------------------------------------------------------------

/// `INTERFACE_INFO_CMD` (§6.4.2.1).
pub type InterfaceInfoCmd = i32;
constants! { "§6.4.2", InterfaceInfoCmd;
    INTERFACE_INFO_ID = 0,
    INTERFACE_INFO_DISPLAYNAME = 1,
    INTERFACE_INFO_TLTYPE = 2,
}

// ---------------------------------------------------------------------------
// §6.4.3 Device enumerations
// ---------------------------------------------------------------------------

/// `DEVICE_ACCESS_FLAGS` (§6.4.3.1).
pub type DeviceAccessFlags = i32;
constants! { "§6.4.3", DeviceAccessFlags;
    DEVICE_ACCESS_UNKNOWN = 0,
    DEVICE_ACCESS_NONE = 1,
    DEVICE_ACCESS_READONLY = 2,
    DEVICE_ACCESS_CONTROL = 3,
    DEVICE_ACCESS_EXCLUSIVE = 4,
}

/// `DEVICE_ACCESS_STATUS` (§6.4.3.2).
pub type DeviceAccessStatus = i32;
constants! { "§6.4.3", DeviceAccessStatus;
    DEVICE_ACCESS_STATUS_UNKNOWN = 0,
    DEVICE_ACCESS_STATUS_READWRITE = 1,
    DEVICE_ACCESS_STATUS_READONLY = 2,
    DEVICE_ACCESS_STATUS_NOACCESS = 3,
    DEVICE_ACCESS_STATUS_BUSY = 4,
    DEVICE_ACCESS_STATUS_OPEN_READWRITE = 5,
    DEVICE_ACCESS_STATUS_OPEN_READONLY = 6,
}

/// `DEVICE_INFO_CMD` (§6.4.3.3).
pub type DeviceInfoCmd = i32;
constants! { "§6.4.3", DeviceInfoCmd;
    DEVICE_INFO_ID = 0,
    DEVICE_INFO_VENDOR = 1,
    DEVICE_INFO_MODEL = 2,
    DEVICE_INFO_TLTYPE = 3,
    DEVICE_INFO_DISPLAYNAME = 4,
    DEVICE_INFO_ACCESS_STATUS = 5,
    DEVICE_INFO_USER_DEFINED_NAME = 6,
    DEVICE_INFO_SERIAL_NUMBER = 7,
    DEVICE_INFO_VERSION = 8,
    DEVICE_INFO_TIMESTAMP_FREQUENCY = 9,
}

// ---------------------------------------------------------------------------
// §6.4.4 Data stream enumerations
// ---------------------------------------------------------------------------

/// `ACQ_QUEUE_TYPE` (§6.4.4.1).
pub type AcqQueueType = i32;
constants! { "§6.4.4", AcqQueueType;
    ACQ_QUEUE_INPUT_TO_OUTPUT = 0,
    ACQ_QUEUE_OUTPUT_DISCARD = 1,
    ACQ_QUEUE_ALL_TO_INPUT = 2,
    ACQ_QUEUE_UNQUEUED_TO_INPUT = 3,
    ACQ_QUEUE_ALL_DISCARD = 4,
}

/// `ACQ_START_FLAGS` (§6.4.4.2).
pub type AcqStartFlags = i32;
constants! { "§6.4.4", AcqStartFlags;
    ACQ_START_FLAGS_DEFAULT = 0,
}

/// `ACQ_STOP_FLAGS` (§6.4.4.3).
pub type AcqStopFlags = i32;
constants! { "§6.4.4", AcqStopFlags;
    ACQ_STOP_FLAGS_DEFAULT = 0,
    ACQ_STOP_FLAGS_KILL = 1,
}

/// `BUFFER_INFO_CMD` (§6.4.4.4).
pub type BufferInfoCmd = i32;
constants! { "§6.4.4", BufferInfoCmd;
    BUFFER_INFO_BASE = 0,
    BUFFER_INFO_SIZE = 1,
    BUFFER_INFO_USER_PTR = 2,
    BUFFER_INFO_TIMESTAMP = 3,
    BUFFER_INFO_NEW_DATA = 4,
    BUFFER_INFO_IS_QUEUED = 5,
    BUFFER_INFO_IS_ACQUIRING = 6,
    BUFFER_INFO_IS_INCOMPLETE = 7,
    BUFFER_INFO_TLTYPE = 8,
    BUFFER_INFO_SIZE_FILLED = 9,
    BUFFER_INFO_WIDTH = 10,
    BUFFER_INFO_HEIGHT = 11,
    BUFFER_INFO_XOFFSET = 12,
    BUFFER_INFO_YOFFSET = 13,
    BUFFER_INFO_XPADDING = 14,
    BUFFER_INFO_YPADDING = 15,
    BUFFER_INFO_FRAMEID = 16,
    BUFFER_INFO_IMAGEPRESENT = 17,
    BUFFER_INFO_IMAGEOFFSET = 18,
    BUFFER_INFO_PAYLOADTYPE = 19,
    BUFFER_INFO_PIXELFORMAT = 20,
    BUFFER_INFO_PIXELFORMAT_NAMESPACE = 21,
    BUFFER_INFO_DELIVERED_IMAGEHEIGHT = 22,
    BUFFER_INFO_DELIVERED_CHUNKPAYLOADSIZE = 23,
    BUFFER_INFO_CHUNKLAYOUTID = 24,
    BUFFER_INFO_FILENAME = 25,
    BUFFER_INFO_PIXEL_ENDIANNESS = 26,
    BUFFER_INFO_DATA_SIZE = 27,
    BUFFER_INFO_TIMESTAMP_NS = 28,
    BUFFER_INFO_DATA_LARGER_THAN_BUFFER = 29,
    BUFFER_INFO_CONTAINS_CHUNKDATA = 30,
    BUFFER_INFO_IS_COMPOSITE = 31,
}

// `PAYLOADTYPE_INFO_IDS` (§6.4.4.5).
constants! { "§6.4.4", usize;
    PAYLOAD_TYPE_UNKNOWN = 0,
    PAYLOAD_TYPE_IMAGE = 1,
    PAYLOAD_TYPE_RAW_DATA = 2,
    PAYLOAD_TYPE_FILE = 3,
    PAYLOAD_TYPE_CHUNK_DATA = 4,
    PAYLOAD_TYPE_JPEG = 5,
    PAYLOAD_TYPE_JPEG2000 = 6,
    PAYLOAD_TYPE_H264 = 7,
    PAYLOAD_TYPE_CHUNK_ONLY = 8,
    PAYLOAD_TYPE_DEVICE_SPECIFIC = 9,
    PAYLOAD_TYPE_MULTI_PART = 10,
    PAYLOAD_TYPE_GENDC = 11,
}

// `PIXELFORMAT_NAMESPACE_IDS` (§6.4.4.6).
constants! { "§6.4.4", u64;
    PIXELFORMAT_NAMESPACE_UNKNOWN = 0,
    PIXELFORMAT_NAMESPACE_GEV = 1,
    PIXELFORMAT_NAMESPACE_IIDC = 2,
    PIXELFORMAT_NAMESPACE_PFNC_16BIT = 3,
    PIXELFORMAT_NAMESPACE_PFNC_32BIT = 4,
}

// `PIXELENDIANNESS_IDS` (§6.4.4.7).
constants! { "§6.4.4", i32;
    PIXELENDIANNESS_UNKNOWN = 0,
    PIXELENDIANNESS_LITTLE = 1,
    PIXELENDIANNESS_BIG = 2,
}

/// `STREAM_INFO_CMD` (§6.4.4.8).
pub type StreamInfoCmd = i32;
constants! { "§6.4.4", StreamInfoCmd;
    STREAM_INFO_ID = 0,
    STREAM_INFO_NUM_DELIVERED = 1,
    STREAM_INFO_NUM_UNDERRUN = 2,
    STREAM_INFO_NUM_ANNOUNCED = 3,
    STREAM_INFO_NUM_QUEUED = 4,
    STREAM_INFO_NUM_AWAIT_DELIVERY = 5,
    STREAM_INFO_NUM_STARTED = 6,
    STREAM_INFO_PAYLOAD_SIZE = 7,
    STREAM_INFO_IS_GRABBING = 8,
    STREAM_INFO_DEFINES_PAYLOADSIZE = 9,
    STREAM_INFO_TLTYPE = 10,
    STREAM_INFO_NUM_CHUNKS_MAX = 11,
    STREAM_INFO_BUF_ANNOUNCE_MIN = 12,
    STREAM_INFO_BUF_ALIGNMENT = 13,
    STREAM_INFO_FLOW_TABLE = 14,
    STREAM_INFO_GENDC_PREFETCH_DESCRIPTOR = 15,
}

/// `BUFFER_PART_INFO_CMD` (§6.4.4.9).
pub type BufferPartInfoCmd = i32;
/// `FLOW_INFO_CMD` (§6.4.4.11).
pub type FlowInfoCmd = i32;
/// `SEGMENT_INFO_CMD` (§6.4.4.12).
pub type SegmentInfoCmd = i32;

// ---------------------------------------------------------------------------
// §6.4.5 Port enumerations
// ---------------------------------------------------------------------------

/// `PORT_INFO_CMD` (§6.4.5.1).
pub type PortInfoCmd = i32;
constants! { "§6.4.5", PortInfoCmd;
    PORT_INFO_ID = 0,
    PORT_INFO_VENDOR = 1,
    PORT_INFO_MODEL = 2,
    PORT_INFO_TLTYPE = 3,
    PORT_INFO_MODULE = 4,
    PORT_INFO_LITTLE_ENDIAN = 5,
    PORT_INFO_BIG_ENDIAN = 6,
    PORT_INFO_ACCESS_READ = 7,
    PORT_INFO_ACCESS_WRITE = 8,
    PORT_INFO_ACCESS_NA = 9,
    PORT_INFO_ACCESS_NI = 10,
    PORT_INFO_VERSION = 11,
    PORT_INFO_PORTNAME = 12,
}

// `URL_SCHEME_IDS` (§6.4.5.3).
constants! { "§6.4.5", i32;
    URL_SCHEME_LOCAL = 0,
    URL_SCHEME_HTTP = 1,
    URL_SCHEME_FILE = 2,
}

/// `URL_INFO_CMD` (§6.4.5.2).
pub type UrlInfoCmd = i32;
constants! { "§6.4.5", UrlInfoCmd;
    URL_INFO_URL = 0,
    URL_INFO_SCHEMA_VER_MAJOR = 1,
    URL_INFO_SCHEMA_VER_MINOR = 2,
    URL_INFO_FILE_VER_MAJOR = 3,
    URL_INFO_FILE_VER_MINOR = 4,
    URL_INFO_FILE_VER_SUBMINOR = 5,
    URL_INFO_FILE_SHA1_HASH = 6,
    URL_INFO_FILE_REGISTER_ADDRESS = 7,
    URL_INFO_FILE_SIZE = 8,
    URL_INFO_SCHEME = 9,
    URL_INFO_FILENAME = 10,
}

// ---------------------------------------------------------------------------
// §6.4.6 Signalling enumerations
// ---------------------------------------------------------------------------

/// `EVENT_TYPE` (§6.4.6.3).
pub type EventType = i32;
constants! { "§6.4.6", EventType;
    EVENT_ERROR = 0,
    EVENT_NEW_BUFFER = 1,
    EVENT_FEATURE_INVALIDATE = 2,
    EVENT_FEATURE_CHANGE = 3,
    EVENT_REMOTE_DEVICE = 4,
    EVENT_MODULE = 5,
}

/// `EVENT_INFO_CMD` (§6.4.6.2).
pub type EventInfoCmd = i32;
constants! { "§6.4.6", EventInfoCmd;
    EVENT_EVENT_TYPE = 0,
    EVENT_NUM_IN_QUEUE = 1,
    EVENT_NUM_FIRED = 2,
    EVENT_SIZE_MAX = 3,
    EVENT_INFO_DATA_SIZE_MAX = 4,
}

/// `EVENT_DATA_INFO_CMD` (§6.4.6.1).
pub type EventDataInfoCmd = i32;
constants! { "§6.4.6", EventDataInfoCmd;
    EVENT_DATA_ID = 0,
    EVENT_DATA_VALUE = 1,
    EVENT_DATA_NUMID = 2,
}

// ---------------------------------------------------------------------------
// §6.5 Structures — "byte aligned" (`#pragma pack(push, 1)` in the header)
// ---------------------------------------------------------------------------

/// `EVENT_NEW_BUFFER_DATA` (§6.5.2.1).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct EventNewBufferData {
    /// `BufferHandle`: the buffer that holds new data.
    pub buffer_handle: Handle,
    /// `pUserPointer`: the private pointer given when the buffer was announced.
    pub user_pointer: *mut c_void,
}

/// `PORT_REGISTER_STACK_ENTRY` (§6.5.3.1).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct PortRegisterStackEntry {
    /// `Address`: the register address.
    pub address: u64,
    /// `pBuffer`: the data read or to be written.
    pub buffer: *mut c_void,
    /// `Size`: bytes to read or write.
    pub size: usize,
}

/// `SINGLE_CHUNK_DATA` (§6.5.1.1).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SingleChunkData {
    /// `ChunkID`: the chunk's numeric ID.
    pub chunk_id: u64,
    /// `ChunkOffset`: where the chunk's data starts in the buffer.
    pub chunk_offset: isize,
    /// `ChunkLength`: the chunk data's size in bytes.
    pub chunk_length: usize,
}

/// `DS_BUFFER_INFO_STACKED` (§6.5.1.2).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DsBufferInfoStacked {
    /// `iInfoCmd` (in): the buffer info to retrieve.
    pub info_cmd: BufferInfoCmd,
    /// `iType` (out): the info's data type.
    pub info_type: InfoDatatype,
    /// `pBuffer` (in, out): where the info goes, or null to query its size.
    pub buffer: *mut c_void,
    /// `iSize` (in, out): the size of `buffer`, then of the info.
    pub size: usize,
    /// `iResult` (out): this query's result.
    pub result: GcError,
}

/// `DS_BUFFER_PART_INFO_STACKED` (§6.5.1.3).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DsBufferPartInfoStacked {
    /// `iPartIndex` (in): the buffer part to query.
    pub part_index: u32,
    /// `iInfoCmd` (in): the part info to retrieve.
    pub info_cmd: BufferPartInfoCmd,
    /// `iResult` (out): this query's result.
    pub result: GcError,
    /// `iType` (out): the info's data type.
    pub info_type: InfoDatatype,
    /// `pBuffer` (in, out): where the info goes, or null to query its size.
    pub buffer: *mut c_void,
    /// `iSize` (in, out): the size of `buffer`, then of the info.
    pub size: usize,
}

// The packed layouts on the 64-bit targets the producer is built for.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::size_of::<EventNewBufferData>() == 16);
    assert!(std::mem::size_of::<PortRegisterStackEntry>() == 24);
    assert!(std::mem::size_of::<SingleChunkData>() == 24);
    assert!(std::mem::size_of::<DsBufferInfoStacked>() == 28);
    assert!(std::mem::size_of::<DsBufferPartInfoStacked>() == 32);
};

// ---------------------------------------------------------------------------
// §6.6 String constants, and the module names of the header
// ---------------------------------------------------------------------------

/// `TLTypeGEVName` (§6.6.1).
pub const TL_TYPE_GEV: &str = "GEV";
/// `TLSystemModuleName`.
pub const MODULE_SYSTEM: &str = "TLSystem";
/// `TLInterfaceModuleName`.
pub const MODULE_INTERFACE: &str = "TLInterface";
/// `TLDeviceModuleName`.
pub const MODULE_DEVICE: &str = "TLDevice";
/// `TLDataStreamModuleName`.
pub const MODULE_DATA_STREAM: &str = "TLDataStream";
/// `TLRemoteDeviceModuleName`.
pub const MODULE_REMOTE_DEVICE: &str = "Device";

/// The GenTL standard version this producer implements
/// (`TL_INFO_GENTL_VER_MAJOR` / `_MINOR`).
pub const GENTL_VERSION: (u32, u32) = (1, 6);
