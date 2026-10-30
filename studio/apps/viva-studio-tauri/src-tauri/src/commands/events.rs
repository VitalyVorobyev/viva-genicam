//! Camera event monitoring commands, the same in both modes (backlog ST-51).

use tauri::State;
use viva_zenoh_api::EventMonitorStatus;

use crate::backend::BackendState;
use crate::error::HumanizeExt;
use crate::state::SharedModelState;

/// The `EventSelector` entries the camera offers now; empty for a camera
/// whose XML declares no `EventSelector`.
#[tauri::command]
pub async fn list_camera_events(
    backend: State<'_, BackendState>,
    model: State<'_, SharedModelState>,
) -> Result<Vec<String>, String> {
    let declared = model.read().await.graph.as_ref().is_some_and(|graph| {
        graph
            .nodes_by_name
            .contains_key(viva_genicam::sfnc::EVENT_SELECTOR)
    });
    if !declared {
        return Ok(Vec::new());
    }
    let state = backend
        .get_feature_state(viva_genicam::sfnc::EVENT_SELECTOR)
        .await
        .humanize()?;
    Ok(state.enum_available.unwrap_or_default())
}

/// Turn notification on for `events` and forward each one as a
/// `camera-event`, replacing any monitoring already on.
#[tauri::command]
pub async fn start_camera_events(
    events: Vec<String>,
    backend: State<'_, BackendState>,
) -> Result<EventMonitorStatus, String> {
    backend.start_events(events).await.humanize()
}

/// Stop monitoring and restore the camera's event settings; the status lists
/// what could not be restored.
#[tauri::command]
pub async fn stop_camera_events(
    backend: State<'_, BackendState>,
) -> Result<EventMonitorStatus, String> {
    backend.stop_events().await.humanize()
}
