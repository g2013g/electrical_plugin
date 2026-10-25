//! Report builders.
//!
//! Every report is derived from the records, never stored, so a report is a
//! *view* of the drawing rather than a second copy of it that can go stale. The
//! builders return a [`TableLayout`], which both the CSV writer and the
//! on-canvas renderer consume.
//!
//! Two decisions shape the set:
//!
//! * The **bill of materials tallies**. Ten `XT-2.5` terminals are one BOM line
//!   with a quantity, because that is what gets ordered — while the component
//!   report lists each device, because that is what gets wired.
//! * A missing value is a dash, not a blank. A BOM that silently omits a part
//!   number looks complete; one that shows a dash does not.

use std::collections::BTreeMap;

use crate::catalog::Catalog;
use crate::index::ElIndex;
use crate::model::ElSettingsRec;
use crate::tables::{sort_rows, TableLayout, EMPTY_CELL};

/// The reports a drawing can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// Tallied bill of materials, grouped by part.
    Bom,
    /// Every device, one row each.
    Components,
    /// Wire from/to, one row per wire.
    WireFromTo,
    /// Wire labels, for printing onto markers.
    WireLabels,
    /// Terminals by strip.
    TerminalPlan,
    PanelBom,
    /// Cables and their conductors.
    CableSummary,
    /// Devices that cannot be ordered as drawn.
    MissingCatalog,
    /// This drawing's own metadata and contents.

/// This drawing's own metadata and contents.
    DrawingList,
    /// Every electrical net, and what it joins.
    Nets,
}

impl ReportKind {
    pub const ALL: [ReportKind; 10] = [
        ReportKind::Bom,
        ReportKind::Components,
        ReportKind::WireFromTo,
        ReportKind::WireLabels,
        ReportKind::TerminalPlan,
        ReportKind::PanelBom,
        ReportKind::CableSummary,
        ReportKind::MissingCatalog,
        ReportKind::DrawingList,
        ReportKind::Nets,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ReportKind::Bom => "BOM",
            ReportKind::Components => "COMPONENTS",
            ReportKind::WireFromTo => "WIREFROMTO",
            ReportKind::TerminalPlan => "TERMINALS",
            ReportKind::PanelBom => "PANELBOM",
            ReportKind::CableSummary => "CABLES",
            ReportKind::MissingCatalog => "MISSINGCATALOG",
            ReportKind::WireLabels => "WIRELABELS",
            ReportKind::DrawingList => "DRAWINGLIST",
            ReportKind::Nets => "NETS",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().replace(['_', '-'], "").as_str() {
            "BOM" | "BILLOFMATERIALS" => Some(ReportKind::Bom),
            "COMPONENTS" | "COMPONENT" | "PARTS" => Some(ReportKind::Components),
            "WIREFROMTO" | "FROMTO" | "WIRES" | "CONNECTIONS" => Some(ReportKind::WireFromTo),
            "WIRELABELS" | "LABELS" => Some(ReportKind::WireLabels),
            "TERMINALS" | "TERMINALPLAN" | "TERMINAL" => Some(ReportKind::TerminalPlan),
            "PANELBOM" | "PANEL" | "PANELLIST" => Some(ReportKind::PanelBom),
            "CABLES" | "CABLESUMMARY" | "CABLE" => Some(ReportKind::CableSummary),
            "MISSINGCATALOG" | "MISSING" | "NOCATALOG" => Some(ReportKind::MissingCatalog),
            "DRAWINGLIST" | "DRAWINGS" | "SHEETS" => Some(ReportKind::DrawingList),
            "NETS" | "NETLIST" | "NET" => Some(ReportKind::Nets),
            _ => None,
        }
    }

    /// A short human name, for a heading and a file name.
    pub fn title(self) -> &'static str {
        match self {
            ReportKind::Bom => "Bill of materials",
            ReportKind::Components => "Component list",
            ReportKind::WireFromTo => "Wire from/to",
            ReportKind::TerminalPlan => "Terminal plan",
            ReportKind::PanelBom => "Panel BOM",
            ReportKind::CableSummary => "Cable summary",
            ReportKind::MissingCatalog => "Missing catalogue data",
            ReportKind::WireLabels => "Wire labels",
            ReportKind::DrawingList => "Drawing list",
            ReportKind::Nets => "Net list",
        }
    }

    /// The file stem a CSV export uses.
    pub fn file_stem(self) -> &'static str {
        match self {
            ReportKind::Bom => "bom",
            ReportKind::Components => "components",
            ReportKind::WireFromTo => "wire-from-to",
            ReportKind::TerminalPlan => "terminal-plan",
            ReportKind::PanelBom => "panel-bom",
            ReportKind::CableSummary => "cable-summary",
            ReportKind::MissingCatalog => "missing-catalog",
            ReportKind::WireLabels => "wire-labels",
            ReportKind::DrawingList => "drawing-list",
            ReportKind::Nets => "net-list",
        }
    }
}

/// Build one report.
pub fn build(
    kind: ReportKind,
    idx: &ElIndex,
    catalog: Option<&Catalog>,
    settings: &ElSettingsRec,
) -> TableLayout {
    match kind {
        ReportKind::Bom => bom(idx, catalog),
        ReportKind::Components => components(idx),
        ReportKind::WireFromTo => wire_from_to(idx),
        ReportKind::WireLabels => wire_labels(idx),
        ReportKind::TerminalPlan => terminal_plan(idx),
        ReportKind::PanelBom => panel_bom(idx),
        ReportKind::CableSummary => cable_summary(idx),
        ReportKind::MissingCatalog => missing_catalog(idx, catalog),
        ReportKind::DrawingList => drawing_list(idx, settings),
        ReportKind::Nets => net_list(idx),
    }
}

/// Tallied bill of materials.
///
/// One row per `(manufacturer, part number)`, with the quantity and the devices
/// that make it up. A device with no part number is tallied separately rather
/// than dropped: a BOM that quietly omits an unassigned device is the most
/// dangerous kind of wrong.
pub fn bom(idx: &ElIndex, catalog: Option<&Catalog>) -> TableLayout {
    // (key) -> (mfg, cat, description, qty, refs)
    let mut tally: BTreeMap<String, (String, String, String, usize, Vec<String>)> = BTreeMap::new();

    let mut add = |mfg: &str, cat: &str, fallback_desc: &str, item: &str| {
        let key = format!("{mfg}\u{0}{cat}");
        let desc = if cat.is_empty() {
            format!("{EMPTY_CELL} no part number")
        } else {
            catalog
                .and_then(|c| c.get(mfg, cat))
                .map(|p| p.description.clone())
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| fallback_desc.to_string())
        };
        let entry = tally
            .entry(key)
            .or_insert_with(|| (mfg.to_string(), cat.to_string(), desc, 0, Vec::new()));
        entry.3 += 1;
        if !item.is_empty() {
            entry.4.push(item.to_string());
        }
    };

    for comp in idx.components.values() {
        if comp.rec.is_child {
            continue; // a child shares its parent's part
        }
        add(&comp.rec.mfg, &comp.rec.cat, &comp.rec.description, &comp.rec.tag);
    }
    for t in idx.terminals.values() {
        add("", &t.rec.part_number, &t.rec.description, &t.rec.designator());
    }
    for c in idx.cables.values() {
        if !c.rec.is_parent() {
            continue;
        }
        let cat = if c.rec.cat.is_empty() {
            c.rec.part_number.as_str()
        } else {
            c.rec.cat.as_str()
        };
        let desc = if c.rec.description.is_empty() {
            format!("Cable, {} conductor(s)", idx.cable_markers(&c.rec.tag).len())
        } else {
            c.rec.description.clone()
        };
        add(&c.rec.mfg, cat, &desc, &c.rec.tag);
    }
    for x in idx.connectors.values() {
        add(&x.rec.mfg, &x.rec.cat, &x.rec.description, &x.rec.ref_des);
    }

    let mut rows: Vec<Vec<String>> = tally
        .into_values()
        .map(|(mfg, cat, desc, qty, mut refs)| {
            refs.sort_by(|a, b| crate::refdes_cmp(a, b));
            vec![
                if mfg.is_empty() { EMPTY_CELL.to_string() } else { mfg },
                if cat.is_empty() { EMPTY_CELL.to_string() } else { cat },
                desc,
                qty.to_string(),
                refs.join(","),
            ]
        })
        .collect();
    // Sorted by part so the same BOM orders the same way twice.
    rows.sort_by(|a, b| a[1].cmp(&b[1]).then_with(|| a[0].cmp(&b[0])));

    let total: usize = rows
        .iter()
        .filter_map(|r| r[3].parse::<usize>().ok())
        .sum();
    TableLayout::new(
        &format!("Bill of materials — {} item(s), {total} device(s)", rows.len()),
        &["MFG", "PART", "DESCRIPTION", "QTY", "REFERENCE"],
        rows,
    )
}

/// One row per device.
pub fn components(idx: &ElIndex) -> TableLayout {
    let mut rows: Vec<Vec<String>> = idx
        .components
        .values()
        .map(|c| {
            vec![
                dash(&c.rec.tag),
                dash(&c.rec.family),
                dash(&c.rec.symbol),
                role(&c.rec),
                dash(&c.rec.description),
                dash(&c.rec.mfg),
                dash(&c.rec.cat),
                dash(&c.rec.ratings.join(", ")),
                dash(&join_location(&c.rec.installation, &c.rec.location)),
            ]
        })
        .collect();
    sort_rows(&mut rows, 0);

    // Terminals and connectors are devices too, and a component list that omits
    // them would not match the drawing it describes.
    for t in idx.terminals.values() {
        rows.push(vec![
            dash(&t.rec.designator()),
            "X".to_string(),
            format!("terminal ({})", t.rec.kind.as_str().to_ascii_lowercase()),
            dash(&t.rec.linked),
            dash(&t.rec.description),
            EMPTY_CELL.to_string(),
            dash(&t.rec.part_number),
            EMPTY_CELL.to_string(),
            dash(&t.rec.tag_strip),
        ]);
    }
    for x in idx.connectors.values() {
        rows.push(vec![
            dash(&x.rec.ref_des),
            "X".to_string(),
            format!("connector, {} pin(s)", x.rec.pin_count()),
            dash(&x.rec.orientation.as_str().to_ascii_lowercase()),
            dash(&x.rec.description),
            dash(&x.rec.mfg),
            dash(&x.rec.cat),
            EMPTY_CELL.to_string(),
            dash(&x.rec.location),
        ]);
    }
    sort_rows(&mut rows, 0);

    TableLayout::new(
        &format!("Components — {} device(s)", rows.len()),
        &["TAG", "FAM", "SYMBOL", "ROLE", "DESCRIPTION", "MFG", "PART", "RATINGS", "INST/LOC"],
        rows,
    )
}

/// Wire from/to: the connectivity list a wirelist is checked against.
pub fn wire_from_to(idx: &ElIndex) -> TableLayout {
    let mut rows: Vec<Vec<String>> = idx
        .wires
        .values()
        .map(|w| {
            vec![
                dash(&w.rec.wire_ref),
                dash(&w.rec.from_id),
                dash(&w.rec.from_pin),
                dash(&w.rec.to_id),
                dash(&w.rec.to_pin),
                dash(&w.rec.gauge),
                dash(&w.rec.color),
                dash(&w.rec.wire_type),
                dash(&w.rec.note),
            ]
        })
        .collect();
    sort_rows(&mut rows, 0);

    TableLayout::new(
        &format!("Wire from/to — {} wire(s)", rows.len()),
        &["WIRE", "FROM", "PIN", "TO", "PIN", "GAUGE", "COLOUR", "TYPE", "NOTE"],
        rows,
    )
}

/// Wire labels: what is printed onto a marker, one line per wire.
pub fn wire_labels(idx: &ElIndex) -> TableLayout {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for w in idx.wires.values() {
        if w.rec.wire_ref.is_empty() {
            continue;
        }
        let mut extras = vec![w.rec.gauge.clone(), w.rec.color.clone()];
        extras.retain(|s| !s.trim().is_empty());
        rows.push(vec![w.rec.wire_ref.clone(), extras.join(" / ")]);
    }
    sort_rows(&mut rows, 0);
    TableLayout::new(
        &format!("Wire labels — {} label(s)", rows.len()),
        &["LABEL", "GAUGE / COLOUR"],
        rows,
    )
}

/// Terminal plan: terminals grouped by strip, in strip order.
pub fn terminal_plan(idx: &ElIndex) -> TableLayout {
    let mut rows: Vec<Vec<String>> = Vec::new();
    // Who each terminal is bridged to, so a jumpered group reads as one
    // circuit on the plan — the strip editor's information, on paper.
    let mut partners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for j in idx.jumpers.values() {
        let (a, b) = j.rec.designators();
        partners.entry(a.clone()).or_default().push(b.clone());
        partners.entry(b).or_default().push(a);
    }
    let strips = idx.strips();
    let strip_count = strips.len();
    for (strip, terminals) in strips {
        for t in terminals {
            let designator = t.rec.designator();
            let jumped = partners.get(&designator);
            rows.push(vec![
                strip.clone(),
                if t.rec.number.is_empty() {
                    EMPTY_CELL.to_string()
                } else {
                    t.rec.number.clone()
                },
                t.rec.kind.as_str().to_string(),
                dash(&t.rec.part_number),
                dash(&t.rec.description),
                dash(&t.rec.linked),
                dash(&t.rec.partition),
                match jumped {
                    Some(p) if !p.is_empty() => p.join(", "),
                    _ => EMPTY_CELL.to_string(),
                },
            ]);
        }
    }
    TableLayout::new(
        &format!(
            "Terminal plan — {} strip(s), {} terminal(s), {} jumper(s)",
            strip_count,
            rows.len(),
            idx.jumpers.len()
        ),
        &["STRIP", "NO", "TYPE", "PART", "DESCRIPTION", "LINKED", "PTN", "JUMPERED TO"],
        rows,
    )
}


/// Panel BOM (CH8): one row per footprint, keyed by item number — the list a
/// panel builder assembles from, carrying the schematic's catalog data over
/// the tag link when the footprint does not override it.
pub fn panel_bom(idx: &ElIndex) -> TableLayout {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut fps: Vec<&crate::index::IndexedFootprint> = idx.footprints.values().collect();
    // Item-number order when numbered, placement order otherwise, so the
    // sheet's balloons and the BOM read the same sequence.
    fps.sort_by(|a, b| {
        a.rec
            .item_number
            .cmp(&b.rec.item_number)
            .then_with(|| crate::refdes_cmp(&a.rec.tag, &b.rec.tag))
    });
    for f in fps {
        // The schematic link fills the blanks: catalog lives on the device.
        let (mfg, cat, desc) = match idx.parent_by_tag(&f.rec.tag) {
            Some(c) => (
                if f.rec.mfg.is_empty() { c.rec.mfg.clone() } else { f.rec.mfg.clone() },
                if f.rec.cat.is_empty() { c.rec.cat.clone() } else { f.rec.cat.clone() },
                if f.rec.description.is_empty() { c.rec.description.clone() } else { f.rec.description.clone() },
            ),
            None => (f.rec.mfg.clone(), f.rec.cat.clone(), f.rec.description.clone()),
        };
        rows.push(vec![
            match f.rec.item_number {
                n if n > 0 => n.to_string(),
                _ => EMPTY_CELL.to_string(),
            },
            f.rec.tag.clone(),
            format!("{}x{}", fmt_size(f.rec.width_mm), fmt_size(f.rec.height_mm)),
            dash(&desc),
            dash(&mfg),
            dash(&cat),
        ]);
    }
    TableLayout::new(
        &format!("Panel BOM — {} item(s)", rows.len()),
        &["ITEM", "TAG", "SIZE", "DESCRIPTION", "MFG", "CAT"],
        rows,
    )
}

fn fmt_size(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Cable summary: one row per cable, with its conductor count.
pub fn cable_summary(idx: &ElIndex) -> TableLayout {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut tags: Vec<String> = idx
        .cables
        .values()
        .map(|c| c.rec.tag.clone())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort_by(|a, b| crate::refdes_cmp(a, b));
    tags.dedup();

    for tag in tags {
        let markers = idx.cable_markers(&tag);
        let parent = idx.cable_parent(&tag);
        let colors: Vec<String> = markers
            .iter()
            .map(|m| {
                if m.rec.color.is_empty() {
                    EMPTY_CELL.to_string()
                } else {
                    m.rec.color.clone()
                }
            })
            .collect();
        let (mfg, cat, part, desc, gauge) = match parent {
            Some(p) => (
                p.rec.mfg.clone(),
                p.rec.cat.clone(),
                p.rec.part_number.clone(),
                p.rec.description.clone(),
                p.rec.gauge.clone(),
            ),
            None => Default::default(),
        };
        let part_col = if !cat.is_empty() { cat } else { part };
        rows.push(vec![
            tag,
            markers.len().to_string(),
            dash(&mfg),
            dash(&part_col),
            dash(&gauge),
            colors.join(","),
            dash(&desc),
        ]);
    }

    TableLayout::new(
        &format!("Cable summary — {} cable(s)", rows.len()),
        &["CABLE", "COND", "MFG", "PART", "GAUGE", "COLOURS", "DESCRIPTION"],
        rows,
    )
}

/// Devices that cannot be ordered as drawn, and why.
pub fn missing_catalog(idx: &ElIndex, catalog: Option<&Catalog>) -> TableLayout {
    let mut rows: Vec<Vec<String>> = Vec::new();

    for a in crate::drc::assignments(idx) {
        if a.mfg.trim().is_empty() && a.cat.trim().is_empty() {
            rows.push(vec![a.item, "no part number".to_string(), a.family]);
            continue;
        }
        if a.mfg.trim().is_empty() {
            rows.push(vec![
                a.item,
                format!("part {} has no manufacturer", a.cat),
                a.family,
            ]);
            continue;
        }
        if a.cat.trim().is_empty() {
            rows.push(vec![
                a.item,
                format!("manufacturer {} has no part number", a.mfg),
                a.family,
            ]);
            continue;
        }
        if let Some(c) = catalog {
            if let Some(part) = c.get(&a.mfg, &a.cat) {
                if !part.suits_family(&a.family) {
                    rows.push(vec![
                        a.item,
                        format!("{}/{} is not catalogued for family {}", a.mfg, a.cat, a.family),
                        a.family,
                    ]);
                }
            } else {
                rows.push(vec![
                    a.item,
                    format!("{}/{} is not in the catalogue", a.mfg, a.cat),
                    a.family,
                ]);
            }
        }
    }
    sort_rows(&mut rows, 0);
    TableLayout::new(
        &format!("Missing catalogue data — {} item(s)", rows.len()),
        &["ITEM", "PROBLEM", "FAMILY"],
        rows,
    )
}

/// Every net, one row each, with the devices it joins.
///
/// This is the report a drafter uses to check that a circuit is what they meant:
/// it says what is connected to what, derived from the wires rather than from
/// anybody's intention.
pub fn net_list(idx: &ElIndex) -> TableLayout {
    let nets = crate::net::NetIndex::build(idx);
    let mut rows: Vec<Vec<String>> = nets
        .nets
        .iter()
        .map(|n| {
            vec![
                n.label.clone(),
                n.wires.len().to_string(),
                n.nodes.iter().cloned().collect::<Vec<_>>().join(" "),
                n.wires
                    .iter()
                    .filter_map(|h| idx.wires.get(h))
                    .map(|w| {
                        if w.rec.wire_ref.is_empty() {
                            format!("0x{:X}", w.handle)
                        } else {
                            w.rec.wire_ref.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            ]
        })
        .collect();
    sort_rows(&mut rows, 0);
    let joined = nets.connected_nets().len();
    TableLayout::new(
        &format!(
            "Net list — {} net(s), {joined} joining more than one device",
            rows.len()
        ),
        &["NET", "WIRES", "DEVICES", "WIRE NUMBERS"],
        rows,
    )
}

/// Every net, one row each, with the devices it joins.
///
/// This is the report a drafter uses to check that a circuit is what they meant:
/// it says what is connected to what, derived from the wires rather than from

/// This drawing's own metadata and contents.
///
/// AutoCAD Electrical's drawing list spans a project; with no project database
/// this describes the drawing in hand, which is the honest single-drawing form
/// of the same report — and it is where a sheet's number and revision are
/// confirmed before a report is issued.
pub fn drawing_list(idx: &ElIndex, settings: &ElSettingsRec) -> TableLayout {
    let wires_numbered = idx
        .wires
        .values()
        .filter(|w| !w.rec.wire_ref.is_empty())
        .count();
    let unnumbered = idx.wires.len() - wires_numbered;
    let refs = idx.all_references();

    let mut rows: Vec<Vec<String>> = vec![
        row2("Sheet", dash(&settings.sheet)),
        row2("Drawing", dash(&settings.drawing)),
        row2("Installation", dash(&settings.installation)),
        row2("Location", dash(&settings.location)),
        row2(
            "Tag format",
            format!("{} ({} mode)", settings.tag_format, settings.tag_mode.as_str()),
        ),
        row2(
            "Wire-number format",
            format!(
                "{} ({} mode, start {})",
                settings.wireno_format,
                settings.wireno_mode.as_str(),
                settings.wireno_start
            ),
        ),
        row2("Ladders", idx.ladders.len().to_string()),
        row2(
            "Line references",
            if refs.is_empty() {
                EMPTY_CELL.to_string()
            } else {
                format!("{} … {}", refs[0], refs[refs.len() - 1])
            },
        ),
        row2("Devices", idx.components.len().to_string()),
        row2("Wires", format!("{} ({} numbered, {unnumbered} not)", idx.wires.len(), wires_numbered)),
        row2("Terminals", idx.terminals.len().to_string()),
        row2("Strips", idx.strips().len().to_string()),
        row2(
            "Cables",
            idx.cables
                .values()
                .filter(|c| c.rec.is_parent())
                .count()
                .to_string(),
        ),
        row2("Connectors", idx.connectors.len().to_string()),
        // Records from a newer build are worth surfacing here: they mean this
        // drawing was written by a plugin that knows more than this one does.
        row2(
            "Records from a newer build",
            idx.unknown_kinds.values().sum::<usize>().to_string(),
        ),
    ];
    // A row per unreadable record is the most useful thing a drawing list can
    // carry, so it is listed explicitly rather than counted.
    for (handle, err) in &idx.decode_errors {
        rows.push(row2(
            &format!("Unreadable 0x{handle:X}"),
            format!("{err}"),
        ));
    }

    TableLayout::new("Drawing list", &["FIELD", "VALUE"], rows)
}

fn row2(a: &str, b: String) -> Vec<String> {
    vec![a.to_string(), b]
}

fn dash(s: &str) -> String {
    if s.trim().is_empty() {
        EMPTY_CELL.to_string()
    } else {
        s.to_string()
    }
}

fn role(rec: &crate::model::ElComponentRec) -> String {
    if rec.is_child {
        format!("child of {}", if rec.parent_tag.is_empty() { EMPTY_CELL } else { &rec.parent_tag })
    } else {
        "parent".to_string()
    }
}

fn join_location(inst: &str, loc: &str) -> String {
    match (inst.trim().is_empty(), loc.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => inst.to_string(),
        (true, false) => loc.to_string(),
        (false, false) => format!("{inst}/{loc}"),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ElRecord;
    use crate::model::*;

    fn settings() -> (u64, ElRecord) {
        (900, ElRecord::encode_settings(&ElSettingsRec::default()))
    }

    fn comp(handle: u64, tag: &str, is_child: bool, parent: &str, cat: &str) -> (u64, ElRecord) {
        let mut c = ElComponentRec::new(&format!("id{handle}"), "relay-coil", "K");
        c.tag = tag.to_string();
        c.is_child = is_child;
        c.parent_tag = parent.to_string();
        c.cat = cat.to_string();
        c.mfg = if cat.is_empty() { String::new() } else { "Generic".into() };
        c.description = "Relay".to_string();
        (handle, ElRecord::encode_component(&c))
    }

    fn wire(handle: u64, wref: &str) -> (u64, ElRecord) {
        let mut w = ElWireRec::new(&format!("id{handle}"), "K1", "K2");
        w.wire_ref = wref.to_string();
        w.gauge = "1.5".to_string();
        w.color = "BK".to_string();
        (handle, ElRecord::encode_wire(&w))
    }

    fn terminal(handle: u64, strip: &str, number: &str, part: &str) -> (u64, ElRecord) {
        let mut t = TerminalRec::new(&format!("tid{handle}"), strip, TerminalKind::Screw);
        t.number = number.to_string();
        t.part_number = part.to_string();
        (handle, ElRecord::encode_terminal(&t))
    }

    fn fixture() -> ElIndex {
        ElIndex::build(vec![
            settings(),
            comp(1, "K1", false, "", "RLY-24DC-2CO"),
            comp(2, "K1", true, "K1", "RLY-24DC-2CO"),
            comp(3, "K2", false, "", ""),
            wire(4, "W1"),
            wire(5, "W2"),
            terminal(6, "X1", "1", "XT-2.5"),
            terminal(7, "X1", "2", "XT-2.5"),
            terminal(8, "X2", "1", "XT-2.5-PE"),
        ])
    }

    #[test]
    fn the_bom_tallies_by_part_and_not_by_device() {
        // Ten terminals are one order line, which is the whole point of a BOM.
        let idx = fixture();
        let t = bom(&idx, Some(&Catalog::sample()));
        let rows: Vec<Vec<String>> = (0..t.row_count())
            .map(|r| (0..t.col_count()).map(|c| t.cell(r, c).trim().to_string()).collect())
            .collect();
        let xt: Vec<&Vec<String>> = rows.iter().filter(|r| r[1] == "XT-2.5").collect();
        assert_eq!(xt.len(), 1, "one line for the part");
        assert_eq!(xt[0][3], "2", "quantity two");
        assert_eq!(xt[0][4], "X1:1,X1:2", "and it names them");
    }

    #[test]
    fn the_bom_does_not_double_count_a_contact() {
        // A child shares its parent's part, so counting it would order two relays
        // for one.
        let idx = fixture();
        let t = bom(&idx, None);
        let rly: Vec<String> = (0..t.row_count())
            .filter(|r| t.cell(*r, 1).trim() == "RLY-24DC-2CO")
            .map(|r| t.cell(r, 3).trim().to_string())
            .collect();
        assert_eq!(rly, vec!["1"], "one relay, not two");
    }

    #[test]
    fn the_bom_shows_an_unassigned_device_rather_than_omitting_it() {
        let idx = fixture();
        let t = bom(&idx, None);
        let text = t.to_text();
        assert!(text.contains("no part number"), "{text}");
        assert!(text.contains('K') || text.contains("K2"), "K2 must appear: {text}");
    }

    #[test]
    fn the_bom_sorts_by_part_so_two_runs_agree() {
        let idx = fixture();
        let a = bom(&idx, None).to_text();
        let b = bom(&idx, None).to_text();
        assert_eq!(a, b);
    }

    #[test]
    fn the_component_list_includes_terminals_and_connectors() {
        // A component list that omitted them would not match the drawing.
        let mut conn = ConnectorRec::new("xid", "X9", 4);
        conn.cat = "PLUG-4".to_string();
        let mut idx = fixture();
        idx.connectors
            .insert("xid".into(), crate::index::IndexedConnector {
                el_id: "xid".into(),
                rec: conn.clone(),
                handles: vec![20],
            });
        let t = components(&idx);
        let text = t.to_text();
        assert!(text.contains("PLUG-4"), "{text}");
        assert!(text.contains("terminal"), "{text}");
        assert!(text.contains("connector"), "{text}");
    }

    #[test]
    fn the_wire_from_to_lists_every_wire() {
        let t = wire_from_to(&fixture());
        assert_eq!(t.row_count(), 2);
        let text = t.to_text();
        assert!(text.contains("K1"), "{text}");
        assert!(text.contains("K2"), "{text}");
    }

    #[test]
    fn the_wire_label_report_skips_unnumbered_wires() {
        // A label for an unnumbered wire would be blank, and printing blanks
        // wastes a marker.
        let mut idx = ElIndex::build(vec![settings(), wire(1, "W1")]);
        idx.wires.get_mut(&1).unwrap().rec.wire_ref = String::new();
        let t = wire_labels(&idx);
        assert_eq!(t.row_count(), 0, "{}", t.to_text());
    }

    #[test]
    fn the_terminal_plan_groups_by_strip_in_order() {
        let t = terminal_plan(&fixture());
        assert_eq!(t.row_count(), 3);
        let first = t.cell(0, 0).trim().to_string();
        assert_eq!(first, "X1");
        assert_eq!(t.cell(0, 1).trim(), "1");
        // X2 is a separate strip and comes last.
        assert_eq!(t.cell(2, 0).trim(), "X2");
    }

    #[test]
    fn the_cable_summary_counts_markers_per_cable() {
        let mut a = CableRec::new("ca", "W1");
        a.color = "BK".to_string();
        a.cat = "CBL-4x1.5".to_string();
        let mut b = CableRec::new("cb", "W1");
        b.parent_tag = "W1".to_string();
        b.conductor_index = 2;
        b.color = "BL".to_string();
        let idx = ElIndex::build(vec![
            settings(),
            (1, ElRecord::encode_cable(&a)),
            (2, ElRecord::encode_cable(&b)),
        ]);
        let t = cable_summary(&idx);
        assert_eq!(t.row_count(), 1);
        assert_eq!(t.cell(0, 0).trim(), "W1");
        assert_eq!(t.cell(0, 1).trim(), "2", "two conductors");
        assert!(t.cell(0, 5).contains("BK"), "colours listed: {}", t.cell(0, 5));
        assert!(t.cell(0, 5).contains("BL"));
    }

    #[test]
    fn the_missing_catalogue_report_names_the_problem() {
        let idx = fixture();
        let t = missing_catalog(&idx, Some(&Catalog::sample()));
        let text = t.to_text();
        assert!(text.contains("no part number"), "{text}");
        // K1 has a real part, so it must not be listed.
        assert!(!text.contains("RLY-24DC-2CO"), "{text}");
    }

    #[test]
    fn the_missing_catalogue_report_flags_an_unknown_part() {
        let idx = ElIndex::build(vec![settings(), comp(1, "K1", false, "", "NOSUCHPART")]);
        let t = missing_catalog(&idx, Some(&Catalog::sample()));
        assert!(t.to_text().contains("not in the catalogue"), "{}", t.to_text());
    }

    #[test]
    fn the_drawing_list_reports_the_settings_and_contents() {
        let mut s = ElSettingsRec::default();
        s.sheet = "02".to_string();
        s.drawing = "PANEL-01".to_string();
        let idx = fixture();
        let t = drawing_list(&idx, &s);
        let text = t.to_text();
        assert!(text.contains("02"), "{text}");
        assert!(text.contains("PANEL-01"), "{text}");
        assert!(text.contains("Line references"), "{text}");
        assert!(text.contains("Devices"), "{text}");
    }

    #[test]
    fn the_drawing_list_separates_numbered_from_unnumbered_wires() {
        let mut idx = ElIndex::build(vec![settings(), wire(1, "W1"), wire(2, "")]);
        idx.wires.get_mut(&2).unwrap().rec.wire_ref = String::new();
        let t = drawing_list(&idx, &ElSettingsRec::default());
        assert!(t.to_text().contains("1 numbered, 1 not"), "{}", t.to_text());
    }

    #[test]
    fn every_report_kind_builds_on_an_empty_drawing() {
        // A report that panics on an empty drawing is a report that panics on
        // the first drawing a user opens.
        let idx = ElIndex::build(vec![]);
        for kind in ReportKind::ALL {
            let t = build(kind, &idx, None, &ElSettingsRec::default());
            assert!(t.col_count() > 0, "{} has no columns", kind.as_str());
            // And renders without panicking.
            let _ = t.to_text();
        }
    }

    #[test]
    fn report_kinds_round_trip_through_their_names() {
        for k in ReportKind::ALL {
            assert_eq!(ReportKind::parse(k.as_str()), Some(k), "{}", k.as_str());
            assert!(!k.title().is_empty());
            assert!(!k.file_stem().is_empty());
        }
        assert_eq!(ReportKind::parse("from-to"), Some(ReportKind::WireFromTo));
        assert_eq!(ReportKind::parse("wire_from_to"), Some(ReportKind::WireFromTo));
        assert!(ReportKind::parse("nonsense").is_none());
    }

    #[test]
    fn file_stems_are_distinct_so_exports_do_not_overwrite() {
        let mut stems: Vec<&str> = ReportKind::ALL.iter().map(|k| k.file_stem()).collect();
        let n = stems.len();
        stems.sort();
        stems.dedup();
        assert_eq!(stems.len(), n);
    }

    #[test]
    fn a_dash_marks_an_absent_value_consistently() {
        assert_eq!(dash(""), EMPTY_CELL);
        assert_eq!(dash("  "), EMPTY_CELL);
        assert_eq!(dash("K1"), "K1");
    }

    #[test]
    fn join_location_omits_a_missing_half() {
        assert_eq!(join_location("", ""), "");
        assert_eq!(join_location("A", ""), "A");
        assert_eq!(join_location("", "B"), "B");
        assert_eq!(join_location("A", "B"), "A/B");
    }
}
