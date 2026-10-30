//! GenICam device events — GigE Vision `EVENT`/`EVENTDATA` messages and USB3
//! Vision `EVENT_CMD`s — and the control that turns their delivery on and
//! off.
//!
//! Not event-vision blocks: those are [`crate::EvsHeader`] on
//! [`crate::keys::evs`].

use serde::{Deserialize, Serialize};

/// One event a camera sent, with the features its XML reads from it.
///
/// Published on [`crate::keys::events`] while event monitoring is on.
/// 64-bit counters are decimal strings, so a JavaScript client keeps every
/// digit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct CameraEvent {
    /// The event identifier the device sent, as `0x` and four hex digits.
    pub event_id: String,
    /// The `EventSelector` entry the identifier belongs to, when the XML says
    /// (`viva_genicam::Camera::event_name`).
    pub name: Option<String>,
    /// Device timestamp in ticks, decimal.
    pub ts_dev: String,
    /// Calibrated Unix nanoseconds; absent without a fitted device clock.
    /// Never the time the event arrived. Negative values precede the epoch.
    pub ts_host_ns: Option<String>,
    /// GigE Vision: the stream channel the event concerns. Absent for USB3
    /// Vision, whose events carry none.
    pub stream_channel: Option<u16>,
    /// GigE Vision: the block the event concerns, decimal. Absent for USB3
    /// Vision.
    pub block_id: Option<String>,
    /// The features the event's port feeds, each with its value or the
    /// error that kept it from being read, sorted by name.
    pub features: Vec<EventFeature>,
}

/// A feature read from one event, or why it could not be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EventFeature {
    /// Feature name from the camera XML.
    pub name: String,
    /// GenApi's textual value.
    pub value: Option<String>,
    /// The read error, when there is no value.
    pub error: Option<String>,
}

impl CameraEvent {
    /// Attach the features read from this event, as
    /// `viva_genicam::Camera::event_features` returns them.
    pub fn with_features<E: std::fmt::Display>(
        mut self,
        features: impl IntoIterator<Item = (String, Result<String, E>)>,
    ) -> Self {
        self.features = features
            .into_iter()
            .map(|(name, value)| match value {
                Ok(value) => EventFeature {
                    name,
                    value: Some(value),
                    error: None,
                },
                Err(error) => EventFeature {
                    name,
                    value: None,
                    error: Some(error.to_string()),
                },
            })
            .collect();
        self
    }

    /// Set the resolved `EventSelector` entry.
    pub fn with_name(mut self, name: Option<String>) -> Self {
        self.name = name;
        self
    }
}

#[cfg(feature = "genicam")]
#[cfg_attr(docsrs, doc(cfg(feature = "genicam")))]
impl CameraEvent {
    /// The wire form of `event`, without a name or features yet. A USB3
    /// Vision event (`usb`) has no stream channel or block of its own: both
    /// are left absent rather than reporting the zeros the library fills in.
    pub fn new(event: &viva_genicam::Event, usb: bool) -> Self {
        let ts_host_ns = event
            .ts_host
            .map(|t| match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => d.as_nanos().to_string(),
                Err(e) => format!("-{}", e.duration().as_nanos()),
            });
        Self {
            event_id: format!("0x{:04X}", event.id),
            name: None,
            ts_dev: event.ts_dev.to_string(),
            ts_host_ns,
            stream_channel: (!usb).then_some(event.stream_channel),
            block_id: (!usb).then(|| event.block_id.to_string()),
            features: Vec::new(),
        }
    }
}

/// Request payload for the [`crate::keys::events_control`] queryable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EventControlRequest {
    /// Start or stop monitoring.
    pub command: EventCommand,
    /// For `Start`: the `EventSelector` entries to turn notification on for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
}

impl EventControlRequest {
    /// Start monitoring `events`, replacing any monitoring already on.
    pub fn start(events: Vec<String>) -> Self {
        Self {
            command: EventCommand::Start,
            events,
        }
    }

    /// Stop monitoring and restore the settings starting it changed.
    pub fn stop() -> Self {
        Self {
            command: EventCommand::Stop,
            events: Vec::new(),
        }
    }
}

/// What an event control request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventCommand {
    /// Configure the camera's event delivery and start forwarding events.
    Start,
    /// Stop forwarding and restore the camera's previous event settings.
    Stop,
}

/// Whether events are being monitored, and what went wrong putting the
/// camera's event settings back.
///
/// The reply to every [`EventControlRequest`], and published on
/// [`crate::keys::events_status`] whenever monitoring starts or stops —
/// including when it stops by itself, because the camera was lost or its
/// event channel failed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[non_exhaustive]
pub struct EventMonitorStatus {
    /// Whether events are being forwarded.
    pub active: bool,
    /// The `EventSelector` entries monitored; empty when inactive.
    pub events: Vec<String>,
    /// Why monitoring stopped by itself, and each setting that could not be
    /// restored when it stopped. Empty when stopping restored everything.
    pub warnings: Vec<String>,
}

impl EventMonitorStatus {
    /// Monitoring `events`.
    pub fn active(events: Vec<String>) -> Self {
        Self {
            active: true,
            events,
            warnings: Vec::new(),
        }
    }

    /// Not monitoring, with what stopping could not undo.
    pub fn inactive(warnings: Vec<String>) -> Self {
        Self {
            active: false,
            events: Vec::new(),
            warnings,
        }
    }
}

/// What an incomplete restoration leaves to tell the user, one line per
/// setting: never an empty list for a report that is not complete.
#[cfg(feature = "genicam")]
#[cfg_attr(docsrs, doc(cfg(feature = "genicam")))]
pub fn restore_warnings(report: &viva_genicam::EventRestoreReport) -> Vec<String> {
    let mut warnings: Vec<String> = report
        .failures
        .iter()
        .map(|failure| format!("{} was not restored: {}", failure.setting, failure.error))
        .collect();
    if let Some(skipped) = report.skipped {
        use viva_genicam::EventRestoreSkipped;
        warnings.push(match skipped {
            EventRestoreSkipped::DifferentCamera => {
                "Event settings were not restored: the camera session was replaced".into()
            }
            EventRestoreSkipped::NotInControl(state) => {
                format!("Event settings were not restored: camera control is {state:?}")
            }
            EventRestoreSkipped::Disconnected => {
                "Event settings were not restored: the camera is disconnected".into()
            }
            _ => "Event settings were not restored".into(),
        });
    }
    warnings
}

/// A failed event setup as one message: the error, and each setting its
/// rollback could not restore.
#[cfg(feature = "genicam")]
#[cfg_attr(docsrs, doc(cfg(feature = "genicam")))]
pub fn setup_error_message(error: &viva_genicam::EventSetupError) -> String {
    let mut message = format!("Starting events failed: {}", error.source);
    for warning in restore_warnings(&error.restoration) {
        message.push_str("; ");
        message.push_str(&warning);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The control request and the status round-trip in the shape the
    /// service and Studio agree on.
    #[test]
    fn control_and_status_have_a_stable_wire_shape() {
        let start = EventControlRequest::start(vec!["ExposureEnd".into()]);
        let json = serde_json::to_value(&start).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"command": "start", "events": ["ExposureEnd"]})
        );
        assert_eq!(
            serde_json::from_value::<EventControlRequest>(serde_json::json!({"command": "stop"}))
                .unwrap(),
            EventControlRequest::stop()
        );
        let status = EventMonitorStatus::inactive(vec!["EventNotification[First] failed".into()]);
        let back: EventMonitorStatus =
            serde_json::from_slice(&serde_json::to_vec(&status).unwrap()).unwrap();
        assert_eq!(back, status);
    }

    #[test]
    fn features_keep_their_value_or_error() {
        let event = CameraEvent {
            event_id: "0x9001".into(),
            name: None,
            ts_dev: "18446744073709551615".into(),
            ts_host_ns: None,
            stream_channel: None,
            block_id: None,
            features: Vec::new(),
        }
        .with_features([
            ("A".to_string(), Ok::<_, String>("1".to_string())),
            ("B".to_string(), Err("unreadable".to_string())),
        ]);
        assert_eq!(event.features[0].value.as_deref(), Some("1"));
        assert_eq!(event.features[1].error.as_deref(), Some("unreadable"));
        assert_eq!(event.ts_dev, u64::MAX.to_string());
    }

    #[cfg(feature = "genicam")]
    #[test]
    fn a_usb_event_has_no_stream_channel_or_block() {
        let event = viva_genicam::Event {
            id: 0x9001,
            stream_channel: 0,
            block_id: 0,
            ts_dev: 42,
            ts_host: None,
            data: Default::default(),
            raw: Default::default(),
        };
        let usb = CameraEvent::new(&event, true);
        assert_eq!(usb.event_id, "0x9001");
        assert_eq!((usb.stream_channel, usb.block_id), (None, None));
        let gige = CameraEvent::new(&event, false);
        assert_eq!(gige.stream_channel, Some(0));
        assert_eq!(gige.block_id.as_deref(), Some("0"));
    }
}
