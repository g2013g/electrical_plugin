//! `EL_XREF` — write cross-reference annotations beside devices.
//!
//! A coil has to show where its contacts are, and a contact has to show which
//! coil it belongs to. Both are the same fact read in two directions, so both
//! are derived from the records on every run.
//!
//! The `EL_XREF` layer is a **pure render**, exactly like the wire-number layer:
//! every run clears it and redraws from the data. That is what makes a
//! cross-reference impossible to leave stale, which matters more here than
//! anywhere — a coil listing a contact that has moved is worse than no
//! annotation at all, because it is confidently wrong.

use ocs_plugin_api::host::HostApi;

use el_core::model::RUNG_TOLERANCE_MM;
use el_core::tagging::{self, XrefGroup};
use el_core::ElIndex;

use crate::cmds::common::{remove_entities_on_layer, Args};
use crate::hostops::DocEntityExt;
use crate::geometry::{carrier_anchor, xref_label};

/// Run `EL_XREF [ALL | <tag>] | CLEAR`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let a = Args::parse(args);
    let word = a.first_word().map(str::to_ascii_uppercase);

    if word.as_deref() == Some("CLEAR") {
        host.push_undo("EL_XREF CLEAR");
        let n = remove_entities_on_layer(host, el_core::model::LAYER_XREF);
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!("EL_XREF CLEAR: removed {n} annotation(s)"));
        return;
    }

    let groups = refresh(host, a.first_word());
    if groups.is_empty() {
        return;
    }
}

/// Rebuild the annotations and report what was written.
///
/// Returns the groups that were processed, so callers can report on them.
pub fn refresh(host: &mut dyn HostApi, only: Option<&str>) -> Vec<XrefGroup> {
    let idx = crate::codec::build_index(host.document());

    if idx.components.is_empty() {
        host.push_error("EL_XREF: this drawing has no components");
        return Vec::new();
    }

    // A device's line reference needs its drawn position, which only the drawing
    // can supply.
    let positions = component_positions(host, &idx);
    let mut groups = tagging::build_xrefs(&idx, &positions);

    if let Some(filter) = only {
        if !filter.eq_ignore_ascii_case("ALL") {
            groups.retain(|g| g.parent_tag.eq_ignore_ascii_case(filter));
            if groups.is_empty() {
                host.push_error(&format!("EL_XREF: no device tagged {filter:?}"));
                return Vec::new();
            }
        }
    }

    // Where each group's annotation goes: beside the parent, and beside each
    // child.
    let mut labels = Vec::new();
    let mut written = 0usize;
    let mut linked = 0usize;

    for g in &groups {
        // The parent shows its children's locations.
        if let Some(parent) = idx.parent_by_tag(&g.parent_tag) {
            let anchor = anchor_of(host, parent.primary()).unwrap_or([0.0, 0.0, 0.0]);
            let text = g.parent_text();
            if !text.is_empty() {
                labels.push(xref_label(
                    &text,
                    [anchor[0], anchor[1] - 8.0, anchor[2]],
                ));
                written += 1;
            }
        } else if !g.children.is_empty() {
            // No parent placed: make that visible rather than silent.
            let anchor = anchor_of(host, idx.components.values()
                .find(|c| c.rec.is_child && c.rec.parent_tag == g.parent_tag)
                .map(|c| c.primary())
                .unwrap_or(0))
                .unwrap_or([0.0, 0.0, 0.0]);
            labels.push(xref_label("?", [anchor[0], anchor[1] - 8.0, anchor[2]]));
            written += 1;
        }

        // Each child shows where its parent is.
        let child_text = g.child_text();
        if !child_text.is_empty() {
            for c in &g.children {
                let Some(child) = idx.components.values().find(|x| {
                    x.rec.is_child && x.rec.tag == c.tag && x.rec.parent_tag == g.parent_tag
                }) else {
                    continue;
                };
                let anchor = anchor_of(host, child.primary()).unwrap_or([0.0, 0.0, 0.0]);
                labels.push(xref_label(
                    &child_text,
                    [anchor[0], anchor[1] - 6.0, anchor[2]],
                ));
                written += 1;
            }
            linked += 1;
        }
    }

    host.push_undo("EL_XREF");
    let cleared = remove_entities_on_layer(host, el_core::model::LAYER_XREF);
    host.add_entities(labels);
    host.bump_geometry();
    host.set_dirty();

    let orphans = tagging::unlinked_children(&idx);
    host.push_output(&format!(
        "EL_XREF: {} group(s), {written} annotation(s) written \
         ({cleared} stale annotation(s) replaced)",
        groups.len()
    ));
    for g in &groups {
        if !g.children.is_empty() {
            host.push_output(&format!(
                "  {} at {} ← {}",
                g.parent_tag,
                g.parent_ref.clone().unwrap_or_else(|| "—".to_string()),
                g.parent_text()
            ));
        }
    }
    if !orphans.is_empty() {
        host.push_error(&format!(
            "EL_XREF: {} child device(s) reference a parent that is not in this drawing: {}",
            orphans.len(),
            orphans
                .iter()
                .map(|(c, p)| format!("{c} → {p}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let _ = linked;
    groups
}

/// Each component's handle paired with the line reference it sits on.
fn component_positions(
    host: &dyn HostApi,
    idx: &ElIndex,
) -> Vec<(u64, Option<String>)> {
    let mut out = Vec::new();
    for comp in idx.components.values() {
        // The glyph's anchor is the first vertex of its first drawn part, so any
        // part of the symbol answers the question.
        let handle = comp.primary();
        let anchor = anchor_of(host, handle);
        let rung = anchor
            .and_then(|a| idx.rung_reference_at([a[0], a[1]], RUNG_TOLERANCE_MM * 3.0));
        out.push((handle, rung));
    }
    out
}

/// The drawn anchor of any part of an object.
fn anchor_of(host: &dyn HostApi, handle: u64) -> Option<[f64; 3]> {
    let entity = host
        .document()
        .get_entity_owned(ocs_plugin_api::host::codec::Handle::new(handle))?;
    carrier_anchor(&entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use el_core::codec::ElRecord;
    use el_core::model::ElComponentRec;

    fn comp(handle: u64, tag: &str, is_child: bool, parent: &str) -> (u64, ElRecord) {
        let mut c = ElComponentRec::new(&format!("id{handle}"), "sym", "K");
        c.tag = tag.to_string();
        c.is_child = is_child;
        c.parent_tag = parent.to_string();
        (handle, ElRecord::encode_component(&c))
    }

    #[test]
    fn a_group_carries_both_directions_of_the_relationship() {
        // The round trip is the property that matters: what a parent lists must
        // be what its children say about it.
        let idx = ElIndex::build(vec![
            comp(1, "K1", false, ""),
            comp(2, "K1", true, "K1"),
            comp(3, "K1", true, "K1"),
        ]);
        let groups = tagging::build_xrefs(
            &idx,
            &[
                (1, Some("104".into())),
                (2, Some("110".into())),
                (3, Some("112".into())),
            ],
        );
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.parent_text(), "110,112");
        assert_eq!(g.child_text(), "104");
        for child in &g.children {
            assert_eq!(child.parent_tag, "K1");
        }
    }

    #[test]
    fn an_orphan_group_is_visible_with_a_placeholder_parent_reference() {
        let idx = ElIndex::build(vec![comp(2, "K9", true, "K9")]);
        let groups = tagging::build_xrefs(&idx, &[(2, Some("110".into()))]);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].parent_ref.is_none());
        // The parent side prints "?" for a missing parent, the child side "—".
        assert_eq!(groups[0].child_text(), "—");
        assert_eq!(groups[0].parent_text(), "110");
    }
}
