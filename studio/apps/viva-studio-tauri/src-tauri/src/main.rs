#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
#[cfg(test)]
mod bindings;
mod commands;
mod error;
mod state;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing_subscriber::prelude::*;

use backend::BackendState;
use state::connection::Connection;
use state::events::Events;

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

    // ModelState: holds the last parsed UiGraph (used by xml_model commands).
    let model_state = RwLock::new(state::ModelState::default());

    // SfncGroupsState: cached SFNC group config loaded once from the bundled JSON.
    let sfnc_groups_state: commands::sfnc_groups::SfncGroupsState =
        Arc::new(RwLock::new(None::<Vec<commands::sfnc_groups::SfncGroup>>));

    // Events reach the UI once the app exists; the backends exist before it.
    let events = Arc::new(Events::default());
    let connection = Arc::new(Connection::new(events.clone()));

    // One backend; the commands never ask which.
    let start: Box<dyn FnOnce() + Send>;
    let backend_state: BackendState = match zenoh_config {
        Some(config) => {
            tracing::info!("Starting in REMOTE mode (Zenoh service)");
            let remote = backend::remote::RemoteBackend::new(events.clone(), connection.clone());
            let starter = remote.clone();
            start = Box::new(move || starter.start(config));
            remote
        }
        None => {
            tracing::info!("Starting in EMBEDDED mode (direct camera access)");
            let embedded = Arc::new(backend::embedded::EmbeddedBackend::new(events.clone()));
            let starter = embedded.clone();
            start =
                Box::new(move || starter.start_discovery_task(std::time::Duration::from_secs(3)));
            embedded
        }
    };
    let backend_for_exit = backend_state.clone();
    let events_for_setup = events.clone();

    if let Err(err) = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(model_state)
        .manage(sfnc_groups_state)
        .manage(backend_state)
        .manage(events)
        .manage(connection)
        .manage(commands::backend::StartupDiagnostics { zenoh_config_error })
        .setup(move |app| {
            events_for_setup.bind(app.handle().clone());
            start();
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
            // Device discovery & connection
            commands::device::list_discovered_devices,
            commands::device::get_connection_state,
            commands::device::connect_device,
            commands::device::disconnect_device,
            // Features (FeatureState contract — authoritative for the UI)
            commands::nodes::query_feature_state,
            commands::nodes::query_feature_states_bulk,
            commands::nodes::read_nodes_bulk,
            commands::nodes::write_node,
            commands::nodes::execute_command,
            // Acquisition
            commands::acquisition::start_acquisition,
            commands::acquisition::stop_acquisition,
            // SFNC groups config
            commands::sfnc_groups::get_sfnc_groups,
            // IP configuration (embedded mode only)
            commands::ip_config::force_ip,
            commands::ip_config::get_network_config,
            commands::ip_config::set_persistent_ip,
        ])
        .build(tauri::generate_context!())
        .map(|app| app.run(move |_, event| on_run_event(&event, &backend_for_exit)))
    {
        tracing::error!("error while running tauri application: {err}");
    }
}

/// How long the exit hook waits for the camera to be released.
const EXIT_RELEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Release the camera as the event loop ends.
///
/// Tauri exits the process without dropping managed state, so this is the only
/// chance: without it an embedded camera keeps acquiring and stays claimed
/// until its heartbeat expires, and a remote one keeps streaming.
fn on_run_event(event: &tauri::RunEvent, backend: &BackendState) {
    if let tauri::RunEvent::Exit = event {
        release_on_exit(backend.clone(), EXIT_RELEASE_TIMEOUT);
    }
}

/// Run [`DeviceBackend::shutdown`](backend::DeviceBackend::shutdown) and wait
/// at most `timeout` for it.
///
/// The release runs on its own task. Awaited in place, a release stuck on the
/// camera's `std::sync::Mutex` or inside `block_in_place` never yields, so the
/// timer could not fire and the app could not exit (Codex review of #168).
fn release_on_exit(backend: BackendState, timeout: std::time::Duration) {
    let release = tauri::async_runtime::spawn(async move { backend.shutdown().await });
    tauri::async_runtime::block_on(async {
        if tokio::time::timeout(timeout, release).await.is_err() {
            tracing::warn!("the camera was not released before exit");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tauri ends the process without dropping managed state, so the exit
    /// hook is the only place an embedded camera can be released.
    #[test]
    fn exit_releases_the_embedded_camera() {
        use backend::embedded::fake_camera;

        let _port = fake_camera::lock();
        let (fake, backend) = tauri::async_runtime::block_on(async {
            let fake = fake_camera::start().await;
            let backend = fake_camera::connect_and_acquire(&fake).await;
            (fake, backend)
        });

        // As in the app: on the main thread, outside the runtime.
        let backend: BackendState = Arc::new(backend);
        on_run_event(&tauri::RunEvent::Exit, &backend);

        tauri::async_runtime::block_on(fake_camera::assert_released(&fake));
    }

    /// A release that blocks without yielding — on the camera mutex, or
    /// inside `block_in_place` — must not hold the app open past the timeout.
    #[test]
    fn exit_does_not_wait_past_its_timeout_for_a_stuck_release() {
        let backend: BackendState = Arc::new(StuckBackend);
        let started = std::time::Instant::now();
        release_on_exit(backend, std::time::Duration::from_millis(200));
        let waited = started.elapsed();
        assert!(
            waited < std::time::Duration::from_secs(2),
            "waited {waited:?}"
        );
    }

    /// A backend whose release never yields.
    struct StuckBackend;

    #[async_trait::async_trait]
    impl backend::DeviceBackend for StuckBackend {
        fn mode(&self) -> backend::BackendMode {
            backend::BackendMode::Embedded
        }
        async fn discover(&self) -> Vec<state::device_state::DeviceInfo> {
            Vec::new()
        }
        async fn connect(&self, _: &str) -> Result<backend::ConnectResult, String> {
            Err("stuck".into())
        }
        async fn disconnect(&self) -> Result<(), String> {
            Ok(())
        }
        async fn get_feature_state(&self, _: &str) -> Result<viva_zenoh_api::FeatureState, String> {
            Err("stuck".into())
        }
        async fn bulk_feature_state(
            &self,
            _: &[String],
        ) -> Result<std::collections::HashMap<String, viva_zenoh_api::FeatureState>, String>
        {
            Err("stuck".into())
        }
        async fn set_feature(&self, _: &str, _: &serde_json::Value) -> Result<Vec<String>, String> {
            Err("stuck".into())
        }
        async fn exec_command(&self, _: &str) -> Result<Vec<String>, String> {
            Err("stuck".into())
        }
        async fn start_acquisition(&self) -> Result<state::device_state::StreamerInfo, String> {
            Err("stuck".into())
        }
        async fn stop_acquisition(&self) -> Result<(), String> {
            Ok(())
        }
        async fn shutdown(&self) {
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    }

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
