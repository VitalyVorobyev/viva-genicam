use serde::Serialize;
use viva_xml_model::UiGraph;

// Diagnostics are intentionally minimal in the MVP so the UI can surface parser
// warnings/errors without committing to a heavy schema.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[allow(dead_code)]
#[serde(rename_all = "lowercase")]
pub enum DiagLevel {
    Warning,
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
