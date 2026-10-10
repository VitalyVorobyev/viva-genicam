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

    /// Record `state` and tell the UI.
    pub async fn set(&self, state: ConnectionState) {
        let mut current = self.state.lock().await;
        *current = state.clone();
        self.events.emit("connection-state-changed", state);
    }
}
