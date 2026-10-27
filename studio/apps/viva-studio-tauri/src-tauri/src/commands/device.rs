//! Discovery and connection commands, the same in both modes.

use std::sync::Arc;

use tauri::State;

use crate::backend::BackendState;
use crate::commands::xml_model::{ParseXmlResponse, parse_camera_model};
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
    connection
        .set(ConnectionState::Connecting {
            device_id: device_id.clone(),
        })
        .await;

    let result = match backend.connect(&device_id).await.humanize() {
        Ok(r) => r,
        Err(message) => {
            connection
                .set(ConnectionState::Error {
                    message: message.clone(),
                })
                .await;
            return Err(message);
        }
    };

    let response = match parse_camera_model(result.xml) {
        Ok(response) => response,
        Err(message) => {
            backend.disconnect().await.ok();
            connection
                .set(ConnectionState::Error {
                    message: message.clone(),
                })
                .await;
            return Err(message);
        }
    };
    model.write().await.update(
        response.xml.clone(),
        response.graph.clone(),
        response.diags.clone(),
        Some(device_id.clone()),
    );

    connection
        .set(ConnectionState::Connected {
            device_id,
            device_name: result.device_name,
            model: result.model,
        })
        .await;
    Ok(response)
}

#[tauri::command]
pub async fn disconnect_device(
    connection: State<'_, Arc<Connection>>,
    backend: State<'_, BackendState>,
) -> Result<(), String> {
    let result = backend.disconnect().await;
    connection.set(ConnectionState::Disconnected).await;
    result.humanize()
}
