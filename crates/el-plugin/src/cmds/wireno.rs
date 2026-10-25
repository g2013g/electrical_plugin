//! `EL_WIRENO` — assign wire numbers.
//!
//! This is the capability the whole ladder exists to serve. Two modes:
//!
//! * **Sequential** — a running counter in drawing order (top to bottom, left to
//!   right), skipping numbers a fixed wire has reserved.
//! * **Reference** — the wire's number *is* the line reference of the rung it
//!   sits on, suffixed when a rung carries more than one wire.
//!
//! The numbers themselves are planned in `el-core`; this module supplies what
//! only the drawing can: where each wire is, whether its type is numbered, and
//! which rung it lies on.
//!
//! Wire-number labels are **derived, not stored**: the whole `EL_WIRENO` layer is
//! cleared and redrawn from the records on every run. That makes a renumber
//! idempotent and impossible to desync from the data, at the cost of a
//! hand-moved label returning to its wire's midpoint.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::geom::{self, Pt};
use el_core::model::{NumberType, RUNG_TOLERANCE_MM};
use el_core::numbering::{self, WireNumberInput, WireNumberPlan};
use el_core::ElIndex;

use crate::cmds::common::{remove_entities_on_layer, Args};
use crate::codec::build_index;
use crate::geometry::wire_number_label;

/// Run `EL_WIRENO [ALL | <wire>] | CLEAR | FIX <wire> | UNFIX <wire>`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let action = a
        .first_word()
        .map(|w| w.to_ascii_uppercase())
        .unwrap_or_else(|| "ALL".to_string());

    match action.as_str() {
        "CLEAR" => clear(host),
        "FIX" | "UNFIX" => pin(host, &mut a, action == "FIX"),
        "ALL" | "RENUMBER" => renumber(host, None),
        other => {
            // A wire number, or a wire's handle, renumbers just that wire.
            renumber(host, Some(other.to_string()));
        }
    }
}

/// Remove every drawn wire number, leaving the records' numbers intact.
fn clear(host: &mut dyn HostApi) {
    host.push_undo("EL_WIRENO CLEAR");
    let n = remove_entities_on_layer(host, el_core::model::LAYER_WIRENO);
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_WIRENO CLEAR: removed {n} wire-number label(s); stored numbers are unchanged"
    ));
}

/// Mark a wire's number fixed (or unfixed).
fn pin(host: &mut dyn HostApi, a: &mut Args, fixed: bool) {
    let idx = build_index(host.document());
    let Some(target) = a.free.iter().skip(1).find(|t| !t.contains(',')).cloned() else {
        host.push_error(&format!(
            "EL_WIRENO {}: name the wire, e.g. EL_WIRENO {} W3",
            if fixed { "FIX" } else { "UNFIX" },
            if fixed { "FIX" } else { "UNFIX" }
        ));
        return;
    };

    let Some(wire) = idx.wire_by_ref(&target) else {
        host.push_error(&format!("EL_WIRENO: no wire numbered {target:?}"));
        return;
    };
    if fixed && wire.rec.wire_ref.is_empty() {
        host.push_error(&format!(
            "EL_WIRENO FIX: {target} has no number yet — run EL_WIRENO first, then fix it"
        ));
        return;
    }

    let mut rec = wire.rec.clone();
    rec.number_type = if fixed {
        NumberType::Fixed
    } else {
        NumberType::Normal
    };
    let record = ElRecord::encode_wire(&rec);

    host.push_undo(if fixed { "EL_WIRENO FIX" } else { "EL_WIRENO UNFIX" });
    if crate::codec::write_el(host, Handle::new(wire.handle), &record) {
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!(
            "EL_WIRENO: {} is now {} ({})",
            rec.wire_ref,
            if fixed { "fixed" } else { "normal" },
            if fixed {
                "it keeps this number when the drawing is renumbered"
            } else {
                "it will be renumbered"
            }
        ));
    } else {
        host.push_error("EL_WIRENO: the host refused the write");
    }
}

/// Compute and apply numbers. `only` restricts the run to one wire.
fn renumber(host: &mut dyn HostApi, only: Option<String>) {
    let _ = std::fs::write("wn-1-index", "1");
    let idx = build_index(host.document());
    let _ = std::fs::write("wn-2-index-built", "1");
    let settings = idx.settings_or_default();

    if idx.wires.is_empty() {
        host.push_error("EL_WIRENO: this drawing has no wires");
        return;
    }

    // What only the drawing knows: where each wire is, and which rung it is on.
    let mut inputs: Vec<WireNumberInput> = Vec::new();

    let _ = std::fs::write("wn-3-loop-start", "1");
    for (handle, wire) in &idx.wires {
        if let Some(ref want) = only {
            let matches = wire.rec.wire_ref.eq_ignore_ascii_case(want)
                || format!("{:#x}", handle).eq_ignore_ascii_case(want)
                || crate::hostops::parse_handle(want).map(|h| h.value()) == Some(*handle);
            if !matches {
                continue;
            }
        }

        let _ = std::fs::write("wn-4-pre-midpoint", "1");
        let Some(mid) = wire_midpoint_of(host, &idx, *handle) else {
            // No drawn geometry: nothing can be numbered or labelled, and
            // guessing a position would put a label in the wrong place.
            host.push_error(&format!(
                "EL_WIRENO: wire {:#x} has no polyline geometry, so it was skipped",
                handle
            ));
            continue;
        };

        let _ = std::fs::write("wn-5-post-midpoint", "1");
        let type_name = if wire.rec.wire_type.is_empty() {
            idx.wire_layer_for("")
        } else {
            wire.rec.wire_type.clone()
        };
        let numbered = idx.type_is_numbered(&type_name);
        let rung_ref = idx.rung_reference_at(mid, RUNG_TOLERANCE_MM * 2.0);

        inputs.push(WireNumberInput {
            handle: *handle,
            existing: wire.rec.wire_ref.clone(),
            fixed: wire.rec.number_type == NumberType::Fixed,
            numbered,
            rung_ref,
        });
    }

    if inputs.is_empty() {
        host.push_error(&format!(
            "EL_WIRENO: no wire matches {:?}",
            only.unwrap_or_default()
        ));
        return;
    }

    // Order by drawing position so sequential numbering runs the way a drafter
    // reads the sheet, not the order the scan happened to find entities.
    let positions: Vec<(u64, Pt)> = inputs
        .iter()
        .filter_map(|i| wire_midpoint_of(host, &idx, i.handle).map(|m| (i.handle, m)))
        .collect();
    let order = numbering::drawing_order(&positions);
    let ordered: Vec<WireNumberInput> = order
        .iter()
        .filter_map(|h| inputs.iter().find(|i| i.handle == *h).cloned())
        .collect();

    let plan = numbering::plan(&ordered, &settings);
    report_plan(host, &plan, &idx);

    if plan.assignments.is_empty() {
        // A renumber that does nothing is the outcome a user most needs
        // explained, and it is the one a scripted caller can actually see:
        // only errors reach the command line in a headless session.
        let mut reasons: Vec<String> = Vec::new();
        for (_, reason) in &plan.skipped {
            if reason == "already correct" || reason == "fixed number kept" {
                continue;
            }
            if !reasons.contains(reason) {
                reasons.push(reason.clone());
            }
        }
        // Reference mode with no ladder is the classic misconfiguration, and the
        // *fix* belongs in this one message: the host surfaces only the last
        // error pushed, so a separate hint would be overwritten by this one.
        let hint = if settings.wireno_mode == el_core::model::WireNoMode::Reference
            && idx.ladders.is_empty()
        {
            " — reference mode numbers a wire from the rung it sits on, and this drawing has no \
             ladder; run EL_LADDER, or switch to sequential with EL_SETTINGS WIREMODE=S"
        } else {
            ""
        };
        host.push_error(&format!(
            "EL_WIRENO: nothing to number ({} wire(s) considered) — {}{hint}",
            ordered.len(),
            if reasons.is_empty() {
                "every wire already carries its number".to_string()
            } else {
                reasons.join("; ")
            }
        ));
    }

    // One undo window for the whole renumber: the record writes and the label
    // rebuild are one user-visible action.
    host.push_undo(&format!(
        "EL_WIRENO ({}, {} wire(s))",
        settings.wireno_mode.as_str(),
        plan.assignments.len()
    ));

    let mut applied: Vec<(u64, String)> = Vec::new();
    let mut refused: Vec<u64> = Vec::new();
    for (handle, number) in &plan.assignments {
        let Some(wire) = idx.wires.get(handle) else {
            continue;
        };
        let mut rec = wire.rec.clone();
        rec.wire_ref = number.clone();
        let record = ElRecord::encode_wire(&rec);
        // Every run of a gapped wire carries the record, so the number goes to
        // all of them — the fold makes the runs read as one wire.
        let mut written = false;
        for h in idx.handles_for_el_id(&rec.el_id) {
            written |= crate::codec::write_el(host, Handle::new(*h), &record);
        }
        if written {
            applied.push((*handle, number.clone()));
        } else {
            refused.push(*handle);
        }
    }

    // Labels come from what was actually stored, not from what we intended: a
    // wire whose write the host refused must not be labelled with a number it
    // does not carry. The list is built rather than re-read because a record
    // written moments ago is not yet in this dispatch's document snapshot.
    let numbers = apply_numbers(current_numbers(&idx), &applied);
    let label_count = redraw_labels(host, &idx, &numbers);

    host.bump_geometry();
    host.set_dirty();

    // A refused write used to be invisible here: the labels would appear and the
    // data would silently not be stored.
    if !refused.is_empty() {
        host.push_error(&format!(
            "EL_WIRENO: {} of {} record(s) could not be written (is a layer locked?), so those              wires are unnumbered and unlabelled",
            refused.len(),
            plan.assignments.len()
        ));
    }
    host.push_output(&format!(
        "EL_WIRENO: {} number(s) written, {label_count} label(s) drawn",
        applied.len()
    ));
}

/// Redraw every wire-number label from an explicit `(handle, number)` list.
///
/// The numbers are **passed in rather than re-read**, because a record written
/// earlier in the same dispatch is not visible in this plugin's cached document
/// snapshot — `host.document()` still holds the state from the start of the
/// command. Re-reading would find every wire unnumbered and draw nothing.
/// Callers that changed numbers know what they are; callers that did not take
/// them from the index they already hold.
///
/// Geometry *is* read from the document, which is right: a wire's position did
/// not change, and a handle that no longer resolves (a wire just deleted) is
/// skipped by that lookup.
///
/// Returns the number of labels drawn.
pub fn redraw_labels(
    host: &mut dyn HostApi,
    idx: &ElIndex,
    numbers: &[(u64, String)],
) -> usize {
    remove_entities_on_layer(host, el_core::model::LAYER_WIRENO);
    let mut labels = Vec::new();
    for (handle, number) in numbers {
        if number.is_empty() {
            continue;
        }
        // A gapped wire's label sits at the midpoint of its *whole* path — the
        // runs folded back together — not of whichever run the handle names.
        let Some(pts) = crate::cmds::wiregap::folded_points_of_handle(host, idx, *handle) else {
            continue;
        };
        let flat: Vec<Pt> = pts.iter().map(|p| [p[0], p[1]]).collect();
        if let Some(mid) = geom::polyline_midpoint(&flat) {
            labels.push(wire_number_label(number, [mid[0], mid[1], 0.0]));
        }
    }
    let n = labels.len();
    if !labels.is_empty() {
        host.add_entities(labels);
    }
    n
}

/// The numbers the drawing currently carries, straight from the index.
pub fn current_numbers(idx: &ElIndex) -> Vec<(u64, String)> {
    idx.wires
        .iter()
        .filter(|(_, w)| !w.rec.wire_ref.is_empty())
        .map(|(h, w)| (*h, w.rec.wire_ref.clone()))
        .collect()
}

/// Overlay `assignments` onto a list of current numbers.
pub fn apply_numbers(
    mut numbers: Vec<(u64, String)>,
    assignments: &[(u64, String)],
) -> Vec<(u64, String)> {
    for (handle, number) in assignments {
        numbers.retain(|(h, _)| h != handle);
        numbers.push((*handle, number.clone()));
    }
    numbers.sort_by_key(|(h, _)| *h);
    numbers
}

/// Explain the plan before applying it.
fn report_plan(host: &mut dyn HostApi, plan: &WireNumberPlan, idx: &ElIndex) {
    host.push_output(&format!("EL_WIRENO — {}", plan.summary()));

    // Surface the first few assignments so the numbering is visible without
    // opening the records.
    for (handle, number) in plan.assignments.iter().take(8) {
        let from = idx
            .wires
            .get(handle)
            .map(|w| {
                if w.rec.wire_ref.is_empty() {
                    "(none)".to_string()
                } else {
                    w.rec.wire_ref.clone()
                }
            })
            .unwrap_or_default();
        host.push_output(&format!("  {from} → {number}"));
    }
    if plan.assignments.len() > 8 {
        host.push_output(&format!("  … and {} more", plan.assignments.len() - 8));
    }

    // Group the skip reasons: a long list of identical reasons is noise, but
    // the *reasons* are exactly what the user needs to fix.
    let mut reasons: Vec<(String, usize)> = Vec::new();
    for (_, reason) in &plan.skipped {
        match reasons.iter_mut().find(|(r, _)| r == reason) {
            Some((_, n)) => *n += 1,
            None => reasons.push((reason.clone(), 1)),
        }
    }
    for (reason, n) in &reasons {
        if reason == "already correct" || reason == "fixed number kept" {
            continue;
        }
        host.push_info(&format!("  {n} skipped: {reason}"));
    }
}

/// The midpoint of a wire's drawn geometry — its whole path, runs folded.
fn wire_midpoint_of(host: &dyn HostApi, idx: &ElIndex, handle: u64) -> Option<Pt> {
    let pts = crate::cmds::wiregap::folded_points_of_handle(host, idx, handle)?;
    let flat: Vec<Pt> = pts.iter().map(|p| [p[0], p[1]]).collect();
    geom::polyline_midpoint(&flat)
}
