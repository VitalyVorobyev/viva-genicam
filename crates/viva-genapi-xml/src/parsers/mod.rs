//! Node-specific parsers and shared parsing helpers.

mod control;
mod converter;
mod numeric;
mod port;
mod register;
mod struct_reg;
mod swissknife;
mod symbolic;

pub(crate) use control::{
    missing_command_value, parse_category, parse_category_empty, parse_command,
};
pub(crate) use converter::{parse_converter, parse_int_converter, parse_string};
pub(crate) use numeric::{parse_float, parse_integer};
pub(crate) use port::{parse_port, parse_port_empty};
pub(crate) use register::parse_register;
pub(crate) use struct_reg::parse_struct_reg;
pub(crate) use swissknife::parse_swissknife;
pub(crate) use symbolic::{parse_boolean, parse_enum};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::builders::AddressingBuilder;
use crate::util::{attribute_value, parse_address, parse_u64, read_text_start};
use crate::{IndexOffset, NodeDecl, NodeMeta, PredicateRefs, Representation, Visibility, XmlError};

/// XML element name referencing another node that provides an address.
pub(crate) const TAG_P_ADDRESS: &str = "pAddress";

/// `<pPort>`: the port a register's address is relative to.
pub(crate) const TAG_P_PORT: &str = "pPort";

/// `<pIndex>`: an index node whose value is scaled and added to the address.
pub(crate) const TAG_P_INDEX: &str = "pIndex";

/// `<IntSwissKnife>` inside a register: an address term computed by a formula
/// (GenICam Standard v2.1.1 §2.8.3, and the schema's address group). The
/// embedded knife is a node of the document — the schema's node-name key
/// covers it — so the parser declares it as one ([`embedded_knives`]) and the
/// register refers to it by name, as `<pAddress>` would.
pub(crate) const TAG_INT_SWISS_KNIFE: &str = "IntSwissKnife";

/// The name an embedded `<IntSwissKnife>` is declared under.
pub(crate) fn embedded_knife_name(event: &BytesStart<'_>) -> Result<String, XmlError> {
    let name = crate::util::attribute_value_required(event, "Name")?;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(XmlError::invalid(
            "<IntSwissKnife>",
            "embedded in a register has an empty Name",
        ));
    }
    Ok(trimmed.to_string())
}

/// The `<IntSwissKnife>` elements a node element embeds as address terms —
/// its direct children — each parsed as a node of its own.
///
/// The node's own parser records each one as an address term by name
/// ([`handle_addressing_start`]); this declares the nodes those names refer
/// to. Backlog GA-17: 143 such terms in nine corpus documents were dropped,
/// so their registers had no address.
pub(crate) fn embedded_knives(element: &str) -> Result<Vec<NodeDecl>, XmlError> {
    let mut reader = Reader::from_str(element);
    reader.config_mut().trim_text(true);
    reader.config_mut().allow_dangling_amp = true;
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut knives = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth == 2 && e.name().as_ref() == TAG_INT_SWISS_KNIFE {
                    let start = e.into_owned();
                    knives.push(parse_swissknife(&mut reader, start)?);
                    depth -= 1;
                } else if depth >= 2 {
                    crate::util::skip_element(&mut reader, e.name().as_ref())?;
                    depth -= 1;
                }
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(XmlError::malformed(err)),
            _ => {}
        }
        buf.clear();
    }
    Ok(knives)
}
/// XML element holding an inline literal value.
pub(crate) const TAG_VALUE: &str = "Value";
/// XML element referencing another node supplying the value at runtime.
pub(crate) const TAG_P_VALUE: &str = "pValue";
/// XML element specifying a user friendly label.
pub(crate) const TAG_DISPLAY_NAME: &str = "DisplayName";
/// XML element for node visibility level.
pub(crate) const TAG_VISIBILITY: &str = "Visibility";
pub(crate) const TAG_IMPOSED_ACCESS_MODE: &str = "ImposedAccessMode";
/// XML element marking a feature that belongs in a saved configuration.
pub(crate) const TAG_STREAMABLE: &str = "Streamable";
/// `<Cachable>`: how a register's value may be cached (GA-05).
pub(crate) const TAG_CACHABLE: &str = "Cachable";
/// The spelling GenICam Standard v2.1.1 §2.8.3's prose uses for
/// [`TAG_CACHABLE`]; its example and the schema write `Cachable`. Both are
/// accepted, as `LSB` and `Lsb` are.
pub(crate) const TAG_CACHEABLE: &str = "Cacheable";
/// `<PollingTime>`: milliseconds after which a value should be read again.
pub(crate) const TAG_POLLING_TIME: &str = "PollingTime";
/// XML element for the long-form description.
pub(crate) const TAG_DESCRIPTION: &str = "Description";
/// XML element for short tooltip text.
pub(crate) const TAG_TOOLTIP: &str = "ToolTip";
/// XML element for the recommended numeric representation.
pub(crate) const TAG_REPRESENTATION: &str = "Representation";
/// XML element describing the least significant bit of a bitfield.
///
/// The GenICam schema spells it `LSB`, and every one of the 1 419 declarations
/// in register nodes across the vendor corpus uses that spelling — none uses
/// `Lsb`. Both are accepted: matching only the mixed-case form silently dropped
/// the bit range from every `<MaskedIntReg>` that declared one, leaving the node
/// reading its whole register (issue #120).
pub(crate) const TAG_LSB: &str = "LSB";
/// Mixed-case spelling of [`TAG_LSB`], accepted alongside it.
pub(crate) const TAG_LSB_MIXED: &str = "Lsb";
/// XML element describing the most significant bit of a bitfield.
///
/// See [`TAG_LSB`] for why both spellings are accepted.
pub(crate) const TAG_MSB: &str = "MSB";
/// Mixed-case spelling of [`TAG_MSB`], accepted alongside it.
pub(crate) const TAG_MSB_MIXED: &str = "Msb";
/// XML element describing the starting bit index of a bitfield.
pub(crate) const TAG_BIT: &str = "Bit";
/// XML element describing a bitmask for a bitfield.
pub(crate) const TAG_MASK: &str = "Mask";
/// XML element providing the register byte order (common spelling).
pub(crate) const TAG_ENDIANNESS: &str = "Endianness";
/// XML element providing the register byte order (alternate spelling).
pub(crate) const TAG_ENDIANESS: &str = "Endianess";
/// XML element providing the register byte order (PFNC style).
pub(crate) const TAG_BYTE_ORDER: &str = "ByteOrder";
/// XML element referencing a node whose value gates feature implementation.
pub(crate) const TAG_P_IS_IMPLEMENTED: &str = "pIsImplemented";
/// XML element referencing a node whose value gates feature availability.
pub(crate) const TAG_P_IS_AVAILABLE: &str = "pIsAvailable";
/// XML element referencing a node whose value locks the feature (RW→RO).
pub(crate) const TAG_P_IS_LOCKED: &str = "pIsLocked";
/// XML element naming a node whose change makes a register's cached value stale.
pub(crate) const TAG_P_INVALIDATOR: &str = "pInvalidator";

/// Handle a `<pIsImplemented>` / `<pIsAvailable>` / `<pIsLocked>` element.
///
/// Returns `true` when the element was consumed (caller should continue),
/// `false` when the element name does not match and the caller should fall
/// through to its own handling (typically `skip_element`).
///
/// The element text is a single node reference; empty text is treated as
/// absent so that vendor XMLs with placeholder empty tags do not fabricate
/// references to a node named "".
pub(crate) fn handle_predicate_start(
    reader: &mut Reader<&[u8]>,
    event: &BytesStart<'_>,
    prefs: &mut PredicateRefs,
) -> Result<bool, XmlError> {
    let slot: &mut Option<String> = match event.name().as_ref() {
        TAG_P_IS_IMPLEMENTED => &mut prefs.p_is_implemented,
        TAG_P_IS_AVAILABLE => &mut prefs.p_is_available,
        TAG_P_IS_LOCKED => &mut prefs.p_is_locked,
        _ => return Ok(false),
    };
    let text = read_text_start(reader, event)?;
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        *slot = Some(trimmed.to_string());
    }
    Ok(true)
}

/// Handle a `<pInvalidator>` element on a register node.
///
/// Returns `true` when the element was consumed (caller should continue),
/// `false` when the element name does not match and the caller should fall
/// through to its own handling.
///
/// A register may declare any number of these — the vendor corpus has nodes
/// with nineteen — so each non-empty reference is appended in declaration
/// order. Empty text is ignored, as in [`handle_predicate_start`].
pub(crate) fn handle_invalidator_start(
    reader: &mut Reader<&[u8]>,
    event: &BytesStart<'_>,
    invalidators: &mut Vec<String>,
) -> Result<bool, XmlError> {
    if event.name().as_ref() != TAG_P_INVALIDATOR {
        return Ok(false);
    }
    let text = read_text_start(reader, event)?;
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        invalidators.push(trimmed.to_string());
    }
    Ok(true)
}

/// XML element on a selector naming a feature it selects.
pub(crate) const TAG_P_SELECTED: &str = "pSelected";

/// Record a `<pSelected>` element's target in `selected`.
///
/// GenICam Standard v2.1.1 §2.8.4: `<pSelected>` sits on the *selector* — an
/// `<Integer>` or `<Enumeration>` — and names a feature whose value depends on
/// it. The element text is that feature's name; empty text is ignored, as in
/// [`handle_predicate_start`].
pub(crate) fn handle_p_selected_start(
    reader: &mut Reader<&[u8]>,
    event: &BytesStart<'_>,
    selected: &mut Vec<String>,
) -> Result<(), XmlError> {
    let text = read_text_start(reader, event)?;
    let target = text.trim();
    if !target.is_empty() {
        selected.push(target.to_string());
    }
    Ok(())
}

/// Read the stride of a `<pIndex>` element.
///
/// `Offset` gives a literal stride, `pOffset` names a node supplying one, and
/// neither means the stride is the register length.
pub(crate) fn index_offset(event: &BytesStart<'_>) -> Result<IndexOffset, XmlError> {
    if let Some(offset) = attribute_value(event, "Offset")? {
        return Ok(IndexOffset::Fixed(parse_u64(&offset)?));
    }
    if let Some(node) = attribute_value(event, "pOffset")? {
        let trimmed = node.trim();
        if !trimmed.is_empty() {
            return Ok(IndexOffset::Node(trimmed.to_string()));
        }
    }
    Ok(IndexOffset::Length)
}

/// Handle common addressing elements for start events.
///
/// Returns `true` if the element was handled, `false` otherwise.
pub(crate) fn handle_addressing_start(
    reader: &mut Reader<&[u8]>,
    event: &BytesStart<'_>,
    name: &str,
    addressing: &mut AddressingBuilder,
) -> Result<bool, XmlError> {
    match event.name().as_ref() {
        "Address" => {
            let text = read_text_start(reader, event)?;
            addressing.push_fixed_address(parse_address(&text)?);
            Ok(true)
        }
        TAG_P_PORT => {
            let text = read_text_start(reader, event)?;
            let target = text.trim();
            if !target.is_empty() {
                addressing.set_port(target);
            }
            Ok(true)
        }
        TAG_P_ADDRESS => {
            let text = read_text_start(reader, event)?;
            let target = text.trim();
            if !target.is_empty() {
                addressing.push_p_address(target);
            }
            Ok(true)
        }
        TAG_P_INDEX => {
            let offset = index_offset(event)?;
            let text = read_text_start(reader, event)?;
            let target = text.trim();
            if !target.is_empty() {
                addressing.push_index(target, offset);
            }
            Ok(true)
        }
        TAG_INT_SWISS_KNIFE => {
            addressing.push_p_address(&embedded_knife_name(event)?);
            crate::util::skip_element(reader, event.name().as_ref())?;
            Ok(true)
        }
        "Length" => {
            let text = read_text_start(reader, event)?;
            let value = parse_u64(&text)?;
            let len = u32::try_from(value)
                .map_err(|_| XmlError::invalid(name, "declares a <Length> out of range"))?;
            addressing.set_length(len);
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Accumulates metadata fields during node parsing.
#[derive(Debug, Default)]
pub(crate) struct NodeMetaBuilder {
    pub visibility: Option<Visibility>,
    pub description: Option<String>,
    pub tooltip: Option<String>,
    pub display_name: Option<String>,
    pub representation: Option<Representation>,
    pub imposed_access: Option<crate::AccessMode>,
    pub streamable: bool,
    pub cachable: Option<crate::CachingMode>,
    pub polling_time_ms: Option<u64>,
}

impl NodeMetaBuilder {
    /// Try to handle a start element as a metadata tag.
    ///
    /// Returns `true` if the tag was consumed, `false` otherwise.
    pub(crate) fn handle_start(
        &mut self,
        reader: &mut Reader<&[u8]>,
        event: &BytesStart<'_>,
    ) -> Result<bool, XmlError> {
        match event.name().as_ref() {
            TAG_VISIBILITY => {
                let text = read_text_start(reader, event)?;
                self.visibility = Visibility::parse(&text);
                Ok(true)
            }
            TAG_DESCRIPTION => {
                let text = read_text_start(reader, event)?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.description = Some(trimmed.to_string());
                }
                Ok(true)
            }
            TAG_TOOLTIP => {
                let text = read_text_start(reader, event)?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.tooltip = Some(trimmed.to_string());
                }
                Ok(true)
            }
            TAG_DISPLAY_NAME => {
                let text = read_text_start(reader, event)?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.display_name = Some(trimmed.to_string());
                }
                Ok(true)
            }
            TAG_REPRESENTATION => {
                let text = read_text_start(reader, event)?;
                self.representation = Representation::parse(&text);
                Ok(true)
            }
            TAG_IMPOSED_ACCESS_MODE => {
                let text = read_text_start(reader, event)?;
                self.imposed_access = Some(crate::AccessMode::parse(&text)?);
                Ok(true)
            }
            // The schema's values are `Yes` and `No`. Anything else — the
            // empty `<Streamable/>` arrives as an empty element and never
            // reaches here — keeps the standard's default, `No`.
            TAG_STREAMABLE => {
                let text = read_text_start(reader, event)?;
                self.streamable = text.trim() == "Yes";
                Ok(true)
            }
            TAG_CACHABLE | TAG_CACHEABLE => {
                let text = read_text_start(reader, event)?;
                self.cachable = Some(crate::CachingMode::parse(&text));
                Ok(true)
            }
            TAG_POLLING_TIME => {
                let text = read_text_start(reader, event)?;
                self.polling_time_ms = parse_polling_time(&text);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Build the final [`NodeMeta`].
    pub(crate) fn build(self) -> NodeMeta {
        NodeMeta {
            visibility: self.visibility.unwrap_or_default(),
            description: self.description,
            tooltip: self.tooltip,
            display_name: self.display_name,
            representation: self.representation,
            imposed_access: self.imposed_access,
            streamable: self.streamable,
            cachable: self.cachable,
            polling_time_ms: self.polling_time_ms,
        }
    }
}

/// Read a `<PollingTime>`. One that is not a number is a hint lost, logged
/// and dropped: it costs no more than the node's polling, not the node.
pub(crate) fn parse_polling_time(text: &str) -> Option<u64> {
    match parse_u64(text) {
        Ok(ms) => Some(ms),
        Err(err) => {
            tracing::warn!(value = text.trim(), error = %err, "ignoring an unreadable <PollingTime>");
            None
        }
    }
}

/// Handle common addressing elements for empty events.
///
/// Returns `true` if the element was handled, `false` otherwise.
pub(crate) fn handle_addressing_empty(
    event: &BytesStart<'_>,
    addressing: &mut AddressingBuilder,
) -> Result<bool, XmlError> {
    match event.name().as_ref() {
        TAG_P_ADDRESS => {
            if let Some(value) = attribute_value(event, "Name")? {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    addressing.push_p_address(trimmed);
                }
            }
            Ok(true)
        }
        TAG_P_INDEX => {
            if let Some(value) = attribute_value(event, "Name")? {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    addressing.push_index(trimmed, index_offset(event)?);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}
