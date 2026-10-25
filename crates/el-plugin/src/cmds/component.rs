//! `EL_COMPONENT` — place a schematic component from the library.
//!
//! A placement does three things that must agree: it draws the symbol's glyph,
//! it allocates a reference designator, and it records the device's identity and
//! connection points. All three are derived from one library definition, so a
//! symbol cannot be drawn without its data or tagged without its family.
//!
//! A **child** (a contact) is placed by naming its parent: `EL_COMPONENT
//! contact-no K1` puts a contact on the drawing carrying `K1`'s tag. That is the
//! whole parent/child mechanism — the child stores the parent's tag, and every
//! cross-reference is derived from that later.

use ocs_plugin_api::host::{CommandStep, HostApi, InteractiveCommand};

use el_core::codec::ElRecord;
use el_core::model::{ElComponentRec, RUNG_TOLERANCE_MM};
use el_core::symbol::{SymbolDef, SymbolLibrary};
use el_core::tagging;
use el_core::ElIndex;

use crate::cmds::common::Args;
use crate::geometry::component_entities;
use crate::library;
use crate::state::{self, PendingLine};

/// Run `EL_COMPONENT [symbol|family] [tag] [x,y] [ROT=deg] [KEY=VALUE …]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);

    let lib = match library::load(host) {
        Ok(l) => l,
        Err(e) => {
            host.push_error(&format!("EL_COMPONENT: {e}"));
            return;
        }
    };

    // The first bare token that is not a coordinate names the symbol or its
    // family; the second names the tag.
    let words: Vec<String> = a
        .free
        .iter()
        .filter(|t| !t.contains(','))
        .cloned()
        .collect();
    let symbol_token = words.first().cloned();
    let tag_token = words.get(1).cloned();

    let Some(def) = resolve_symbol(&lib, symbol_token.as_deref()) else {
        let token = symbol_token.clone().unwrap_or_default();
        // `EL_COMPONENT K1 10,20` reads like "place K1" but names a symbol, so
        // the failure is worth a hint rather than a list of families.
        let looks_like_a_tag = el_core::model::family_of(&token).len() < token.len()
            && lib.first_parent(&el_core::model::family_of(&token)).is_some();
        if looks_like_a_tag {
            host.push_error(&format!(
                "EL_COMPONENT: {token:?} looks like a reference designator, not a symbol — write \
                 `EL_COMPONENT {} {token}` to place a {} tagged {token}",
                el_core::model::family_of(&token),
                el_core::model::family_of(&token)
            ));
            return;
        }
        let names: Vec<String> = lib
            .families()
            .into_iter()
            .map(|(f, d)| format!("{f} ({d})"))
            .collect();
        host.push_error(&format!(
            "EL_COMPONENT: {token:?} is not a symbol or family in {} — families: {}",
            lib.name,
            names.join(", ")
        ));
        return;
    };

    // A child needs a parent to belong to; without one it would carry an empty
    // tag and be invisible to every report.
    if def.is_child {
        let Some(parent) = tag_token.clone().filter(|t| !t.is_empty()) else {
            host.push_error(&format!(
                "EL_COMPONENT: {} is a child symbol, so it needs its parent's tag — \
                 e.g. EL_COMPONENT {} K1",
                def.name, def.name
            ));
            return;
        };
        let idx = crate::codec::build_index(host.document());
        if idx.parent_by_tag(&parent).is_none() {
            host.push_error(&format!(
                "EL_COMPONENT: no parent device tagged {parent:?} in this drawing — \
                 place the parent first, or use EL_LINK to attach an existing contact"
            ));
            return;
        }
    }

    let at = a.first_coord().map(|c| [c[0], c[1], 0.0]);
    match at {
        Some(at) => place(host, &lib, def, at, tag_token.as_deref(), &mut a),
        None => start_interactive(host, def.name.clone(), tag_token),
    }
}

/// Find the definition a token names: a symbol name, or the first parent of a
/// family code.
fn resolve_symbol<'a>(lib: &'a SymbolLibrary, token: Option<&str>) -> Option<&'a SymbolDef> {
    let token = token?;
    if let Some(d) = lib.get(token) {
        return Some(d);
    }
    // A bare family code is the common shortcut: `EL_COMPONENT K`.
    lib.first_parent(token)
}

/// The record for a new component, with its identity and connection points.
fn build_record(
    def: &SymbolDef,
    tag: Option<&str>,
    is_child: bool,
    parent_tag: &str,
    a: &mut Args,
) -> ElComponentRec {
    let mut rec = ElComponentRec::new(&el_core::new_el_id(), &def.name, &def.family);
    rec.is_child = is_child;
    rec.parent_tag = parent_tag.to_string();
    rec.description = def.description.clone();
    rec.ratings = def.default_ratings.clone();
    rec.pins = def.pins.iter().map(|p| p.name.clone()).collect();
    if is_child {
        // A child's designator *is* its parent's tag, so it is drawn too — a
        // contact with no visible tag cannot be cross-referenced by eye.
        rec.tag = parent_tag.to_string();
    } else if let Some(t) = tag.filter(|t| !t.is_empty()) {
        rec.tag = t.to_string();
    }

    // Any of the catalogue fields may be supplied at placement.
    if let Some(v) = a.get("DESC") {
        rec.description = v.to_string();
    }
    if let Some(v) = a.get("MFG") {
        rec.mfg = v.to_string();
    }
    if let Some(v) = a.get("CAT") {
        rec.cat = v.to_string();
    }
    if let Some(v) = a.get("ASSY") {
        rec.assy = v.to_string();
    }
    if let Some(v) = a.get("INST") {
        rec.installation = v.to_string();
    }
    if let Some(v) = a.get("LOC") {
        rec.location = v.to_string();
    }
    if let Some(v) = a.get("RATING") {
        rec.ratings = v
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    rec
}

/// Place a component synchronously, allocating a tag when none was given.
fn place(
    host: &mut dyn HostApi,
    lib: &SymbolLibrary,
    def: &SymbolDef,
    at: [f64; 3],
    tag_token: Option<&str>,
    a: &mut Args,
) {
    let idx = crate::codec::build_index(host.document());
    let settings = idx.settings_or_default();
    let rot = a.number("ROT").unwrap_or(0.0).to_radians();

    let parent_tag = if def.is_child {
        tag_token.unwrap_or_default().to_string()
    } else {
        String::new()
    };
    let explicit = if def.is_child { None } else { tag_token };
    let mut rec = build_record(def, explicit, def.is_child, &parent_tag, a);
    // A symbol's anchor is not recoverable from its primitives, so the placement
    // is part of the record — that is what lets a wire name the pin it reaches.
    rec.origin_x = at[0];
    rec.origin_y = at[1];
    rec.rotation_deg = rot.to_degrees();

    // Allocate a tag unless the caller named one. A child is never allocated
    // one: it shares its parent's.
    let rung_ref = idx.rung_reference_at([at[0], at[1]], RUNG_TOLERANCE_MM * 2.0);
    let mut assigned = String::new();
    if rec.tag.is_empty() && !def.is_child {
        match tagging::next_tag(&idx, &rec.family, &settings, rung_ref.as_deref()) {
            Some(tag) => {
                rec.tag = tag.clone();
                assigned = tag;
            }
            None => {
                host.push_error(&format!(
                    "EL_COMPONENT: cannot allocate a {} tag — check the tag format and \
                     TAG_START in EL_SETTINGS",
                    rec.family
                ));
                return;
            }
        }
    }

    let record = ElRecord::encode_component(&rec);
    let parts = component_entities(&record, def, &rec.tag, at, rot);
    let count = parts.len();

    // Components and their wires are independent objects, so this is one entity
    // batch in one undo window.
    host.push_undo("EL_COMPONENT");
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();

    host.push_output(&format!(
        "EL_COMPONENT: {} {} at ({}, {}) — {} entit(ies) on {}",
        def.name,
        if rec.tag.is_empty() {
            "(untagged)".to_string()
        } else {
            rec.tag.clone()
        },
        el_core::fmt_mm(at[0]),
        el_core::fmt_mm(at[1]),
        count,
        el_core::model::LAYER_COMPONENTS
    ));
    if !rec.tag.is_empty() {
        host.push_output(&format!(
            "  tag {} {}{}",
            rec.tag,
            if def.is_child {
                format!("(contact of {})", rec.parent_tag)
            } else {
                String::new()
            },
            if assigned.is_empty() {
                String::new()
            } else {
                " (auto-numbered)".to_string()
            }
        ));
    }
    if !rec.pins.is_empty() {
        host.push_output(&format!("  pins: {}", rec.pins.join(", ")));
    }
    let _ = lib;
}

/// Interactive placement: pick the anchor point.
fn start_interactive(host: &mut dyn HostApi, symbol: String, tag: Option<String>) {
    let lib = match library::load(host) {
        Ok(l) => l,
        Err(e) => {
            host.push_error(&format!("EL_COMPONENT: {e}"));
            return;
        }
    };
    let Some(def) = lib.get(&symbol).cloned() else {
        host.push_error(&format!("EL_COMPONENT: {symbol} vanished from the library"));
        return;
    };

    // Capture everything the callback needs: it runs against `NullHost`.
    let capture = crate::hostops::Capture::take(host);
    host.push_info(&format!(
        "EL_COMPONENT {}: pick the insertion point ({})",
        def.name, def.description
    ));
    host.start_interactive(Box::new(ComponentCmd {
        def,
        settings: capture.index.settings_or_default(),
        idx: capture.index,
        tag,
        placed: false,
    }));
}

struct ComponentCmd {
    def: SymbolDef,
    settings: el_core::ElSettingsRec,
    idx: ElIndex,
    tag: Option<String>,
    placed: bool,
}

impl InteractiveCommand for ComponentCmd {
    fn prompt(&self) -> String {
        format!("EL_COMPONENT {}: pick the insertion point", self.def.name)
    }

    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep {
        if self.placed {
            return CommandStep::Done;
        }
        self.placed = true;

        let mut rec = ElComponentRec::new(&el_core::new_el_id(), &self.def.name, &self.def.family);
        rec.origin_x = pt[0];
        rec.origin_y = pt[1];
        rec.is_child = self.def.is_child;
        rec.description = self.def.description.clone();
        rec.ratings = self.def.default_ratings.clone();
        rec.pins = self.def.pins.iter().map(|p| p.name.clone()).collect();

        if self.def.is_child {
            let Some(parent) = self.tag.clone() else {
                state::emit(PendingLine::Error(
                    "EL_COMPONENT: a child symbol needs its parent's tag".to_string(),
                ));
                return CommandStep::Done;
            };
            rec.parent_tag = parent.clone();
            rec.tag = parent;
        } else if let Some(t) = self.tag.clone().filter(|t| !t.is_empty()) {
            rec.tag = t;
        }

        // A picked point snaps onto a rung, so the device lands where reference
        // tagging will find it.
        let at = snap_to_rung(&self.idx, pt);
        if rec.tag.is_empty() && !self.def.is_child {
            let rung_ref = self
                .idx
                .rung_reference_at([at[0], at[1]], RUNG_TOLERANCE_MM * 2.0);
            match tagging::next_tag(&self.idx, &rec.family, &self.settings, rung_ref.as_deref()) {
                Some(tag) => rec.tag = tag,
                None => {
                    state::emit(PendingLine::Error(
                        "EL_COMPONENT: cannot allocate a tag for this placement".to_string(),
                    ));
                    return CommandStep::Done;
                }
            }
        }

        let record = ElRecord::encode_component(&rec);
        let parts = component_entities(&record, &self.def, &rec.tag, at, 0.0);
        let count = parts.len();

        // Several entities, so the deferred path: one undo window for the whole
        // symbol (see `state::spawn_flush`).
        state::push_entities(parts);
        state::spawn_flush(Some("EL_COMPONENT".to_string()));
        state::emit(PendingLine::Output(format!(
            "EL_COMPONENT: {} {} at ({}, {}) — {count} entit(ies)",
            self.def.name,
            rec.tag,
            el_core::fmt_mm(at[0]),
            el_core::fmt_mm(at[1])
        )));
        CommandStep::Done
    }

    fn on_enter(&mut self) -> CommandStep {
        CommandStep::Cancel
    }
}

/// Pull a picked point onto a rung, so a device placed by eye still lands on the
/// rung that will name it.
fn snap_to_rung(idx: &ElIndex, pt: [f64; 3]) -> [f64; 3] {
    let Some(ladder) = idx.ladder_at([pt[0], pt[1]], RUNG_TOLERANCE_MM * 3.0) else {
        return pt;
    };
    let Some(index) = ladder.rec.nearest_rung(pt[1], RUNG_TOLERANCE_MM * 3.0) else {
        return pt;
    };
    [pt[0], ladder.rec.rung_y(index), pt[2]]
}




#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symbol_name_resolves_directly() {
        let lib = SymbolLibrary::default();
        assert_eq!(resolve_symbol(&lib, Some("relay-coil")).unwrap().name, "relay-coil");
    }

    #[test]
    fn a_bare_family_code_resolves_to_its_first_parent() {
        let lib = SymbolLibrary::default();
        assert_eq!(resolve_symbol(&lib, Some("K")).unwrap().name, "relay-coil");
        assert_eq!(resolve_symbol(&lib, Some("F")).unwrap().name, "fuse");
        assert!(resolve_symbol(&lib, Some("ZZ")).is_none());
        assert!(resolve_symbol(&lib, None).is_none());
    }

    #[test]
    fn a_family_code_never_resolves_to_a_contact() {
        // `EL_COMPONENT K` must place a coil, not a contact: a contact needs a
        // parent and would be placed untagged.
        let lib = SymbolLibrary::default();
        for family in ["K", "S", "Q"] {
            let d = resolve_symbol(&lib, Some(family)).unwrap();
            assert!(!d.is_child, "{family} resolved to the child {}", d.name);
        }
    }

    #[test]
    fn a_record_carries_the_definitions_pins_and_ratings() {
        let lib = SymbolLibrary::default();
        let def = lib.get("relay-coil").unwrap();
        let mut a = Args::parse(&[]);
        let rec = build_record(def, None, false, "", &mut a);
        assert_eq!(rec.family, "K");
        assert_eq!(rec.symbol, "relay-coil");
        assert_eq!(rec.pins, vec!["A1", "A2"]);
        assert_eq!(rec.ratings, vec!["24VDC"]);
        assert_eq!(rec.description, "Relay coil");
    }

    #[test]
    fn placement_keys_override_the_definition() {
        let lib = SymbolLibrary::default();
        let def = lib.get("relay-coil").unwrap();
        let mut a = Args::parse(&["DESC=K1+relay", "MFG=Siemens", "CAT=3RT", "RATING=230VAC,10A"]);
        let rec = build_record(def, Some("K1"), false, "", &mut a);
        assert_eq!(rec.tag, "K1");
        assert_eq!(rec.description, "K1 relay", "a + stands in for a space");
        assert_eq!(rec.mfg, "Siemens");
        assert_eq!(rec.cat, "3RT");
        assert_eq!(rec.ratings, vec!["230VAC", "10A"]);
    }

    #[test]
    fn a_child_record_carries_its_parents_tag() {
        let lib = SymbolLibrary::default();
        let def = lib.get("contact-no").unwrap();
        let mut a = Args::parse(&[]);
        let rec = build_record(def, None, true, "K1", &mut a);
        assert!(rec.is_child);
        assert_eq!(rec.parent_tag, "K1");
        // The child shows its parent's tag, so the designator is not blank.
        assert_eq!(rec.tag, "K1");
    }

    #[test]
    fn an_empty_tag_token_leaves_the_record_untagged() {
        // The caller may pass `""` to mean "allocate one"; it must not become a
        // tag of zero length that then shadows nothing.
        let lib = SymbolLibrary::default();
        let def = lib.get("relay-coil").unwrap();
        let mut a = Args::parse(&[]);
        let rec = build_record(def, Some(""), false, "", &mut a);
        assert!(rec.tag.is_empty());
    }

    #[test]
    fn snapping_only_moves_a_point_near_a_rung() {
        let mut l = el_core::model::LadderRec::new("l1", [0.0, 100.0], 100.0, 10.0, 3);
        l.first_ref = "100".to_string();
        let idx = ElIndex::build(vec![(1, ElRecord::encode_ladder(&l))]);
        // 100.5 is within tolerance of the first rung.
        assert_eq!(snap_to_rung(&idx, [50.0, 100.5, 0.0]), [50.0, 100.0, 0.0]);
        // 130 is not near any rung, so the point is left alone.
        assert_eq!(snap_to_rung(&idx, [50.0, 130.0, 0.0]), [50.0, 130.0, 0.0]);
    }
}
