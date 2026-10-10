#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod commands;
mod error;
mod state;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing_subscriber::prelude::*;

use backend::BackendState;

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

/// Outcome of resolving the app's Zenoh configuration from the environment.
struct ZenohConfigOutcome {
    /// `Some` only in remote mode.
    config: Option<zenoh::Config>,
    /// Set only when `ZENOH_CONFIG` was present but could not be loaded.
    error: Option<String>,
}

/// Explain a `ZENOH_CONFIG` that was set but could not be loaded.
///
/// Kept separate from [`detect_zenoh_config`] so it can be tested without
/// constructing a `zenoh::Config`. The message has to carry the trap that
/// produced [#132]: the app keeps running, but in a mode that cannot see the
/// camera the user was pointing it at.
///
/// [#132]: https://github.com/VitalyVorobyev/viva-genicam/issues/132
fn zenoh_config_error_message(path: &str, err: &str) -> String {
    format!(
        "Failed to load ZENOH_CONFIG={path}: {err}. \
         Continuing in embedded mode (direct camera access), which does not \
         discover cameras on loopback — so a fake camera on 127.0.0.1 will not \
         appear. If the path is relative, make it absolute: the app's working \
         directory is not the repository root."
    )
}

/// Determine whether the app should run in remote (Zenoh) mode.
///
/// Remote mode requires the `ZENOH_CONFIG` environment variable to name a
/// loadable Zenoh config file. There is no default path and no dev-mode
/// auto-detection: embedded mode (direct camera access) is the default for
/// every other case, including a `ZENOH_CONFIG` that fails to load.
fn detect_zenoh_config() -> ZenohConfigOutcome {
    if let Ok(path) = std::env::var("ZENOH_CONFIG") {
        match zenoh::Config::from_file(&path) {
            Ok(cfg) => {
                tracing::info!("Loaded Zenoh config from ZENOH_CONFIG={path} — remote mode");
                return ZenohConfigOutcome {
                    config: Some(cfg),
                    error: None,
                };
            }
            Err(e) => {
                // The user explicitly asked for remote mode; starting embedded
                // instead is a surprise, so this is an error rather than a warning.
                let message = zenoh_config_error_message(&path, &e.to_string());
                tracing::error!("{message}");
                return ZenohConfigOutcome {
                    config: None,
                    error: Some(message),
                };
            }
        }
    }

    tracing::info!("No ZENOH_CONFIG set — using embedded mode (direct camera access)");
    ZenohConfigOutcome {
        config: None,
        error: None,
    }
}

fn dirs_log_path() -> std::path::PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let log_dir = base.join(".viva-studio").join("logs");
    std::fs::create_dir_all(&log_dir).ok();
    log_dir
}

fn main() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "viva_studio=info,warn".into());

    // File appender: daily rotation in ~/.viva-studio/logs/
    let log_dir = dirs_log_path();
    let file_appender = tracing_appender::rolling::daily(&log_dir, "studio.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(non_blocking),
        )
        .init();

    tracing::info!("Log directory: {}", log_dir.display());

    // Detect backend mode: embedded (default) or remote (Zenoh).
    let ZenohConfigOutcome {
        config: zenoh_config,
        error: zenoh_config_error,
    } = detect_zenoh_config();
    let is_remote_mode = zenoh_config.is_some();

    // ModelState: holds the last parsed UiGraph (used by xml_model commands).
    let model_state = RwLock::new(state::ModelState::default());

    // ZenohState: device discovery, connection, node cache, acquisition.
    // Wrapped in Arc so background tasks can hold a clone independently.
    let zenoh_state = Arc::new(state::ZenohState::new());
    let zenoh_for_setup = zenoh_state.clone();

    // SfncGroupsState: cached SFNC group config loaded once from the bundled JSON.
    let sfnc_groups_state: commands::sfnc_groups::SfncGroupsState =
        Arc::new(RwLock::new(None::<Vec<commands::sfnc_groups::SfncGroup>>));

    // Create the appropriate backend.
    let embedded_backend: Option<Arc<backend::embedded::EmbeddedBackend>> = if is_remote_mode {
        tracing::info!("Starting in REMOTE mode (Zenoh service)");
        None
    } else {
        tracing::info!("Starting in EMBEDDED mode (direct camera access)");
        Some(Arc::new(backend::embedded::EmbeddedBackend::new()))
    };

    let backend_state: BackendState = match &embedded_backend {
        Some(eb) => eb.clone(),
        None => Arc::new(backend::remote::RemoteBackend),
    };

    // Clone for the setup closure.
    let embedded_for_setup = embedded_backend.clone();

    if let Err(err) = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(model_state)
        .manage(zenoh_state)
        .manage(sfnc_groups_state)
        .manage(backend_state)
        .manage(commands::backend::StartupDiagnostics { zenoh_config_error })
        .setup(move |app| {
            let app_handle = app.handle().clone();

            if is_remote_mode {
                // Remote mode: initialize Zenoh and start discovery via Zenoh subscribers.
                let zenoh = zenoh_for_setup.clone();
                let zenoh_config = zenoh_config.expect("zenoh_config must be Some in remote mode");
                tauri::async_runtime::spawn(async move {
                    match zenoh::open(zenoh_config).await {
                        Ok(session) => {
                            tracing::info!("Zenoh session open, ZID: {}", session.zid());
                            *zenoh.session.lock().await = Some(Arc::new(session));
                            commands::device::start_discovery_task(zenoh, app_handle);
                        }
                        Err(e) => {
                            tracing::error!("Failed to open Zenoh session: {e}");
                        }
                    }
                });
            } else if let Some(embedded) = embedded_for_setup {
                // Embedded mode: start direct device discovery.
                embedded.start_discovery_task(app_handle, std::time::Duration::from_secs(3));
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            // Backend mode query
            commands::backend::backend_status,
            // XML model (offline + fixture mode)
            commands::xml_model::parse_xml,
            commands::xml_model::get_current_model,
            commands::xml_model::list_fixtures,
            commands::xml_model::load_fixture,
            // Device discovery & connection (Zenoh-based, backward compatible)
            commands::device::list_discovered_devices,
            commands::device::get_connection_state,
            commands::device::connect_device,
            commands::device::disconnect_device,
            // Node operations (Zenoh-based, backward compatible)
            commands::nodes::get_node_value,
            commands::nodes::write_node,
            commands::nodes::execute_command,
            commands::nodes::read_nodes_bulk,
            // Live feature state (FeatureState contract — authoritative for the UI)
            commands::nodes::query_feature_state,
            commands::nodes::query_feature_states_bulk,
            // Acquisition (Zenoh-based, backward compatible)
            commands::acquisition::get_acquisition_status,
            commands::acquisition::start_acquisition,
            commands::acquisition::stop_acquisition,
            // SFNC groups config
            commands::sfnc_groups::get_sfnc_groups,
            // Embedded backend commands
            commands::embedded_device::embedded_discover,
            commands::embedded_device::embedded_connect,
            commands::embedded_device::embedded_disconnect,
            commands::embedded_device::embedded_get_feature,
            commands::embedded_device::embedded_set_feature,
            commands::embedded_device::embedded_exec_command,
            commands::embedded_device::embedded_bulk_read,
            commands::embedded_device::embedded_start_acquisition,
            commands::embedded_device::embedded_stop_acquisition,
            // IP configuration (embedded mode only)
            commands::ip_config::force_ip,
            commands::ip_config::get_network_config,
            commands::ip_config::set_persistent_ip,
        ])
        .run(tauri::generate_context!())
    {
        tracing::error!("error while running tauri application: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_error_message_names_the_path_and_the_cause() {
        let msg = zenoh_config_error_message("/tmp/nope.json5", "No such file or directory");
        assert!(msg.contains("/tmp/nope.json5"));
        assert!(msg.contains("No such file or directory"));
    }

    #[test]
    fn config_error_message_explains_the_fallback() {
        let msg = zenoh_config_error_message("cfg.json5", "parse error");
        // The whole point of #132: the app keeps running, and the mode it keeps
        // running in cannot see a camera on loopback. Both halves must be said.
        assert!(msg.contains("embedded mode"));
        assert!(msg.contains("loopback"));
    }

    #[test]
    fn config_error_message_points_at_the_relative_path_trap() {
        let msg = zenoh_config_error_message("config/zenoh-studio.json5", "not found");
        assert!(msg.contains("absolute"));
    }
}
