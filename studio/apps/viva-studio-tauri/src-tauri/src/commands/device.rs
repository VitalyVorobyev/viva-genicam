//! Discovery and connection commands, the same in both modes.

use std::sync::Arc;

use tauri::State;
use tokio::sync::RwLock;

use crate::backend::BackendState;
use crate::commands::xml_model::{ModelSummary, ParseXmlResponse};
use crate::error::HumanizeExt;
use crate::state::connection::Connection;
use crate::state::device_state::{ConnectionState, DeviceInfo};
use crate::state::{Diag, ModelState};
use viva_xml_model::parse_genicam_xml;

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
    model: State<'_, RwLock<ModelState>>,
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

    let graph = match parse_genicam_xml(&result.xml) {
        Ok(graph) => graph,
        Err(e) => {
            let message = format!("The camera's GenICam XML could not be read: {e}");
            backend.disconnect().await.ok();
            connection
                .set(ConnectionState::Error {
                    message: message.clone(),
                })
                .await;
            return Err(message);
        }
    };
    let response = ParseXmlResponse {
        summary: ModelSummary {
            node_count: graph.nodes_by_name.len(),
            category_count: graph.categories.len(),
            root_category: graph.root_category.clone(),
        },
        graph: graph.clone(),
        xml: result.xml.clone(),
        diags: Diag::for_graph(&graph),
    };
    let diags = response.diags.clone();
    model
        .write()
        .await
        .update(result.xml, graph, diags, Some(device_id.clone()));

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
