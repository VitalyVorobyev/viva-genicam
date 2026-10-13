//! Ports: where a register's address points (GA-12, ADR-0026).
//!
//! GenICam Standard v2.1.1 §2.8.16: every register reaches its bytes through a
//! `<Port>`, named by `<pPort>`. The device port forwards to the transport —
//! here the [`RegisterIo`] passed to each call. A port with a `<ChunkID>`
//! "does not give access to a real device, but rather to the chunk of data
//! residing in memory", and a port with an `<EventID>` likewise to the data an
//! event message carries. Their addresses count from the start of that chunk
//! or event, so reading one from device memory reads something else entirely:
//! GVCP bootstrap registers, or a `BAD_ALIGNMENT` refusal for a 2-byte read at
//! address 0x2 (#112).
//!
//! So a register on a chunk or event port reads only data attached to the
//! nodemap — [`NodeMap::attach_chunks`], [`NodeMap::attach_event`] — and is
//! an error, never a device read, when nothing is. Attaching or detaching
//! invalidates everything that depends on the port, so a cached value never
//! outlives the data it was read from. Writes to those ports are refused.

use std::collections::{HashMap, HashSet};

use bytes::Bytes;
use viva_genapi_xml::{Addressing, PortKind};

use super::NodeMap;
use crate::nodes::{Node, node_addressing};
use crate::{GenApiError, RegisterIo};

/// The chunks of one stream buffer, by chunk ID: what a nodemap's chunk ports
/// read.
///
/// Built by the transport layer, which knows the buffer's chunk layout —
/// `viva-genicam`'s `Frame` makes one from a GigE Vision payload. Each chunk
/// is a [`Bytes`] slice of the buffer, so building and attaching one copies no
/// image data.
///
/// When a buffer carries two chunks with the same ID, the first one pushed is
/// the one ports read.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ChunkData {
    chunks: Vec<(u64, Bytes)>,
}

impl ChunkData {
    /// No chunks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add the data of chunk `id`.
    pub fn push(&mut self, id: u64, data: Bytes) {
        self.chunks.push((id, data));
    }

    /// The data of chunk `id`, if the buffer carried one.
    pub fn get(&self, id: u64) -> Option<&Bytes> {
        self.chunks
            .iter()
            .find(|(chunk, _)| *chunk == id)
            .map(|(_, data)| data)
    }

    /// Every chunk, in the order pushed.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &Bytes)> {
        self.chunks.iter().map(|(id, data)| (*id, data))
    }

    /// The chunk IDs, in the order pushed.
    pub fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.chunks.iter().map(|(id, _)| *id)
    }

    /// Number of chunks.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

impl FromIterator<(u64, Bytes)> for ChunkData {
    fn from_iter<I: IntoIterator<Item = (u64, Bytes)>>(iter: I) -> Self {
        Self {
            chunks: iter.into_iter().collect(),
        }
    }
}

/// Lengths only: a chunk can be a whole image.
impl std::fmt::Debug for ChunkData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(
                self.chunks
                    .iter()
                    .map(|(id, data)| (format!("{id:#x}"), data.len())),
            )
            .finish()
    }
}

/// Data attached to a nodemap's chunk and event ports.
#[derive(Default)]
pub(crate) struct Attached {
    pub(crate) chunks: ChunkData,
    pub(crate) events: HashMap<u64, Bytes>,
}

impl std::fmt::Debug for Attached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut events: Vec<(String, usize)> = self
            .events
            .iter()
            .map(|(id, data)| (format!("{id:#x}"), data.len()))
            .collect();
        events.sort();
        f.debug_struct("Attached")
            .field("chunks", &self.chunks)
            .field("events", &events)
            .finish()
    }
}

/// Where a register's bytes are, once its address is resolved.
pub(crate) enum Location<'a> {
    /// Device memory, through the transport.
    Device { address: u64, len: u32 },
    /// Bytes attached to a chunk or event port.
    Data {
        port: &'a str,
        bytes: &'a Bytes,
        /// Resolved offset; negative only on a chunk port.
        offset: i64,
        len: u32,
        /// Whether a negative offset counts from the end (chunk ports).
        from_end: bool,
    },
}

/// The port a register is on, before anything is read.
enum Route<'a> {
    Device,
    Chunk { port: &'a str, id: u64 },
    Event { port: &'a str, id: u64 },
}

impl NodeMap {
    /// Attach one stream buffer's chunks, replacing any attached before.
    ///
    /// Every register on a chunk port reads from `chunks` until the next
    /// attach or [`NodeMap::detach_chunks`]; a chunk port whose ID `chunks`
    /// lacks reads as [`GenApiError::NotAttached`]. Everything that depends on
    /// a chunk port is invalidated, so nothing read from the previous buffer
    /// survives. The chunk data is shared, not copied.
    pub fn attach_chunks(&mut self, chunks: &ChunkData) {
        self.attached.chunks = chunks.clone();
        self.invalidate_ports(|kind| matches!(kind, PortKind::Chunk { .. }));
    }

    /// Detach the chunks attached by [`NodeMap::attach_chunks`]. Chunk
    /// features then read as [`GenApiError::NotAttached`].
    pub fn detach_chunks(&mut self) {
        self.attached.chunks = ChunkData::default();
        self.invalidate_ports(|kind| matches!(kind, PortKind::Chunk { .. }));
    }

    /// Attach one event's bytes to the ports whose `<EventID>` is `event_id`,
    /// replacing the previous event with that ID.
    ///
    /// `data` is the event as the device sent it, header included: event-port
    /// addresses count from its first byte. Every corpus document that names
    /// an event's timestamp puts it at 0x8 on a 16-bit-block-id GigE Vision
    /// event, where the header's timestamp field is. Other event IDs keep
    /// what was attached for them, as GenApi's event model keeps the last
    /// event of each kind.
    pub fn attach_event(&mut self, event_id: u64, data: Bytes) {
        self.attached.events.insert(event_id, data);
        self.invalidate_ports(|kind| *kind == PortKind::Event { event_id });
    }

    /// Detach every event attached by [`NodeMap::attach_event`].
    pub fn detach_events(&mut self) {
        self.attached.events.clear();
        self.invalidate_ports(|kind| matches!(kind, PortKind::Event { .. }));
    }

    /// The ports the document declares, with what each reads, in name order.
    pub fn ports(&self) -> Vec<(&str, PortKind)> {
        let mut ports: Vec<(&str, PortKind)> = self
            .ports
            .iter()
            .map(|(name, kind)| (name.as_str(), *kind))
            .collect();
        ports.sort_by(|a, b| a.0.cmp(b.0));
        ports
    }

    /// The features whose value depends on `port`: category members reached
    /// from the registers on it through `<pValue>` chains, formulas and the
    /// other edges [`NodeMap::dependents`] records. Sorted by name; empty for
    /// a port nothing reads.
    ///
    /// For FLIR's `TimestampPort` that is `ChunkTimestamp`, an `<Integer>`
    /// over the `ChunkTimestamp_Val` register on the port — the feature, not
    /// the register behind it.
    pub fn port_features(&self, port: &str) -> Vec<&str> {
        let features: HashSet<&str> = self
            .nodes
            .values()
            .filter_map(|node| match node {
                Node::Category(cat) => Some(cat.children.iter().map(String::as_str)),
                _ => None,
            })
            .flatten()
            .collect();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut pending: Vec<&str> = self.dependents(port).iter().map(String::as_str).collect();
        while let Some(name) = pending.pop() {
            if !seen.insert(name) {
                continue;
            }
            pending.extend(self.dependents(name).iter().map(String::as_str));
        }
        let mut out: Vec<&str> = seen
            .into_iter()
            .filter(|name| features.contains(name) && self.nodes.contains_key(*name))
            .collect();
        out.sort_unstable();
        out
    }

    /// Record each register on a chunk or event port as a dependent of the
    /// port, so that attaching new data invalidates it and everything above
    /// it.
    pub(super) fn link_port_dependents(&mut self) {
        let mut links: Vec<(String, String)> = Vec::new();
        for node in self.nodes.values() {
            let Some(port) = node_addressing(node).and_then(Addressing::port) else {
                continue;
            };
            if matches!(
                self.ports.get(port),
                Some(PortKind::Chunk { .. } | PortKind::Event { .. })
            ) {
                links.push((port.to_string(), node.name().to_string()));
            }
        }
        for (port, node) in links {
            let list = self.dependents.entry(port).or_default();
            if !list.contains(&node) {
                list.push(node);
            }
        }
    }

    fn invalidate_ports(&self, which: impl Fn(&PortKind) -> bool) {
        for (name, kind) in &self.ports {
            if which(kind) {
                self.invalidate_dependents(name);
            }
        }
        self.bump_generation();
    }

    /// The port `addressing` is on. A `<pPort>` naming no declared port is
    /// refused unless it names `Device`, which documents also use without
    /// declaring it.
    fn route<'a>(
        &'a self,
        node: &str,
        addressing: &'a Addressing,
    ) -> Result<Route<'a>, GenApiError> {
        let Some(port) = addressing.port() else {
            return Ok(Route::Device);
        };
        match self.ports.get(port) {
            Some(PortKind::Device) => Ok(Route::Device),
            Some(PortKind::Chunk { chunk_id }) => Ok(Route::Chunk {
                port,
                id: *chunk_id,
            }),
            Some(PortKind::Event { event_id }) => Ok(Route::Event {
                port,
                id: *event_id,
            }),
            None if port.eq_ignore_ascii_case("Device") => Ok(Route::Device),
            _ => Err(GenApiError::UnknownPort {
                node: node.to_string(),
                port: port.to_string(),
            }),
        }
    }

    /// Where `node`'s bytes are for a read.
    ///
    /// The port comes first: a chunk or event port with nothing attached is
    /// [`GenApiError::NotAttached`] before any address term is evaluated, so
    /// the device is not touched at all.
    pub(crate) fn locate<'a>(
        &'a self,
        node: &str,
        addressing: &'a Addressing,
        io: &dyn RegisterIo,
    ) -> Result<Location<'a>, GenApiError> {
        let (port, bytes, from_end) = match self.route(node, addressing)? {
            Route::Device => {
                let (address, len) = self.resolve_device_address(node, addressing, io)?;
                return Ok(Location::Device { address, len });
            }
            Route::Chunk { port, id } => (port, self.attached.chunks.get(id), true),
            Route::Event { port, id } => (port, self.attached.events.get(&id), false),
        };
        let bytes = bytes.ok_or_else(|| GenApiError::NotAttached {
            node: node.to_string(),
            port: port.to_string(),
        })?;
        let (offset, len) = self.resolve_address(node, addressing, io)?;
        Ok(Location::Data {
            port,
            bytes,
            // The sum wraps like the device address does; on a chunk port
            // its two's-complement reading is the signed offset §2.8.16
            // allows.
            offset: offset as i64,
            len,
            from_end,
        })
    }

    /// The device address `node` writes to. A chunk or event port is refused
    /// whether or not anything is attached to it.
    pub(crate) fn locate_for_write(
        &self,
        node: &str,
        addressing: &Addressing,
        io: &dyn RegisterIo,
    ) -> Result<(u64, u32), GenApiError> {
        match self.route(node, addressing)? {
            Route::Device => self.resolve_device_address(node, addressing, io),
            Route::Chunk { port, .. } | Route::Event { port, .. } => {
                Err(GenApiError::PortNotWritable {
                    node: node.to_string(),
                    port: port.to_string(),
                })
            }
        }
    }

    /// Read the bytes at `location`.
    pub(crate) fn read_location(
        &self,
        node: &str,
        location: &Location<'_>,
        io: &dyn RegisterIo,
    ) -> Result<Vec<u8>, GenApiError> {
        match *location {
            Location::Device { address, len } => io.read(address, len as usize),
            Location::Data {
                port,
                bytes,
                offset,
                len,
                from_end,
            } => slice_port_data(node, port, bytes, offset, len, from_end).map(<[u8]>::to_vec),
        }
    }
}

/// `len` bytes at `offset` of a port's data. On a chunk port a negative offset
/// is "interpreted as offset from the back of the chunk" (GenICam Standard
/// v2.1.1 §2.8.16): `-4` is the last four bytes. Anything outside the data is
/// an error naming the offset.
fn slice_port_data<'a>(
    node: &str,
    port: &str,
    bytes: &'a [u8],
    offset: i64,
    len: u32,
    from_end: bool,
) -> Result<&'a [u8], GenApiError> {
    let size = bytes.len();
    let out_of_range = || GenApiError::PortOutOfRange {
        node: node.to_string(),
        port: port.to_string(),
        offset,
        len,
        size,
    };
    let start = if offset < 0 {
        if !from_end {
            return Err(out_of_range());
        }
        let back = usize::try_from(offset.unsigned_abs()).map_err(|_| out_of_range())?;
        size.checked_sub(back).ok_or_else(out_of_range)?
    } else {
        usize::try_from(offset).map_err(|_| out_of_range())?
    };
    let end = start
        .checked_add(len as usize)
        .filter(|end| *end <= size)
        .ok_or_else(out_of_range)?;
    Ok(&bytes[start..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_offset_counts_from_the_end_of_a_chunk() {
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(
            slice_port_data("n", "p", &data, -4, 4, true).expect("in range"),
            &[5, 6, 7, 8]
        );
        assert_eq!(
            slice_port_data("n", "p", &data, -8, 2, true).expect("in range"),
            &[1, 2]
        );
    }

    #[test]
    fn out_of_range_is_an_error_not_a_panic() {
        let data = [0u8; 8];
        for (offset, len, from_end) in [
            (-9, 1, true),
            (-4, 8, true),
            (-1, 1, false),
            (7, 2, false),
            (8, 1, false),
            (i64::MIN, 4, true),
            (i64::MAX, 4, true),
            (0, u32::MAX, true),
        ] {
            let err = slice_port_data("n", "p", &data, offset, len, from_end)
                .expect_err("outside the data");
            assert!(
                matches!(err, GenApiError::PortOutOfRange { size: 8, .. }),
                "{err}"
            );
        }
    }

    #[test]
    fn chunk_data_reads_the_first_chunk_with_an_id() {
        let chunks: ChunkData = [
            (1, Bytes::from_static(b"one")),
            (2, Bytes::from_static(b"two")),
            (1, Bytes::from_static(b"again")),
        ]
        .into_iter()
        .collect();
        assert_eq!(chunks.get(1).map(|b| &b[..]), Some(&b"one"[..]));
        assert_eq!(chunks.get(3), None);
        assert_eq!(chunks.ids().collect::<Vec<_>>(), vec![1, 2, 1]);
        assert_eq!(format!("{chunks:?}"), r#"{"0x1": 3, "0x2": 3, "0x1": 5}"#);
    }
}
