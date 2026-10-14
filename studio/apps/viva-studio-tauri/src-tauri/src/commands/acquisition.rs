//! Acquisition commands, the same in both modes.

use std::sync::Arc;

use tauri::State;
use viva_zenoh_api::AcquisitionStatus;

use crate::backend::{BackendState, StartOptions};
use crate::error::HumanizeExt;
use crate::state::device_state::StreamerInfo;
use crate::state::events::Events;

fn status(active: bool) -> AcquisitionStatus {
    if active {
        AcquisitionStatus::active(None, None)
    } else {
        AcquisitionStatus::inactive()
    }
}

/// Start streaming. `packet_size` is the GVSP packet size the user set for
/// this camera's link, a ceiling the path probe stays under; absent, it is
/// negotiated from the NIC's MTU (backlog ST-22).
#[tauri::command]
pub async fn start_acquisition(
    packet_size: Option<u32>,
    backend: State<'_, BackendState>,
    events: State<'_, Arc<Events>>,
) -> Result<StreamerInfo, String> {
    let info = backend
        .start_acquisition(StartOptions { packet_size })
        .await
        .humanize()?;
    events.send(&status(true));
    Ok(info)
}

#[tauri::command]
pub async fn stop_acquisition(
    backend: State<'_, BackendState>,
    events: State<'_, Arc<Events>>,
) -> Result<(), String> {
    let result = backend.stop_acquisition().await;
    events.send(&status(false));
    result.humanize()
}
