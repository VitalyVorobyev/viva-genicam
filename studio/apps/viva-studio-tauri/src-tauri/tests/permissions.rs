//! Tauri refuses an app command that no permission granted to the window
//! allows: `permissions/*.toml` makes the build declare an app ACL manifest,
//! and from then on the ACL is enforced. A command missing from the sets
//! fails in the running app only — "Generate report" did (Codex review of
//! #174) — so the sets are checked here against the commands `main.rs`
//! registers and the commands the UI invokes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

/// The app commands the main window's capability allows.
fn granted() -> BTreeSet<String> {
    let capability: serde_json::Value =
        serde_json::from_str(&read(&crate_dir().join("capabilities/main.json"))).unwrap();
    let granted: BTreeSet<&str> = capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.as_str())
        .collect();

    let mut allowed = BTreeSet::new();
    for entry in std::fs::read_dir(crate_dir().join("permissions")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let file: toml::Table = toml::from_str(&read(&path)).unwrap();
        for permission in file["permission"].as_array().unwrap() {
            if !granted.contains(permission["identifier"].as_str().unwrap()) {
                continue;
            }
            let commands = permission["commands"]["allow"].as_array().unwrap();
            allowed.extend(commands.iter().map(|c| c.as_str().unwrap().to_string()));
        }
    }
    allowed
}

/// The commands `main.rs` hands `generate_handler!`, by function name.
fn registered() -> BTreeSet<String> {
    let main = read(&crate_dir().join("src/main.rs"));
    let start = main.find("generate_handler![").expect("generate_handler!") + 18;
    let end = start + main[start..].find(']').unwrap();
    main[start..end]
        .lines()
        .map(|line| line.trim().trim_end_matches(','))
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .map(|path| path.rsplit("::").next().unwrap().to_string())
        .collect()
}

/// The command names the UI passes to `invoke`, as string literals.
fn invoked_by_ui() -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut dirs = vec![crate_dir().join("../../../ui/viva-studio-ui/src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path: PathBuf = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy();
            let source = name.ends_with(".ts") || name.ends_with(".tsx");
            if !source || name.contains(".test.") {
                continue;
            }
            names.extend(invoke_literals(&read(&path)));
        }
    }
    names
}

/// `invoke("name"` and `invoke<T>("name"`, wherever they appear.
fn invoke_literals(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("invoke") {
        rest = &rest[at + "invoke".len()..];
        let mut call = rest;
        if call.starts_with('<') {
            let Some(close) = call.find(">(") else {
                continue;
            };
            call = &call[close + 1..];
        }
        if let Some(literal) = call.strip_prefix("(\"")
            && let Some(end) = literal.find('"')
        {
            names.push(literal[..end].to_string());
        }
    }
    names
}

#[test]
fn every_registered_command_is_granted() {
    let granted = granted();
    let refused: Vec<_> = registered()
        .into_iter()
        .filter(|c| !granted.contains(c))
        .collect();
    assert!(
        refused.is_empty(),
        "registered but no granted permission allows them, so Tauri refuses them: {refused:?}"
    );
}

#[test]
fn every_command_the_ui_invokes_is_registered() {
    let registered = registered();
    let invoked = invoked_by_ui();
    assert!(
        invoked.contains("generate_report"),
        "the scan finds the UI's calls"
    );
    let missing: Vec<_> = invoked.difference(&registered).collect();
    assert!(
        missing.is_empty(),
        "the UI invokes unregistered commands: {missing:?}"
    );
}

#[tauri::command]
fn generate_report() {}

#[tauri::command]
fn in_no_permission() {}

/// The same check as above, made by Tauri: calls sent through its IPC with
/// this app's generated context — its capabilities and permission sets — are
/// refused for a command no granted permission allows.
#[test]
fn tauri_refuses_what_no_permission_allows() {
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![generate_report, in_no_permission])
        .build(tauri::generate_context!())
        .unwrap();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    // The app's own origin: a debug build serves the UI from `devUrl`.
    let local = app.config().build.dev_url.clone().expect("devUrl");
    let call = |cmd: &str| {
        tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: local.clone(),
                body: tauri::ipc::InvokeBody::default(),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
    };
    let refused = call("in_no_permission").expect_err("the ACL lets anything through");
    assert!(refused.to_string().contains("not allowed"), "{refused}");
    call("generate_report").expect("generate_report is refused");
}
