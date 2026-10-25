//! `EL_RETAG` — renumber every component tag in the drawing.
//!
//! Retagging is dangerous in a way renumbering wires is not: a child carries its
//! parent's tag, so changing a parent must change its children with it or the
//! cross-references break silently. Both are written in one pass from one plan,
//! which is the only way to keep them consistent.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::{ElComponentRec, RUNG_TOLERANCE_MM};
use el_core::tagging::{self, TagInput};
use el_core::ElIndex;

use crate::cmds::common::Args;
use crate::hostops::DocEntityExt;
use crate::geometry::{carrier_anchor, component_entities};
use crate::library;

/// Run `EL_RETAG [ALL | <family>] [START=n] [DRYRUN] | FIX <tag> | UNFIX <tag>`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let word = a.first_word().map(|w| w.to_ascii_uppercase());

    match word.as_deref() {
        Some("FIX") | Some("UNFIX") => {
            pin(host, &mut a, word.as_deref() == Some("FIX"));
            return;
        }
        _ => {}
    }

    let dry = a.flag("DRYRUN").unwrap_or(false);
    let family_filter = a
        .first_word()
        .filter(|w| !w.eq_ignore_ascii_case("ALL"))
        .map(str::to_string);

    if let Some(n) = a.integer("START") {
        if n <= 0 {
            host.push_error("EL_RETAG: START must be positive");
            return;
        }
    }

    retag(host, family_filter.as_deref(), a.integer("START"), dry);
}

/// Retag the drawing, or one family within it.
fn retag(host: &mut dyn HostApi, family: Option<&str>, start: Option<i32>, dry: bool) {
    let idx = crate::codec::build_index(host.document());
    let mut settings = idx.settings_or_default();
    if let Some(n) = start {
        settings.tag_start = n;
    }

    // A parent is retagged; a child follows its parent. Children are filtered
    // out here rather than skipped in the planner so the drawing order below
    // does not waste slots on them.
    let mut inputs: Vec<TagInput> = Vec::new();

    for (device_handle, comp) in ordered_by_position(host, &idx) {
        if comp.rec.is_child || comp.rec.tag.is_empty() && comp.rec.family.is_empty() {
            continue;
        }
        if let Some(f) = family {
            if !comp.rec.family.eq_ignore_ascii_case(f) {
                continue;
            }
        }
        // A device with neither a family nor a tag cannot be tagged: there is no
        // %F to expand. Skipping keeps the rest of the drawing retaggable.
        let family_code = if comp.rec.family.is_empty() {
            el_core::model::family_of(&comp.rec.tag)
        } else {
            comp.rec.family.clone()
        };
        if family_code.is_empty() {
            continue;
        }

        let rung_ref = anchor(host, device_handle)
            .and_then(|a| idx.rung_reference_at([a[0], a[1]], RUNG_TOLERANCE_MM * 3.0));
        inputs.push(TagInput {
            handle: device_handle,
            existing: comp.rec.tag.clone(),
            fixed: comp.rec.tag_fixed,
            family: family_code,
            is_child: false,
            rung_ref,
        });
        }

    if inputs.is_empty() {
        let msg = match family {
            Some(f) => format!("EL_RETAG: no device in family {f:?} to retag"),
            None => "EL_RETAG: this drawing has no tagged components".to_string(),
        };
        host.push_error(&msg);
        return;
    }

    let plan = tagging::plan_tags(&inputs, &settings);
    host.push_output(&format!("EL_RETAG — {}", plan.summary()));
    for (handle, tag) in plan.assignments.iter().take(10) {
        let from = idx
            .component_of_handle(*handle)
            .map(|c| {
                if c.rec.tag.is_empty() {
                    "(none)".to_string()
                } else {
                    c.rec.tag.clone()
                }
            })
            .unwrap_or_default();
        host.push_output(&format!("  {from} → {tag}"));
    }
    if plan.assignments.len() > 10 {
        host.push_output(&format!("  … and {} more", plan.assignments.len() - 10));
    }
    for reason in plan.skip_reasons() {
        host.push_info(&format!("  skipped: {reason}"));
    }

    if dry {
        host.push_info("EL_RETAG DRYRUN: nothing was changed");
        return;
    }
    if plan.assignments.is_empty() {
        host.push_output("EL_RETAG: every component already carries the right tag");
        return;
    }

    // Build the rename map *before* writing anything, so a child can be
    // resolved against the old tags while they are still the old tags.
    let renames: Vec<(String, String)> = plan
        .assignments
        .iter()
        .filter_map(|(handle, new)| {
            idx.component_of_handle(*handle)
                .map(|c| (c.rec.tag.clone(), new.clone()))
        })
        .collect();

    let lib = library::load(host).ok();
    host.push_undo(&format!("EL_RETAG ({} device(s))", plan.assignments.len()));

    let mut written = 0usize;
    let mut children = 0usize;

    // Parents first, so a child's new parent tag is already in place.
    for (handle, new_tag) in &plan.assignments {
        let Some(comp) = idx.component_of_handle(*handle) else {
            continue;
        };
        let mut rec = comp.rec.clone();
        rec.tag = new_tag.clone();
        if write_parts(host, &idx, &rec) {
            written += 1;
        }
    }

    // Then children: each keeps its own tag (it *is* the parent's tag) but must
    // be rewritten because the parent's tag changed.
    for comp in idx.components.values() {
        if !comp.rec.is_child || comp.rec.parent_tag.is_empty() {
            continue;
        }
        let Some((_, new_parent)) = renames
            .iter()
            .find(|(old, _)| *old == comp.rec.parent_tag)
        else {
            continue;
        };
        let child_handle = comp.primary();
        let mut rec = comp.rec.clone();
        rec.parent_tag = new_parent.clone();
        rec.tag = new_parent.clone();
        if write_parts(host, &idx, &rec) {
            children += 1;
            // The drawn label carries the old tag, so the glyph is rebuilt.
            if let Some(lib) = lib.as_ref() {
                redraw(host, &idx, child_handle, &rec, lib);
            }
        }
    }

    // Rebuild the parent glyphs too, since their tags are drawn text.
    if let Some(lib) = lib.as_ref() {
        for (handle, new_tag) in &plan.assignments {
            let Some(comp) = idx.component_of_handle(*handle) else {
                continue;
            };
            let mut rec = comp.rec.clone();
            rec.tag = new_tag.clone();
            redraw(host, &idx, *handle, &rec, lib);
        }
    }

    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_RETAG: {written} device(s) retagged, {children} child device(s) followed"
    ));
    if children > 0 || !plan.assignments.is_empty() {
        host.push_info("  run EL_XREF to refresh the cross-reference annotations");
    }
}

/// Rewrite a device's record on every part, returning whether it worked.
fn write_parts(host: &mut dyn HostApi, idx: &ElIndex, rec: &ElComponentRec) -> bool {
    let record = ElRecord::encode_component(rec);
    let handles: Vec<u64> = idx.handles_for_el_id(&rec.el_id).to_vec();
    if handles.is_empty() {
        return false;
    }
    let mut ok = true;
    for h in handles {
        if !crate::codec::write_el(host, Handle::new(h), &record) {
            ok = false;
        }
    }
    ok
}

/// Replace a device's drawn glyph so its label shows the new tag.
///
/// The glyph is a render of the record, so it is rebuilt rather than patched:
/// finding "the label part" among a symbol's primitives would be guesswork, and
/// a stale designator is the most visible defect a schematic can have.
fn redraw(
    host: &mut dyn HostApi,
    idx: &ElIndex,
    handle: u64,
    rec: &ElComponentRec,
    lib: &el_core::symbol::SymbolLibrary,
) {
    let Some(def) = lib.get(&rec.symbol) else {
        return;
    };
    let Some(anchor) = anchor(host, handle) else {
        return;
    };
    let rot = 0.0;

    // Remove the old parts, then add the rebuilt glyph.
    let old: Vec<u64> = idx.handles_for_el_id(&rec.el_id).to_vec();
    for h in &old {
        host.remove_entity(Handle::new(*h));
    }
    let record = ElRecord::encode_component(rec);
    let parts = component_entities(&record, def, &rec.tag, anchor, rot);
    host.add_entities(parts);
}

/// Components in drawing order: top to bottom, then left to right.
fn ordered_by_position<'a>(
    host: &dyn HostApi,
    idx: &'a ElIndex,
) -> Vec<(u64, &'a el_core::index::IndexedComponent)> {
    let mut v: Vec<(u64, &el_core::index::IndexedComponent, [f64; 3])> = idx
        .components
        .values()
        .map(|c| {
            let h = c.primary();
            (h, c, anchor(host, h).unwrap_or([0.0, 0.0, 0.0]))
        })
        .collect();
    v.sort_by(|a, b| {
        b.2[1]
            .partial_cmp(&a.2[1])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2[0].partial_cmp(&b.2[0]).unwrap_or(std::cmp::Ordering::Equal))
    });
    v.into_iter().map(|(h, c, _)| (h, c)).collect()
}

fn anchor(host: &dyn HostApi, handle: u64) -> Option<[f64; 3]> {
    let entity = host.document().get_entity_owned(Handle::new(handle))?;
    carrier_anchor(&entity)
}

/// Fix or unfix a device's tag.
///
/// A pinned tag keeps its number through a drawing-wide retag — the counterpart
/// of a fixed wire number, and the reason a retag is safe to run on a drawing
/// whose key devices someone has already numbered by hand.
fn pin(host: &mut dyn HostApi, a: &mut Args, fixed: bool) {
    let Some(token) = a.free.iter().skip(1).find(|t| !t.contains(',')).cloned() else {
        host.push_error(&format!(
            "EL_RETAG {}: name the device, e.g. EL_RETAG {} K1",
            if fixed { "FIX" } else { "UNFIX" },
            if fixed { "FIX" } else { "UNFIX" }
        ));
        return;
    };
    let idx = crate::codec::build_index(host.document());
    let Some(comp) = idx.parent_by_tag(&token) else {
        host.push_error(&format!("EL_RETAG: no parent device tagged {token:?}"));
        return;
    };
    if fixed && comp.rec.tag.is_empty() {
        host.push_error("EL_RETAG FIX: the device has no tag yet — run EL_RETAG first");
        return;
    }

    let mut rec = comp.rec.clone();
    rec.tag_fixed = fixed;

    host.push_undo(if fixed { "EL_RETAG FIX" } else { "EL_RETAG UNFIX" });
    if !write_parts(host, &idx, &rec) {
        host.push_error("EL_RETAG: the host refused the write (is the layer locked?)");
        return;
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_RETAG: {} is now {}{}",
        rec.tag,
        if fixed { "fixed" } else { "retaggable" },
        if fixed {
            " — a drawing-wide retag will leave it alone"
        } else {
            ""
        }
    ));
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_with_no_family_falls_back_to_its_tag_prefix() {
        // A pasted device may arrive with a tag but no family; the tag's own
        // prefix is the honest answer, and refusing to retag it would leave one
        // device stuck out of a renumber.
        assert_eq!(el_core::model::family_of("K12"), "K");
        assert_eq!(el_core::model::family_of(""), "");
    }

    #[test]
    fn a_child_is_excluded_from_the_retag_plan() {
        // Children share their parent's tag, so allocating them one would
        // invent a second identity for one device.
        let mut child = TagInput::new(1, "K");
        child.is_child = true;
        let plan = tagging::plan_tags(&[child], &el_core::model::ElSettingsRec::default());
        assert!(plan.assignments.is_empty());
        assert!(plan.skipped[0].1.contains("shares its parent"));
    }
}
