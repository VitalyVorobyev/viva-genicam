//! A camera's configuration, saved and restored, and its raw `<Register>`
//! nodes: the queries behind Viva Studio's configuration and register tools
//! (backlog ST-50, ST-53).
//!
//! A configuration travels as the text of a GenApi feature file (ADR-0028):
//! the service saves and restores it on its camera, and the client reads and
//! writes the file where it runs.

use serde::{Deserialize, Serialize};

// ── Configuration ────────────────────────────────────────────────────────────

/// A feature a save left out, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct ConfigurationOmission {
    /// The feature, with the selector entries it was read under when only
    /// some of its values were left out: `Gain[GainSelector=DigitalAll]`.
    pub feature: String,
    /// Why: "not available", "locked by ExposureAuto", ...
    pub reason: String,
}

impl ConfigurationOmission {
    /// `feature` was left out for `reason`.
    pub fn new(feature: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            feature: feature.into(),
            reason: reason.into(),
        }
    }
}

/// Reply of the [`crate::keys::configuration_save`] queryable: the camera's
/// configuration as a GenApi feature file, and what the save left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ConfigurationSaved {
    /// The file, in the layout `viva_genicam::Camera::save_features` writes.
    pub text: String,
    /// How many values the file holds.
    pub values: usize,
    /// What was left out, and why; the file carries the same as comments.
    pub omitted: Vec<ConfigurationOmission>,
}

impl ConfigurationSaved {
    /// A saved configuration of `values` values, with nothing left out.
    pub fn new(text: impl Into<String>, values: usize) -> Self {
        Self {
            text: text.into(),
            values,
            omitted: Vec::new(),
        }
    }
}

/// Request payload for the [`crate::keys::configuration_load`] queryable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ConfigurationLoadRequest {
    /// The file to restore, as [`ConfigurationSaved::text`] holds one.
    pub text: String,
}

impl ConfigurationLoadRequest {
    /// Restore the configuration `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// What restoring a configuration did: the reply of the
/// [`crate::keys::configuration_load`] queryable. A file that cannot be read
/// at all is an error reply instead.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct ConfigurationLoaded {
    /// Values written.
    pub written: usize,
    /// Of `written`, those written only on the second pass, once the values
    /// they depend on were in place.
    pub written_on_retry: usize,
    /// Each value that could not be written, with the reason:
    /// `Gain[GainSelector=DigitalAll] = 9: <why>`.
    pub failures: Vec<String>,
    /// Doubts about the file as a whole: saved from another camera model, or
    /// from another version of its XML.
    pub warnings: Vec<String>,
}

#[cfg(feature = "genapi")]
#[cfg_attr(docsrs, doc(cfg(feature = "genapi")))]
impl From<&viva_genapi::persistence::Omission> for ConfigurationOmission {
    fn from(omission: &viva_genapi::persistence::Omission) -> Self {
        let mut feature = omission.feature.clone();
        if !omission.selectors.is_empty() {
            let selectors: Vec<String> = omission
                .selectors
                .iter()
                .map(|(selector, entry)| format!("{selector}={entry}"))
                .collect();
            feature = format!("{feature}[{}]", selectors.join(", "));
        }
        Self::new(feature, omission.reason.clone())
    }
}

#[cfg(feature = "genapi")]
#[cfg_attr(docsrs, doc(cfg(feature = "genapi")))]
impl From<&viva_genapi::Configuration> for ConfigurationSaved {
    fn from(configuration: &viva_genapi::Configuration) -> Self {
        Self {
            text: configuration.to_text(),
            values: configuration.values.len(),
            omitted: configuration.omitted.iter().map(Into::into).collect(),
        }
    }
}

#[cfg(feature = "genapi")]
#[cfg_attr(docsrs, doc(cfg(feature = "genapi")))]
impl From<&viva_genapi::LoadReport> for ConfigurationLoaded {
    fn from(report: &viva_genapi::LoadReport) -> Self {
        Self {
            written: report.written,
            written_on_retry: report.written_on_retry,
            failures: report.failures.iter().map(ToString::to_string).collect(),
            warnings: report.warnings.clone(),
        }
    }
}

// ── Registers ────────────────────────────────────────────────────────────────

/// Reply of the [`crate::keys::node_register`] queryable: a `<Register>`
/// node's bytes, in the order the device holds them. A register that cannot
/// be read is an error reply instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RegisterData {
    /// The bytes.
    pub data: Vec<u8>,
}

impl RegisterData {
    /// A register holding `data`.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

/// Request payload for the [`crate::keys::node_register_set`] queryable. The
/// reply is a [`crate::NodeOpResponse`] naming what the write invalidated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RegisterWriteRequest {
    /// The bytes to write: exactly the register's length, or the write is
    /// refused.
    pub data: Vec<u8>,
}

impl RegisterWriteRequest {
    /// Write `data`.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payloads round-trip through JSON, as they travel.
    #[test]
    fn the_payloads_round_trip() {
        let mut saved = ConfigurationSaved::new("# file\nWidth\t640\n", 1);
        saved
            .omitted
            .push(ConfigurationOmission::new("ExposureTime", "locked"));
        let back: ConfigurationSaved =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(back, saved);

        let loaded = ConfigurationLoaded {
            written: 3,
            written_on_retry: 1,
            failures: vec!["Gain = 9: refused".into()],
            warnings: vec!["saved from another model".into()],
        };
        let back: ConfigurationLoaded =
            serde_json::from_str(&serde_json::to_string(&loaded).unwrap()).unwrap();
        assert_eq!(back, loaded);

        let write = RegisterWriteRequest::new(vec![0, 1, 0xFF]);
        let json = serde_json::to_string(&write).unwrap();
        assert_eq!(json, r#"{"data":[0,1,255]}"#);
        let back: RegisterWriteRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, write);
    }

    /// A save's omissions and a load's failures keep the selector entries
    /// they concern, as a reader of the file sees them.
    #[cfg(feature = "genapi")]
    #[test]
    fn genapi_results_name_their_selector_entries() {
        let omission = viva_genapi::persistence::Omission {
            feature: "Gain".into(),
            selectors: vec![("GainSelector".into(), "DigitalAll".into())],
            reason: "not available".into(),
        };
        assert_eq!(
            ConfigurationOmission::from(&omission),
            ConfigurationOmission::new("Gain[GainSelector=DigitalAll]", "not available")
        );

        let configuration = viva_genapi::Configuration {
            omitted: vec![omission],
            ..Default::default()
        };
        let saved = ConfigurationSaved::from(&configuration);
        assert_eq!(saved.values, 0);
        assert_eq!(saved.omitted.len(), 1);
        assert!(
            saved.text.contains("viva-genicam configuration"),
            "{}",
            saved.text
        );
    }
}
