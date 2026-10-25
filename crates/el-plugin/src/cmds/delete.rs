//! `EL_DELETE` — remove an electrical object, with everything drawn for it.
//!
//! Deletion is by `el_id`, not by handle, because an object may be several
//! entities — deleting one part of a ladder must remove the whole frame, or the
//! drawing is left with orphaned rungs and no record.

use ocs_plugin_api::host::HostApi;


use crate::cmds::common::Args;
use crate::hostops::Capture;
use crate::state;

/// Run `EL_DELETE <wirenr|tag|ladder-id|handle|@SEL> … [FORCE]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let force = a.flag("FORCE").unwrap_or(false);

    let mut tokens: Vec<String> = a
        .free
        .iter()
        .filter(|t| !t.eq_ignore_ascii_case("FORCE"))
        .cloned()
        .collect();
    let _ = &mut a;

    // Bare `EL_DELETE` acts on the inspected entity, matching the ribbon
    // workflow where the user inspects and then deletes.
    if tokens.is_empty() {
        match state::last_target() {
            Some(_) => tokens.push("@SEL".to_string()),
            None => {
                host.push_error(
                    "EL_DELETE: name what to delete, or run EL_INFO first and pass @SEL",
                );
                return;
            }
        }
    }

    let capture = Capture::take(host);
    let mut plan: Vec<(String, Vec<u64>, &'static str)> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for token in &tokens {
        match resolve(&capture, token) {
            Ok(resolved) => plan.push(resolved),
            Err(e) => failures.push(e),
        }
    }

    for e in &failures {
        host.push_error(&format!("EL_DELETE: {e}"));
    }
    if plan.is_empty() {
        return;
    }

    // A device is drawn as many entities and is a normal thing to delete, so it
    // needs no confirmation — the report below says how much went. A *ladder* is
    // the one object where a mistaken name is expensive and the name is short, so
    // that is where an explicit FORCE is asked for.
    if !force {
        let ladders: Vec<String> = plan
            .iter()
            .filter(|(_, handles, kind)| *kind == "ladder" && handles.len() > 1)
            .map(|(n, h, _)| format!("{n} ({} entit(ies))", h.len()))
            .collect();
        if !ladders.is_empty() {
            host.push_error(&format!(
                "EL_DELETE: this would remove a whole ladder — pass FORCE to confirm: {}",
                ladders.join("; ")
            ));
            return;
        }
    }

    host.push_undo(&format!("EL_DELETE ({} object(s))", plan.len()));
    let mut removed = 0usize;
    let mut summary: Vec<String> = Vec::new();
    let mut wires_gone = false;

    for (name, handles, kind) in &plan {
        let mut n = 0;
        for h in handles {
            if host.remove_entity(ocs_plugin_api::host::codec::Handle::new(*h)) {
                n += 1;
            }
        }
        if n != handles.len() {
            host.push_error(&format!(
                "EL_DELETE: {name} — only {n} of {} entit(ies) could be removed \
                 (is a layer locked?)",
                handles.len()
            ));
        }
        removed += n;
        if *kind == "wire" {
            wires_gone = true;
        }
        summary.push(format!("{name} ({kind})"));
    }

    // The label layer is a render of the records, so a deleted wire's number has
    // to go with it. Rebuild from the numbers the drawing started this command
    // with, minus the wires just removed — a re-read would still show them,
    // because this dispatch's snapshot predates the removal.
    if wires_gone {
        let deleted: std::collections::BTreeSet<u64> =
            plan.iter().flat_map(|(_, h, _)| h.iter().copied()).collect();
        let numbers: Vec<(u64, String)> = crate::cmds::wireno::current_numbers(&capture.index)
            .into_iter()
            .filter(|(h, _)| !deleted.contains(h))
            .collect();
        crate::cmds::wireno::redraw_labels(host, &capture.index, &numbers);
    }

    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_DELETE: {} — {removed} entit(ies) removed",
        summary.join(", ")
    ));
}

/// Resolve one token to what should actually be removed.
fn resolve(
    capture: &Capture,
    token: &str,
) -> Result<(String, Vec<u64>, &'static str), String> {
    if let Some(pinned) = state::resolve_sel_token(token) {
        let Some(handle) = pinned else {
            return Err("nothing inspected yet — pick an object first".to_string());
        };
        return resolve_handle(capture, handle, token);
    }

    if let Some(w) = capture.index.wire_by_ref(token) {
        let handles = capture.index.handles_for_el_id(&w.rec.el_id).to_vec();
        let label = if w.rec.wire_ref.is_empty() {
            format!("0x{:X}", w.handle)
        } else {
            w.rec.wire_ref.clone()
        };
        return Ok((label, handles, "wire"));
    }

    if let Some(l) = capture.index.ladder_by_el_id(token) {
        return Ok((format!("ladder {}", l.el_id), l.handles.clone(), "ladder"));
    }

    if let Some(s) = capture
        .index
        .signals
        .values()
        .find(|s| s.rec.name.eq_ignore_ascii_case(token))
    {
        let handles = capture.index.handles_for_el_id(&s.rec.el_id).to_vec();
        return Ok((format!("signal {}", s.rec.name), handles, "signal"));
    }

    if let Some(m) = capture.index.plc_by_tag(token) {
        let handles = capture.index.handles_for_el_id(&m.rec.el_id).to_vec();
        return Ok((format!("PLC {}", m.rec.tag), handles, "plc"));
    }

    if let Some(t) = capture
        .index
        .terminals
        .values()
        .find(|t| t.rec.designator().eq_ignore_ascii_case(token))
    {
        // A jumper bridging this terminal has no reason to outlive it: the
        // bridge names a terminal that would no longer exist.
        let mut handles = capture.index.handles_for_el_id(&t.rec.el_id).to_vec();
        for j in capture
            .index
            .jumpers
            .values()
            .filter(|j| j.rec.touches(&t.rec.designator()))
        {
            handles.extend(capture.index.handles_for_el_id(&j.rec.el_id).iter().copied());
        }
        return Ok((t.rec.designator(), handles, "terminal"));
    }

    // A tag names a *device*, and a device is its parent plus every contact
    // sharing its tag. Deleting only one of them would leave the rest behind
    // with a tag that no longer resolves — so the whole group goes.
    if let Some(parent) = capture.index.parent_by_tag(token) {
        let mut handles = capture.index.handles_for_el_id(&parent.rec.el_id).to_vec();
        let mut kids = 0usize;
        for child in capture.index.children_of(token) {
            handles.extend(capture.index.handles_for_el_id(&child.rec.el_id));
            kids += 1;
        }
        // The panel footprint linked to this tag goes with the device — a
        // body with no device is exactly the audit's dangling case.
        if let Some(f) = capture.index.footprint_by_tag(token) {
            handles.extend(capture.index.handles_for_el_id(&f.rec.el_id));
        }
        let label = if kids == 0 {
            parent.rec.tag.clone()
        } else {
            format!("{} (+{kids} child device(s))", parent.rec.tag)
        };
        return Ok((label, handles, "component"));
    }
    // A child with no parent placed is still deletable by tag.
    if let Some(c) = capture.index.component_by_tag(token) {
        let handles = capture.index.handles_for_el_id(&c.rec.el_id).to_vec();
        return Ok((c.rec.tag.clone(), handles, "component"));
    }

    if let Some(h) = crate::hostops::parse_handle(token) {
        return resolve_handle(capture, h.value(), token);
    }

    Err(format!(
        "no electrical object matches {token:?} — expected a wire number, a tag, a ladder id, or a \
         0x-prefixed handle"
    ))
}

fn resolve_handle(
    capture: &Capture,
    handle: u64,
    token: &str,
) -> Result<(String, Vec<u64>, &'static str), String> {
    if let Some(l) = capture.index.ladder_of_handle(handle) {
        return Ok((format!("ladder {}", l.el_id), l.handles.clone(), "ladder"));
    }
    if let Some(w) = capture.index.wire_of_handle(handle) {
        let handles = capture.index.handles_for_el_id(&w.rec.el_id).to_vec();
        let label = if w.rec.wire_ref.is_empty() {
            token.to_string()
        } else {
            w.rec.wire_ref.clone()
        };
        return Ok((label, handles, "wire"));
    }
    if let Some(c) = capture.index.component_of_handle(handle) {
        let handles = capture.index.handles_for_el_id(&c.rec.el_id).to_vec();
        return Ok((c.rec.tag.clone(), handles, "component"));
    }
    if let Some(s) = capture.index.signal_of_handle(handle) {
        let handles = capture.index.handles_for_el_id(&s.rec.el_id).to_vec();
        return Ok((format!("signal {}", s.rec.name), handles, "signal"));
    }
    if let Some(t) = capture.index.terminal_of_handle(handle) {
        let mut handles = capture.index.handles_for_el_id(&t.rec.el_id).to_vec();
        for j in capture
            .index
            .jumpers
            .values()
            .filter(|j| j.rec.touches(&t.rec.designator()))
        {
            handles.extend(capture.index.handles_for_el_id(&j.rec.el_id).iter().copied());
        }
        return Ok((t.rec.designator(), handles, "terminal"));
    }
    if let Some(t) = capture.index.titleblock_of_handle(handle) {
        let handles = capture.index.handles_for_el_id(&t.rec.el_id).to_vec();
        return Ok(("title block".to_string(), handles, "titleblock"));
    }
    if let Some(m) = capture.index.plc_of_handle(handle) {
        let handles = capture.index.handles_for_el_id(&m.rec.el_id).to_vec();
        return Ok((format!("PLC {}", m.rec.tag), handles, "plc"));
    }
    if let Some(f) = capture.index.footprint_of_handle(handle) {
        let handles = capture.index.handles_for_el_id(&f.rec.el_id).to_vec();
        return Ok((format!("footprint {}", f.rec.tag), handles, "footprint"));
    }
    if let Some(j) = capture.index.jumper_of_handle(handle) {
        let handles = capture.index.handles_for_el_id(&j.rec.el_id).to_vec();
        let (a, b) = j.rec.designators();
        return Ok((format!("jumper {a}–{b}"), handles, "jumper"));
    }
    Err(format!("{token} is not an electrical object"))
}

