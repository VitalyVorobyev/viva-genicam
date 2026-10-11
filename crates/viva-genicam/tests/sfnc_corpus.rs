//! API-14 against real vendor XML: every standard feature the `Camera`
//! helpers use, resolved on every vendor-corpus document. Each resolves to a
//! node of a kind the helper can drive, or reports that the camera has none
//! of the names it goes by — and names them.
//!
//! The kinds a helper accepts are listed here, not taken from the facade: the
//! test must be able to disagree with what the helpers chose.
//!
//! ```sh
//! scripts/fetch-xml-corpus.sh
//! cargo test -p viva-genicam --test sfnc_corpus -- --nocapture
//! ```
//!
//! `VIVA_GENICAM_XML_CORPUS` points it at another directory. Without the
//! corpus the test passes with a note.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use viva_genicam::genapi::{NodeMap, NullIo};
use viva_genicam::{Camera, GenicamError, StandardFeature};

fn corpus_dir() -> PathBuf {
    std::env::var("VIVA_GENICAM_XML_CORPUS").map_or_else(
        |_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/vendor-xml"),
        PathBuf::from,
    )
}

/// The node kinds each helper can drive.
fn accepted(feature: StandardFeature) -> &'static [&'static str] {
    use StandardFeature as F;
    const NUMBER: &[&str] = &["Float", "Converter", "Integer", "IntConverter"];
    const INTEGER: &[&str] = &["Integer", "IntConverter"];
    match feature {
        F::ExposureTime | F::Gain | F::FrameRate => NUMBER,
        F::Width | F::Height | F::OffsetX | F::OffsetY => INTEGER,
        F::BinningHorizontal | F::BinningVertical => &["Integer", "IntConverter", "Enumeration"],
        F::TriggerSoftware | F::UserSetLoad | F::UserSetSave => &["Command"],
        F::FrameRateEnable => &["Boolean"],
        F::TlParamsLocked => &["Integer", "Boolean"],
        _ => &["Enumeration"],
    }
}

#[test]
fn every_standard_feature_resolves_to_a_drivable_node_or_names_what_it_tried() {
    let dir = corpus_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!(
            "corpus not present at {}; run scripts/fetch-xml-corpus.sh",
            dir.display()
        );
        return;
    };
    let mut documents: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
        .collect();
    documents.sort();

    // feature -> (resolved name, or "-" for absent) -> documents
    let mut counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut failures = Vec::new();
    for path in &documents {
        let document = path.file_name().unwrap().to_string_lossy().into_owned();
        let xml = String::from_utf8_lossy(&std::fs::read(path).expect("read")).into_owned();
        let model = viva_genicam::genapi::NodeMap::try_from_xml(
            viva_genapi_xml::parse(&xml).expect("the XML corpus test covers parsing"),
        );
        let nodemap: NodeMap = model.expect("the GenApi corpus test covers building");
        let mut camera = Camera::new(NullIo, nodemap);

        for &feature in StandardFeature::ALL {
            let outcome = match camera.resolve(feature) {
                Ok(resolved) => {
                    if !accepted(feature).contains(&resolved.kind) {
                        failures.push(format!(
                            "{document}: {feature:?} resolved to {} ({}), which its helper cannot drive",
                            resolved.name, resolved.kind
                        ));
                    }
                    format!("{} ({})", resolved.name, resolved.kind)
                }
                Err(err @ GenicamError::MissingFeature { tried, .. }) => {
                    let message = err.to_string();
                    if tried != feature.names() || !tried.iter().all(|name| message.contains(name))
                    {
                        failures.push(format!("{document}: {feature:?}: {message}"));
                    }
                    "-".to_string()
                }
                Err(err) => {
                    failures.push(format!("{document}: {feature:?}: {err}"));
                    "error".to_string()
                }
            };
            *counts
                .entry(format!("{feature:?}"))
                .or_default()
                .entry(outcome)
                .or_default() += 1;
        }

        // `acquisition_start` writes TLParamsLocked; the write must go
        // through on every document that has it, floating or not.
        if let Ok(locked) = camera.resolve(StandardFeature::TlParamsLocked) {
            let written = match locked.kind {
                "Boolean" => camera.set_bool(locked.name, true),
                _ => camera.set_integer(locked.name, 1),
            };
            if let Err(err) = written {
                failures.push(format!("{document}: writing {}: {err}", locked.name));
            }
        }
    }

    println!("{} documents", documents.len());
    for (feature, outcomes) in &counts {
        let summary: Vec<String> = outcomes
            .iter()
            .map(|(outcome, n)| format!("{outcome}: {n}"))
            .collect();
        println!("{feature:18} {}", summary.join(", "));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
