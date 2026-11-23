//! AI tools (host plugin API v25): typed, read-only operations an AI client
//! calls over the automation bridge (`ocs_read` op `plugins` to list them,
//! `ocs_execute` op `plugin_call {plugin, tool, arguments}` to run one).
//!
//! Every tool is whole-drawing and side-effect free — the BOM tool serializes
//! the same tally `EL_BOM` prints, the project tool is the drawing census
//! `EL_INFO`'s neighbour. The host wraps each call in one undo step named
//! `<plugin id> <tool>`; these tools write nothing, so that step stays empty
//! and the host discards it. `arguments` is unused for now (whole-drawing),
//! so every schema is the empty object.

use ocs_plugin_api::host::{HostApi, PluginTool};
use serde_json::json;

use crate::codec;
use el_core::ElIndex;

pub fn tools() -> Vec<PluginTool> {
    fn tool(name: &str, description: &str) -> PluginTool {
        PluginTool {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: "{}".to_string(),
        }
    }
    vec![
        tool(
            "el_bom",
            "Return the schematic's bill of materials as structured JSON: \
             {title, headers, rows} — one row per (manufacturer, part number) \
             with quantities and the device tags that make it up. Same tally as \
             EL_BOM; devices without a part number are tallied under an \
             explicit \"no part number\" row, never dropped.",
        ),
        tool(
            "el_project_info",
            "Census of the active electrical drawing: {path, components, wires, \
             terminals, cables, connectors, ladders, signals, plcs, \
             terminal_strip_plots, footprints, title_block_present}. Read-only.",
        ),
    ]
}

pub fn call_tool(host: &mut dyn HostApi, name: &str, _arguments: &str) -> Result<String, String> {
    match name {
        "el_bom" => tool_bom(host),
        "el_project_info" => tool_project_info(host),
        other => Err(unknown_tool_error(other)),
    }
}

/// The unknown-name message: names the catalogue so a client can recover
/// without a second round-trip.
fn unknown_tool_error(name: &str) -> String {
    format!("unknown tool '{name}' — try el_bom, el_project_info")
}

/// `el_bom`: `el_core::report::bom`'s tally, serialized as structured JSON
/// (the TableLayout's public headers/rows, not a formatted text table).
fn tool_bom(host: &mut dyn HostApi) -> Result<String, String> {
    let idx = ElIndex::build(codec::scan(host.document()));
    let table = el_core::report::bom(&idx, None);
    Ok(json!({
        "title": table.title,
        "headers": table.headers,
        "rows": table.rows,
    })
    .to_string())
}

/// `el_project_info`: the drawing census, straight from the index buckets.
fn tool_project_info(host: &mut dyn HostApi) -> Result<String, String> {
    let idx = ElIndex::build(codec::scan(host.document()));
    Ok(json!({
        "path": host
            .document_path(host.tab_id())
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        "components": idx.components.len(),
        "wires": idx.wires.len(),
        "terminals": idx.terminals.len(),
        "cables": idx.cables.len(),
        "connectors": idx.connectors.len(),
        "ladders": idx.ladders.len(),
        "signals": idx.signals.len(),
        "plcs": idx.plcs.len(),
        "terminal_strip_plots": idx.strip_plots.len(),
        "footprints": idx.footprints.len(),
        "title_block_present": idx.title_block.is_some(),
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_catalog_is_named_and_schemaed() {
        let tools = tools();
        assert_eq!(tools.len(), 2);
        for t in &tools {
            assert!(
                t.name.starts_with("el_") && t.name.len() > 3,
                "tool names are snake_case and prefixed: {}",
                t.name
            );
            assert!(t.description.len() > 20, "{} needs a real description", t.name);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&t.input_schema)
                    .expect("schema is JSON"),
                serde_json::json!({}),
                "{} takes no arguments today",
                t.name
            );
        }
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["el_bom", "el_project_info"]);
    }

    #[test]
    fn unknown_tool_error_names_the_catalogue() {
        let err = unknown_tool_error("nope");
        assert!(err.contains("unknown tool 'nope'"), "{err}");
        assert!(err.contains("el_bom"), "error names the catalogue: {err}");
    }
}
