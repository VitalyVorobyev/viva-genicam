//! Events the backend sends to the UI.
//!
//! Both backends and the commands emit through [`Events`], not through a
//! `tauri::AppHandle` directly: the backends exist before the app does, and
//! the backend tests run without one.
//!
//! Every event is a [`StudioEvent`]: a payload type that carries its event
//! name. The UI's TypeScript types, `StudioEvents` among them, are generated
//! from these (`bindings.rs`, backlog ST-28), so an event's name and shape
//! cannot drift from what the UI listens for.

use std::sync::OnceLock;
#[cfg(test)]
use std::sync::{Arc, Mutex};

use serde::Serialize;
use viva_zenoh_api::{AcquisitionStatus, CameraEvent, EventMonitorStatus, FeatureState, ImageMeta};

use super::device_state::{ApiVersionMismatch, ConnectionState, DeviceInfo, DisconnectReason};

/// A payload the backend sends to the UI, and the event name it goes under.
pub trait StudioEvent: Serialize {
    const NAME: &'static str;
}

/// A feature's new state, the same in both modes.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct FeatureStateChanged {
    pub name: String,
    pub state: FeatureState,
}

/// The features a write or command may have changed, for the UI to re-read
/// the ones it shows (backlog ST-30).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct FeaturesInvalidated {
    /// The feature written or the command executed.
    pub cause: String,
    pub names: Vec<String>,
}

/// A device stopped announcing itself (remote discovery).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DeviceLost {
    pub device_id: String,
}

/// A lost camera was reopened or its remote service was resubscribed.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DeviceReconnected {
    pub device_id: String,
    pub node_count: usize,
    /// Freshly parsed replacement XML; the frontend must not reuse its old graph.
    pub model: crate::commands::xml_model::ParseXmlResponse,
}

/// The Zenoh session stopped answering (remote mode).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ZenohSessionLost {
    pub message: String,
}

/// The Zenoh session answers again (remote mode).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ZenohSessionRestored {
    pub message: String,
}

macro_rules! studio_events {
    ($($ty:ty => $name:literal),* $(,)?) => {
        $(impl StudioEvent for $ty {
            const NAME: &'static str = $name;
        })*

        /// Every event, as `(name, TypeScript payload type)`; the generated
        /// `StudioEvents` map is built from this.
        #[cfg(test)]
        pub fn all(cfg: &ts_rs::Config) -> Vec<(&'static str, String)> {
            use ts_rs::TS;
            vec![$(($name, <$ty>::name(cfg))),*]
        }
    };
}

studio_events! {
    DeviceInfo => "device-discovered",
    DeviceLost => "device-lost",
    ApiVersionMismatch => "api-version-mismatch",
    ConnectionState => "connection-state-changed",
    DisconnectReason => "disconnect-reason",
    DeviceReconnected => "device-reconnected",
    ZenohSessionLost => "zenoh-session-lost",
    ZenohSessionRestored => "zenoh-session-restored",
    FeatureStateChanged => "feature-state-changed",
    FeaturesInvalidated => "features-invalidated",
    AcquisitionStatus => "acquisition-status",
    ImageMeta => "image-meta-changed",
    CameraEvent => "camera-event",
    EventMonitorStatus => "camera-events-status",
}

/// Where events go: the webview in the app, a recorder in tests.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

impl EventSink for tauri::AppHandle {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if let Err(error) = tauri::Emitter::emit(self, event, payload) {
            tracing::debug!(event, %error, "event not delivered");
        }
    }
}

/// The app's event channel, bound once the app exists. Events sent before
/// that, or in a test that binds nothing, are dropped.
#[derive(Default)]
pub struct Events(OnceLock<Box<dyn EventSink>>);

impl Events {
    /// Send every later event to `sink`. Only the first call takes effect.
    pub fn bind(&self, sink: impl EventSink) {
        let _ = self.0.set(Box::new(sink));
    }

    pub fn send<E: StudioEvent>(&self, event: &E) {
        let Some(sink) = self.0.get() else {
            return;
        };
        match serde_json::to_value(event) {
            Ok(payload) => sink.emit(E::NAME, payload),
            Err(error) => tracing::warn!(event = E::NAME, %error, "event payload not serializable"),
        }
    }

    /// A feature's new state.
    pub fn node_state(&self, node_name: &str, state: &FeatureState) {
        self.send(&FeatureStateChanged {
            name: node_name.to_string(),
            state: state.clone(),
        });
    }

    /// The features `cause` may have changed, if any.
    pub fn invalidated(&self, cause: &str, names: &[String]) {
        if !names.is_empty() {
            self.send(&FeaturesInvalidated {
                cause: cause.to_string(),
                names: names.to_vec(),
            });
        }
    }
}

/// An [`EventSink`] that keeps what it is sent, for tests.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct Recorder(pub Arc<Mutex<Vec<(String, serde_json::Value)>>>);

#[cfg(test)]
impl EventSink for Recorder {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

#[cfg(test)]
impl Recorder {
    /// Events bound to a fresh recorder, and the recorder.
    pub fn events() -> (Arc<Events>, Self) {
        let recorder = Self::default();
        let events = Arc::new(Events::default());
        events.bind(recorder.clone());
        (events, recorder)
    }

    /// Payloads of every `event` received so far.
    pub fn of(&self, event: &str) -> Vec<serde_json::Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == event)
            .map(|(_, payload)| payload.clone())
            .collect()
    }
}
