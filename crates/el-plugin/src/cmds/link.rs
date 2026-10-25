//! `EL_LINK` — attach a child device to its parent, and detach it again.
//!
//! Parent/child is stored as a single field: a child carries its parent's tag.
//! Everything else — cross-reference annotation, orphan detection, the
//! exception list — is derived from that one fact, so linking cannot leave a
//! second copy of the relationship to drift.
//!
//! `EL_LINK` exists because placement is not the only way a relationship
//! happens: a contact drawn before its coil, a device pasted from another
//! drawing, or a symbol converted from plain geometry all need attaching after
//! the fact.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::ElIndex;

use crate::cmds::common::Args;
use crate::hostops::Capture;
use crate::state;

/// Run `EL_LINK <child> <parent> | EL_LINK CLEAR <child> | EL_LINK [tag]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let words: Vec<String> = a
        .free
        .iter()
        .filter(|t| !t.contains(','))
        .cloned()
        .collect();

    let capture = Capture::take(host);

    if words.is_empty() {
        report(host, &capture.index);
        return;
    }

    if words[0].eq_ignore_ascii_case("CLEAR") {
        let Some(child) = words.get(1) else {
            host.push_error("EL_LINK CLEAR: name the child to detach, e.g. EL_LINK CLEAR K1");
            return;
        };
        detach(host, &capture, child);
        return;
    }

    let Some(parent_token) = words.get(1) else {
        // One argument is a query, not an instruction: it lists what a tag
        // currently references.
        show_one(host, &capture.index, &words[0]);
        return;
    };

    link(host, &capture, &words[0], parent_token, &mut a);
}

/// Attach `child_token` to `parent_token`.
fn link(host: &mut dyn HostApi, capture: &Capture, child_token: &str, parent_token: &str, a: &mut Args) {
    let Some(child_handle) = resolve_component(capture, child_token) else {
        host.push_error(&format!(
            "EL_LINK: no component matches {child_token:?} — expected a tag or a 0x-handle"
        ));
        return;
    };
    // The parent must be a *parent*: a child is tagged with its parent's tag, so
    // a bare tag lookup could attach a contact to another contact and produce a
    // chain that no report can resolve.
    let Some(parent) = capture.index.parent_by_tag(parent_token) else {
        host.push_error(&format!(
            "EL_LINK: no parent device tagged {parent_token:?} — a child cannot belong to another \
             child, and the parent must be placed in this drawing"
        ));
        return;
    };

    let Some(child) = capture.index.component_of_handle(child_handle) else {
        host.push_error("EL_LINK: the child vanished between resolution and edit");
        return;
    };
    if child_handle == parent.primary() {
        host.push_error("EL_LINK: a device cannot be its own parent");
        return;
    }

    // A device that would become its own ancestor must be refused, or the
    // cross-reference walk would never terminate.
    if let Some(cycle) = would_cycle(&capture.index, child_handle, parent.primary()) {
        host.push_error(&format!("EL_LINK: that would create a loop — {cycle}"));
        return;
    }

    let mut rec = child.rec.clone();
    rec.is_child = true;
    rec.parent_tag = parent.rec.tag.clone();
    let record = ElRecord::encode_component(&rec);

    host.push_undo("EL_LINK");
    if !write_all_parts(host, &capture.index, &rec.el_id, &record) {
        host.push_error("EL_LINK: the host refused the write (is the layer locked?)");
        return;
    }
    host.bump_geometry();
    host.set_dirty();

    let _ = a;
    host.push_output(&format!(
        "EL_LINK: {} is now a child of {} ({} child device(s) now)",
        display(child),
        parent.rec.tag,
        capture.index.children_of(&parent.rec.tag).len() + 1
    ));
    host.push_info("  run EL_XREF to refresh the cross-reference annotations");
}

/// Detach a child from its parent, making it standalone.
fn detach(host: &mut dyn HostApi, capture: &Capture, token: &str) {
    let Some(handle) = resolve_component(capture, token) else {
        host.push_error(&format!("EL_LINK CLEAR: no component matches {token:?}"));
        return;
    };
    let Some(child) = capture.index.component_of_handle(handle) else {
        return;
    };
    if !child.rec.is_child {
        host.push_output(&format!(
            "EL_LINK CLEAR: {} is not a child of anything",
            display(child)
        ));
        return;
    }

    let mut rec = child.rec.clone();
    let old_parent = rec.parent_tag.clone();
    rec.is_child = false;
    rec.parent_tag = String::new();
    let record = ElRecord::encode_component(&rec);

    host.push_undo("EL_LINK CLEAR");
    if !write_all_parts(host, &capture.index, &rec.el_id, &record) {
        host.push_error("EL_LINK CLEAR: the host refused the write");
        return;
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_LINK CLEAR: {} detached from {old_parent}",
        display(child)
    ));
}

/// Whether linking `child` under `parent` would form a loop, and the chain that
/// shows it.
///
/// Walks *up* from the proposed parent: if the child is reached, the new edge
/// would close a cycle.
fn would_cycle(idx: &ElIndex, child: u64, parent: u64) -> Option<String> {
    let mut chain: Vec<String> = Vec::new();
    let mut cursor = Some(parent);
    let mut guard = 0usize;
    while let Some(handle) = cursor {
        guard += 1;
        if guard > 64 {
            // A defensive bound: a pre-existing cycle in the data must not hang
            // the command.
            return Some("the existing parent chain is already circular".to_string());
        }
        if handle == child {
            chain.push("…".to_string());
            return Some(chain.join(" → "));
        }
        let Some(comp) = idx.component_of_handle(handle) else {
            break;
        };
        chain.push(display(comp));
        if !comp.rec.is_child || comp.rec.parent_tag.is_empty() {
            break;
        }
        cursor = idx
            .parent_by_tag(&comp.rec.parent_tag)
            .map(|p| p.primary());
    }
    None
}

/// Resolve a token to a component: a tag, or a `0x`-prefixed handle.
fn resolve_component(capture: &Capture, token: &str) -> Option<u64> {
    if let Some(c) = capture.index.component_by_tag(token) {
        return Some(c.primary());
    }
    if let Some(h) = crate::hostops::parse_handle(token) {
        if capture.index.component_of_handle(h.value()).is_some() {
            return Some(h.value());
        }
        // A label or glyph part resolves through the carrier index.
        if let Some(carrier) = capture.carrier_of(h) {
            if capture.index.component_of_handle(carrier).is_some() {
                return Some(carrier);
            }
        }
    }
    if let Some(pinned) = state::resolve_sel_token(token) {
        if let Some(handle) = pinned {
            if capture.index.component_of_handle(handle).is_some() {
                return Some(handle);
            }
            if let Some(carrier) = capture.carrier_of(Handle::new(handle)) {
                if capture.index.component_of_handle(carrier).is_some() {
                    return Some(carrier);
                }
            }
        }
    }
    None
}

/// Write a record onto every drawn part of an object.
///
/// Every part must carry the same record or the index would see a symbol whose
/// parts disagree about what they are.
fn write_all_parts(host: &mut dyn HostApi, idx: &ElIndex, el_id: &str, record: &ElRecord) -> bool {
    let handles: Vec<u64> = idx.handles_for_el_id(el_id).to_vec();
    if handles.is_empty() {
        return false;
    }
    let mut ok = true;
    for h in handles {
        if !crate::codec::write_el(host, Handle::new(h), record) {
            ok = false;
        }
    }
    ok
}

fn display(c: &el_core::index::IndexedComponent) -> String {
    if c.rec.tag.is_empty() {
        format!("0x{:X}", c.primary())
    } else {
        c.rec.tag.clone()
    }
}

/// List every parent/child relationship in the drawing.
fn report(host: &mut dyn HostApi, idx: &ElIndex) {
    let groups = el_core::tagging::build_xrefs(idx, &[]);
    let linked = groups.iter().filter(|g| !g.children.is_empty()).count();
    if groups.is_empty() {
        host.push_output("EL_LINK: this drawing has no tagged components yet");
        return;
    }
    host.push_output(&format!(
        "EL_LINK — {} device(s), {linked} with children",
        groups.len()
    ));

    for g in &groups {
        if g.children.is_empty() {
            continue;
        }
        let kids: Vec<String> = g.children.iter().map(|c| c.tag.clone()).collect();
        // All of a device's children share its tag, so list the count and the
        // states rather than repeating one tag many times.
        host.push_output(&format!(
            "  {} ← {} contact(s)",
            g.parent_tag,
            kids.len()
        ));
    }

    let orphans = el_core::tagging::unlinked_children(idx);
    if !orphans.is_empty() {
        host.push_error(&format!(
            "EL_LINK: {} child device(s) have no parent in this drawing: {}",
            orphans.len(),
            orphans
                .iter()
                .map(|(c, p)| format!("{c} (wants {p})"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
}

/// Report one tag's relationships.
fn show_one(host: &mut dyn HostApi, idx: &ElIndex, token: &str) {
    let Some(comp) = idx.component_by_tag(token) else {
        host.push_error(&format!("EL_LINK: no component tagged {token:?}"));
        return;
    };
    if comp.rec.is_child {
        match idx.parent_by_tag(&comp.rec.parent_tag) {
            Some(p) => host.push_output(&format!(
                "EL_LINK: {} is a child of {} ({} contact(s))",
                token,
                p.rec.tag,
                idx.children_of(&p.rec.tag).len()
            )),
            None => host.push_error(&format!(
                "EL_LINK: {} names {token} as its parent, but no such parent is placed",
                token
            )),
        }
        return;
    }

    let children = idx.children_of(token);
    if children.is_empty() {
        host.push_output(&format!("EL_LINK: {token} has no children"));
    } else {
        host.push_output(&format!(
            "EL_LINK: {token} has {} child device(s)",
            children.len()
        ));
    }
    if !comp.rec.cat.is_empty() {
        host.push_output(&format!(
            "  catalog {}/{}",
            comp.rec.mfg, comp.rec.cat
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use el_core::model::ElComponentRec;

    fn comp(handle: u64, tag: &str, is_child: bool, parent: &str) -> (u64, ElRecord) {
        let mut c = ElComponentRec::new(&format!("id{handle}"), "sym", "K");
        c.tag = tag.to_string();
        c.is_child = is_child;
        c.parent_tag = parent.to_string();
        (handle, ElRecord::encode_component(&c))
    }

    #[test]
    fn linking_a_grandchild_is_detected_as_a_loop() {
        // K2 is a child of K1. Making K1 a child of K2 would close a cycle the
        // cross-reference walk could never escape.
        let idx = ElIndex::build(vec![
            comp(1, "K1", false, ""),
            comp(2, "K1", true, "K1"),
            comp(3, "K2", false, ""),
        ]);
        // Attach nothing yet: a plain chain is fine.
        assert_eq!(would_cycle(&idx, 3, 1), None);

        // Now attach K1 under K2, then try to attach K2 under K1.
        let mut rec = idx.component_of_handle(3).unwrap().rec.clone();
        rec.is_child = true;
        rec.parent_tag = "K1".to_string();
        let idx2 = ElIndex::build(vec![
            comp(1, "K1", false, ""),
            comp(2, "K1", true, "K1"),
            (3, ElRecord::encode_component(&rec)),
        ]);
        assert!(
            would_cycle(&idx2, 1, 3).is_some(),
            "K1 under K2 would close the loop K2 → K1"
        );
    }

    #[test]
    fn linking_a_device_to_itself_is_a_loop() {
        let idx = ElIndex::build(vec![comp(1, "K1", false, "")]);
        assert!(would_cycle(&idx, 1, 1).is_some());
    }

    #[test]
    fn a_parent_chain_that_never_resolves_does_not_hang() {
        // A child whose parent is absent terminates the walk rather than
        // spinning.
        let idx = ElIndex::build(vec![comp(1, "K1", true, "MISSING")]);
        assert_eq!(would_cycle(&idx, 9, 1), None);
    }

    #[test]
    fn display_prefers_the_tag_over_the_handle() {
        let idx = ElIndex::build(vec![comp(1, "K1", false, "")]);
        assert_eq!(display(idx.component_of_handle(1).unwrap()), "K1");
        // An untagged device is named by one of its drawn parts.
        let idx = ElIndex::build(vec![comp(7, "", false, "")]);
        assert_eq!(display(idx.component_of_handle(7).unwrap()), "0x7");
    }
}
