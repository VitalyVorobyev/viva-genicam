//! Discovery and connection commands, the same in both modes.

use std::sync::Arc;

use tauri::State;

use crate::backend::BackendState;
use crate::commands::xml_model::ParseXmlResponse;
use crate::error::HumanizeExt;
use crate::state::connection::Connection;
use crate::state::device_state::{ConnectionState, DeviceInfo};

#[tauri::command]
pub async fn list_discovered_devices(
    backend: State<'_, BackendState>,
) -> Result<Vec<DeviceInfo>, String> {
    Ok(backend.discover().await)
}

#[tauri::command]
pub async fn get_connection_state(
    connection: State<'_, Arc<Connection>>,
) -> Result<ConnectionState, String> {
    Ok(connection.get().await)
}

#[tauri::command]
pub async fn connect_device(
    device_id: String,
    connection: State<'_, Arc<Connection>>,
    model: State<'_, crate::state::SharedModelState>,
    backend: State<'_, BackendState>,
) -> Result<ParseXmlResponse, String> {
    backend
        .connect_model(&device_id, &connection, &model)
        .await
        .humanize()
}

#[tauri::command]
pub async fn disconnect_device(
    connection: State<'_, Arc<Connection>>,
    backend: State<'_, BackendState>,
) -> Result<(), String> {
    backend.disconnect_model(&connection).await.humanize()
}
