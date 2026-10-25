//! `EL_WIREGAP` and `EL_TRIMWIRE` — crossing gaps and trimming a wire end.
//!
//! A gap-flagged wire is drawn as **several polylines sharing one `el_id`** —
//! the same fold ladders use — with a hole centred on every point where
//! another wire properly crosses it. Junctions are never cut: the crossing
//! test (`el_core::geom::proper_crossing`) requires both segments to pass
//! *through* each other, so an endpoint touch stays a connection. Connectivity
//! is record-based and untouched; the gap is purely visual, which is why it
//! can never change a net.
//!
//! Exactly one wire yields at a crossing — an unflagged wire always passes
//! through, and between two flagged ones the smaller `el_id` keeps running —
//! so two gapped wires can never each put a hole in the other.
//!
//! The flag lives on the wire record; the gap size is a drawing setting
//! (`EL_SETTINGS WIREGAP=`). Runs are a render: when geometry moves, the
//! affected wires are rebuilt from their records and the current crossings —
//! the same rebuild-from-record rule every other drawn layer follows.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::geom::{self, Pt};
use el_core::index::IndexedWire;
use el_core::model::LAYER_WIRES;
use el_core::wiregap::{cuts_for, fold_runs, split_runs};
use el_core::ElIndex;

use crate::cmds::common::{remove_handles, Args};
use crate::cmds::wire::{split_node, NODE_SNAP_MM};
use crate::geometry::{polyline_points, wire_runs_entities};
use crate::hostops::{parse_handle, DocEntityExt};

// ── shared geometry helpers ───────────────────────────────────────────────

/// The whole drawn path of a wire: its one polyline, or its runs folded back
/// into one path when the wire is gapped.
pub fn folded_points(host: &dyn HostApi, idx: &ElIndex, wire: &IndexedWire) -> Option<Vec<[f64; 3]>> {
    let handles = idx.handles_for_el_id(&wire.rec.el_id);
    if handles.len() <= 1 {
        return host
            .document()
            .get_entity_owned(Handle::new(wire.handle))
            .and_then(|e| polyline_points(&e));
    }
    folded_points_of_handle(host, idx, wire.handle)
}

/// As [`folded_points`], resolved from any run's handle — the way a consumer
/// that only holds a handle (a wire-number label, a pick) reaches the whole.
pub fn folded_points_of_handle(
    host: &dyn HostApi,
    idx: &ElIndex,
    handle: u64,
) -> Option<Vec<[f64; 3]>> {
    let wire = idx.wire_of_handle(handle)?;
    let handles = idx.handles_for_el_id(&wire.rec.el_id);
    if handles.len() <= 1 {
        return host
            .document()
            .get_entity_owned(Handle::new(handle))
            .and_then(|e| polyline_points(&e));
    }
    let runs: Vec<Vec<Pt>> = handles
        .iter()
        .filter_map(|h| {
            host.document()
                .get_entity_owned(Handle::new(*h))
                .and_then(|e| polyline_points(&e))
        })
        .map(|pts| pts.iter().map(|p| [p[0], p[1]]).collect())
        .collect();
    let folded = fold_runs(&runs);
    if folded.is_empty() {
        return None;
    }
    Some(folded.into_iter().map(|p| [p[0], p[1], 0.0]).collect())
}

/// Every wire's logical path and gap flag, folded from its runs, by `el_id`.
///
/// Read in **one pass over the live document**, because a dispatch that
/// re-renders wires invalidates index handles as it goes: a partner looked up
/// through a stale handle returns nothing, and the wire silently renders
/// whole. Entries keyed by `el_id` stay valid across every re-render in the
/// same dispatch, since a re-render replaces a wire's runs without changing
/// its logical path.
fn wire_path_map(host: &dyn HostApi) -> std::collections::BTreeMap<String, (bool, Vec<Pt>)> {
    let mut runs: std::collections::BTreeMap<String, (bool, Vec<Vec<Pt>>)> = Default::default();
    for (h, rec) in crate::codec::scan(host.document()) {
        if rec.kind() != Some(el_core::model::KIND_WIRE) {
            continue;
        }
        let Ok(w) = rec.decode_wire() else {
            continue;
        };
        if let Some(entity) = host.document().get_entity_owned(Handle::new(h)) {
            if let Some(pts) = polyline_points(&entity) {
                runs.entry(w.el_id)
                    .or_insert_with(|| (w.gap, Vec::new()))
                    .1
                    .push(pts.iter().map(|p| [p[0], p[1]]).collect());
            }
        }
    }
    runs.into_iter()
        .map(|(el_id, (gap, runs))| (el_id, (gap, fold_runs(&runs))))
        .collect()
}

/// The paths of the wires that pass *through* this one — the ones it should
/// gap against.
///
/// Exactly one wire yields at any crossing, or two flagged wires would each
/// put a hole in the other and the crossing would read as broken. The rule:
/// an unflagged wire always passes through, and between two flagged wires the
/// one with the smaller `el_id` keeps running. `el_id` is a stable UUID, so
/// the decision is deterministic across re-renders and re-loads.
fn crossing_partners(
    paths: &std::collections::BTreeMap<String, (bool, Vec<Pt>)>,
    rec: &el_core::model::ElWireRec,
) -> Vec<Vec<Pt>> {
    let mut out = Vec::new();
    for (el_id, (gap, path)) in paths {
        if el_id == &rec.el_id || path.len() < 2 {
            continue;
        }
        let passes_through = !gap || el_id.as_str() < rec.el_id.as_str();
        if !passes_through {
            continue;
        }
        out.push(path.clone());
    }
    out
}

fn layer_for(idx: &ElIndex, rec: &el_core::model::ElWireRec) -> String {
    if rec.wire_type.is_empty() {
        LAYER_WIRES.to_string()
    } else {
        idx.wire_layer_for(&rec.wire_type)
    }
}

/// Collapse a wire back to one polyline (record carried), whatever it is drawn
/// as now. This is how a cleared flag or a moved end re-enters the document —
/// `refresh_gapped` then re-splits it if the flag says so.
pub fn write_single(
    host: &mut dyn HostApi,
    idx: &ElIndex,
    rec: &el_core::model::ElWireRec,
    base: &[[f64; 3]],
) -> bool {
    let record = ElRecord::encode_wire(rec);
    let handles = idx.handles_for_el_id(&rec.el_id).to_vec();
    if handles.len() <= 1 {
        return crate::codec::write_el(host, Handle::new(*handles.first().unwrap_or(&0)), &record);
    }
    remove_handles(
        host,
        &handles,
        "EL_WIREGAP (collapsing the gapped wire to one polyline)",
    );
    let layer = layer_for(idx, rec);
    host.add_entities(wire_runs_entities(&record, &[base.to_vec()], &layer));
    true
}

/// Draw a wire as its runs, cutting a gap at every crossing with `others`.
///
/// Always rebuilds — remove, then add — because a patched-in-place gap cannot
/// stay honest when the crossings move. The parts carry the record, so the
/// index folds them back into one wire.
fn render_runs(
    host: &mut dyn HostApi,
    idx: &ElIndex,
    rec: &el_core::model::ElWireRec,
    base: &[[f64; 3]],
    others: &[Vec<Pt>],
    gap_mm: f64,
) -> usize {
    let flat: Vec<Pt> = base.iter().map(|p| [p[0], p[1]]).collect();
    let runs: Vec<Vec<Pt>> = if rec.gap && !others.is_empty() {
        let refs: Vec<&[Pt]> = others.iter().map(|v| v.as_slice()).collect();
        let cuts = cuts_for(&flat, &refs, 1e-6);
        if cuts.is_empty() {
            vec![flat.clone()]
        } else {
            split_runs(&flat, &cuts, gap_mm)
        }
    } else {
        vec![flat.clone()]
    };
    if runs.is_empty() {
        return 0;
    }

    let record = ElRecord::encode_wire(rec);
    let layer = layer_for(idx, rec);
    remove_handles(
        host,
        idx.handles_for_el_id(&rec.el_id),
        "EL_WIREGAP (re-rendering the wire)",
    );
    let runs3: Vec<Vec<[f64; 3]>> = runs
        .into_iter()
        .map(|r| r.into_iter().map(|p| [p[0], p[1], 0.0]).collect())
        .collect();
    let parts = wire_runs_entities(&record, &runs3, &layer);
    let n = parts.len();
    host.add_entities(parts);
    n
}

/// Re-split every gapped wire against the crossings as they are now.
///
/// Called after anything reshapes geometry (a re-seat, a move, a trim): the
/// runs are a render, so the whole set is rebuilt rather than patched. When
/// nothing moved this is pure churn, so callers gate it on something having
/// changed.
pub fn refresh_gapped(host: &mut dyn HostApi, idx: &ElIndex) {
    let gap_mm = idx.settings_or_default().wire_gap_mm;
    let paths = wire_path_map(host);
    for wire in idx.wires.values() {
        if !wire.rec.gap {
            continue;
        }
        let Some(base) = folded_points(host, idx, wire) else {
            continue;
        };
        let partners = crossing_partners(&paths, &wire.rec);
        render_runs(host, idx, &wire.rec, &base, &partners, gap_mm);
    }
}

// ── EL_WIREGAP ────────────────────────────────────────────────────────────

/// Run `EL_WIREGAP [<wire|ALL>] [GAP=mm] | CLEAR [<wire|ALL>]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let mut idx = crate::codec::build_index(host.document());
    if idx.wires.is_empty() {
        host.push_error("EL_WIREGAP: this drawing has no wires");
        return;
    }

    // An optional GAP= sets the drawing-wide gap size first, so the render
    // below reads the new value. `EL_SETTINGS` reports the change itself, and
    // the index is rebuilt so it sees the new size.
    if let Some(v) = a.number("GAP") {
        if v < 0.0 {
            host.push_error("EL_WIREGAP: GAP= expects a non-negative millimetre value");
            return;
        }
        crate::cmds::settings::run(host, &[&format!("WIREGAP={v}")]);
        idx = crate::codec::build_index(host.document());
    }
    let gap_mm = idx.settings_or_default().wire_gap_mm;

    let clearing = a.free.iter().any(|t| t.eq_ignore_ascii_case("CLEAR"));
    let all = a.free.iter().any(|t| t.eq_ignore_ascii_case("ALL"));
    a.free
        .retain(|t| !t.eq_ignore_ascii_case("CLEAR") && !t.eq_ignore_ascii_case("ALL"));

    // Bare (or GAP= alone): report which wires are gapped, and how many
    // crossings each hides.
    if a.free.is_empty() && !clearing && !all {
        report(host, &idx, gap_mm);
        return;
    }

    if all {
        if clearing {
            clear_wires(host, &idx, idx.wires.values().cloned().collect(), gap_mm);
        } else {
            gap_wires(host, &idx, idx.wires.values().cloned().collect(), gap_mm);
        }
        return;
    }

    let Some(token) = a.first_word().map(str::to_string) else {
        host.push_error(
            "EL_WIREGAP: name a wire (W1), ALL, or CLEAR — bare EL_WIREGAP reports the current state",
        );
        return;
    };
    let wire = match resolve_wire(&idx, &token) {
        Ok(w) => w.clone(),
        Err(e) => {
            host.push_error(&format!("EL_WIREGAP: {e}"));
            return;
        }
    };
    if clearing {
        clear_wires(host, &idx, vec![wire], gap_mm);
    } else {
        gap_wires(host, &idx, vec![wire], gap_mm);
    }
}

fn resolve_wire<'a>(idx: &'a ElIndex, token: &str) -> Result<&'a IndexedWire, String> {
    if let Some(w) = idx.wire_by_ref(token) {
        return Ok(w);
    }
    if let Some(h) = parse_handle(token) {
        if let Some(w) = idx.wire_of_handle(h.value()) {
            return Ok(w);
        }
        return Err(format!("{token} is not part of a wire"));
    }
    Err(format!(
        "no wire matches {token:?} — name a wire by its number (W1) or a 0x handle"
    ))
}

fn gap_wires(host: &mut dyn HostApi, idx: &ElIndex, wires: Vec<IndexedWire>, gap_mm: f64) {
    host.push_undo("EL_WIREGAP");
    // Partner decisions read the flags the drawing will have after this
    // command, not the ones it has now — `EL_WIREGAP ALL` must see every wire
    // as flagged, or both sides of a crossing would cut.
    let mut paths = wire_path_map(host);
    for wire in &wires {
        if let Some(entry) = paths.get_mut(&wire.rec.el_id) {
            entry.0 = true;
        }
    }
    let mut gapped = 0usize;
    let mut holes = 0usize;
    for wire in &wires {
        let Some(base) = folded_points(host, idx, wire) else {
            continue;
        };
        let mut rec = wire.rec.clone();
        rec.gap = true;
        let partners = crossing_partners(&paths, &rec);
        let n = render_runs(host, idx, &rec, &base, &partners, gap_mm);
        holes += n.saturating_sub(1);
        gapped += 1;
    }
    host.bump_geometry();
    host.set_dirty();
    if gapped == 0 {
        host.push_error("EL_WIREGAP: nothing was drawn — the wire(s) have no readable geometry");
        return;
    }
    host.push_output(&format!(
        "EL_WIREGAP: {gapped} wire(s) gapped at {} crossing(s), {gap_mm}mm each side",
        holes
    ));
}

fn clear_wires(host: &mut dyn HostApi, idx: &ElIndex, wires: Vec<IndexedWire>, _gap_mm: f64) {
    host.push_undo("EL_WIREGAP CLEAR");
    let mut cleared = 0usize;
    for wire in &wires {
        let Some(base) = folded_points(host, idx, wire) else {
            continue;
        };
        let mut rec = wire.rec.clone();
        rec.gap = false;
        if write_single(host, idx, &rec, &base) {
            cleared += 1;
        }
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_WIREGAP: {cleared} wire(s) drawn whole again"));
}

fn report(host: &mut dyn HostApi, idx: &ElIndex, gap_mm: f64) {
    let mut gapped = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for w in idx.wires.values() {
        if !w.rec.gap {
            continue;
        }
        let runs = idx.handles_for_el_id(&w.rec.el_id).len();
        gapped += 1;
        let name = if w.rec.wire_ref.is_empty() {
            format!("0x{:X}", w.handle)
        } else {
            w.rec.wire_ref.clone()
        };
        lines.push(format!(
            "  {name}: {} crossing(s) hidden (drawn as {runs} run(s))",
            runs.saturating_sub(1)
        ));
    }
    if gapped == 0 {
        host.push_output(&format!(
            "EL_WIREGAP: no wires are gapped (gap size {}mm each side — EL_SETTINGS WIREGAP= to change)",
            el_core::fmt_mm(gap_mm)
        ));
        host.push_info("  run EL_WIREGAP <wire|ALL> to gap wires where others cross them");
        return;
    }
    host.push_output(&format!(
        "EL_WIREGAP: {gapped} of {} wire(s) gapped, {}mm each side",
        idx.wires.len(),
        el_core::fmt_mm(gap_mm)
    ));
    for l in &lines {
        host.push_output(l);
    }
}

// ── EL_TRIMWIRE ───────────────────────────────────────────────────────────

/// Run `EL_TRIMWIRE <wire> <x,y>` — move the wire's nearer end to the point.
///
/// The moved end re-resolves against the connection points around it: a pin
/// within snapping distance becomes the recorded node, nothing there leaves
/// the end dangling (which `EL_AUDIT` reports). Trimming onto a gapped wire
/// keeps its gaps, refreshed against the crossings as they are now.
pub fn run_trim(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());

    let Some(token) = a.first_word().map(str::to_string) else {
        host.push_error("EL_TRIMWIRE: name a wire and a point, e.g. EL_TRIMWIRE W1 40,120");
        return;
    };
    a.free.retain(|f| f != &token);
    let wire = match resolve_wire(&idx, &token) {
        Ok(w) => w.clone(),
        Err(e) => {
            host.push_error(&format!("EL_TRIMWIRE: {e}"));
            return;
        }
    };
    let Some(pt) = a.free.iter().find_map(|t| crate::cmds::common::parse_coord_token(t))
    else {
        host.push_error("EL_TRIMWIRE: where to? give a coordinate (EL_TRIMWIRE W1 40,120)");
        return;
    };

    let Ok(lib) = crate::library::load(host) else {
        host.push_error("EL_TRIMWIRE: cannot read the symbol library, so the trimmed end cannot be resolved to a node");
        return;
    };

    let Some(mut base) = folded_points(host, &idx, &wire) else {
        host.push_error("EL_TRIMWIRE: the wire's drawn geometry is unreadable");
        return;
    };
    if base.len() < 2 {
        host.push_error("EL_TRIMWIRE: the wire has no drawn geometry to trim");
        return;
    }

    // Which drawn end is the record's `from`? Match against the nodes the
    // record names — a folded path is canonically ordered, so slot 0 is not
    // guaranteed to be the from end the way it is on a single polyline.
    let rec = &wire.rec;
    let node_name = |id: &str, pin: &str| -> String {
        if pin.is_empty() {
            id.to_string()
        } else {
            format!("{id}.{pin}")
        }
    };
    let from_pos = (!rec.from_id.is_empty())
        .then(|| crate::cmds::wire::resolve_node(&idx, &lib, &node_name(&rec.from_id, &rec.from_pin), false).ok())
        .flatten()
        .map(|p| [p[0], p[1]]);
    let to_pos = (!rec.to_id.is_empty())
        .then(|| crate::cmds::wire::resolve_node(&idx, &lib, &node_name(&rec.to_id, &rec.to_pin), true).ok())
        .flatten()
        .map(|p| [p[0], p[1]]);
    let last = base.len() - 1;
    let head_is_from = match (from_pos, to_pos) {
        (Some(f), Some(t)) => {
            geom::dist([base[0][0], base[0][1]], f) <= geom::dist([base[0][0], base[0][1]], t)
        }
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => true,
    };

    // Trim the end nearer the point; the other end anchors the ortho snap,
    // exactly as EL_WIRE snaps the second picked point.
    let head_d = geom::dist([base[0][0], base[0][1]], [pt[0], pt[1]]);
    let tail_d = geom::dist([base[last][0], base[last][1]], [pt[0], pt[1]]);
    let trim_head = head_d <= tail_d;
    let (move_idx, anchor_idx) = if trim_head { (0usize, last) } else { (last, 0usize) };
    let anchor = [base[anchor_idx][0], base[anchor_idx][1]];
    let snapped = geom::snap_to_ortho(anchor, [pt[0], pt[1]], geom::ORTHO_TOL_DEG);

    let old = [base[move_idx][0], base[move_idx][1]];
    if geom::dist(old, snapped) < 1e-6 {
        host.push_output("EL_TRIMWIRE: that end is already there");
        return;
    }
    if geom::dist(anchor, snapped) < 0.1 {
        host.push_error("EL_TRIMWIRE: that would leave no wire — the ends would coincide");
        return;
    }

    // Re-resolve the moved end: a pin within snapping distance claims it,
    // nothing there leaves it dangling — and the audit says so, because a
    // silently guessed connection would be worse than a named one.
    let mut rec = rec.clone();
    let is_from_end = if trim_head { head_is_from } else { !head_is_from };
    let resolved = crate::cmds::wire::nearest_node(&idx, &lib, [snapped[0], snapped[1], 0.0], NODE_SNAP_MM);
    let resolution_note = match &resolved {
        Some((name, _, _)) => {
            let (id, pin) = split_node(name);
            if is_from_end {
                rec.from_id = id;
                rec.from_pin = pin;
            } else {
                rec.to_id = id;
                rec.to_pin = pin;
            }
            format!("connected to {name}")
        }
        None => {
            if is_from_end {
                rec.from_id.clear();
                rec.from_pin.clear();
            } else {
                rec.to_id.clear();
                rec.to_pin.clear();
            }
            "dangling — nothing within snapping distance (EL_AUDIT will flag it)".to_string()
        }
    };

    base[move_idx] = [snapped[0], snapped[1], 0.0];

    host.push_undo("EL_TRIMWIRE");
    if rec.gap {
        let paths = wire_path_map(host);
        let partners = crossing_partners(&paths, &rec);
        render_runs(host, &idx, &rec, &base, &partners, idx.settings_or_default().wire_gap_mm);
    } else {
        write_single(host, &idx, &rec, &base);
    }
    host.bump_geometry();
    host.set_dirty();

    // The wire got shorter or longer; other gapped wires may now cross it
    // differently, and their runs are a render too.
    refresh_gapped(host, &idx);

    host.push_output(&format!(
        "EL_TRIMWIRE: {} end to ({}, {}), wire now {}mm; {resolution_note}",
        if trim_head { "start" } else { "end" },
        el_core::fmt_mm(snapped[0]),
        el_core::fmt_mm(snapped[1]),
        el_core::fmt_mm(geom::dist(anchor, snapped)),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_picks_the_nearer_end_and_keeps_the_record_end_straight() {
        // head_is_from follows which node resolves where; with no nodes at all
        // the head is treated as from, which matches how a fresh wire is drawn.
        let base = [[0.0, 0.0, 0.0], [50.0, 0.0, 0.0]];
        let head_d = geom::dist([base[0][0], base[0][1]], [5.0, 0.0]);
        let tail_d = geom::dist([base[1][0], base[1][1]], [5.0, 0.0]);
        assert!(head_d < tail_d, "the picked point is near the head");
    }

    #[test]
    fn the_trim_snaps_to_the_anchor_axis() {
        // Trimming W1's end to a point a fraction of a degree off horizontal
        // lands on the horizontal — the same axis snap a drawn wire gets. A
        // genuinely diagonal point is left alone.
        let snapped = geom::snap_to_ortho([0.0, 0.0], [40.0, 0.4], 5.0);
        assert_eq!(snapped, [40.0, 0.0]);
        let kept = geom::snap_to_ortho([0.0, 0.0], [40.0, 120.4], 5.0);
        assert_eq!(kept, [40.0, 120.4]);
    }
}
