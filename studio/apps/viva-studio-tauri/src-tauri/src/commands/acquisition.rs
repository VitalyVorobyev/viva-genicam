//! Acquisition commands, the same in both modes.

use std::sync::Arc;

use tauri::State;
use viva_zenoh_api::AcquisitionStatus;

use crate::backend::BackendState;
use crate::error::HumanizeExt;
use crate::state::device_state::StreamerInfo;
use crate::state::events::Events;

fn status(active: bool) -> AcquisitionStatus {
    AcquisitionStatus {
        active,
        fps: None,
        dropped: 0,
    }
}

#[tauri::command]
pub async fn start_acquisition(
    backend: State<'_, BackendState>,
    events: State<'_, Arc<Events>>,
) -> Result<StreamerInfo, String> {
    let info = backend.start_acquisition().await.humanize()?;
    events.emit("acquisition-status", status(true));
    Ok(info)
}

#[tauri::command]
pub async fn stop_acquisition(
    backend: State<'_, BackendState>,
    events: State<'_, Arc<Events>>,
) -> Result<(), String> {
    let result = backend.stop_acquisition().await;
    events.emit("acquisition-status", status(false));
    result.humanize()
}
