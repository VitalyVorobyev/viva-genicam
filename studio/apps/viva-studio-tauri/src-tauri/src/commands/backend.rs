//! Backend status query command.

use tauri::State;

use crate::backend::{BackendMode, BackendState};

/// Diagnostics collected while the app was starting, before any window existed.
///
/// The only entry so far is a `ZENOH_CONFIG` that was set but could not be
/// loaded. The app deliberately keeps running in embedded mode in that case, so
/// the frontend needs a way to tell the user why it is not in the mode they
/// asked for — otherwise the mismatch is visible only in the log.
#[derive(Default)]
pub struct StartupDiagnostics {
    pub zenoh_config_error: Option<String>,
}

/// What the frontend needs to show which backend the app is talking to.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct BackendStatus {
    /// The mode the app actually started in.
    pub mode: BackendMode,
    /// Why remote mode was requested but not entered, if that happened.
    pub zenoh_config_error: Option<String>,
}

/// The diagnostic report `viva-camctl report` writes, for the user to attach
/// to an issue (backlog ST-41).
#[tauri::command]
pub async fn generate_report(
    include_xml: bool,
    backend: State<'_, BackendState>,
) -> Result<String, String> {
    backend.report(include_xml).await
}

/// Return the current backend mode plus any startup diagnostic explaining it.
#[tauri::command]
pub async fn backend_status(
    backend: State<'_, BackendState>,
    diagnostics: State<'_, StartupDiagnostics>,
) -> Result<BackendStatus, String> {
    Ok(BackendStatus {
        mode: backend.mode(),
        zenoh_config_error: diagnostics.zenoh_config_error.clone(),
    })
}
