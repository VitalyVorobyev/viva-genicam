//! Studio is released with the library under one version (ADR-0024, backlog
//! ST-06): the release bundles carry it, so a bump that misses one of
//! Studio's manifests would ship an app that misreports what it is.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// The `version` under `[workspace.package]` in the manifest at `path`.
fn workspace_version(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    text.lines()
        .skip_while(|line| line.trim() != "[workspace.package]")
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .find_map(|line| line.trim().strip_prefix("version = "))
        .map(|version| version.trim_matches('"').to_string())
        .unwrap_or_else(|| panic!("{path:?} has no [workspace.package] version"))
}

#[test]
fn studio_is_versioned_with_the_library() {
    let library = workspace_version(&repo_root().join("Cargo.toml"));
    assert_eq!(
        env!("CARGO_PKG_VERSION"),
        library,
        "studio/apps/viva-studio-tauri/src-tauri/Cargo.toml"
    );
    assert_eq!(
        workspace_version(&repo_root().join("studio/Cargo.toml")),
        library,
        "studio/Cargo.toml"
    );
}

/// The bundles take their version from `tauri.conf.json`, or from this
/// crate's `Cargo.toml` when it names none; naming none leaves one place less
/// to bump.
#[test]
fn the_bundle_version_comes_from_cargo() {
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert!(
        conf.get("version").is_none(),
        "tauri.conf.json sets its own version"
    );
}
