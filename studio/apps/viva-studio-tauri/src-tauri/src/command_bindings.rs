//! Test-time command contract, read from the Rust signatures Tauri registers.
//!
//! Unsupported types fail generation rather than silently weakening the UI's types.

use std::collections::{BTreeMap, BTreeSet};
use syn::{FnArg, GenericArgument, Item, Pat, PathArguments, ReturnType, Type};
use ts_rs::{Config, TS};

fn camel_case(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in name.trim_start_matches("r#").chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn arguments(ty: &syn::PathSegment) -> Vec<&Type> {
    match &ty.arguments {
        PathArguments::AngleBracketed(args) => args
            .args
            .iter()
            .filter_map(|a| match a {
                GenericArgument::Type(ty) => Some(ty),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn type_name(ty: &Type, cfg: &Config) -> String {
    match ty {
        Type::Reference(ty) => type_name(&ty.elem, cfg),
        Type::Tuple(ty) if ty.elems.is_empty() => "null".into(),
        Type::Path(ty) => {
            let segment = ty.path.segments.last().expect("type path");
            let args = arguments(segment);
            match segment.ident.to_string().as_str() {
                "Result" => type_name(args[0], cfg),
                "Option" => format!("{} | null", type_name(args[0], cfg)),
                "Vec" => format!("Array<{}>", type_name(args[0], cfg)),
                "HashMap" | "BTreeMap" => format!(
                    "Record<{}, {}>",
                    type_name(args[0], cfg),
                    type_name(args[1], cfg)
                ),
                "String" | "str" => <String>::name(cfg),
                "bool" => <bool>::name(cfg),
                "u8" => <u8>::name(cfg),
                "u32" => <u32>::name(cfg),
                "Value" => <serde_json::Value>::name(cfg),
                "FeatureState" => <viva_zenoh_api::FeatureState>::name(cfg),
                "CommandResult" => <viva_zenoh_api::CommandResult>::name(cfg),
                "EventMonitorStatus" => <viva_zenoh_api::EventMonitorStatus>::name(cfg),
                "ParseXmlResponse" => <crate::commands::xml_model::ParseXmlResponse>::name(cfg),
                "ConnectionAccess" => <crate::state::device_state::ConnectionAccess>::name(cfg),
                "DeviceInfo" => <crate::state::device_state::DeviceInfo>::name(cfg),
                "ConnectionState" => <crate::state::device_state::ConnectionState>::name(cfg),
                "StreamerInfo" => <crate::state::device_state::StreamerInfo>::name(cfg),
                "BackendStatus" => <crate::commands::backend::BackendStatus>::name(cfg),
                "NetworkConfig" => <crate::backend::NetworkConfig>::name(cfg),
                "SfncGroup" => <crate::commands::sfnc_groups::SfncGroup>::name(cfg),
                "ConfigurationLoaded" => <viva_zenoh_api::ConfigurationLoaded>::name(cfg),
                "ConfigurationSaveReport" => {
                    <crate::commands::configuration::ConfigurationSaveReport>::name(cfg)
                }
                "ActionCommandResult" => <crate::commands::action::ActionCommandResult>::name(cfg),
                name => panic!("unsupported IPC type {name}: add its ts-rs mapping"),
            }
        }
        _ => panic!("unsupported IPC type"),
    }
}

fn command(function: &syn::ItemFn, cfg: &Config) -> (String, String) {
    let mut fields = Vec::new();
    for input in &function.sig.inputs {
        let FnArg::Typed(input) = input else {
            panic!("command with self")
        };
        let Type::Path(ty) = &*input.ty else {
            panic!("command arguments must be owned")
        };
        let segment = ty.path.segments.last().unwrap();
        if segment.ident == "State" || segment.ident == "AppHandle" {
            continue;
        }
        let Pat::Ident(name) = &*input.pat else {
            panic!("destructured command argument")
        };
        let optional = if segment.ident == "Option" { "?" } else { "" };
        fields.push(format!(
            "{}{}: {}",
            camel_case(&name.ident.to_string()),
            optional,
            type_name(&input.ty, cfg)
        ));
    }
    let result = match &function.sig.output {
        ReturnType::Default => "null".into(),
        ReturnType::Type(_, ty) => type_name(ty, cfg),
    };
    let args = if fields.is_empty() {
        "Record<string, never>".to_string()
    } else {
        format!("{{ {} }}", fields.join("; "))
    };
    (
        function.sig.ident.to_string(),
        format!("{{ args: {args}; result: {result} }}"),
    )
}

/// Names and signatures of exactly the functions handed to `generate_handler!`.
pub(super) fn all(cfg: &Config) -> BTreeMap<String, String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let main = std::fs::read_to_string(root.join("main.rs")).unwrap();
    let handler = main
        .split("tauri::generate_handler![")
        .nth(1)
        .expect("handler")
        .split(']')
        .next()
        .unwrap();
    let registered: BTreeSet<_> = handler
        .lines()
        .map(|l| l.split("//").next().unwrap().trim().trim_end_matches(','))
        .filter(|l| !l.is_empty())
        .map(|l| l.rsplit("::").next().unwrap().to_owned())
        .collect();
    let mut files = vec![root.join("main.rs")];
    files.extend(
        std::fs::read_dir(root.join("commands"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "rs")),
    );
    let mut commands = BTreeMap::new();
    for file in files {
        let parsed = syn::parse_file(&std::fs::read_to_string(file).unwrap()).expect("Rust source");
        for item in parsed.items {
            let Item::Fn(function) = item else { continue };
            if !function.attrs.iter().any(|a| {
                a.path()
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    == ["tauri", "command"]
            }) {
                continue;
            }
            assert!(
                function
                    .attrs
                    .iter()
                    .filter(|a| a
                        .path()
                        .segments
                        .last()
                        .is_some_and(|s| s.ident == "command"))
                    .all(|a| matches!(a.meta, syn::Meta::Path(_))),
                "command options need explicit binding support"
            );
            if registered.contains(&function.sig.ident.to_string()) {
                let (name, signature) = command(&function, cfg);
                assert!(
                    commands.insert(name, signature).is_none(),
                    "duplicate command name"
                );
            }
        }
    }
    assert_eq!(
        commands.keys().cloned().collect::<BTreeSet<_>>(),
        registered,
        "every registered command must have a generated signature"
    );
    commands
}

#[test]
fn signature_uses_wire_arguments_and_success_type() {
    let function = syn::parse_quote! {
        async fn renamed_command(node_name: String, packet_size: Option<u32>, state: tauri::State<'_, String>, app: tauri::AppHandle) -> Result<Vec<String>, String> { todo!() }
    };
    let (name, signature) = command(&function, &Config::new());
    assert_eq!(name, "renamed_command");
    assert_eq!(
        signature,
        "{ args: { nodeName: string; packetSize?: number | null }; result: Array<string> }"
    );
    let unit = syn::parse_quote! { fn no_arguments() {} };
    assert_eq!(
        command(&unit, &Config::new()).1,
        "{ args: Record<string, never>; result: null }"
    );
}
