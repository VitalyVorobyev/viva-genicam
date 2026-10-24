#![cfg_attr(docsrs, feature(doc_cfg))]
//! GigE Vision transport: discovery and control (GVCP), streaming (GVSP),
//! events and action commands.
//!
//! The modules are private; what is public is re-exported here (ADR-0030).
//! [`consts`] is the one public namespace: GVCP's opcodes, bootstrap register
//! addresses and protocol limits, the vocabulary the fake camera shares.

mod action;
pub mod consts;
mod gvcp;
mod gvsp;
mod message;
mod nic;
mod status;
mod time;

pub use action::{AckSummary, ActionParams, send_action};
pub use gvcp::{
    DeviceInfo, DiscoveryOptions, FORCE_READMEM_ENV, GVCP_PORT, GigeDevice, GigeError,
    GvcpCapability, StreamParams, discover, discover_all, discover_ip, discover_on_interface,
    discover_with, encode_packet_resend, force_ip,
};
pub use gvsp::{
    ChunkError, ChunkRaw, GVSP_EXTENDED_HEADER_SIZE, GVSP_HEADER_SIZE, GvspContent, GvspError,
    GvspHeader, GvspLeader, GvspPacketRef, GvspTrailer, ImageLeader, PAYLOAD_TYPE_CHUNK_DATA,
    PAYLOAD_TYPE_EXTENDED_CHUNK, PAYLOAD_TYPE_EXTENDED_CHUNK_DATA, PAYLOAD_TYPE_IMAGE,
    PacketBitmap, StreamConfig, StreamDest, has_extended_chunks, is_chunk_data_payload,
    is_image_payload, parse_chunks, parse_header, parse_packet_ref,
};
pub use message::{EVENT_QUEUE_CAPACITY, EventPacket, EventSocket, EventStats};
pub use nic::{
    ASSUMED_MTU, DEFAULT_RCVBUF_BYTES, Iface, IfaceSelector, McOptions, Mtu, RecvBuffer,
    best_packet_size, bind_multicast, bind_udp, mtu, set_recv_buffer, wait_readable,
};
pub use status::GigeStatus;
pub use time::{DEFAULT_TIME_WINDOW, TimeSync};
