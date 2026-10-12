//! Helpers for the standard features (SFNC) every application touches —
//! exposure, gain, triggers, region of interest, binning, frame rate, user
//! sets — that tolerate the names cameras actually use for them.

use crate::genapi::{GenApiError, RegisterIo};
use crate::{COMMAND_TIMEOUT, Camera, GenicamError};

/// A standard feature the helpers look up, by the names cameras declare it
/// under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StandardFeature {
    /// `ExposureTime`, `ExposureTimeAbs` or `ExposureTimeRaw`.
    ExposureTime,
    /// `Gain` or `GainRaw`.
    Gain,
    /// `GainSelector`.
    GainSelector,
    /// `AcquisitionMode`.
    AcquisitionMode,
    /// `TriggerSelector`.
    TriggerSelector,
    /// `TriggerMode`.
    TriggerMode,
    /// `TriggerSource`.
    TriggerSource,
    /// `TriggerSoftware`.
    TriggerSoftware,
    /// `Width`.
    Width,
    /// `Height`.
    Height,
    /// `OffsetX`.
    OffsetX,
    /// `OffsetY`.
    OffsetY,
    /// `BinningHorizontal`.
    BinningHorizontal,
    /// `BinningVertical`.
    BinningVertical,
    /// `AcquisitionFrameRate` or `AcquisitionFrameRateAbs`.
    FrameRate,
    /// `AcquisitionFrameRateEnable` or `AcquisitionFrameRateEnabled`.
    FrameRateEnable,
    /// `UserSetSelector`.
    UserSetSelector,
    /// `UserSetLoad`.
    UserSetLoad,
    /// `UserSetSave`.
    UserSetSave,
    /// `UserSetDefault` or `UserSetDefaultSelector`.
    UserSetDefault,
    /// `TLParamsLocked`.
    TlParamsLocked,
}

impl StandardFeature {
    /// Every standard feature the helpers use.
    pub const ALL: &'static [StandardFeature] = &[
        Self::ExposureTime,
        Self::Gain,
        Self::GainSelector,
        Self::AcquisitionMode,
        Self::TriggerSelector,
        Self::TriggerMode,
        Self::TriggerSource,
        Self::TriggerSoftware,
        Self::Width,
        Self::Height,
        Self::OffsetX,
        Self::OffsetY,
        Self::BinningHorizontal,
        Self::BinningVertical,
        Self::FrameRate,
        Self::FrameRateEnable,
        Self::UserSetSelector,
        Self::UserSetLoad,
        Self::UserSetSave,
        Self::UserSetDefault,
        Self::TlParamsLocked,
    ];

    /// The names cameras declare it under, in the order they are tried.
    pub fn names(self) -> &'static [&'static str] {
        use viva_sfnc as n;
        match self {
            Self::ExposureTime => n::EXPOSURE_TIME_NAMES,
            Self::Gain => n::GAIN_NAMES,
            Self::GainSelector => &[n::GAIN_SELECTOR],
            Self::AcquisitionMode => &[n::ACQUISITION_MODE],
            Self::TriggerSelector => &[n::TRIGGER_SELECTOR],
            Self::TriggerMode => &[n::TRIGGER_MODE],
            Self::TriggerSource => &[n::TRIGGER_SOURCE],
            Self::TriggerSoftware => &[n::TRIGGER_SOFTWARE],
            Self::Width => &[n::WIDTH],
            Self::Height => &[n::HEIGHT],
            Self::OffsetX => &[n::OFFSET_X],
            Self::OffsetY => &[n::OFFSET_Y],
            Self::BinningHorizontal => &[n::BINNING_HORIZONTAL],
            Self::BinningVertical => &[n::BINNING_VERTICAL],
            Self::FrameRate => n::FRAME_RATE_NAMES,
            Self::FrameRateEnable => n::FRAME_RATE_ENABLE_NAMES,
            Self::UserSetSelector => &[n::USER_SET_SELECTOR],
            Self::UserSetLoad => &[n::USER_SET_LOAD],
            Self::UserSetSave => &[n::USER_SET_SAVE],
            Self::UserSetDefault => n::USER_SET_DEFAULT_NAMES,
            Self::TlParamsLocked => &[n::TL_PARAMS_LOCKED],
        }
    }

    /// The node kinds its helper can drive.
    fn kinds(self) -> &'static [&'static str] {
        const NUMBER: &[&str] = &["Float", "Converter", "Integer", "IntConverter"];
        const INTEGER: &[&str] = &["Integer", "IntConverter"];
        match self {
            Self::ExposureTime | Self::Gain | Self::FrameRate => NUMBER,
            Self::Width | Self::Height | Self::OffsetX | Self::OffsetY => INTEGER,
            Self::BinningHorizontal | Self::BinningVertical => {
                &["Integer", "IntConverter", "Enumeration"]
            }
            Self::TriggerSoftware | Self::UserSetLoad | Self::UserSetSave => &["Command"],
            Self::FrameRateEnable => &["Boolean"],
            Self::TlParamsLocked => &["Integer", "Boolean"],
            Self::GainSelector
            | Self::AcquisitionMode
            | Self::TriggerSelector
            | Self::TriggerMode
            | Self::TriggerSource
            | Self::UserSetSelector
            | Self::UserSetDefault => &["Enumeration"],
        }
    }

    /// What it is, for an error message.
    pub fn description(self) -> &'static str {
        match self {
            Self::ExposureTime => "exposure time",
            Self::Gain => "gain",
            Self::GainSelector => "gain selector",
            Self::AcquisitionMode => "acquisition mode",
            Self::TriggerSelector => "trigger selector",
            Self::TriggerMode => "trigger mode",
            Self::TriggerSource => "trigger source",
            Self::TriggerSoftware => "software trigger",
            Self::Width => "image width",
            Self::Height => "image height",
            Self::OffsetX => "horizontal offset",
            Self::OffsetY => "vertical offset",
            Self::BinningHorizontal => "horizontal binning",
            Self::BinningVertical => "vertical binning",
            Self::FrameRate => "frame rate",
            Self::FrameRateEnable => "frame rate enable",
            Self::UserSetSelector => "user set selector",
            Self::UserSetLoad => "user set load command",
            Self::UserSetSave => "user set save command",
            Self::UserSetDefault => "power-up user set",
            Self::TlParamsLocked => "transport-layer lock",
        }
    }
}

/// The node a standard feature resolved to on this camera.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedFeature {
    /// Its name in this camera's XML, such as `ExposureTimeAbs`.
    pub name: &'static str,
    /// Its node type, as [`Node::kind_name`](crate::genapi::Node::kind_name)
    /// spells it.
    pub kind: &'static str,
    /// Its unit as the XML declares it. A raw integer feature often
    /// declares none: its unit is the device's own.
    pub unit: Option<String>,
}

/// A region of interest: the offsets of its top-left corner on the sensor,
/// and its size, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Roi {
    /// `OffsetX`.
    pub x: i64,
    /// `OffsetY`.
    pub y: i64,
    /// `Width`.
    pub width: i64,
    /// `Height`.
    pub height: i64,
}

impl ResolvedFeature {
    fn is_integer(&self) -> bool {
        matches!(self.kind, "Integer" | "IntConverter")
    }
}

/// The nearest integer to `value`, for a feature that holds one.
fn nearest_integer(name: &str, value: f64) -> Result<i64, GenicamError> {
    let rounded = value.round();
    if rounded.is_finite() && rounded >= i64::MIN as f64 && rounded < i64::MAX as f64 {
        Ok(rounded as i64)
    } else {
        Err(GenApiError::Range {
            name: name.to_string(),
            value: crate::genapi::swissknife::Value::Float(value),
            violation: crate::genapi::RangeViolation::Unrepresentable,
        }
        .into())
    }
}

/// The binning factor an enumeration entry names, by the digits it ends in:
/// `BinningHorizontal2` is 2 (Hikrobot MV-CS050-10GC). `Off` names none.
fn factor_of(entry: &str) -> Option<i64> {
    let digits = entry.len() - entry.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    entry[entry.len() - digits..].parse().ok()
}

/// How one direction of binning is written.
enum BinningWrite {
    Integer(&'static str, i64),
    Entry(&'static str, String),
}

/// The helpers for standard features. Each looks its feature up by the
/// names cameras declare it under ([`StandardFeature::names`]), in that
/// order, and takes the first of a kind it can drive; one the camera has
/// under none of them is [`GenicamError::MissingFeature`], naming them all.
///
/// The lookup is by declaration: a feature declared but not implemented on
/// this device reports so when it is used, rather than being skipped for the
/// next name.
impl<T: RegisterIo> Camera<T> {
    /// Find the node `feature` goes by on this camera.
    pub fn resolve(&self, feature: StandardFeature) -> Result<ResolvedFeature, GenicamError> {
        let names = feature.names();
        names
            .iter()
            .find_map(|&name| {
                let node = self.nodemap().node(name)?;
                let kind = node.kind_name();
                feature.kinds().contains(&kind).then(|| ResolvedFeature {
                    name,
                    kind,
                    unit: node.unit().map(str::to_string),
                })
            })
            .ok_or(GenicamError::MissingFeature {
                what: feature.description(),
                tried: names,
            })
    }

    /// `feature` if the camera has it, `None` if it has none of its names.
    fn resolve_optional(
        &self,
        feature: StandardFeature,
    ) -> Result<Option<ResolvedFeature>, GenicamError> {
        match self.resolve(feature) {
            Ok(resolved) => Ok(Some(resolved)),
            Err(GenicamError::MissingFeature { .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn read_number(&self, feature: &ResolvedFeature) -> Result<f64, GenicamError> {
        if feature.is_integer() {
            Ok(self.get_integer(feature.name)? as f64)
        } else {
            self.get_float(feature.name)
        }
    }

    fn write_number(&mut self, feature: &ResolvedFeature, value: f64) -> Result<(), GenicamError> {
        if feature.is_integer() {
            let value = nearest_integer(feature.name, value)?;
            self.set_integer(feature.name, value)
        } else {
            self.set_float(feature.name, value)
        }
    }

    /// The exposure time, in the unit of the feature that holds it:
    /// microseconds for `ExposureTime` and `ExposureTimeAbs`, the device's
    /// own for `ExposureTimeRaw` ([`StandardFeature::ExposureTime`] says
    /// which this camera has, and its declared unit).
    pub fn exposure_time(&self) -> Result<f64, GenicamError> {
        let feature = self.resolve(StandardFeature::ExposureTime)?;
        self.read_number(&feature)
    }

    /// Write the exposure time, in the unit of the feature that holds it —
    /// rounded to the nearest integer for `ExposureTimeRaw` — and say which
    /// feature that was.
    pub fn set_exposure_time(&mut self, value: f64) -> Result<ResolvedFeature, GenicamError> {
        let feature = self.resolve(StandardFeature::ExposureTime)?;
        self.write_number(&feature, value)?;
        Ok(feature)
    }

    /// The gain of the selected gain control, in the unit of the feature
    /// that holds it: decibels for SFNC's `Gain`, the device's own for
    /// `GainRaw`.
    pub fn gain(&self) -> Result<f64, GenicamError> {
        let feature = self.resolve(StandardFeature::Gain)?;
        self.read_number(&feature)
    }

    /// Write the gain of the selected gain control ([`Camera::select_gain`]),
    /// in the unit of the feature that holds it — rounded to the nearest
    /// integer for an integer `GainRaw` — and say which feature that was.
    pub fn set_gain(&mut self, value: f64) -> Result<ResolvedFeature, GenicamError> {
        let feature = self.resolve(StandardFeature::Gain)?;
        self.write_number(&feature, value)?;
        Ok(feature)
    }

    /// The gain controls `GainSelector` offers, such as `All` or `Tap1`, in
    /// the XML's order. 26 of the 38 vendor-corpus documents declare one.
    pub fn gain_selectors(&self) -> Result<Vec<String>, GenicamError> {
        let selector = self.resolve(StandardFeature::GainSelector)?;
        Ok(self.nodemap().enum_entries(selector.name)?)
    }

    /// Select the gain control [`Camera::gain`] and [`Camera::set_gain`]
    /// act on. Without a call, they act on whichever the camera has
    /// selected.
    pub fn select_gain(&mut self, selector: &str) -> Result<(), GenicamError> {
        let feature = self.resolve(StandardFeature::GainSelector)?;
        self.set_enum(feature.name, selector)
    }

    /// The acquisition mode: `Continuous`, `SingleFrame`, `MultiFrame`, or
    /// one of a vendor's own.
    pub fn acquisition_mode(&self) -> Result<String, GenicamError> {
        let feature = self.resolve(StandardFeature::AcquisitionMode)?;
        self.get_enum(feature.name)
    }

    /// Write the acquisition mode by entry name.
    pub fn set_acquisition_mode(&mut self, mode: &str) -> Result<(), GenicamError> {
        let feature = self.resolve(StandardFeature::AcquisitionMode)?;
        self.set_enum(feature.name, mode)
    }

    /// Select trigger `selector`, where the camera has more than one.
    ///
    /// A camera without `TriggerSelector` has a single trigger, which
    /// `selector` is then taken to name.
    fn select_trigger(&mut self, selector: &str) -> Result<(), GenicamError> {
        match self.resolve_optional(StandardFeature::TriggerSelector)? {
            Some(feature) => self.set_enum(feature.name, selector),
            None => Ok(()),
        }
    }

    /// Make trigger `selector` — `FrameStart`, most often — wait for
    /// `source`: `Software`, `Line1`, ... It is selected, given its source,
    /// then turned on: the source first, since a camera may refuse to change
    /// it while the trigger is on. It stays selected, for
    /// [`Camera::trigger_software`].
    pub fn enable_trigger(&mut self, selector: &str, source: &str) -> Result<(), GenicamError> {
        self.select_trigger(selector)?;
        let source_feature = self.resolve(StandardFeature::TriggerSource)?;
        self.set_enum(source_feature.name, source)?;
        let mode = self.resolve(StandardFeature::TriggerMode)?;
        self.set_enum(mode.name, "On")
    }

    /// Turn trigger `selector` off: the camera runs free again.
    pub fn disable_trigger(&mut self, selector: &str) -> Result<(), GenicamError> {
        self.select_trigger(selector)?;
        let mode = self.resolve(StandardFeature::TriggerMode)?;
        self.set_enum(mode.name, "Off")
    }

    /// Fire the selected trigger once (`TriggerSoftware`). It acts on a
    /// trigger whose source is `Software` ([`Camera::enable_trigger`]).
    pub fn trigger_software(&mut self) -> Result<(), GenicamError> {
        let command = self.resolve(StandardFeature::TriggerSoftware)?;
        self.execute_command(command.name)
    }

    /// The region of interest. A camera without offsets reads as one at
    /// the origin.
    pub fn roi(&self) -> Result<Roi, GenicamError> {
        let offset = |feature| -> Result<i64, GenicamError> {
            match self.resolve_optional(feature)? {
                Some(offset) => self.get_integer(offset.name),
                None => Ok(0),
            }
        };
        Ok(Roi {
            x: offset(StandardFeature::OffsetX)?,
            y: offset(StandardFeature::OffsetY)?,
            width: self.get_integer(self.resolve(StandardFeature::Width)?.name)?,
            height: self.get_integer(self.resolve(StandardFeature::Height)?.name)?,
        })
    }

    /// Move and size the region of interest.
    ///
    /// A size's maximum shrinks as its offset grows, and an offset's as the
    /// size grows, so the order of writes decides whether a region can be
    /// reached at all: the offsets go to zero first, which only widens the
    /// sizes' range, then the sizes, then the offsets. Every value is
    /// checked against its feature's increment before anything is written.
    ///
    /// A camera without offsets accepts only a region at the origin.
    pub fn set_roi(&mut self, roi: Roi) -> Result<(), GenicamError> {
        let width = self.resolve(StandardFeature::Width)?;
        let height = self.resolve(StandardFeature::Height)?;
        let mut offsets = Vec::new();
        for (feature, value) in [
            (StandardFeature::OffsetX, roi.x),
            (StandardFeature::OffsetY, roi.y),
        ] {
            match self.resolve_optional(feature)? {
                Some(offset) => offsets.push((offset, value)),
                None if value == 0 => {}
                None => {
                    return Err(GenicamError::MissingFeature {
                        what: feature.description(),
                        tried: feature.names(),
                    });
                }
            }
        }

        for (name, value) in [(width.name, roi.width), (height.name, roi.height)]
            .into_iter()
            .chain(offsets.iter().map(|(offset, value)| (offset.name, *value)))
        {
            self.check_increment(name, value)?;
        }

        for (offset, _) in &offsets {
            if self.get_integer(offset.name)? != 0 {
                self.set_integer(offset.name, 0)?;
            }
        }
        self.set_integer(width.name, roi.width)?;
        self.set_integer(height.name, roi.height)?;
        for (offset, value) in &offsets {
            self.set_integer(offset.name, *value)?;
        }
        Ok(())
    }

    /// Refuse `value` for `name` when it is off its increment, naming the
    /// values on either side of it.
    fn check_increment(&self, name: &str, value: i64) -> Result<(), GenicamError> {
        let Some(range) = self.feature_state(name)?.range else {
            return Ok(());
        };
        let Some(inc) = range.inc.map(|inc| inc as i64).filter(|&inc| inc > 1) else {
            return Ok(());
        };
        let min = range.min as i64;
        let off = (value - min).rem_euclid(inc);
        if off == 0 {
            return Ok(());
        }
        Err(GenApiError::Range {
            name: name.to_string(),
            value: crate::genapi::swissknife::Value::Int(value),
            violation: crate::genapi::RangeViolation::OffIncrement { base: min, inc },
        }
        .into())
    }

    /// The binning, horizontal and vertical.
    pub fn binning(&self) -> Result<(i64, i64), GenicamError> {
        Ok((
            self.read_binning(StandardFeature::BinningHorizontal)?,
            self.read_binning(StandardFeature::BinningVertical)?,
        ))
    }

    fn read_binning(&self, feature: StandardFeature) -> Result<i64, GenicamError> {
        let resolved = self.resolve(feature)?;
        if resolved.kind != "Enumeration" {
            return self.get_integer(resolved.name);
        }
        let entry = self.get_enum(resolved.name)?;
        factor_of(&entry).ok_or_else(|| GenicamError::NoBinningFactor {
            feature: resolved.name,
            entry: entry.clone(),
        })
    }

    /// Combine `horizontal` × `vertical` pixels into one.
    ///
    /// An `<Integer>` is written the factor. An `<Enumeration>`, as the
    /// Hikrobot MV-CS050-10GC declares its binning, is written the entry
    /// whose name ends in the factor (`BinningHorizontal2`); one whose
    /// entries name no factor — SVS's `Off`/`On` — is refused with its
    /// entries listed. Both directions are checked before either is written.
    pub fn set_binning(&mut self, horizontal: i64, vertical: i64) -> Result<(), GenicamError> {
        let writes = [
            self.binning_write(StandardFeature::BinningHorizontal, horizontal)?,
            self.binning_write(StandardFeature::BinningVertical, vertical)?,
        ];
        for write in writes {
            match write {
                BinningWrite::Integer(name, factor) => self.set_integer(name, factor)?,
                BinningWrite::Entry(name, entry) => self.set_enum(name, &entry)?,
            }
        }
        Ok(())
    }

    fn binning_write(
        &self,
        feature: StandardFeature,
        factor: i64,
    ) -> Result<BinningWrite, GenicamError> {
        let resolved = self.resolve(feature)?;
        if resolved.kind != "Enumeration" {
            return Ok(BinningWrite::Integer(resolved.name, factor));
        }
        let entries = self.nodemap().enum_entries(resolved.name)?;
        let entry = entries
            .iter()
            .find(|entry| factor_of(entry) == Some(factor))
            .cloned()
            .ok_or_else(|| GenicamError::NoBinningEntry {
                feature: resolved.name,
                factor,
                entries: entries.clone(),
            })?;
        Ok(BinningWrite::Entry(resolved.name, entry))
    }

    /// The acquisition frame rate, in frames per second.
    pub fn frame_rate(&self) -> Result<f64, GenicamError> {
        let feature = self.resolve(StandardFeature::FrameRate)?;
        self.read_number(&feature)
    }

    /// Fix the acquisition frame rate at `hz`, and say which feature holds
    /// it. Where the camera has a switch for it
    /// ([`StandardFeature::FrameRateEnable`]) it is turned on first: until
    /// then the rate is often unavailable, or not applied.
    pub fn set_frame_rate(&mut self, hz: f64) -> Result<ResolvedFeature, GenicamError> {
        let rate = self.resolve(StandardFeature::FrameRate)?;
        if let Some(enable) = self.resolve_optional(StandardFeature::FrameRateEnable)? {
            self.set_bool(enable.name, true)?;
        }
        self.write_number(&rate, hz)?;
        Ok(rate)
    }

    /// Let the camera choose its frame rate again: turn
    /// [`StandardFeature::FrameRateEnable`] off. A camera without one has
    /// nothing to turn off, which is an error.
    pub fn disable_frame_rate(&mut self) -> Result<(), GenicamError> {
        let enable = self.resolve(StandardFeature::FrameRateEnable)?;
        self.set_bool(enable.name, false)
    }

    /// The user sets `UserSetSelector` offers, `Default` among them, in the
    /// XML's order.
    pub fn user_sets(&self) -> Result<Vec<String>, GenicamError> {
        let selector = self.resolve(StandardFeature::UserSetSelector)?;
        Ok(self.nodemap().enum_entries(selector.name)?)
    }

    /// Load user set `set` into the camera's settings, waiting on the
    /// calling thread for the camera to finish (up to [`COMMAND_TIMEOUT`]).
    ///
    /// A load changes features the XML need not name, so every cached value
    /// is forgotten afterwards.
    pub fn load_user_set(&mut self, set: &str) -> Result<(), GenicamError> {
        let loaded = self.run_user_set(StandardFeature::UserSetLoad, set);
        self.nodemap().invalidate_all();
        loaded
    }

    /// Save the camera's current settings into user set `set`, waiting on the
    /// calling thread for the camera to finish (up to [`COMMAND_TIMEOUT`]).
    /// A camera refuses to overwrite `Default`.
    pub fn save_user_set(&mut self, set: &str) -> Result<(), GenicamError> {
        self.run_user_set(StandardFeature::UserSetSave, set)
    }

    fn run_user_set(&mut self, command: StandardFeature, set: &str) -> Result<(), GenicamError> {
        let selector = self.resolve(StandardFeature::UserSetSelector)?;
        let command = self.resolve(command)?;
        self.set_enum(selector.name, set)?;
        self.execute_and_wait_blocking(command.name, COMMAND_TIMEOUT)
    }

    /// Make user set `set` the one the camera loads at power-up, and say
    /// which feature holds that: SFNC 2's `UserSetDefault`, or the older
    /// `UserSetDefaultSelector`.
    pub fn set_default_user_set(&mut self, set: &str) -> Result<ResolvedFeature, GenicamError> {
        let feature = self.resolve(StandardFeature::UserSetDefault)?;
        self.set_enum(feature.name, set)?;
        Ok(feature)
    }
}

#[cfg(test)]
mod tests {
    use super::factor_of;

    #[test]
    fn a_binning_entry_names_its_factor_by_its_last_digits() {
        assert_eq!(factor_of("BinningHorizontal2"), Some(2));
        assert_eq!(factor_of("X12"), Some(12));
        assert_eq!(factor_of("Off"), None);
        assert_eq!(factor_of(""), None);
    }
}
