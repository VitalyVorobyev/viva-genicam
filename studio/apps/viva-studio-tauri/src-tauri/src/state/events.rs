//! Events the backend sends to the UI.
//!
//! Both backends and the commands emit through [`Events`], not through a
//! `tauri::AppHandle` directly: the backends exist before the app does, and
//! the backend tests run without one.

use std::sync::OnceLock;
#[cfg(test)]
use std::sync::{Arc, Mutex};

use serde::Serialize;
use viva_zenoh_api::FeatureState;

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

    pub fn emit(&self, event: &str, payload: impl Serialize) {
        let Some(sink) = self.0.get() else {
            return;
        };
        match serde_json::to_value(payload) {
            Ok(payload) => sink.emit(event, payload),
            Err(error) => tracing::warn!(event, %error, "event payload not serializable"),
        }
    }

    /// A feature's new state, in the one shape both modes send: the legacy
    /// flat `value` and `access_mode`, plus the whole `state`.
    pub fn node_state(&self, node_name: &str, state: &FeatureState) {
        self.emit(
            "node-value-changed",
            serde_json::json!({
                "node_name": node_name,
                "value": state.value,
                "access_mode": state.access_mode,
                "state": state,
            }),
        );
    }

    /// The features a write or command may have changed; the UI re-reads the
    /// ones it shows (backlog ST-30).
    pub fn invalidated(&self, cause: &str, names: &[String]) {
        if !names.is_empty() {
            self.emit(
                "features-invalidated",
                serde_json::json!({ "cause": cause, "names": names }),
            );
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
