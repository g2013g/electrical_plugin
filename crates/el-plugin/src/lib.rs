//! Electrical plugin for Open CAD Studio (host v2026.36.0, plugin API 5).
//!
//! ABI contract: for api_version >= 4 the host load gate requires
//! `[opencad] rustc_version` in `plugin.toml` to match its own `rustc --version`
//! token-for-token — see `crates/el-plugin/plugin.toml` for the host's embedded
//! version record and `rust-toolchain.toml` for the toolchain that satisfies it.
//!
//! Electrical control-schematic workflow, modelled on AutoCAD Electrical's
//! three-tier design as far as a plugin with no project database can reach:
//! a per-drawing settings record (ACADE's invisible `WD_M` block) carries every
//! tag and wire-number format; ladder rungs supply line references; components,
//! wires, terminals and cables are records attached to entities as XDATA under
//! the `OCSEL` application id, so the data survives DWG/DXF round-trips.
//!
//! Layer map: `el-core` (host-free domain logic, plain `cargo test`) → this
//! crate (host adapter + commands). The host is never imported beyond
//! `ocs_plugin_api`.
//!
//! Two host limits shape nearly every command here, and both are load-bearing:
//! there is no way to create a block definition, so symbols are drawn as
//! primitives; and interactive callbacks run against a `NullHost`, so anything
//! a click handler needs must be captured at dispatch time (see `hostops`).

mod cmds;
mod codec;
mod library;
mod geometry;
mod hostops;
mod probe;
mod state;

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

pub use el_core::model::APPID;

static MANIFEST: PluginManifest = PluginManifest {
    id: "opencad.electrical",
    name: "Electrical",
    // Read from Cargo.toml (workspace `version`) so the DLL's manifest can
    // never drift from the built package version.
    version: env!("CARGO_PKG_VERSION"),
    description: "Electrical control-schematic design: ladders, wire numbering, component tagging, reports, audit",
    api_version: ApiVersion::CURRENT,
    // The wiring-harness plugin claims 60; plugin tabs sort ascending, so this
    // one lands to its right.
    ribbon_order: 70,
    xdata_apps: &["OCSEL"],
    // Autocomplete hint only — the host offers every command to every plugin
    // in load order, so a distinct prefix is collision avoidance, not routing.
    command_prefixes: &["EL_"],
};

fn tool(id: &'static str, label: &'static str, icon: &'static str, cmd: &str) -> RibbonItem {
    RibbonItem::Tool(ToolDef {
        id,
        label,
        icon: IconKind::Glyph(icon),
        event: ModuleEvent::Command(cmd.to_string()),
    })
}

fn large_tool(id: &'static str, label: &'static str, icon: &'static str, cmd: &str) -> RibbonItem {
    RibbonItem::LargeTool(ToolDef {
        id,
        label,
        icon: IconKind::Glyph(icon),
        event: ModuleEvent::Command(cmd.to_string()),
    })
}

/// The plugin's ribbon tab.
struct ElectricalModule;

impl CadModule for ElectricalModule {
    fn id(&self) -> &'static str {
        "electrical"
    }

    fn title(&self) -> &'static str {
        "Electrical"
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        // The host may call this repeatedly, so the Vec is built once and the
        // returned slice stays stable.
        static GROUPS: std::sync::OnceLock<Vec<RibbonGroup>> = std::sync::OnceLock::new();
        GROUPS.get_or_init(|| {
            vec![
                // Drawing setup: the settings record everything else reads its
                // formats from, and the capability probe.
                RibbonGroup {
                    title: "Setup",
                    tools: vec![
                        large_tool("EL_SETTINGS", "Settings", "⚙", "EL_SETTINGS"),
                        tool("EL_WIRETYPE", "Wire Types", "≡", "EL_WIRETYPE"),
                        tool("EL_PROJECT", "Project", "☰", "EL_PROJECT"),
                        tool("EL_PROBE", "Probe", "⌖", "EL_PROBE"),
                    ],
                },
                // Ladders: the reference substrate that reference-based
                // numbering reads from.
                RibbonGroup {
                    title: "Ladder",
                    tools: vec![
                        large_tool("EL_LADDER", "Ladder", "☰", "EL_LADDER"),
                        tool("EL_RUNG", "Add Rung", "⁝", "EL_RUNG ADD=1"),
                    ],
                },
                // Wiring: drawing conductors and numbering them.
                RibbonGroup {
                    title: "Wiring",
                    tools: vec![
                        large_tool("EL_WIRE", "Wire", "⤳", "EL_WIRE"),
                        RibbonItem::LargeDropdown {
                            id: "EL_WIRENO_DD",
                            label: "Number",
                            icon: IconKind::Glyph("#"),
                            items: vec![
                                ("EL_WIRENO", "Number all wires", IconKind::Glyph("#")),
                                ("EL_WIRENO CLEAR", "Clear drawn numbers", IconKind::Glyph("⌫")),
                            ],
                            default: "EL_WIRENO",
                        },
                        RibbonItem::Dropdown {
                            id: "EL_WIREGAP_DD",
                            icon: IconKind::Glyph("⌒"),
                            items: vec![
                                ("EL_WIREGAP ALL", "Gap every wire at crossings", IconKind::Glyph("⌒")),
                                ("EL_WIREGAP", "Which wires are gapped", IconKind::Glyph("≡")),
                                ("EL_WIREGAP CLEAR ALL", "Draw every wire whole again", IconKind::Glyph("⌫")),
                                ("EL_WIREGAP GAP=1.6", "Widen the gap to 1.6mm", IconKind::Glyph("±")),
                            ],
                            default: "EL_WIREGAP ALL",
                        },
                        tool("EL_TRIMWIRE", "Trim", "✂", "EL_TRIMWIRE"),
                    ],
                },
                // Devices: placing symbols from the library, and the
                // parent/child links that make contacts belong to coils.
                RibbonGroup {
                    title: "Devices",
                    tools: vec![
                        RibbonItem::LargeDropdown {
                            id: "EL_COMPONENT_DD",
                            label: "Component",
                            icon: IconKind::Glyph("▣"),
                            items: vec![
                                ("EL_COMPONENT", "Pick a symbol", IconKind::Glyph("▣")),
                                ("EL_COMPONENT K", "Relay / contactor coil", IconKind::Glyph("▭")),
                                ("EL_COMPONENT contact-no", "Contact, normally open", IconKind::Glyph("⌇")),
                                ("EL_COMPONENT contact-nc", "Contact, normally closed", IconKind::Glyph("⌇")),
                                ("EL_COMPONENT pushbutton-no", "Pushbutton, NO", IconKind::Glyph("⎍")),
                                ("EL_COMPONENT Q", "Circuit breaker", IconKind::Glyph("⏻")),
                                ("EL_COMPONENT F", "Fuse", IconKind::Glyph("▭")),
                                ("EL_COMPONENT M", "Motor", IconKind::Glyph("Ⓜ")),
                                ("EL_COMPONENT H", "Indicator lamp", IconKind::Glyph("⊗")),
                                ("EL_COMPONENT X", "Terminal", IconKind::Glyph("○")),
                            ],
                            default: "EL_COMPONENT",
                        },
                        tool("EL_LINK", "Link", "⇄", "EL_LINK"),
                        tool("EL_XREF", "Cross-ref", "⇉", "EL_XREF"),
                        tool("EL_RETAG", "Retag", "⟲", "EL_RETAG"),
                        RibbonItem::Dropdown {
                            id: "EL_SIGNAL_DD",
                            icon: IconKind::Glyph("⇥"),
                            items: vec![
                                ("EL_SIGNAL", "Place a signal arrow", IconKind::Glyph("⇥")),
                                ("EL_SIGNAL LIST", "List arrows and their pairing", IconKind::Glyph("≡")),
                                ("EL_SIGNAL CLEAR ALL", "Remove every arrow", IconKind::Glyph("⌫")),
                            ],
                            default: "EL_SIGNAL",
                        },
                    ],
                },
                // Terminals, cables and connectors: the objects that make a
                // schematic a wiring diagram rather than a device list.
                RibbonGroup {
                    title: "Wiring Devices",
                    tools: vec![
                        RibbonItem::LargeDropdown {
                            id: "EL_TERMINAL_DD",
                            label: "Terminal",
                            icon: IconKind::Glyph("▢"),
                            items: vec![
                                ("EL_TERMINAL screw", "Screw terminal", IconKind::Glyph("▢")),
                                ("EL_TERMINAL spring", "Spring terminal", IconKind::Glyph("▢")),
                                ("EL_TERMINAL ground", "Earth terminal", IconKind::Glyph("⏚")),
                                ("EL_TERMINAL fused", "Fused terminal", IconKind::Glyph("▤")),
                                ("EL_TERMINAL disconnect", "Disconnect terminal", IconKind::Glyph("⏻")),
                                ("EL_TERMINAL multi", "Multi-level terminal", IconKind::Glyph("▥")),
                                ("EL_TERMINAL test", "Test terminal", IconKind::Glyph("⊙")),
                                ("EL_TERMINAL NUMBER", "Renumber strips", IconKind::Glyph("#")),
                                ("EL_TERMINAL JUMPER", "Bridge two terminals", IconKind::Glyph("⌢")),
                                ("EL_TERMINAL JUMPER LIST", "List jumpers", IconKind::Glyph("≡")),
                                ("EL_TERMINAL REORDER", "Renumber in a stated order", IconKind::Glyph("⇄")),
                                ("EL_TERMINAL PARTITION", "Assign a partition group", IconKind::Glyph("│")),
                                ("EL_TERMINAL", "List terminals", IconKind::Glyph("≡")),
                            ],
                            default: "EL_TERMINAL screw",
                        },
                        large_tool("EL_CABLE", "Cable", "⌇", "EL_CABLE"),
                        RibbonItem::Dropdown {
                            id: "EL_PANEL_DD",
                            icon: IconKind::Glyph("▭"),
                            items: vec![
                                ("EL_PANEL", "Place a footprint", IconKind::Glyph("▭")),
                                ("EL_PANEL LIST", "List footprints", IconKind::Glyph("≡")),
                                ("EL_PANEL NUMBER", "Renumber items", IconKind::Glyph("#")),
                                ("EL_BALLOON ALL", "Balloon everything", IconKind::Glyph("○")),
                                ("EL_REPORT PANELBOM", "Panel BOM", IconKind::Glyph("▤")),
                                ("EL_PLC", "PLC module", IconKind::Glyph("▥")),
                                ("EL_PLC LIST", "List modules", IconKind::Glyph("≡")),
                                ("EL_REPORT PLCIO", "PLC I/O list", IconKind::Glyph("⌗")),
                            ],
                            default: "EL_PANEL",
                        },
                        large_tool("EL_CONNECTOR", "Connector", "⊞", "EL_CONNECTOR"),
                    ],
                },
                // Parts and deliverables.
                RibbonGroup {
                    title: "Parts",
                    tools: vec![
                        large_tool("EL_CATALOG", "Catalogue", "▤", "EL_CATALOG"),
                        RibbonItem::Dropdown {
                            id: "EL_CATALOG_DD",
                            icon: IconKind::Glyph("⋯"),
                            items: vec![
                                ("EL_CATALOG LIST", "List parts", IconKind::Glyph("≡")),
                                ("EL_CATALOG SEARCH relay", "Search parts", IconKind::Glyph("⌕")),
                                ("EL_CATALOG FAMILY K", "Parts for a family", IconKind::Glyph("⌗")),
                                ("EL_CATALOG PATH", "Where the catalogue lives", IconKind::Glyph("⌂")),
                            ],
                            default: "EL_CATALOG LIST",
                        },
                        RibbonItem::LargeDropdown {
                            id: "EL_REPORT_DD",
                            label: "Report",
                            icon: IconKind::Glyph("⎙"),
                            items: vec![
                                ("EL_REPORT BOM", "Bill of materials", IconKind::Glyph("▤")),
                                ("EL_REPORT COMPONENTS", "Component list", IconKind::Glyph("≡")),
                                ("EL_REPORT WIREFROMTO", "Wire from/to", IconKind::Glyph("⇄")),
                                ("EL_REPORT WIRELABELS", "Wire labels", IconKind::Glyph("⊏")),
                                ("EL_REPORT TERMINALS", "Terminal plan", IconKind::Glyph("▢")),
                                ("EL_STRIP", "Draw a strip diagram", IconKind::Glyph("▤")),
                                ("EL_REPORT CABLES", "Cable summary", IconKind::Glyph("⌇")),
                                ("EL_REPORT MISSINGCATALOG", "Missing catalogue data", IconKind::Glyph("⚠")),
                                ("EL_REPORT DRAWINGLIST", "Drawing list", IconKind::Glyph("☰")),
                                ("EL_REPORT BOM TABLE", "Draw the BOM on the sheet", IconKind::Glyph("▦")),
                                ("EL_REPORT ALL", "Export every report", IconKind::Glyph("⎘")),
                                ("EL_REPORT LIST", "List the reports", IconKind::Glyph("?")),
                            ],
                            default: "EL_REPORT BOM",
                        },
                        tool("EL_AUDIT", "Audit", "⚠", "EL_AUDIT"),
                    ],
                },
                // Arrange: moving things without breaking their wiring.
                RibbonGroup {
                    title: "Arrange",
                    tools: vec![
                        large_tool("EL_MOVE", "Move", "✥", "EL_MOVE"),
                        tool("EL_ROTATE", "Rotate", "⟳", "EL_ROTATE @SEL 90"),
                        tool("EL_REATTACH", "Re-attach", "⇥", "EL_REATTACH"),
                    ],
                },
                // Inspect: the dashboard and the net trace.
                RibbonGroup {
                    title: "Inspect",
                    tools: vec![
                        RibbonItem::LargeDropdown {
                            id: "EL_LIST_DD",
                            label: "Inventory",
                            icon: IconKind::Glyph("☰"),
                            items: vec![
                                ("EL_LIST", "Everything", IconKind::Glyph("☰")),
                                ("EL_LIST devices", "Devices", IconKind::Glyph("▣")),
                                ("EL_LIST wires", "Wires", IconKind::Glyph("⤳")),
                                ("EL_LIST terminals", "Terminals", IconKind::Glyph("▢")),
                                ("EL_LIST cables", "Cables", IconKind::Glyph("⌇")),
                                ("EL_LIST connectors", "Connectors", IconKind::Glyph("⊞")),
                                ("EL_LIST nets", "Nets", IconKind::Glyph("⇄")),
                                ("EL_LIST qc", "What needs fixing", IconKind::Glyph("⚠")),
                            ],
                            default: "EL_LIST",
                        },
                        RibbonItem::Dropdown {
                            id: "EL_HIGHLIGHT_DD",
                            icon: IconKind::Glyph("◎"),
                            items: vec![
                                ("EL_HIGHLIGHT @SEL", "Trace the inspected net", IconKind::Glyph("◎")),
                                ("EL_HIGHLIGHT CLEAR", "Clear the trace", IconKind::Glyph("⌫")),
                            ],
                            default: "EL_HIGHLIGHT @SEL",
                        },
                    ],
                },
                // The symbol library itself.
                RibbonGroup {
                    title: "Library",
                    tools: vec![
                        large_tool("EL_LIB", "Symbols", "▤", "EL_LIB"),
                        RibbonItem::Dropdown {
                            id: "EL_LIB_DD",
                            icon: IconKind::Glyph("⋯"),
                            items: vec![
                                ("EL_LIB LIST", "List symbols", IconKind::Glyph("≡")),
                                ("EL_LIB FAMILIES", "List families", IconKind::Glyph("⌗")),
                                ("EL_LIB PATH", "Which library file is in effect", IconKind::Glyph("⌂")),
                                ("EL_SYMBOL", "Capture a drawn symbol", IconKind::Glyph("✚")),
                            ],
                            default: "EL_LIB LIST",
                        },
                    ],
                },
                // Correct.
                RibbonGroup {
                    title: "Edit",
                    tools: vec![
                        large_tool("EL_INFO", "Info", "ⓘ", "EL_INFO"),
                        tool("EL_EDIT", "Edit", "✎", "EL_EDIT @SEL"),
                        tool("EL_DELETE", "Delete", "✖", "EL_DELETE @SEL"),
                    ],
                },
                // Short aliases and the less-used variants. The host builds
                // command-line autocomplete from ribbon ids, so a command that
                // appears nowhere here can only be typed from memory — which is
                // why the aliases live in the ribbon rather than only in
                // `dispatch`.
                RibbonGroup {
                    title: "Utilities",
                    tools: vec![RibbonItem::Dropdown {
                        id: "EL_UTIL_DD",
                        icon: IconKind::Glyph("⋯"),
                        items: vec![
                            ("EL_WN", "Number wires (short)", IconKind::Glyph("#")),
                            ("EL_WIRENO CLEAR", "Clear drawn numbers", IconKind::Glyph("⌫")),
                            ("EL_COMP", "Place a component (short)", IconKind::Glyph("▣")),
                            ("EL_RETAG FIX", "Pin a device tag", IconKind::Glyph("⊘")),
                            ("EL_XREF CLEAR", "Clear cross-references", IconKind::Glyph("⌫")),
                            ("EL_SYMBOL DEL", "Remove a captured symbol", IconKind::Glyph("✖")),
                            ("EL_LS", "Inventory (short)", IconKind::Glyph("☰")),
                            ("EL_HL", "Trace a net (short)", IconKind::Glyph("◎")),
                            ("EL_MV", "Move (short)", IconKind::Glyph("✥")),
                            ("EL_WG", "Wire gaps (short)", IconKind::Glyph("⌒")),
                            ("EL_TRIM", "Trim a wire end (short)", IconKind::Glyph("✂")),
                            ("EL_SIG", "Signal arrow (short)", IconKind::Glyph("⇥")),
                            ("EL_PRJ", "Project register (short)", IconKind::Glyph("☰")),
                            ("EL_BAL", "Balloon a footprint (short)", IconKind::Glyph("○")),
                            ("EL_WIRETYPE LIST", "List wire types", IconKind::Glyph("≡")),
                            ("EL_LADDER CLEAR", "Remove all ladders", IconKind::Glyph("⌫")),
                            ("EL_RUNG ADD=1", "Add one rung", IconKind::Glyph("⁝")),
                            ("EL_INFO", "Inspect an entity", IconKind::Glyph("ⓘ")),
                        ],
                        default: "EL_WN",
                    }],
                },
            ]
        })
    }
}

struct ElectricalPlugin;

impl BuiltinPlugin for ElectricalPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(ElectricalModule)
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        eprintln!("[el] dispatch called: {cmd:?}");
        let mut tokens = cmd.split_whitespace();
        let Some(command) = tokens.next() else {
            return false;
        };
        let args: Vec<&str> = tokens.collect();

        // Remember the host request channel: interactive callbacks run against
        // a `NullHost`, so this is the only way they can report a result.
        if let Some(sender) = host.plugin_request_sender() {
            state::register_sender(std::sync::Arc::from(sender));
        }
        // Work a previous command left queued: output its interactive callback
        // could not print, and entities whose background flush never reached
        // the host. Draining both here is the safety net that makes a deferred
        // write loss-tolerant rather than best-effort.
        state::flush_lines(host);
        state::flush(host);

        match command.to_ascii_uppercase().as_str() {
            "EL_SETTINGS" => cmds::settings::run(host, &args),
            "EL_WIRETYPE" => cmds::wiretype::run(host, &args),
            "EL_LADDER" => cmds::ladder::run(host, &args),
            "EL_RUNG" => cmds::ladder::run_rung(host, &args),
            "EL_WIRE" => cmds::wire::run(host, &args),
            "EL_WIRENO" | "EL_WN" => cmds::wireno::run(host, &args),
            "EL_WIREGAP" | "EL_WG" => cmds::wiregap::run(host, &args),
            "EL_TRIMWIRE" | "EL_TRIM" => cmds::wiregap::run_trim(host, &args),
            "EL_COMPONENT" | "EL_COMP" => cmds::component::run(host, &args),
            "EL_SYMBOL" => cmds::symbol_cmd::run(host, &args),
            "EL_LIB" => cmds::library_cmd::run(host, &args),
            "EL_LINK" => cmds::link::run(host, &args),
            "EL_XREF" => cmds::xref::run(host, &args),
            "EL_RETAG" => cmds::retag::run(host, &args),
            "EL_TERMINAL" | "EL_TERM" => cmds::terminal::run(host, &args),
            "EL_PANEL" => cmds::panel::run(host, &args),
            "EL_PLC" => cmds::plc::run(host, &args),
            "EL_BALLOON" | "EL_BAL" => cmds::panel::run_balloon(host, &args),
            "EL_CABLE" => cmds::cable::run(host, &args),
            "EL_CONNECTOR" | "EL_CONN" => cmds::connector::run(host, &args),
            "EL_CATALOG" | "EL_CAT" => cmds::catalog_cmd::run(host, &args),
            "EL_SIGNAL" | "EL_SIG" => cmds::signal::run(host, &args),
            "EL_PROJECT" | "EL_PRJ" => cmds::project::run(host, &args),
            "EL_STRIP" => cmds::strip::run(host, &args),
            "EL_REPORT" => cmds::report_cmd::run(host, &args),
            "EL_AUDIT" | "EL_CHECK" => cmds::audit::run(host, &args),
            "EL_MOVE" | "EL_MV" => cmds::move_cmd::run(host, &args, false),
            "EL_ROTATE" | "EL_ROT" => cmds::move_cmd::run(host, &args, true),
            "EL_REATTACH" | "EL_ATTACH" => cmds::move_cmd::run_reattach(host, &args),
            "EL_LIST" | "EL_LS" => cmds::list::run(host, &args),
            "EL_HIGHLIGHT" | "EL_HILITE" | "EL_HL" => cmds::highlight::run(host, &args),
            "EL_INFO" => cmds::info::run(host, &args),
            "EL_EDIT" => cmds::edit::run(host, &args),
            "EL_DELETE" => cmds::delete::run(host, &args),
            "EL_PROBE" => probe::run(host, args.first().copied(), args.get(1).copied()),
            other if other.starts_with("EL_") => {
                host.push_error(&format!(
                    "Unknown electrical command {other}. Try EL_SETTINGS, EL_WIRETYPE, EL_LADDER, \
                     EL_RUNG, EL_WIRE, EL_WIRENO, EL_INFO, EL_EDIT, EL_DELETE or EL_PROBE."
                ));
            }
            _ => return false,
        }
        true
    }
}

ocs_plugin_api::export_plugin!(ElectricalPlugin);

#[cfg(test)]
mod completion_tests {
    use super::*;

    /// Every command `dispatch` can act on. The host builds command-line
    /// autocomplete from *ribbon ids*, so a command missing from the ribbon is
    /// reachable only by typing the full name from memory.
    const DISPATCHED: &[&str] = &[
        "EL_SETTINGS",
        "EL_WIRETYPE",
        "EL_LADDER",
        "EL_RUNG",
        "EL_WIRE",
        "EL_WIRENO",
        "EL_WN",
        "EL_WIREGAP",
        "EL_WG",
        "EL_TRIMWIRE",
        "EL_TRIM",
        "EL_SIGNAL",
        "EL_SIG",
        "EL_PANEL",
        "EL_PLC",
        "EL_BALLOON",
        "EL_BAL",
        "EL_PROJECT",
        "EL_PRJ",
        "EL_STRIP",
        "EL_COMPONENT",
        "EL_COMP",
        "EL_SYMBOL",
        "EL_LIB",
        "EL_LINK",
        "EL_XREF",
        "EL_RETAG",
        "EL_INFO",
        "EL_EDIT",
        "EL_DELETE",
        "EL_PROBE",
    ];

    fn collect(item: &RibbonItem, out: &mut Vec<String>) {
        match item {
            RibbonItem::Tool(t) | RibbonItem::LargeTool(t) | RibbonItem::LabeledTool(t) => match &t.event {
                ModuleEvent::Command(c) => out.push(c.clone()),
                ModuleEvent::PluginFileDialog { command, .. } => out.push(command.clone()),
                _ => {}
            },
            RibbonItem::Dropdown { items, default, .. }
            | RibbonItem::LargeDropdown { items, default, .. }
            | RibbonItem::LabeledDropdown { items, default, .. } => {
                for (cmd, _, _) in items {
                    out.push((*cmd).to_string());
                }
                out.push((*default).to_string());
            }
            RibbonItem::ToolGrid { columns } => {
                for row in columns {
                    for t in row {
                        if let ModuleEvent::Command(c) = &t.event {
                            out.push(c.clone());
                        }
                    }
                }
            }
            RibbonItem::LayerComboGroup { row2, row3 } => {
                for t in row2.iter().chain(row3.iter()) {
                    if let ModuleEvent::Command(c) = &t.event {
                        out.push(c.clone());
                    }
                }
            }
            RibbonItem::PropertiesGroup { match_prop } => {
                if let ModuleEvent::Command(c) = &match_prop.event {
                    out.push(c.clone());
                }
            }
            RibbonItem::StyleComboGroup {
                manager_cmd, rows, ..
            } => {
                if let Some(c) = manager_cmd {
                    out.push((*c).to_string());
                }
                for row in rows {
                    for t in row {
                        if let ModuleEvent::Command(c) = &t.event {
                            out.push(c.clone());
                        }
                    }
                }
            }
        }
    }

    fn ribbon_command_ids() -> Vec<String> {
        let module = ElectricalModule;
        let mut out = Vec::new();
        for group in module.ribbon_groups() {
            for item in &group.tools {
                collect(item, &mut out);
            }
        }
        out
    }

    #[test]
    fn every_dispatched_command_is_ribbon_autocompleteable() {
        let ids = ribbon_command_ids();
        for cmd in DISPATCHED {
            // A ribbon entry may carry arguments (`EL_SETTINGS SHEET=02`); the
            // command name is the first token.
            let name = cmd.split_whitespace().next().unwrap();
            assert!(
                ids.iter().any(|id| id.split_whitespace().next() == Some(name)),
                "{name} is dispatched but absent from the ribbon, so the command \
                 line will not autocomplete it"
            );
        }
    }

    #[test]
    fn ribbon_titles_are_unique() {
        let module = ElectricalModule;
        let mut seen = std::collections::BTreeSet::new();
        for group in module.ribbon_groups() {
            assert!(seen.insert(group.title), "duplicate group {}", group.title);
        }
    }

    #[test]
    fn manifest_declares_our_own_app_id_and_prefix() {
        // Both plugins can be installed at once; sharing either would make one
        // plugin read the other's records.
        assert_eq!(MANIFEST.xdata_apps, &["OCSEL"]);
        assert!(MANIFEST.command_prefixes.iter().all(|p| p.starts_with("EL_")));
        assert_eq!(MANIFEST.id, "opencad.electrical");
    }
}
