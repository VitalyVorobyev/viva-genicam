use std::sync::Arc;
use tauri::Manager;
use tokio::sync::RwLock;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SfncFeature {
    pub node: String,
    pub widget: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SfncGroup {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub default_open: bool,
    pub features: Vec<SfncFeature>,
}

#[derive(serde::Deserialize)]
struct SfncGroupsFile {
    groups: Vec<SfncGroup>,
}

pub type SfncGroupsState = Arc<RwLock<Option<Vec<SfncGroup>>>>;

/// Parse raw bytes from a bundled `sfnc-groups.json` into a `Vec<SfncGroup>`.
///
/// Extracted as a pure function (no I/O, no async) so it can be unit-tested
/// without a Tauri app handle.
pub fn parse_sfnc_groups(bytes: &[u8]) -> Result<Vec<SfncGroup>, String> {
    serde_json::from_slice::<SfncGroupsFile>(bytes)
        .map(|f| f.groups)
        .map_err(|e| format!("Failed to parse sfnc-groups.json: {e}"))
}

/// Return the SFNC group configuration, loading from the bundled resource on
/// the first call and serving from an in-memory cache on subsequent calls.
///
/// The read-lock fast path avoids disk I/O after the first successful load.
/// A double-read race on the very first call is harmless — both parses produce
/// identical results, and the last writer wins.
#[tauri::command]
pub async fn get_sfnc_groups(
    app: tauri::AppHandle,
    state: tauri::State<'_, SfncGroupsState>,
) -> Result<Vec<SfncGroup>, String> {
    {
        let guard = state.read().await;
        if let Some(groups) = guard.as_ref() {
            return Ok(groups.clone());
        }
    }
    // Cold path: read from bundled resource.
    let path = app
        .path()
        .resolve(
            "resources/sfnc-groups.json",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|e| format!("Failed to resolve resource path: {e}"))?;
    let bytes =
        std::fs::read(&path).map_err(|e| format!("Failed to read sfnc-groups.json: {e}"))?;
    let groups = parse_sfnc_groups(&bytes)?;
    *state.write().await = Some(groups.clone());
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::parse_sfnc_groups;

    #[test]
    fn test_parse_sfnc_groups_happy_path() {
        let json = r#"{
            "groups": [
                {
                    "id": "acquisition_control",
                    "title": "Acquisition Control",
                    "icon": "▶",
                    "default_open": true,
                    "features": [
                        { "node": "AcquisitionMode", "widget": "enum_select" },
                        { "node": "AcquisitionStart", "widget": "command_button" }
                    ]
                }
            ]
        }"#;
        let result = parse_sfnc_groups(json.as_bytes()).expect("should parse successfully");
        assert_eq!(result.len(), 1);
        let group = &result[0];
        assert_eq!(group.id, "acquisition_control");
        assert_eq!(group.title, "Acquisition Control");
        assert_eq!(group.icon, "▶");
        assert!(group.default_open);
        assert_eq!(group.features.len(), 2);
        assert_eq!(group.features[0].node, "AcquisitionMode");
        assert_eq!(group.features[0].widget, "enum_select");
        assert_eq!(group.features[1].node, "AcquisitionStart");
        assert_eq!(group.features[1].widget, "command_button");
    }

    #[test]
    fn test_parse_sfnc_groups_empty_groups() {
        let json = r#"{"groups":[]}"#;
        let result = parse_sfnc_groups(json.as_bytes())
            .expect("empty groups array should parse without error");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_sfnc_groups_invalid_json() {
        let err = parse_sfnc_groups(b"not json").expect_err("invalid JSON should return Err");
        assert!(
            err.contains("Failed to parse sfnc-groups.json"),
            "error message should contain 'Failed to parse sfnc-groups.json', got: {err}"
        );
    }

    #[test]
    fn test_parse_sfnc_groups_missing_groups_key() {
        let json = r#"{"items":[]}"#;
        let err =
            parse_sfnc_groups(json.as_bytes()).expect_err("missing 'groups' key should return Err");
        assert!(
            err.contains("Failed to parse sfnc-groups.json"),
            "error message should contain 'Failed to parse sfnc-groups.json', got: {err}"
        );
    }

    #[test]
    fn test_parse_sfnc_groups_unknown_widget_string() {
        let json = r#"{
            "groups": [
                {
                    "id": "custom",
                    "title": "Custom Group",
                    "icon": "?",
                    "default_open": false,
                    "features": [
                        { "node": "SomeNode", "widget": "custom_widget" }
                    ]
                }
            ]
        }"#;
        // Rust accepts any string as the widget field — unknown widget types
        // must not cause a parse failure so that future config additions are
        // forward-compatible with already-compiled binaries.
        let result =
            parse_sfnc_groups(json.as_bytes()).expect("unknown widget string should parse Ok");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].features[0].widget, "custom_widget");
    }

    /// The UI draws only the widgets `FeatureControl.tsx` lists in `WIDGETS`;
    /// a feature named with any other falls back to its kind's widget.
    #[test]
    fn bundled_groups_name_only_widgets_the_ui_draws() {
        const DRAWN: &[&str] = &[
            "float_slider",
            "int_slider",
            "int_select",
            "enum_select",
            "bool_toggle",
            "command_button",
            "text",
        ];
        let bytes = include_bytes!("../../resources/sfnc-groups.json");
        for group in parse_sfnc_groups(bytes).expect("bundled groups parse") {
            for feature in &group.features {
                assert!(
                    DRAWN.contains(&feature.widget.as_str()),
                    "{}: {} names widget {:?}",
                    group.id,
                    feature.node,
                    feature.widget
                );
            }
        }
    }
}
