use std::collections::HashMap;

use tokio::sync::{RwLock, broadcast};
use viva_xml_model::{UiGraph, UiNodeKind};
use viva_zenoh_api::NodeValueUpdate;

use crate::config::MockConfig;
use crate::interdependencies::apply_side_effects;

#[derive(Debug, Clone)]
pub struct NodeEntry {
    pub value: serde_json::Value,
    pub access_mode: String,
    pub kind: String,
    pub constraints: Option<NodeConstraints>,
}

#[derive(Debug, Clone)]
pub struct NodeConstraints {
    pub min: Option<f64>,
    pub max: Option<f64>,
    #[allow(dead_code)]
    pub inc: Option<f64>,
    pub enum_values: Vec<String>,
}

pub struct NodeStore {
    values: RwLock<HashMap<String, NodeEntry>>,
    change_tx: broadcast::Sender<(String, NodeValueUpdate)>,
}

impl NodeStore {
    /// Seed from UiGraph: for each node, extract a sensible default value.
    pub fn from_graph(graph: &UiGraph, config: &MockConfig) -> Self {
        let mut values = HashMap::new();

        for (name, node) in &graph.nodes_by_name {
            let entry = match &node.kind {
                UiNodeKind::Integer => {
                    let default =
                        node.constraints.as_ref().and_then(|c| c.min).unwrap_or(0.0) as i64;
                    NodeEntry {
                        value: serde_json::Value::Number(default.into()),
                        access_mode: node.access_mode.clone().unwrap_or_else(|| "RW".to_string()),
                        kind: "Integer".to_string(),
                        constraints: node.constraints.as_ref().map(|c| NodeConstraints {
                            min: c.min,
                            max: c.max,
                            inc: c.inc,
                            enum_values: vec![],
                        }),
                    }
                }
                UiNodeKind::Float => {
                    let default = node.constraints.as_ref().and_then(|c| c.min).unwrap_or(0.0);
                    NodeEntry {
                        value: serde_json::json!(default),
                        access_mode: node.access_mode.clone().unwrap_or_else(|| "RW".to_string()),
                        kind: "Float".to_string(),
                        constraints: node.constraints.as_ref().map(|c| NodeConstraints {
                            min: c.min,
                            max: c.max,
                            inc: c.inc,
                            enum_values: vec![],
                        }),
                    }
                }
                UiNodeKind::Boolean => NodeEntry {
                    value: serde_json::Value::Bool(false),
                    access_mode: node.access_mode.clone().unwrap_or_else(|| "RW".to_string()),
                    kind: "Boolean".to_string(),
                    constraints: None,
                },
                UiNodeKind::Enumeration => {
                    let default = node
                        .enum_entries
                        .first()
                        .map(|e| e.name.clone())
                        .unwrap_or_default();
                    NodeEntry {
                        value: serde_json::Value::String(default),
                        access_mode: node.access_mode.clone().unwrap_or_else(|| "RW".to_string()),
                        kind: "Enumeration".to_string(),
                        constraints: Some(NodeConstraints {
                            min: None,
                            max: None,
                            inc: None,
                            enum_values: node.enum_entries.iter().map(|e| e.name.clone()).collect(),
                        }),
                    }
                }
                UiNodeKind::String => NodeEntry {
                    value: serde_json::Value::String(String::new()),
                    access_mode: node.access_mode.clone().unwrap_or_else(|| "RW".to_string()),
                    kind: "String".to_string(),
                    constraints: None,
                },
                _ => continue, // Skip Category, Command, Register, Unknown
            };
            values.insert(name.clone(), entry);
        }

        Self::apply_sfnc_defaults(&mut values, config);

        let (change_tx, _) = broadcast::channel(256);
        Self {
            values: RwLock::new(values),
            change_tx,
        }
    }

    /// Apply well-known SFNC node defaults.
    fn apply_sfnc_defaults(values: &mut HashMap<String, NodeEntry>, config: &MockConfig) {
        if let Some(entry) = values.get_mut("Width") {
            entry.value = serde_json::json!(config.width);
        }
        if let Some(entry) = values.get_mut("Height") {
            entry.value = serde_json::json!(config.height);
        }
        if let Some(entry) = values.get_mut("WidthMax") {
            entry.value = serde_json::json!(4096);
        }
        if let Some(entry) = values.get_mut("HeightMax") {
            entry.value = serde_json::json!(3072);
        }
        if let Some(entry) = values.get_mut("SensorWidth") {
            entry.value = serde_json::json!(4096);
        }
        if let Some(entry) = values.get_mut("SensorHeight") {
            entry.value = serde_json::json!(3072);
        }
        if let Some(entry) = values.get_mut("ExposureTime") {
            entry.value = serde_json::json!(10000.0);
        }
        if let Some(entry) = values.get_mut("Gain") {
            entry.value = serde_json::json!(1.0);
        }
        if let Some(entry) = values.get_mut("PixelFormat") {
            entry.value = serde_json::json!("Mono8");
        }
        if let Some(entry) = values.get_mut("AcquisitionFrameRate") {
            entry.value = serde_json::json!(config.fps);
        }
        if let Some(entry) = values.get_mut("DeviceTemperature") {
            entry.value = serde_json::json!(42.5);
        }
        if let Some(entry) = values.get_mut("DeviceFirmwareVersion") {
            entry.value = serde_json::json!("1.0.0-mock");
        }
        if let Some(entry) = values.get_mut("DeviceSerialNumber") {
            entry.value = serde_json::json!(config.serial);
        }
        if let Some(entry) = values.get_mut("PayloadSize") {
            // Default pixel format is Mono8; keep in sync with the PixelFormat default above.
            let bpp = viva_zenoh_api::PixelFormat::Mono8.bytes_per_pixel();
            let size = (config.width as f32 * config.height as f32 * bpp) as u64;
            entry.value = serde_json::json!(size);
        }
        if let Some(entry) = values.get_mut("GevSCPSPacketSize") {
            entry.value = serde_json::json!(1500);
        }
        if let Some(entry) = values.get_mut("BlackLevel") {
            entry.value = serde_json::json!(0.0);
        }
        if let Some(entry) = values.get_mut("Gamma") {
            entry.value = serde_json::json!(1.0);
        }
    }

    pub async fn get(&self, name: &str) -> Option<NodeValueUpdate> {
        let values = self.values.read().await;
        values.get(name).map(|entry| NodeValueUpdate {
            value: entry.value.clone(),
            access_mode: entry.access_mode.clone(),
            min: None,
            max: None,
            inc: None,
        })
    }

    pub async fn get_value(&self, name: &str) -> Option<serde_json::Value> {
        let values = self.values.read().await;
        values.get(name).map(|entry| entry.value.clone())
    }

    pub async fn set(
        &self,
        name: &str,
        value: serde_json::Value,
    ) -> Result<NodeValueUpdate, String> {
        let mut values = self.values.write().await;
        let entry = values
            .get_mut(name)
            .ok_or_else(|| format!("Unknown node: {name}"))?;

        if entry.access_mode == "RO" {
            return Err(format!("Node {name} is read-only"));
        }

        // Type validation
        match entry.kind.as_str() {
            "Integer" => {
                if !value.is_i64() && !value.is_u64() {
                    return Err(format!("Node {name} expects an integer value"));
                }
                if let Some(ref c) = entry.constraints {
                    let v = value.as_i64().unwrap_or(0) as f64;
                    if let Some(min) = c.min
                        && v < min
                    {
                        return Err(format!("Value {v} below min {min} for {name}"));
                    }
                    if let Some(max) = c.max
                        && v > max
                    {
                        return Err(format!("Value {v} above max {max} for {name}"));
                    }
                }
            }
            "Float" => {
                if !value.is_f64() && !value.is_i64() && !value.is_u64() {
                    return Err(format!("Node {name} expects a numeric value"));
                }
                if let Some(ref c) = entry.constraints {
                    let v = value.as_f64().unwrap_or(0.0);
                    if let Some(min) = c.min
                        && v < min
                    {
                        return Err(format!("Value {v} below min {min} for {name}"));
                    }
                    if let Some(max) = c.max
                        && v > max
                    {
                        return Err(format!("Value {v} above max {max} for {name}"));
                    }
                }
            }
            "Boolean" if !value.is_boolean() => {
                return Err(format!("Node {name} expects a boolean value"));
            }
            "Enumeration" => {
                let s = value
                    .as_str()
                    .ok_or_else(|| format!("Node {name} expects a string value"))?;
                if let Some(ref c) = entry.constraints
                    && !c.enum_values.contains(&s.to_string())
                {
                    return Err(format!(
                        "Invalid enum value '{s}' for {name}. Valid: {:?}",
                        c.enum_values
                    ));
                }
            }
            "String" if !value.is_string() => {
                return Err(format!("Node {name} expects a string value"));
            }
            _ => {}
        }

        entry.value = value;
        let update = NodeValueUpdate {
            value: entry.value.clone(),
            access_mode: entry.access_mode.clone(),
            min: None,
            max: None,
            inc: None,
        };
        let _ = self.change_tx.send((name.to_string(), update.clone()));

        // Apply interdependency side effects while still holding the write lock.
        let secondary = apply_side_effects(name, &mut values);
        for (sec_name, sec_value) in secondary {
            if let Some(sec_entry) = values.get(&sec_name) {
                let sec_update = NodeValueUpdate {
                    value: sec_value,
                    access_mode: sec_entry.access_mode.clone(),
                    min: None,
                    max: None,
                    inc: None,
                };
                let _ = self.change_tx.send((sec_name, sec_update));
            }
        }

        Ok(update)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, NodeValueUpdate)> {
        self.change_tx.subscribe()
    }

    /// Get all current node values (for initial publish).
    pub async fn all(&self) -> Vec<(String, NodeValueUpdate)> {
        let values = self.values.read().await;
        values
            .iter()
            .map(|(name, entry)| {
                (
                    name.clone(),
                    NodeValueUpdate {
                        value: entry.value.clone(),
                        access_mode: entry.access_mode.clone(),
                        min: None,
                        max: None,
                        inc: None,
                    },
                )
            })
            .collect()
    }
}
