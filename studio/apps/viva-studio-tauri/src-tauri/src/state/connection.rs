//! Which camera the app is connected to, shared by the commands and the
//! remote backend's reconnect loop.

use std::sync::Arc;

use tokio::sync::Mutex;

use super::device_state::ConnectionState;
use super::events::Events;

pub struct Connection {
    state: Mutex<ConnectionState>,
    events: Arc<Events>,
}

impl Connection {
    pub fn new(events: Arc<Events>) -> Self {
        Self {
            state: Mutex::new(ConnectionState::Disconnected),
            events,
        }
    }

    pub async fn get(&self) -> ConnectionState {
        self.state.lock().await.clone()
    }

    /// Publish only while the owner still considers this transition current.
    pub async fn set_when(&self, state: ConnectionState, current: impl FnOnce() -> bool) {
        let mut slot = self.state.lock().await;
        if current() {
            *slot = state.clone();
            self.events.send(&state);
        }
    }

    /// Record `state` and tell the UI.
    pub async fn set(&self, state: ConnectionState) {
        let mut current = self.state.lock().await;
        *current = state.clone();
        self.events.send(&state);
    }
}
