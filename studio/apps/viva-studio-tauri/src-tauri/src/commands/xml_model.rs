use crate::state::{Diag, ModelState};
use serde::Serialize;
use tauri::State;
use tokio::sync::RwLock;
use viva_xml_model::{UiGraph, parse_genicam_xml};

// Keep the response payload aligned with the WASM/TS UiGraph contract so the UI
// can swap providers without changing how it renders.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ModelSummary {
    pub node_count: usize,
    pub category_count: usize,
    pub root_category: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ParseXmlResponse {
    pub graph: UiGraph,
    pub xml: String,
    pub diags: Vec<Diag>,
    pub summary: ModelSummary,
}

// Structured error payload so the UI can show a friendly message and optional details.
#[derive(Debug, Serialize)]
pub struct CommandError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl CommandError {
    fn new(message: impl Into<String>, details: Option<String>) -> Self {
        Self {
            message: message.into(),
            details,
        }
    }
}

impl From<viva_xml_model::ParseError> for CommandError {
    fn from(value: viva_xml_model::ParseError) -> Self {
        Self::new("Failed to parse XML", Some(value.to_string()))
    }
}

struct Fixture {
    name: &'static str,
    xml: &'static str,
}

// Embed fixtures at compile time so the desktop app can load them without
// hitting the filesystem (keeps dev + packaged app behavior consistent).
const FIXTURES: &[Fixture] = &[Fixture {
    name: "minimal.xml",
    xml: include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/viva_xml_model/fixtures/minimal.xml"
    )),
}];

fn build_summary(graph: &UiGraph) -> ModelSummary {
    ModelSummary {
        node_count: graph.nodes_by_name.len(),
        category_count: graph.categories.len(),
        root_category: graph.root_category.clone(),
    }
}

fn build_response(graph: UiGraph, xml: String, diags: Vec<Diag>) -> ParseXmlResponse {
    let summary = build_summary(&graph);
    ParseXmlResponse {
        graph,
        xml,
        diags,
        summary,
    }
}

#[tauri::command]
pub async fn list_fixtures() -> Vec<String> {
    FIXTURES
        .iter()
        .map(|fixture| fixture.name.to_string())
        .collect()
}

#[tauri::command]
pub async fn parse_xml(
    xml: String,
    state: State<'_, RwLock<ModelState>>,
) -> Result<ParseXmlResponse, CommandError> {
    println!("parse_xml (native)");
    let graph = parse_genicam_xml(&xml)?;
    let diags = Diag::for_graph(&graph);
    let response_xml = xml.clone();
    let response_graph = graph.clone();
    let response_diags = diags.clone();

    {
        let mut state = state.write().await;
        state.update(xml, graph, diags, None);
    }

    Ok(build_response(response_graph, response_xml, response_diags))
}

#[tauri::command]
pub async fn load_fixture(
    name: String,
    state: State<'_, RwLock<ModelState>>,
) -> Result<ParseXmlResponse, CommandError> {
    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == name)
        .ok_or_else(|| CommandError::new(format!("Unknown fixture: {name}"), None))?;

    println!("load_fixture (native): {}", fixture.name);
    let xml = fixture.xml.to_string();
    let graph = parse_genicam_xml(&xml)?;
    let diags = Diag::for_graph(&graph);
    let response_xml = xml.clone();
    let response_graph = graph.clone();
    let response_diags = diags.clone();

    {
        let mut state = state.write().await;
        state.update(xml, graph, diags, Some(fixture.name.to_string()));
    }

    Ok(build_response(response_graph, response_xml, response_diags))
}

#[tauri::command]
pub async fn get_current_model(
    state: State<'_, RwLock<ModelState>>,
) -> Result<Option<ParseXmlResponse>, CommandError> {
    let state = state.read().await;
    let (graph, xml) = match (&state.graph, &state.xml) {
        (Some(graph), Some(xml)) => (graph, xml),
        _ => return Ok(None),
    };

    let response = ParseXmlResponse {
        graph: graph.clone(),
        xml: xml.clone(),
        diags: state.diags.clone(),
        summary: build_summary(graph),
    };

    Ok(Some(response))
}
