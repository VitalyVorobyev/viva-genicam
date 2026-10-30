//! Saving the camera's configuration to a file and restoring it, the same in
//! both modes (backlog ST-50). The camera saves and restores the
//! configuration — in this process, or in the service — and the file is read
//! and written here, at the path the user picked in a file dialog.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;
use viva_xml_model::UiNodeKind;
use viva_zenoh_api::{ConfigurationLoaded, ConfigurationOmission};

use crate::backend::BackendState;
use crate::error::HumanizeExt;
use crate::state::SharedModelState;
use crate::state::events::Events;

/// The largest configuration file a load reads. A camera's file is a few
/// kilobytes; this keeps a wrong pick from reading a disk image into memory.
const MAX_CONFIGURATION_BYTES: u64 = 16 * 1024 * 1024;

/// What a save wrote.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ConfigurationSaveReport {
    /// The file written.
    pub path: String,
    /// How many values it holds.
    pub values: usize,
    /// What was left out, and why; the file carries the same as comments.
    pub omitted: Vec<ConfigurationOmission>,
}

/// The file a dialog picked. Only an absolute path is taken: the app's
/// working directory is not anything the user chose.
fn chosen_file(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(format!(
            "Choose the file in the dialog: '{}' is not an absolute path",
            path.display()
        ));
    }
    Ok(path)
}

/// Read a configuration file of at most [`MAX_CONFIGURATION_BYTES`].
fn read_configuration_file(path: &std::path::Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Opening {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIGURATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Reading {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
        return Err(format!(
            "{} is larger than {} MiB: not a camera configuration",
            path.display(),
            MAX_CONFIGURATION_BYTES / (1024 * 1024)
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        format!(
            "{} is not a text file: not a camera configuration",
            path.display()
        )
    })
}

/// Save every feature the camera's XML marks `<Streamable>` to `path`, as a
/// GenApi feature file. Save while the camera is not acquiring: what
/// `TLParamsLocked` locks during acquisition is left out, and listed in
/// `omitted` with the reason.
#[tauri::command]
pub async fn save_camera_configuration(
    path: String,
    backend: State<'_, BackendState>,
) -> Result<ConfigurationSaveReport, String> {
    let file = chosen_file(&path)?;
    let saved = backend.save_configuration().await.humanize()?;
    let text = saved.text;
    let written = file.clone();
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&written, text))
        .await
        .map_err(|e| format!("Writing {}: {e}", file.display()))?
        .map_err(|e| format!("Writing {}: {e}", file.display()))?;
    Ok(ConfigurationSaveReport {
        path,
        values: saved.values,
        omitted: saved.omitted,
    })
}

/// Restore the configuration file at `path` to the camera. Every value that
/// could not be written is in the report, with its reason, as is a file
/// saved from another camera model. Every feature may have changed, so the
/// UI is told to read again whatever it shows.
#[tauri::command]
pub async fn load_camera_configuration(
    path: String,
    backend: State<'_, BackendState>,
    model: State<'_, SharedModelState>,
    events: State<'_, Arc<Events>>,
) -> Result<ConfigurationLoaded, String> {
    let file = chosen_file(&path)?;
    let text = tauri::async_runtime::spawn_blocking(move || read_configuration_file(&file))
        .await
        .map_err(|e| format!("Reading the configuration: {e}"))??;
    let loaded = backend.load_configuration(text).await.humanize()?;
    let features: Vec<String> = model
        .read()
        .await
        .graph
        .as_ref()
        .map(|graph| {
            graph
                .nodes_by_name
                .values()
                .filter(|node| !matches!(node.kind, UiNodeKind::Category))
                .map(|node| node.name.clone())
                .collect()
        })
        .unwrap_or_default();
    events.invalidated("configuration load", &features);
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::{MAX_CONFIGURATION_BYTES, chosen_file, read_configuration_file};

    #[test]
    fn only_an_absolute_path_is_taken() {
        let err = chosen_file("camera.txt").unwrap_err();
        assert!(err.contains("not an absolute path"), "{err}");
        let absolute = std::env::temp_dir().join("camera.txt");
        assert_eq!(chosen_file(absolute.to_str().unwrap()).unwrap(), absolute);
    }

    #[test]
    fn a_file_too_large_or_not_text_is_refused() {
        let dir = std::env::temp_dir().join(format!("viva-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.txt");
        std::fs::write(&big, vec![b'#'; MAX_CONFIGURATION_BYTES as usize + 1]).unwrap();
        assert!(
            read_configuration_file(&big)
                .unwrap_err()
                .contains("larger than")
        );
        let binary = dir.join("binary.txt");
        std::fs::write(&binary, [0xFF, 0xFE, 0x00]).unwrap();
        assert!(
            read_configuration_file(&binary)
                .unwrap_err()
                .contains("not a text file")
        );
        let text = dir.join("text.txt");
        std::fs::write(&text, "# a file\n").unwrap();
        assert_eq!(read_configuration_file(&text).unwrap(), "# a file\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
