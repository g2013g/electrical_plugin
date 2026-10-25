//! `EL_CONNECTOR` — parametric connectors.
//!
//! A connector's pins are *generated* from a list and a spacing rather than drawn
//! one at a time, which is what makes a 24-way connector one command instead of
//! twenty-four. The pin list is ordered data, not a set: the order is the drawn
//! geometry, so `PIN=1,2,3` puts 1 leftmost and swapping two entries moves two
//! pins.
//!
//! Changing the pin list rebuilds the glyph, because the glyph is derived from the
//! record — the same rule that governs ladders, terminals and wire numbers.

use ocs_plugin_api::host::HostApi;
use ocs_plugin_api::host::{CommandStep, InteractiveCommand};

use el_core::codec::ElRecord;
use el_core::model::{ConnectorOrientation, ConnectorRec};

use crate::cmds::common::Args;
use crate::hostops::DocEntityExt;
use crate::geometry::connector_entities;
use crate::state::{self, PendingLine};

/// Run `EL_CONNECTOR [refdes] [x,y] [PINS=a,b,c | COUNT=n] [SPACING=] [PERROW=]
/// [ORIENT=combo|plug|receptacle] [ROT=] [MFG=] [CAT=] [PART=] [DESC=] [LOC=]
/// [WNC]` or `EL_CONNECTOR LIST` / `EL_CONNECTOR CLEAR` / `EL_CONNECTOR EDIT …`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);

    match a.first_word().map(|w| w.to_ascii_uppercase()).as_deref() {
        Some("LIST") => {
            list(host);
            return;
        }
        Some("CLEAR") => {
            clear(host);
            return;
        }
        Some("EDIT") => {
            a.free.retain(|t| !t.eq_ignore_ascii_case("EDIT"));
            edit(host, &mut a);
            return;
        }
        _ => {}
    }

    let idx = crate::codec::build_index(host.document());
    let words: Vec<String> = a
        .free
        .iter()
        .filter(|t| !t.contains(','))
        .cloned()
        .collect();

    // A reference designator is an `X`-family tag; a coordinate is a coordinate.
    let ref_des = words
        .first()
        .cloned()
        .or_else(|| a.get("REF").map(str::to_string))
        .unwrap_or_else(|| {
            // Allocate the next free one in the X family, so a placement needs no
            // reference at all.
            el_core::tagging::next_tag(&idx, "X", &idx.settings_or_default(), None)
                .unwrap_or_else(|| "X1".to_string())
        });

    let pins: Vec<String> = match a.get("PINS") {
        Some(list) => list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        None => {
            let count = a.integer("COUNT").unwrap_or(4).max(1);
            (1..=count).map(|i| i.to_string()).collect()
        }
    };
    if pins.is_empty() {
        host.push_error("EL_CONNECTOR: the pin list is empty — pass PINS=a,b,c or COUNT=n");
        return;
    }

    let mut rec = ConnectorRec::new(&el_core::new_el_id(), &ref_des, pins.len());
    rec.pins = pins;
    if let Some(v) = a.number("SPACING") {
        if v <= 0.0 {
            host.push_error("EL_CONNECTOR: SPACING must be positive");
            return;
        }
        rec.pin_spacing_mm = v;
    }
    rec.pins_per_row = a.integer("PERROW").unwrap_or(0).max(0);
    if let Some(v) = a.get("ORIENT") {
        match ConnectorOrientation::parse(v) {
            Some(o) => rec.orientation = o,
            None => {
                host.push_error(&format!(
                    "EL_CONNECTOR: ORIENT expects combo, plug or receptacle, got {v:?}"
                ));
                return;
            }
        }
    }
    if let Some(v) = a.get("MFG") {
        rec.mfg = v.to_string();
    }
    if let Some(v) = a.get("CAT") {
        rec.cat = v.to_string();
    }
    if let Some(v) = a.get("PART") {
        rec.part_number = v.to_string();
    }
    if let Some(v) = a.get("DESC") {
        rec.description = v.to_string();
    }
    if let Some(v) = a.get("LOC") {
        rec.location = v.to_string();
    }
    rec.wire_number_change = a.flag("WNC").unwrap_or(false);
    if let Some(c) = a.first_coord() {
        rec.origin_x = c[0];
        rec.origin_y = c[1];
    }
    rec.rotation_deg = a.number("ROT").unwrap_or(0.0);

    for e in &a.errors {
        host.push_error(&format!("EL_CONNECTOR: {e}"));
    }

    let rot = a.number("ROT").unwrap_or(0.0).to_radians();
    match a.first_coord() {
        Some(c) => place(host, rec, [c[0], c[1], 0.0], rot),
        None => start_interactive(host, rec),
    }
}

fn place(host: &mut dyn HostApi, rec: ConnectorRec, at: [f64; 3], rot: f64) {
    let record = ElRecord::encode_connector(&rec);
    let parts = connector_entities(&record, &rec, at, rot);
    let count = parts.len();

    host.push_undo("EL_CONNECTOR");
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();

    host.push_output(&format!(
        "EL_CONNECTOR: {} — {} pin(s) at {}mm, {} at ({}, {}) — {count} entit(ies)",
        rec.ref_des,
        rec.pin_count(),
        el_core::fmt_mm(rec.pin_spacing_mm),
        rec.orientation.as_str().to_ascii_lowercase(),
        el_core::fmt_mm(at[0]),
        el_core::fmt_mm(at[1])
    ));
    host.push_output(&format!("  pins: {}", rec.pins.join(",")));
    if rec.pins_per_row > 0 {
        host.push_output(&format!(
            "  wrapped at {} pins per row",
            rec.pins_per_row
        ));
    }
}

fn start_interactive(host: &mut dyn HostApi, rec: ConnectorRec) {
    host.push_info(&format!(
        "EL_CONNECTOR {}: pick the connector position ({} pin(s): {})",
        rec.ref_des,
        rec.pin_count(),
        rec.pins.join(",")
    ));
    host.start_interactive(Box::new(ConnectorCmd { rec, placed: false }));
}

struct ConnectorCmd {
    rec: ConnectorRec,
    placed: bool,
}

impl InteractiveCommand for ConnectorCmd {
    fn prompt(&self) -> String {
        "EL_CONNECTOR: pick the connector position".to_string()
    }

    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep {
        if self.placed {
            return CommandStep::Done;
        }
        self.placed = true;

        let mut rec = self.rec.clone();
        rec.origin_x = pt[0];
        rec.origin_y = pt[1];
        let record = ElRecord::encode_connector(&rec);
        let parts = connector_entities(&record, &rec, pt, 0.0);
        let count = parts.len();

        state::push_entities(parts);
        state::spawn_flush(Some("EL_CONNECTOR".to_string()));
        state::emit(PendingLine::Output(format!(
            "EL_CONNECTOR: {} ({} pins) at ({}, {}) — {count} entit(ies)",
            self.rec.ref_des,
            self.rec.pin_count(),
            el_core::fmt_mm(pt[0]),
            el_core::fmt_mm(pt[1])
        )));
        CommandStep::Done
    }

    fn on_enter(&mut self) -> CommandStep {
        CommandStep::Cancel
    }
}

/// Edit a placed connector and rebuild its glyph.
///
/// The pin field is geometry, so a record-only change to the pin list would leave
/// the drawing showing the old pin count — which is why this rebuilds rather than
/// writes.
fn edit(host: &mut dyn HostApi, a: &mut Args) {
    let idx = crate::codec::build_index(host.document());
    let Some(token) = a.first_word().map(str::to_string) else {
        host.push_error("EL_CONNECTOR EDIT: name the connector, e.g. EL_CONNECTOR EDIT X1 PINS=1,2");
        return;
    };
    let Some(found) = idx.connectors_ordered().into_iter().find(|c| {
        c.rec.ref_des.eq_ignore_ascii_case(&token) || format!("0x{:X}", c.primary()) == token
    }) else {
        host.push_error(&format!("EL_CONNECTOR EDIT: no connector {token:?}"));
        return;
    };

    a.free.retain(|t| t != &token);
    let mut rec = found.rec.clone();
    let mut changed: Vec<String> = Vec::new();

    if let Some(list) = a.get("PINS") {
        rec.pins = list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        changed.push(format!("PINS={}", rec.pins.join(",")));
    }
    if let Some(v) = a.integer("PERROW") {
        rec.pins_per_row = v.max(0);
        changed.push(format!("PERROW={}", rec.pins_per_row));
    }
    if let Some(v) = a.number("SPACING") {
        if v <= 0.0 {
            a.errors.push("SPACING must be positive".to_string());
        } else {
            rec.pin_spacing_mm = v;
            changed.push(format!("SPACING={}", el_core::fmt_mm(v)));
        }
    }
    if let Some(v) = a.get("REF") {
        rec.ref_des = v.to_string();
        changed.push(format!("REF={v}"));
    }
    if let Some(v) = a.get("MFG") {
        rec.mfg = v.to_string();
        changed.push(format!("MFG={v}"));
    }
    if let Some(v) = a.get("CAT") {
        rec.cat = v.to_string();
        changed.push(format!("CAT={v}"));
    }
    if let Some(v) = a.get("PART") {
        rec.part_number = v.to_string();
        changed.push(format!("PART={v}"));
    }
    if let Some(v) = a.get("DESC") {
        rec.description = v.to_string();
        changed.push(format!("DESC={v}"));
    }
    if let Some(v) = a.flag("WNC") {
        rec.wire_number_change = v;
        changed.push(format!("WNC={v}"));
    }

    for e in &a.errors {
        host.push_error(&format!("EL_CONNECTOR EDIT: {e}"));
    }
    if changed.is_empty() {
        host.push_error("EL_CONNECTOR EDIT: nothing to change");
        return;
    }

    host.push_undo("EL_CONNECTOR EDIT");
    let record = ElRecord::encode_connector(&rec);
    for h in idx.handles_for_el_id(&rec.el_id) {
        crate::codec::write_el(host, ocs_plugin_api::host::codec::Handle::new(*h), &record);
    }
    // Rebuild, because the pin field is drawn geometry.
    let anchor = host
        .document()
        .get_entity_owned(ocs_plugin_api::host::codec::Handle::new(found.primary()))
        .and_then(|e| crate::geometry::carrier_anchor(&e))
        .unwrap_or([0.0, 0.0, 0.0]);
    for h in idx.handles_for_el_id(&rec.el_id) {
        host.remove_entity(ocs_plugin_api::host::codec::Handle::new(*h));
    }
    host.add_entities(connector_entities(&record, &rec, anchor, 0.0));

    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_CONNECTOR EDIT: {} — {}; now {} pin(s)",
        rec.ref_des,
        changed.join(", "),
        rec.pin_count()
    ));
}

fn list(host: &mut dyn HostApi) {
    let idx = crate::codec::build_index(host.document());
    let conns = idx.connectors_ordered();
    if conns.is_empty() {
        host.push_output("EL_CONNECTOR: this drawing has no connectors");
        return;
    }
    host.push_output(&format!("EL_CONNECTOR — {} connector(s)", conns.len()));
    for c in conns {
        host.push_output(&format!(
            "  {:<8} {:>3} pin(s) @{}mm  {:<10} {}",
            if c.rec.ref_des.is_empty() {
                format!("0x{:X}", c.primary())
            } else {
                c.rec.ref_des.clone()
            },
            c.rec.pin_count(),
            el_core::fmt_mm(c.rec.pin_spacing_mm),
            c.rec.orientation.as_str().to_ascii_lowercase(),
            c.rec.pins.join(",")
        ));
    }
}

fn clear(host: &mut dyn HostApi) {
    let idx = crate::codec::build_index(host.document());
    if idx.connectors.is_empty() {
        host.push_output("EL_CONNECTOR CLEAR: this drawing has no connectors");
        return;
    }
    host.push_undo("EL_CONNECTOR CLEAR");
    let n = crate::cmds::common::remove_entities_on_layer(host, crate::geometry::LAYER_COMPONENTS);
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_CONNECTOR CLEAR: removed {n} entit(ies) on the component layer"
    ));
}
