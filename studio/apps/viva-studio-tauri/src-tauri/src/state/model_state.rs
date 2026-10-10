use serde::Serialize;
use viva_xml_model::UiGraph;

// Diagnostics are intentionally minimal in the MVP so the UI can surface parser
// warnings/errors without committing to a heavy schema.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum DiagLevel {
    Warning,
    // Not produced yet: a parse that fails outright is a command error.
    #[allow(dead_code)]
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(optional_fields))]
pub struct Diag {
    pub level: DiagLevel,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

impl Diag {
    /// What the UI says about the declarations the library dropped, so a
    /// missing feature reads as dropped rather than absent (backlog DX-05).
    pub fn for_graph(graph: &UiGraph) -> Vec<Diag> {
        graph
            .skipped
            .iter()
            .map(|node| Diag {
                level: DiagLevel::Warning,
                message: format!(
                    "<{}> {}: {} — the camera declares this feature, but Studio cannot show it",
                    node.tag,
                    node.name.as_deref().unwrap_or("<unnamed>"),
                    node.error
                ),
                node: node.name.clone(),
            })
            .collect()
    }
}

// ModelState lives in Rust for Tauri mode so we avoid re-parsing on reload and
// keep a single authoritative UiGraph contract across the app lifecycle.
#[derive(Debug, Default)]
pub struct ModelState {
    pub xml: Option<String>,
    pub graph: Option<UiGraph>,
    pub diags: Vec<Diag>,
    pub loaded_from: Option<String>,
}

impl ModelState {
    pub fn update(
        &mut self,
        xml: String,
        graph: UiGraph,
        diags: Vec<Diag>,
        loaded_from: Option<String>,
    ) {
        self.xml = Some(xml);
        self.graph = Some(graph);
        self.diags = diags;
        self.loaded_from = loaded_from;
    }
}
