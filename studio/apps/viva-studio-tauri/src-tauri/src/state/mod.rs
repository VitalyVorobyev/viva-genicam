pub mod connection;
pub mod device_state;
pub mod events;
mod model_state;

#[cfg(test)]
pub use model_state::DiagLevel;
pub use model_state::{Diag, ModelState};

/// The one parsed model, shared by commands and automatic session replacement.
pub type SharedModelState = std::sync::Arc<tokio::sync::RwLock<ModelState>>;
