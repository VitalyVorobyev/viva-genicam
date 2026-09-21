use std::collections::HashMap;

use viva_zenoh_api::PixelFormat;

use crate::state::NodeEntry;

fn get_i64(values: &HashMap<String, NodeEntry>, name: &str) -> Option<i64> {
    values.get(name)?.value.as_i64()
}

fn parse_pixel_format(s: &str) -> PixelFormat {
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or(PixelFormat::Unknown)
}

fn recalculate_payload_size(values: &HashMap<String, NodeEntry>) -> Option<i64> {
    let width = get_i64(values, "Width")?;
    let height = get_i64(values, "Height")?;
    let pf_str = values
        .get("PixelFormat")?
        .value
        .as_str()
        .unwrap_or("Mono8")
        .to_string();
    let pixel_format = parse_pixel_format(&pf_str);
    let bpp = pixel_format.bytes_per_pixel();
    Some((width as f32 * height as f32 * bpp) as i64)
}

/// Apply side effects for node interdependencies after a primary write.
///
/// Returns a list of `(node_name, new_value)` pairs for secondary changes that
/// were applied in-place to `values`. The caller should broadcast these.
pub fn apply_side_effects(
    changed_node: &str,
    values: &mut HashMap<String, NodeEntry>,
) -> Vec<(String, serde_json::Value)> {
    let mut secondary = Vec::new();

    match changed_node {
        "Width" => {
            // OffsetX max = SensorWidth - Width; clamp current value
            let sensor_width = get_i64(values, "SensorWidth")
                .unwrap_or_else(|| get_i64(values, "WidthMax").unwrap_or(4096));
            let width = get_i64(values, "Width").unwrap_or(0);
            let new_offset_max = (sensor_width - width).max(0);

            if let Some(entry) = values.get_mut("OffsetX") {
                if let Some(ref mut c) = entry.constraints {
                    c.max = Some(new_offset_max as f64);
                }
                let current = entry.value.as_i64().unwrap_or(0);
                if current > new_offset_max {
                    entry.value = serde_json::json!(new_offset_max);
                    secondary.push(("OffsetX".to_string(), entry.value.clone()));
                }
            }

            if let Some(size) = recalculate_payload_size(values)
                && let Some(entry) = values.get_mut("PayloadSize")
            {
                entry.value = serde_json::json!(size);
                secondary.push(("PayloadSize".to_string(), entry.value.clone()));
            }
        }
        "Height" => {
            // OffsetY max = SensorHeight - Height; clamp current value
            let sensor_height = get_i64(values, "SensorHeight")
                .unwrap_or_else(|| get_i64(values, "HeightMax").unwrap_or(3072));
            let height = get_i64(values, "Height").unwrap_or(0);
            let new_offset_max = (sensor_height - height).max(0);

            if let Some(entry) = values.get_mut("OffsetY") {
                if let Some(ref mut c) = entry.constraints {
                    c.max = Some(new_offset_max as f64);
                }
                let current = entry.value.as_i64().unwrap_or(0);
                if current > new_offset_max {
                    entry.value = serde_json::json!(new_offset_max);
                    secondary.push(("OffsetY".to_string(), entry.value.clone()));
                }
            }

            if let Some(size) = recalculate_payload_size(values)
                && let Some(entry) = values.get_mut("PayloadSize")
            {
                entry.value = serde_json::json!(size);
                secondary.push(("PayloadSize".to_string(), entry.value.clone()));
            }
        }
        "PixelFormat" => {
            if let Some(size) = recalculate_payload_size(values)
                && let Some(entry) = values.get_mut("PayloadSize")
            {
                entry.value = serde_json::json!(size);
                secondary.push(("PayloadSize".to_string(), entry.value.clone()));
            }
        }
        _ => {}
    }

    secondary
}
