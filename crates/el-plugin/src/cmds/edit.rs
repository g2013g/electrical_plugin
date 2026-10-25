//! `EL_EDIT` — change one object's record in place.
//!
//! One command rather than one per property, because the host's UI vocabulary is
//! buttons and dropdowns: a dropdown dispatches `EL_EDIT @SEL COLOR=RD`, so a new
//! property needs no new command and no new ribbon plumbing. `@SEL` resolves to
//! the entity the inspector last saw, which is how a ribbon preset applies to
//! what the user just clicked.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::NumberType;
use el_core::ElIndex;

use crate::cmds::common::Args;
use crate::hostops::DocEntityExt;
use crate::hostops::Capture;
use crate::state;

/// What an edit is aimed at.
enum EditTarget {
    Wire(u64),
    Ladder(String),
    Component(u64),
}

/// Run `EL_EDIT <wirenr|tag|handle|@SEL> KEY=VALUE …`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);

    let Some(target_token) = a.first_word().map(str::to_string) else {
        host.push_error(
            "EL_EDIT: name what to edit — a wire number, a handle, or @SEL for the inspected entity",
        );
        return;
    };
    // The target is a bare token, not a property, so drop it from the key list.
    a.free.retain(|f| f != &target_token);

    if a.kv.is_empty() {
        host.push_error("EL_EDIT: nothing to change — pass KEY=VALUE, e.g. EL_EDIT W1 COLOR=RD");
        return;
    }

    let capture = Capture::take(host);
    let target = match resolve(&capture, &target_token) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("EL_EDIT: {e}"));
            return;
        }
    };

    match target {
        EditTarget::Wire(handle) => edit_wire(host, &capture.index, handle, &mut a),
        EditTarget::Ladder(el_id) => edit_ladder(host, &capture.index, &el_id, &mut a),
        EditTarget::Component(handle) => edit_component(host, &capture.index, handle, &mut a),
    }
}

fn resolve(capture: &Capture, token: &str) -> Result<EditTarget, String> {
    if let Some(pinned) = state::resolve_sel_token(token) {
        let Some(handle) = pinned else {
            return Err(
                "nothing inspected yet — run EL_INFO without arguments and pick an entity, or name \
                 the object directly"
                    .to_string(),
            );
        };
        return classify(capture, handle);
    }
    // A device tag is tried first: the commonest thing to edit, and a tag like
    // `K1` is never a wire number or a ladder id. The *parent* is preferred
    // because a child shares the tag, so a bare lookup could land on a contact
    // and edit the wrong record.
    if let Some(p) = capture.index.parent_by_tag(token) {
        return Ok(EditTarget::Component(p.primary()));
    }
    if let Some(c) = capture.index.component_by_tag(token) {
        return Ok(EditTarget::Component(c.primary()));
    }
    if let Some(w) = capture.index.wire_by_ref(token) {
        return Ok(EditTarget::Wire(w.handle));
    }
    if let Some(l) = capture.index.ladder_by_el_id(token) {
        return Ok(EditTarget::Ladder(l.el_id.clone()));
    }
    if let Some(h) = crate::hostops::parse_handle(token) {
        return classify(capture, h.value());
    }
    Err(format!(
        "no wire, device or ladder matches {token:?} — expected a tag (K1), a wire number (W1), a \
         ladder id, or a 0x-prefixed handle"
    ))
}

fn classify(capture: &Capture, handle: u64) -> Result<EditTarget, String> {
    if let Some(l) = capture.index.ladder_of_handle(handle) {
        return Ok(EditTarget::Ladder(l.el_id.clone()));
    }
    // Any run of a gapped wire resolves to the whole wire.
    if let Some(w) = capture.index.wire_of_handle(handle) {
        return Ok(EditTarget::Wire(w.handle));
    }
    if capture.index.component_of_handle(handle).is_some() {
        return Ok(EditTarget::Component(handle));
    }
    // A symbol is drawn as several entities sharing one record, so a picked part
    // resolves through the carrier map.
    if let Some(carrier) = capture.carrier_of(Handle::new(handle)) {
        if capture.index.component_of_handle(carrier).is_some() {
            return Ok(EditTarget::Component(carrier));
        }
        if let Some(w) = capture.index.wire_of_handle(carrier) {
            return Ok(EditTarget::Wire(w.handle));
        }
    }
    Err(format!(
        "handle {handle:#x} is not a wire, device or ladder this build can edit"
    ))
}

/// Edit a wire's record, and refresh its label when the number moved.
fn edit_wire(host: &mut dyn HostApi, idx: &ElIndex, handle: u64, a: &mut Args) {
    let Some(wire) = idx.wires.get(&handle) else {
        host.push_error(&format!("EL_EDIT: no wire at handle {handle:#x}"));
        return;
    };
    let mut rec = wire.rec.clone();
    let mut changed: Vec<String> = Vec::new();
    let mut number_changed = false;

    for (key, value) in a.kv.clone() {
        match key.as_str() {
            "TYPE" | "WIRETYPE" => {
                rec.wire_type = value.clone();
                changed.push(format!("TYPE={value}"));
                // Moving to a type that resolves elsewhere moves the drawn
                // entity, or the layer and the record would disagree. A gapped
                // wire is several runs, and they move together.
                let layer = idx.wire_layer_for(&value);
                for h in idx.handles_for_el_id(&rec.el_id) {
                    if let Some(entity) = host.document().get_entity_owned(Handle::new(*h)) {
                        let mut entity = entity;
                        entity.common_mut().layer = layer.clone();
                        host.update_entity(entity);
                    }
                }
            }
            "GAUGE" => {
                rec.gauge = value.clone();
                changed.push(format!("GAUGE={value}"));
            }
            "COLOR" | "COLOUR" => {
                rec.color = value.clone();
                changed.push(format!("COLOR={value}"));
                // Keep the drawn colour in step with the record, on every run.
                for h in idx.handles_for_el_id(&rec.el_id) {
                    if let Some(mut entity) = host.document().get_entity_owned(Handle::new(*h)) {
                        entity.common_mut().color = match el_core::model::color_to_aci(&value) {
                            Some(aci) => ocs_plugin_api::host::codec::Color::Index(aci),
                            None => ocs_plugin_api::host::codec::Color::ByLayer,
                        };
                        host.update_entity(entity);
                    }
                }
            }
            "WREF" | "NUMBER" => {
                rec.wire_ref = value.clone();
                number_changed = true;
                changed.push(format!("WREF={value}"));
            }
            "NTYPE" | "NUMBERTYPE" => match NumberType::parse(&value) {
                Some(t) => {
                    rec.number_type = t;
                    changed.push(format!("NTYPE={}", t.as_str()));
                }
                None => a.errors.push(format!(
                    "NTYPE expects NORMAL, FIXED or EXTRA, got {value:?}"
                )),
            },
            "NOTE" | "NET" => {
                rec.note = value.clone();
                changed.push(format!("NOTE={value}"));
            }
            other => a.errors.push(format!(
                "unknown wire property {other} — try TYPE, GAUGE, COLOR, WREF, NTYPE or NOTE"
            )),
        }
    }

    for e in &a.errors {
        host.push_error(&format!("EL_EDIT: {e}"));
    }
    if changed.is_empty() {
        return;
    }

    let record = ElRecord::encode_wire(&rec);
    host.push_undo(&format!("EL_EDIT ({} value(s))", changed.len()));
    // Every run of a gapped wire carries the record — write them all, or the
    // runs would disagree about the wire they are part of.
    let mut written = false;
    for h in idx.handles_for_el_id(&rec.el_id) {
        written |= crate::codec::write_el(host, Handle::new(*h), &record);
    }

    // A changed number means the drawn label is stale, and the label layer is a
    // pure render of the records — so rebuild it rather than trying to find and
    // patch the one label.
    if written && number_changed {
        // Rebuild the label layer — but from an explicit map, because the write
        // made a moment ago is not in this dispatch's document snapshot.
        let mut numbers = crate::cmds::wireno::current_numbers(idx);
        numbers.retain(|(h, _)| *h != handle);
        if !rec.wire_ref.is_empty() {
            numbers.push((handle, rec.wire_ref.clone()));
        }
        crate::cmds::wireno::redraw_labels(host, idx, &numbers);
    }

    host.bump_geometry();
    host.set_dirty();
    if written {
        host.push_output(&format!(
            "EL_EDIT: wire {} — {}",
            if rec.wire_ref.is_empty() {
                format!("0x{handle:X}")
            } else {
                rec.wire_ref.clone()
            },
            changed.join(", ")
        ));
    } else {
        host.push_error("EL_EDIT: the host refused the write (is the layer locked?)");
    }
}

/// Edit a ladder by rebuilding its frame from the record.
fn edit_ladder(host: &mut dyn HostApi, idx: &ElIndex, el_id: &str, a: &mut Args) {
    let Some(ladder) = idx.ladder_by_el_id(el_id) else {
        host.push_error(&format!("EL_EDIT: no ladder with id {el_id}"));
        return;
    };
    let mut rec = ladder.rec.clone();
    let mut changed: Vec<String> = Vec::new();

    for (key, value) in a.kv.clone() {
        match key.as_str() {
            "REF" | "FIRST_REF" => {
                rec.first_ref = value.clone();
                changed.push(format!("REF={value}"));
            }
            "INC" => match value.parse::<i32>() {
                Ok(v) if v != 0 => {
                    rec.rung_increment = v;
                    changed.push(format!("INC={v}"));
                }
                _ => a.errors
                    .push(format!("INC expects a non-zero integer, got {value:?}")),
            },
            "WIDTH" => match value.parse::<f64>() {
                Ok(v) if v > 0.0 => {
                    rec.width_mm = v;
                    changed.push(format!("WIDTH={}", el_core::fmt_mm(v)));
                }
                _ => a.errors.push(format!("WIDTH expects a positive number, got {value:?}")),
            },
            "SPACING" => match value.parse::<f64>() {
                Ok(v) if v > 0.0 => {
                    rec.rung_spacing_mm = v;
                    changed.push(format!("SPACING={}", el_core::fmt_mm(v)));
                }
                _ => a.errors
                    .push(format!("SPACING expects a positive number, got {value:?}")),
            },
            "RUNGS" | "ADD" => match value.parse::<i32>() {
                Ok(v) if v > 0 => {
                    rec.rung_count = if key == "ADD" { rec.rung_count + v } else { v };
                    changed.push(format!("{}={v}", if key == "ADD" { "ADD" } else { "RUNGS" }));
                }
                _ => a.errors
                    .push(format!("{key} expects a positive integer, got {value:?}")),
            },
            "PHASE" => match value.parse::<i32>() {
                Ok(v) if (1..=3).contains(&v) => {
                    rec.phase = v as u8;
                    changed.push(format!("PHASE={v}"));
                }
                _ => a.errors.push(format!("PHASE expects 1, 2 or 3, got {value:?}")),
            },
            other => a.errors.push(format!(
                "unknown ladder property {other} — try REF, INC, WIDTH, SPACING, RUNGS or PHASE"
            )),
        }
    }

    for e in &a.errors {
        host.push_error(&format!("EL_EDIT: {e}"));
    }
    if changed.is_empty() {
        return;
    }

    let record = ElRecord::encode_ladder(&rec);
    let parts = crate::geometry::ladder_entities(&record, &rec);

    host.push_undo(&format!("EL_EDIT ({})", changed.join(", ")));
    for h in idx.handles_for_el_id(el_id) {
        host.remove_entity(Handle::new(*h));
    }
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();

    let refs = rec.references();
    host.push_output(&format!(
        "EL_EDIT: ladder {} — {}; references {} … {}",
        rec.el_id,
        changed.join(", "),
        refs.first().cloned().unwrap_or_default(),
        refs.last().cloned().unwrap_or_default()
    ));
}

/// Edit a placed device's identity and catalogue data.
///
/// Everything here is record-only except the tag: a description or part number
/// does not move the symbol, but the reference designator is drawn text, so a
/// change to it rebuilds the glyph. Children inherit a renamed parent in the same
/// edit, because a child that kept the old tag would silently break every
/// cross-reference in the drawing.
fn edit_component(host: &mut dyn HostApi, idx: &ElIndex, handle: u64, a: &mut Args) {
    let Some(comp) = idx.component_of_handle(handle) else {
        host.push_error(&format!("EL_EDIT: no device at handle {handle:#x}"));
        return;
    };
    let mut rec = comp.rec.clone();
    let mut changed: Vec<String> = Vec::new();
    let mut tag_changed = false;
    let old_tag = rec.tag.clone();
    let was_child = rec.is_child;

    for (key, value) in a.kv.clone() {
        match key.as_str() {
            "TAG" => {
                if rec.is_child {
                    a.errors.push(
                        "TAG cannot be set on a child device — it shares its parent's tag"
                            .to_string(),
                    );
                    continue;
                }
                rec.tag = value.clone();
                tag_changed = true;
                changed.push(format!("TAG={value}"));
            }
            "FAMILY" => {
                rec.family = value.clone();
                changed.push(format!("FAMILY={value}"));
            }
            "SYMBOL" => {
                rec.symbol = value.clone();
                changed.push(format!("SYMBOL={value}"));
            }
            "DESC" => {
                rec.description = value.clone();
                changed.push(format!("DESC={value}"));
            }
            "MFG" => {
                rec.mfg = value.clone();
                changed.push(format!("MFG={value}"));
            }
            "CAT" => {
                rec.cat = value.clone();
                changed.push(format!("CAT={value}"));
            }
            "ASSY" => {
                rec.assy = value.clone();
                changed.push(format!("ASSY={value}"));
            }
            "INST" => {
                rec.installation = value.clone();
                changed.push(format!("INST={value}"));
            }
            "LOC" => {
                rec.location = value.clone();
                changed.push(format!("LOC={value}"));
            }
            "RATING" => {
                rec.ratings = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                changed.push(format!("RATING={value}"));
            }
            "PARENT" => {
                // Repointing a child belongs to EL_LINK, which is the one place
                // that can refuse a cycle.
                a.errors.push(
                    "PARENT is not set here — use EL_LINK <child> <parent>, which verifies the \
                     parent exists and cannot form a loop"
                        .to_string(),
                );
            }
            "FIXED" => {
                rec.tag_fixed = crate::cmds::common::parse_bool(&value);
                changed.push(format!("FIXED={}", rec.tag_fixed));
            }
            other => a.errors.push(format!(
                "unknown device property {other} — try TAG, FAMILY, SYMBOL, DESC, MFG, CAT, ASSY, \
                 INST, LOC, RATING or FIXED"
            )),
        }
    }

    for e in &a.errors {
        host.push_error(&format!("EL_EDIT: {e}"));
    }
    if changed.is_empty() {
        return;
    }

    let record = ElRecord::encode_component(&rec);
    host.push_undo(&format!("EL_EDIT ({} value(s))", changed.len()));

    let handles: Vec<u64> = idx.handles_for_el_id(&rec.el_id).to_vec();
    let mut ok = true;
    for h in &handles {
        if !crate::codec::write_el(host, Handle::new(*h), &record) {
            ok = false;
        }
    }

    let mut followed = 0usize;
    if ok && tag_changed && !was_child && !old_tag.is_empty() {
        for child in idx.components.values() {
            if !child.rec.is_child || child.rec.parent_tag != old_tag {
                continue;
            }
            let mut crec = child.rec.clone();
            crec.parent_tag = rec.tag.clone();
            crec.tag = rec.tag.clone();
            let crec_record = ElRecord::encode_component(&crec);
            let parts: Vec<u64> = idx.handles_for_el_id(&crec.el_id).to_vec();
            let mut child_ok = true;
            for h in &parts {
                if !crate::codec::write_el(host, Handle::new(*h), &crec_record) {
                    child_ok = false;
                }
            }
            if child_ok {
                followed += 1;
                redraw_component(host, idx, child.primary(), &crec);
            }
        }
    }

    if ok && tag_changed {
        redraw_component(host, idx, handle, &rec);
    }

    host.bump_geometry();
    host.set_dirty();
    if ok {
        host.push_output(&format!(
            "EL_EDIT: {} — {}{}",
            if rec.tag.is_empty() {
                format!("0x{handle:X}")
            } else {
                rec.tag.clone()
            },
            changed.join(", "),
            if followed > 0 {
                format!("; {followed} child device(s) followed")
            } else {
                String::new()
            }
        ));
    } else {
        host.push_error("EL_EDIT: the host refused the write (is the layer locked?)");
    }
}

/// Rebuild a device's drawn glyph so its designator matches its record.
///
/// The glyph is a render of the definition, so it is re-emitted rather than
/// patched: finding "the label" among a symbol's primitives would be guesswork,
/// and a stale designator is the most visible defect a schematic can have.
fn redraw_component(
    host: &mut dyn HostApi,
    idx: &ElIndex,
    handle: u64,
    rec: &el_core::model::ElComponentRec,
) {
    let Ok(lib) = crate::library::load(host) else {
        return;
    };
    let Some(def) = lib.get(&rec.symbol) else {
        return;
    };
    let Some(anchor) = host
        .document()
        .get_entity_owned(Handle::new(handle))
        .and_then(|e| crate::geometry::carrier_anchor(&e))
    else {
        return;
    };
    for h in idx.handles_for_el_id(&rec.el_id) {
        host.remove_entity(Handle::new(*h));
    }
    let record = ElRecord::encode_component(rec);
    let parts = crate::geometry::component_entities(&record, def, &rec.tag, anchor, 0.0);
    host.add_entities(parts);
}
