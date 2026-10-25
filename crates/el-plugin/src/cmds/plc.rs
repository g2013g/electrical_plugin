//! `EL_PLC` — parametric PLC I/O modules (textbook CH10).
//!
//! A module is **generated from its counts**: `EL_PLC P1 8,6 40,160` draws an
//! 8-input / 6-output card — body, point stubs, per-point addresses — and its
//! points are ordinary pins (`P1.I3`, `P1.Q1`), so wires, reports, nets and
//! audit work through the standing machinery. Re-placing regenerates the
//! glyph from the record; editing the counts or BASE rebuilds addresses.

use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::PlcRec;

use crate::cmds::common::{remove_handles, Args};
use crate::geometry::plc_entities;
use crate::hostops::parse_handle;

fn lib_for(host: &mut dyn HostApi) -> el_core::symbol::SymbolLibrary {
    crate::library::load(host).unwrap_or_default()
}

/// Run `EL_PLC IMPORT <tag> <path.csv>` — wire a module's points from a
/// spreadsheet (textbook CH10's PLC I/O list, inverted: the PLCIO report
/// writes this shape, import reads it back). Rows: `point,node[,net]` — the
/// point on this module (`I3`, `Q1`), the device pin the wire reaches
/// (`K1.A1`), and an optional net label that becomes the wire's note.
fn import_io(
    host: &mut dyn HostApi,
    idx: &el_core::ElIndex,
    lib: &el_core::symbol::SymbolLibrary,
    a: &Args,
) {
    use el_core::model::LAYER_WIRES;
    let words: Vec<String> = a.free.clone();
    let (Some(tag), Some(path_raw)) = (words.first().cloned(), words.get(1).cloned()) else {
        host.push_error("EL_PLC: IMPORT needs the module and a file — EL_PLC IMPORT P1 io.csv");
        return;
    };
    let Some(m) = idx.plc_by_tag(&tag) else {
        host.push_error(&format!("EL_PLC: no module tagged {tag:?} — EL_PLC places one first"));
        return;
    };
    let path = crate::library::input_with_library(host, &path_raw);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!(
                "EL_PLC: cannot read {} ({path_raw}): {e}",
                path.display()
            ));
            return;
        }
    };

    // Rows: point,node[,net]. A header row (point,node…) is skipped, as
    // spreadsheets export one.
    let mut wired = 0usize;
    let mut errors = 0usize;
    let mut rows: Vec<(String, [f64; 3], [f64; 3], String, String, String)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split(',').map(|c| c.trim()).collect();
        if n == 0 && cols.first().map(|c| c.eq_ignore_ascii_case("point")).unwrap_or(false) {
            continue;
        }
        if cols.len() < 2 {
            errors += 1;
            continue;
        }
        let (point, node) = (cols[0], cols[1]);
        let net = cols.get(2).copied().unwrap_or("");
        let Some([px, py]) = m.rec.point_position(point) else {
            host.push_error(&format!(
                "EL_PLC: {point:?} is not a point of {} — its points are {}",
                m.rec.tag,
                m.rec.point_names().join(", ")
            ));
            errors += 1;
            continue;
        };
        match crate::cmds::wire::resolve_node(idx, lib, node, false) {
            Ok(pt) => {
                let (fid, fpin) = crate::cmds::wire::split_node(node);
                rows.push((
                    net.to_string(),
                    [px, py, 0.0],
                    pt,
                    m.rec.tag.clone(),
                    point.to_string(),
                    format!("{fid}|{fpin}"),
                ));
            }
            Err(e) => {
                host.push_error(&format!("EL_PLC: row {n}: {node}: {e}"));
                errors += 1;
            }
        }
    }
    if rows.is_empty() {
        host.push_error(&format!(
            "EL_PLC: nothing importable — {errors} row(s) rejected; rows are point,node[,net]"
        ));
        return;
    }

    host.push_undo(&format!("EL_PLC IMPORT {}", m.rec.tag));
    for (net, from, to, tag, point, far) in &rows {
        let (far_id, far_pin) = far.split_once('|').unwrap_or((far, ""));
        let mut rec = el_core::model::ElWireRec::new(&el_core::new_el_id(), tag, far_id);
        rec.from_pin = point.clone();
        rec.to_pin = far_pin.to_string();
        rec.note = net.clone();
        let layer = if rec.wire_type.is_empty() {
            LAYER_WIRES.to_string()
        } else {
            idx.wire_layer_for(&rec.wire_type)
        };
        let record = ElRecord::encode_wire(&rec);
        host.add_entity(crate::geometry::wire_polyline(&record, &[*from, *to], &layer));
        wired += 1;
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_PLC: {wired} point(s) of {} wired from {}{}",
        m.rec.tag,
        path.display(),
        if errors > 0 { format!(" ({errors} row(s) rejected — see the errors)") } else { String::new() },
    ));
}

/// Run `EL_PLC <tag> [di,do] [x,y] [BASE=I:1/] [BASEQ=] [DESC= MFG= CAT=]
/// | EDIT <tag> [DI= DO= BASEI= BASEQ=] | LIST | CLEAR <tag|ALL>`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());

    match a.first_word().map(|w| w.to_ascii_uppercase()).as_deref() {
        Some("LIST") => {
            list(host, &idx);
            return;
        }
        Some("EDIT") => {
            a.free.remove(0);
            edit(host, &idx, &mut a);
            return;
        }
        Some("CLEAR") => {
            a.free.remove(0);
            clear(host, &idx, &a);
            return;
        }
        Some("IMPORT") => {
            a.free.remove(0);
            let lib = lib_for(host);
            import_io(host, &idx, &lib, &a);
            return;
        }
        _ => {}
    }

    let Some(tag) = a.first_word().map(|s| s.to_string()) else {
        host.push_error(
            "EL_PLC: tag the module and give its point counts — EL_PLC P1 8,6 40,160",
        );
        return;
    };
    a.free.remove(0);

    // `<di>,<do>` — the counts that generate everything.
    let (mut di, mut do_): (i32, i32) = match idx.plc_by_tag(&tag) {
        Some(m) => (m.rec.di, m.rec.do_),
        None => (8, 8),
    };
    if let Some(t) = a
        .free
        .iter()
        .find(|t| t.contains(',') && !t.contains('='))
    {
        let parts: Vec<&str> = t.split(',').collect();
        if parts.len() == 2 {
            match (parts[0].parse::<i32>(), parts[1].parse::<i32>()) {
                (Ok(x), Ok(y)) if x >= 0 && y >= 0 => {
                    di = x;
                    do_ = y;
                }
                _ => {
                    host.push_error("EL_PLC: point counts are non-negative integers — 8,6");
                    return;
                }
            }
        }
    }
    if di == 0 && do_ == 0 {
        host.push_error("EL_PLC: a module needs at least one input or output point");
        return;
    }

    // An existing module is edited in place: re-placing keeps the tag and
    // takes new counts/position, exactly the parametric model.
    let existing = idx.plc_by_tag(&tag).cloned();
    let mut rec = match &existing {
        Some(m) => m.rec.clone(),
        None => PlcRec::new(&el_core::new_el_id(), &tag, di, do_),
    };
    rec.di = di;
    rec.do_ = do_;
    if let Some(pt) = a
        .free
        .iter()
        .find_map(|t| crate::cmds::common::parse_coord_token(t))
    {
        rec.origin_x = pt[0];
        rec.origin_y = pt[1];
    } else if existing.is_none() {
        host.push_error("EL_PLC: where? give a point — EL_PLC P1 8,6 40,160");
        return;
    }
    if let Some(v) = a.get("BASE") {
        rec.base_i = v.to_string();
    }
    if let Some(v) = a.get("BASEI") {
        rec.base_i = v.to_string();
    }
    if let Some(v) = a.get("BASEQ") {
        rec.base_q = v.to_string();
    }
    if let Some(v) = a.get("DESC") {
        rec.description = v.to_string();
    }
    if let Some(v) = a.get("MFG") {
        rec.mfg = v.to_string();
    }
    if let Some(v) = a.get("CAT") {
        rec.cat = v.to_string();
    }

    host.push_undo("EL_PLC");
    if let Some(old) = &existing {
        remove_handles(host, idx.handles_for_el_id(&old.rec.el_id), "EL_PLC (regenerating)");
    }
    let record = ElRecord::encode_plc(&rec);
    let parts = plc_entities(&record, &rec);
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_PLC: {} — {}DI/{}DO at ({}, {}), addresses {}0 … and {}0 …",
        rec.tag,
        rec.di,
        rec.do_,
        el_core::fmt_mm(rec.origin_x),
        el_core::fmt_mm(rec.origin_y),
        rec.base_i,
        rec.base_q,
    ));
}

fn edit(host: &mut dyn HostApi, idx: &el_core::ElIndex, a: &mut Args) {
    let Some(tag) = a.free.first() else {
        host.push_error("EL_PLC: EDIT needs the module — EL_PLC EDIT P1 DI=16");
        return;
    };
    let Some(m) = idx.plc_by_tag(&tag) else {
        host.push_error(&format!("EL_PLC: no module tagged {tag:?}"));
        return;
    };
    let mut rec = m.rec.clone();
    let mut changed: Vec<String> = Vec::new();
    if let Some(v) = a.integer("DI") {
        if v < 0 {
            host.push_error("EL_PLC: DI= is non-negative");
            return;
        }
        rec.di = v;
        changed.push(format!("DI={v}"));
    }
    if let Some(v) = a.integer("DO") {
        if v < 0 {
            host.push_error("EL_PLC: DO= is non-negative");
            return;
        }
        rec.do_ = v;
        changed.push(format!("DO={v}"));
    }
    if rec.di == 0 && rec.do_ == 0 {
        host.push_error("EL_PLC: a module needs at least one point");
        return;
    }
    if let Some(v) = a.get("BASEI") {
        rec.base_i = v.to_string();
        changed.push(format!("BASEI={v}"));
    }
    if let Some(v) = a.get("BASEQ") {
        rec.base_q = v.to_string();
        changed.push(format!("BASEQ={v}"));
    }
    if changed.is_empty() {
        host.push_error("EL_PLC: nothing to change — pass DI=, DO=, BASEI= or BASEQ=");
        return;
    }
    host.push_undo("EL_PLC EDIT");
    let record = ElRecord::encode_plc(&rec);
    remove_handles(host, idx.handles_for_el_id(&rec.el_id), "EL_PLC EDIT (regenerating)");
    let parts = plc_entities(&record, &rec);
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_PLC: {} — {}", rec.tag, changed.join(", ")));
}

fn list(host: &mut dyn HostApi, idx: &el_core::ElIndex) {
    if idx.plcs.is_empty() {
        host.push_output("EL_PLC: no modules — EL_PLC P1 8,6 40,160 places one");
        return;
    }
    host.push_output(&format!("EL_PLC: {} module(s)", idx.plcs.len()));
    for m in idx.plcs.values() {
        let wired = idx
            .wires
            .values()
            .any(|w| w.rec.from_id.eq_ignore_ascii_case(&m.rec.tag)
                || w.rec.to_id.eq_ignore_ascii_case(&m.rec.tag));
        host.push_output(&format!(
            "  {} — {}DI/{}DO, {} … {}{}",
            m.rec.tag,
            m.rec.di,
            m.rec.do_,
            m.rec.address_of("I1").unwrap_or_default(),
            m.rec
                .address_of(&format!("Q{}", m.rec.do_.max(1)))
                .unwrap_or_default(),
            if wired { String::new() } else { "  (no wires)".to_string() },
        ));
    }
}

fn clear(host: &mut dyn HostApi, idx: &el_core::ElIndex, a: &Args) {
    let all = a.free.iter().any(|t| t.eq_ignore_ascii_case("ALL"));
    let targets: Vec<el_core::index::IndexedPlc> = if all {
        idx.plcs.values().cloned().collect()
    } else {
        match a.free.first() {
            Some(t) => {
                // A handle of any part resolves too.
                if let Some(m) = idx.plc_by_tag(t) {
                    vec![m.clone()]
                } else if let Some(h) = parse_handle(t) {
                    match idx.plc_of_handle(h.value()) {
                        Some(m) => vec![m.clone()],
                        None => {
                            host.push_error(&format!("EL_PLC: no module matches {t:?}"));
                            return;
                        }
                    }
                } else {
                    host.push_error(&format!("EL_PLC: no module matches {t:?}"));
                    return;
                }
            }
            None => {
                host.push_error("EL_PLC: CLEAR needs a tag or ALL");
                return;
            }
        }
    };
    host.push_undo("EL_PLC CLEAR");
    for m in &targets {
        remove_handles(host, idx.handles_for_el_id(&m.rec.el_id), "EL_PLC CLEAR");
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_PLC: {} module(s) removed", targets.len()));
}
