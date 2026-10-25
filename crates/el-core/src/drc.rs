//! Design-rule checking: what is wrong with this drawing.
//!
//! A schematic that exports a confident but wrong bill of materials is worse than
//! one that refuses to export, so every report is gated on this: errors block,
//! warnings do not. That split is the whole contract — a `Warning` is worth
//! telling the user about, an `Error` means a number in the output would be
//! untrue.
//!
//! The rules here are the drawing-scope half of ACADE's Electrical Audit. They
//! are pure functions of the resolved index, so they are testable without a host
//! and cannot depend on where an entity happens to sit.

use std::collections::{BTreeMap, BTreeSet};

use crate::catalog::{Assignment, Catalog};
use crate::index::ElIndex;
use crate::model::*;

/// How bad an issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth knowing; the drawing is still reportable.
    Warning,
    /// A number in a report would be wrong, so export is refused.
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "ERROR",
            Severity::Warning => "WARN",
        }
    }
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    /// A stable code, so a test and a user can both refer to one rule.
    pub code: String,
    pub message: String,
}

impl Issue {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Issue {
            severity: Severity::Error,
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn warning(code: &str, message: impl Into<String>) -> Self {
        Issue {
            severity: Severity::Warning,
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// Run every rule.
///
/// Ordered most-structural first, so a report reads top-down: a record that
/// cannot be decoded explains several later findings, and should come before
/// them.
pub fn check(idx: &ElIndex, catalog: Option<&Catalog>) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();

    // ── structural: records that did not decode ──────────────────────────
    for (handle, err) in &idx.decode_errors {
        issues.push(Issue::error(
            "BAD_RECORD",
            format!("entity 0x{handle:X} carries an unreadable record: {err}"),
        ));
    }
    for (kind, count) in &idx.unknown_kinds {
        issues.push(Issue::warning(
            "UNKNOWN_KIND",
            format!("{count} record(s) of kind {kind:?} were written by a newer build"),
        ));
    }

    // ── identity ─────────────────────────────────────────────────────────
    for tag in idx.duplicate_tags() {
        issues.push(Issue::error(
            "DUP_TAG",
            format!("two devices are tagged {tag:?}"),
        ));
    }
    for wire in idx.duplicate_wire_refs() {
        issues.push(Issue::error(
            "DUP_WREF",
            format!("two wires are numbered {wire:?}"),
        ));
    }
    if idx.settings.is_none() {
        issues.push(Issue::warning(
            "NO_SETTINGS",
            "this drawing has no EL_SETTINGS record, so it runs on built-in formats",
        ));
    }

    // ── components ───────────────────────────────────────────────────────
    for comp in idx.components.values() {
        let name = if comp.rec.tag.is_empty() {
            format!("0x{:X}", comp.primary())
        } else {
            comp.rec.tag.clone()
        };

        if comp.rec.tag.is_empty() {
            issues.push(Issue::error(
                "UNTAGGED",
                format!("a {} device at 0x{:X} has no tag", comp.rec.symbol, comp.primary()),
            ));
        }
        if comp.rec.family.is_empty() {
            issues.push(Issue::warning(
                "NO_FAMILY",
                format!("{name} has no family code, so it cannot be retagged"),
            ));
        }
        if comp.rec.cat.is_empty() {
            issues.push(Issue::warning(
                "NO_CATALOG",
                format!("{name} has no catalogue part number, so it cannot be ordered"),
            ));
        } else if comp.rec.mfg.is_empty() {
            issues.push(Issue::error(
                "NO_MFG",
                format!("{name} has part {} but no manufacturer", comp.rec.cat),
            ));
        }
        if comp.rec.pins.is_empty() {
            issues.push(Issue::warning(
                "NO_PINS",
                format!("{name} has no connection points recorded"),
            ));
        }
        if comp.rec.is_child && comp.rec.parent_tag.is_empty() {
            issues.push(Issue::error(
                "CHILD_NO_PARENT",
                format!("{name} is a child device with no parent tag"),
            ));
        }
        if !comp.rec.is_child && !comp.rec.parent_tag.is_empty() {
            issues.push(Issue::warning(
                "PARENT_HAS_PARENT",
                format!("{name} is not a child but names {} as its parent", comp.rec.parent_tag),
            ));
        }
    }

    // ── parent/child links ───────────────────────────────────────────────
    for orphan in idx.orphan_children() {
        issues.push(Issue::error(
            "ORPHAN_CHILD",
            format!(
                "{} names {} as its parent, but no such device is placed",
                orphan.rec.tag, orphan.rec.parent_tag
            ),
        ));
    }
    // A parent with no children is normal for a lamp or a supply; a relay coil
    // with none is a drawing that has lost its contacts.
    for comp in idx.components.values() {
        if comp.rec.is_child || comp.rec.tag.is_empty() {
            continue;
        }
        if idx.children_of(&comp.rec.tag).is_empty() && expects_contacts(&comp.rec.family) {
            issues.push(Issue::warning(
                "NO_CONTACTS",
                format!(
                    "{} is a {} with no contact devices — if its contacts are drawn, link them",
                    comp.rec.tag, comp.rec.symbol
                ),
            ));
        }
    }

    // ── wires ────────────────────────────────────────────────────────────
    for wire in idx.wires.values() {
        let name = if wire.rec.wire_ref.is_empty() {
            format!("0x{:X}", wire.handle)
        } else {
            wire.rec.wire_ref.clone()
        };
        if !wire.rec.is_connected() {
            issues.push(Issue::error(
                "WIRE_DANGLING",
                format!("wire {name} has an unconnected end"),
            ));
        }
        if wire.rec.wire_ref.is_empty() {
            issues.push(Issue::warning(
                "WIRE_UNNUMBERED",
                format!("wire 0x{:X} has no number — run EL_WIRENO", wire.handle),
            ));
        }
        if wire.rec.from_id == wire.rec.to_id && !wire.rec.from_id.is_empty() {
            issues.push(Issue::warning(
                "WIRE_SELF",
                format!("wire {name} connects {} to itself", wire.rec.from_id),
            ));
        }
        if wire.rec.gauge.is_empty() {
            issues.push(Issue::warning(
                "NO_GAUGE",
                format!("wire {name} has no gauge"),
            ));
        }
    }

    // ── signals ──────────────────────────────────────────────────────────
    // Drawing-local rules only: whether a signal's other half exists on
    // another sheet is the project register's knowledge, checked by
    // `EL_SIGNAL LIST`, not here.
    let mut seen_sources: BTreeMap<&str, &str> = BTreeMap::new();
    for s in idx.signals.values() {
        let label = if s.rec.name.is_empty() {
            format!("signal 0x{:X}", s.primary())
        } else {
            s.rec.name.clone()
        };
        if s.rec.name.is_empty() {
            issues.push(Issue::error(
                "SIGNAL_NO_NAME",
                format!("{label} carries no name — it can never pair across sheets"),
            ));
        }
        let wire_present = !s.rec.wire_el_id.is_empty()
            && idx.wires.values().any(|w| w.rec.el_id == s.rec.wire_el_id);
        if !wire_present {
            issues.push(Issue::error(
                "SIGNAL_NO_WIRE",
                format!("{label} anchors to a wire that is not in this drawing"),
            ));
        }
        if s.rec.direction == SignalDirection::Source && !s.rec.name.is_empty() {
            if seen_sources
                .insert(s.rec.name.as_str(), s.rec.el_id.as_str())
                .is_some()
            {
                issues.push(Issue::error(
                    "DUP_SIGNAL_SOURCE",
                    format!("signal {label} has more than one source arrow in this drawing"),
                ));
            }
        }
    }

    // ── terminals ────────────────────────────────────────────────────────
    check_terminals(idx, &mut issues);

    // ── cables ───────────────────────────────────────────────────────────
    check_cables(idx, &mut issues);

    // ── connectors ───────────────────────────────────────────────────────
    for conn in idx.connectors.values() {
        let name = if conn.rec.ref_des.is_empty() {
            format!("0x{:X}", conn.primary())
        } else {
            conn.rec.ref_des.clone()
        };
        if conn.rec.pins.is_empty() {
            issues.push(Issue::error(
                "CONN_NO_PINS",
                format!("connector {name} has no pins"),
            ));
        }
        if conn.rec.pin_spacing_mm <= 0.0 {
            issues.push(Issue::error(
                "CONN_SPACING",
                format!("connector {name} has a non-positive pin spacing"),
            ));
        }
        // Duplicate pin names would make a wirelist reference ambiguous.
        let mut seen: BTreeSet<&String> = BTreeSet::new();
        for pin in &conn.rec.pins {
            if !seen.insert(pin) {
                issues.push(Issue::error(
                    "CONN_DUP_PIN",
                    format!("connector {name} lists pin {pin:?} twice"),
                ));
            }
        }
        if conn.rec.cat.is_empty() {
            issues.push(Issue::warning(
                "NO_CATALOG",
                format!("connector {name} has no catalogue part number"),
            ));
        }
    }

    // ── the catalogue, when one is loaded ────────────────────────────────
    if let Some(catalog) = catalog {
        for a in assignments(idx) {
            if a.is_incomplete() {
                continue; // already reported as NO_CATALOG
            }
            if catalog.get(&a.mfg, &a.cat).is_none() {
                issues.push(Issue::warning(
                    "CATALOG_UNKNOWN",
                    format!(
                        "{} references {}/{} which is not in the catalogue",
                        a.item, a.mfg, a.cat
                    ),
                ));
            }
        }
        for (item, part) in crate::catalog::wrong_family(&assignments(idx), catalog) {
            issues.push(Issue::warning(
                "CATALOG_FAMILY",
                format!("{item} is assigned {part}, which is not catalogued for its family"),
            ));
        }
    }

    issues
}

/// Families whose devices are normally accompanied by contacts. A lamp or a
/// supply stands alone; a relay or a pushbutton does not.
fn expects_contacts(family: &str) -> bool {
    matches!(
        family.to_ascii_uppercase().as_str(),
        "K" | "S" | "Q" | "F"
    )
}

fn check_terminals(idx: &ElIndex, issues: &mut Vec<Issue>) {
    // Strips, so a duplicate terminal number within one strip is caught but the
    // same number on another strip is not.
    let mut by_strip: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

    for t in idx.terminals.values() {
        let name = if t.rec.designator().is_empty() {
            format!("0x{:X}", t.primary())
        } else {
            t.rec.designator()
        };
        if t.rec.tag_strip.is_empty() {
            issues.push(Issue::error(
                "TERM_NO_STRIP",
                format!("terminal at 0x{:X} has no strip designator", t.primary()),
            ));
            continue;
        }
        if t.rec.number.is_empty() {
            issues.push(Issue::warning(
                "TERM_UNNUMBERED",
                format!("terminal {name} has no number on its strip"),
            ));
        } else {
            by_strip
                .entry(t.rec.tag_strip.as_str())
                .or_default()
                .push(t.rec.number.as_str());
        }
        if t.rec.kind == TerminalKind::Ground && t.rec.number.is_empty() {
            // Ground terminals are identified by position, not number, so this is
            // already covered — but they must still sit on a strip.
        }
        if t.rec.linked == t.rec.designator() && !t.rec.linked.is_empty() {
            issues.push(Issue::error(
                "TERM_SELF_LINK",
                format!("terminal {name} links to itself"),
            ));
        }
        if t.rec.kind == TerminalKind::Multi && t.rec.linked.is_empty() {
            issues.push(Issue::warning(
                "TERM_MULTI_UNLINKED",
                format!("multi-level terminal {name} names no other level"),
            ));
        }
    }

    for (strip, numbers) in by_strip {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for n in numbers {
            if !seen.insert(n) {
                issues.push(Issue::error(
                    "TERM_DUP_NUMBER",
                    format!("strip {strip} has two terminals numbered {n:?}"),
                ));
            }
        }
    }
}

fn check_cables(idx: &ElIndex, issues: &mut Vec<Issue>) {
    // Conductors are spent per cable tag, so two markers of one cable must not
    // claim one conductor.
    let mut by_cable: BTreeMap<&str, Vec<i32>> = BTreeMap::new();

    for c in idx.cables.values() {
        let name = if c.rec.tag.is_empty() {
            format!("0x{:X}", c.primary())
        } else {
            format!("{}:{}", c.rec.tag, c.rec.conductor_index)
        };
        if c.rec.tag.is_empty() {
            issues.push(Issue::error(
                "CABLE_NO_TAG",
                format!("cable marker at 0x{:X} has no cable tag", c.primary()),
            ));
            continue;
        }
        by_cable
            .entry(c.rec.tag.as_str())
            .or_default()
            .push(c.rec.conductor_index);

        if c.rec.color.trim().is_empty() {
            issues.push(Issue::warning(
                "CABLE_NO_COLOR",
                format!("cable marker {name} has no conductor colour"),
            ));
        }
        if !c.rec.is_parent() && idx.cable_parent(&c.rec.tag).is_none() {
            issues.push(Issue::error(
                "CABLE_NO_PARENT",
                format!(
                    "cable marker {name} names {} as its cable, but no parent marker exists",
                    c.rec.parent_tag
                ),
            ));
        }
        if c.rec.is_parent() && c.rec.conductor_index != 1 {
            issues.push(Issue::warning(
                "CABLE_PARENT_NOT_FIRST",
                format!("the parent marker of {} is conductor {}", c.rec.tag, c.rec.conductor_index),
            ));
        }
        if c.rec.cat.is_empty() {
            issues.push(Issue::warning(
                "NO_CATALOG",
                format!("cable marker {name} has no catalogue part number"),
            ));
        }
    }

    for (tag, conductors) in by_cable {
        let mut seen: BTreeSet<i32> = BTreeSet::new();
        for n in conductors {
            if !seen.insert(n) {
                issues.push(Issue::error(
                    "CABLE_DUP_CONDUCTOR",
                    format!("cable {tag} has two markers on conductor {n}"),
                ));
            }
        }
    }
}

/// Every catalogue assignment in a drawing, for the BOM and the audit.
pub fn assignments(idx: &ElIndex) -> Vec<Assignment> {
    let mut out: Vec<Assignment> = Vec::new();

    for comp in idx.components.values() {
        // Only parents carry the catalogue data — a child shares its parent's
        // part, so listing it would double-count the BOM.
        if comp.rec.is_child {
            continue;
        }
        out.push(Assignment {
            item: comp.rec.tag.clone(),
            mfg: comp.rec.mfg.clone(),
            cat: comp.rec.cat.clone(),
            family: comp.rec.family.clone(),
        });
    }
    for t in idx.terminals.values() {
        out.push(Assignment {
            item: t.rec.designator(),
            mfg: String::new(),
            cat: t.rec.part_number.clone(),
            family: "X".to_string(),
        });
    }
    for c in idx.cables.values() {
        // One row per cable, from its parent marker.
        if !c.rec.is_parent() {
            continue;
        }
        let cat = if c.rec.cat.is_empty() {
            c.rec.part_number.clone()
        } else {
            c.rec.cat.clone()
        };
        out.push(Assignment {
            item: c.rec.tag.clone(),
            mfg: c.rec.mfg.clone(),
            cat,
            family: String::new(),
        });
    }
    for x in idx.connectors.values() {
        out.push(Assignment {
            item: x.rec.ref_des.clone(),
            mfg: x.rec.mfg.clone(),
            cat: x.rec.cat.clone(),
            family: "X".to_string(),
        });
    }
    out
}

/// Would a report built from this drawing be truthful?
pub fn export_allowed(issues: &[Issue]) -> bool {
    !issues.iter().any(|i| i.severity == Severity::Error)
}

pub fn errors(issues: &[Issue]) -> Vec<&Issue> {
    issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect()
}

pub fn warnings(issues: &[Issue]) -> Vec<&Issue> {
    issues
        .iter()
        .filter(|i| i.severity == Severity::Warning)
        .collect()
}

/// One line summarising a check.
pub fn summary(issues: &[Issue]) -> String {
    let e = errors(issues).len();
    let w = warnings(issues).len();
    if e == 0 && w == 0 {
        return "no issues".to_string();
    }
    let mut parts = Vec::new();
    if e > 0 {
        parts.push(format!("{e} error{}", if e == 1 { "" } else { "s" }));
    }
    if w > 0 {
        parts.push(format!("{w} warning{}", if w == 1 { "" } else { "s" }));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ElRecord;
    use crate::symbol::SymbolLibrary;

    fn comp(handle: u64, tag: &str, family: &str, is_child: bool, parent: &str) -> (u64, ElRecord) {
        let mut c = ElComponentRec::new(&format!("id{handle}"), "sym", family);
        c.tag = tag.to_string();
        c.is_child = is_child;
        c.parent_tag = parent.to_string();
        c.pins = vec!["1".to_string(), "2".to_string()];
        (handle, ElRecord::encode_component(&c))
    }

    fn wire(handle: u64, wref: &str, from: &str, to: &str) -> (u64, ElRecord) {
        let mut w = ElWireRec::new(&format!("id{handle}"), from, to);
        w.wire_ref = wref.to_string();
        w.gauge = "1.5".to_string();
        (handle, ElRecord::encode_wire(&w))
    }

    fn settings() -> (u64, ElRecord) {
        (900, ElRecord::encode_settings(&ElSettingsRec::default()))
    }

    fn codes(issues: &[Issue]) -> Vec<String> {
        issues.iter().map(|i| i.code.clone()).collect()
    }

    #[test]
    fn a_clean_drawing_passes() {
        let idx = ElIndex::build(vec![
            settings(),
            comp(1, "K1", "K", false, ""),
            comp(2, "K1", "K", true, "K1"),
            wire(3, "W1", "K1", "K2"),
        ]);
        let issues = check(&idx, None);
        // Only the missing catalogue data is wrong here, and that is a warning.
        assert!(export_allowed(&issues), "codes: {:?}", codes(&issues));
    }

    #[test]
    fn a_duplicate_tag_blocks_export() {
        // A BOM that lists K1 twice cannot be reconciled against a drawing.
        let idx = ElIndex::build(vec![settings(), comp(1, "K1", "K", false, ""), comp(2, "K1", "K", false, "")]);
        let issues = check(&idx, None);
        assert!(!export_allowed(&issues));
        assert!(codes(&issues).contains(&"DUP_TAG".to_string()));
    }

    #[test]
    fn an_untagged_device_blocks_export() {
        let idx = ElIndex::build(vec![settings(), comp(1, "", "K", false, "")]);
        let issues = check(&idx, None);
        assert!(!export_allowed(&issues));
        assert!(codes(&issues).contains(&"UNTAGGED".to_string()));
    }

    #[test]
    fn an_orphan_child_blocks_export() {
        let idx = ElIndex::build(vec![settings(), comp(1, "K9", "K", true, "K9")]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"ORPHAN_CHILD".to_string()));
        assert!(!export_allowed(&issues));
    }

    #[test]
    fn a_dangling_wire_blocks_export() {
        // A from/to report for a wire with one end is meaningless.
        let idx = ElIndex::build(vec![settings(), wire(1, "W1", "K1", "")]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"WIRE_DANGLING".to_string()));
        assert!(!export_allowed(&issues));
    }

    #[test]
    fn a_missing_manufacturer_blocks_but_a_missing_part_does_not() {
        // A part number cannot be ordered without its manufacturer, but a device
        // with no part at all is a drawing still being filled in.
        let mut c = ElComponentRec::new("id", "sym", "K");
        c.tag = "K1".to_string();
        c.cat = "3RT".to_string();
        let idx = ElIndex::build(vec![settings(), (1, ElRecord::encode_component(&c))]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"NO_MFG".to_string()));
        assert!(!export_allowed(&issues));

        let mut c2 = ElComponentRec::new("id", "sym", "K");
        c2.tag = "K1".to_string();
        let idx2 = ElIndex::build(vec![settings(), (1, ElRecord::encode_component(&c2))]);
        let issues2 = check(&idx2, None);
        assert!(codes(&issues2).contains(&"NO_CATALOG".to_string()));
        assert!(export_allowed(&issues2), "a warning must not block");
    }

    #[test]
    fn a_wire_with_no_gauge_warns_but_does_not_block() {
        let mut w = ElWireRec::new("id", "K1", "K2");
        w.wire_ref = "W1".to_string();
        let idx = ElIndex::build(vec![settings(), (1, ElRecord::encode_wire(&w))]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"NO_GAUGE".to_string()));
        assert!(export_allowed(&issues));
    }

    #[test]
    fn a_relay_with_no_contacts_warns() {
        let idx = ElIndex::build(vec![settings(), comp(1, "K1", "K", false, "")]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"NO_CONTACTS".to_string()));
        // A lamp stands alone, so it must not warn.
        let idx2 = ElIndex::build(vec![settings(), comp(1, "H1", "H", false, "")]);
        assert!(!codes(&check(&idx2, None)).contains(&"NO_CONTACTS".to_string()));
    }

    #[test]
    fn a_child_with_no_parent_tag_blocks_export() {
        let idx = ElIndex::build(vec![settings(), comp(1, "K1", "K", true, "")]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"CHILD_NO_PARENT".to_string()));
    }

    #[test]
    fn a_duplicate_terminal_number_on_one_strip_blocks() {
        // A terminal plan cannot describe two terminals numbered 1 on one strip.
        let mut a = TerminalRec::new("ta", "X1", TerminalKind::Screw);
        a.number = "1".to_string();
        let mut b = TerminalRec::new("tb", "X1", TerminalKind::Screw);
        b.number = "1".to_string();
        let idx = ElIndex::build(vec![
            settings(),
            (1, ElRecord::encode_terminal(&a)),
            (2, ElRecord::encode_terminal(&b)),
        ]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"TERM_DUP_NUMBER".to_string()));
        assert!(!export_allowed(&issues));
    }

    #[test]
    fn the_same_terminal_number_on_two_strips_is_fine() {
        let mut a = TerminalRec::new("ta", "X1", TerminalKind::Screw);
        a.number = "1".to_string();
        let mut b = TerminalRec::new("tb", "X2", TerminalKind::Screw);
        b.number = "1".to_string();
        let idx = ElIndex::build(vec![
            settings(),
            (1, ElRecord::encode_terminal(&a)),
            (2, ElRecord::encode_terminal(&b)),
        ]);
        assert!(!codes(&check(&idx, None)).contains(&"TERM_DUP_NUMBER".to_string()));
    }

    #[test]
    fn a_terminal_with_no_strip_blocks_export() {
        // A terminal belongs to a strip; without one it cannot appear in any
        // terminal report.
        let t = TerminalRec::new("t", "", TerminalKind::Screw);
        let idx = ElIndex::build(vec![settings(), (1, ElRecord::encode_terminal(&t))]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"TERM_NO_STRIP".to_string()));
    }

    #[test]
    fn a_cable_that_reuses_a_conductor_blocks_export() {
        let mut a = CableRec::new("ca", "W1");
        a.color = "BK".to_string();
        let mut b = CableRec::new("cb", "W1");
        b.parent_tag = "W1".to_string();
        b.conductor_index = 1;
        b.color = "BL".to_string();
        let idx = ElIndex::build(vec![
            settings(),
            (1, ElRecord::encode_cable(&a)),
            (2, ElRecord::encode_cable(&b)),
        ]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"CABLE_DUP_CONDUCTOR".to_string()));
    }

    #[test]
    fn a_cable_child_with_no_parent_blocks_export() {
        let mut b = CableRec::new("cb", "W1");
        b.parent_tag = "W1".to_string();
        b.conductor_index = 2;
        let idx = ElIndex::build(vec![settings(), (1, ElRecord::encode_cable(&b))]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"CABLE_NO_PARENT".to_string()));
    }

    #[test]
    fn a_duplicate_connector_pin_blocks_export() {
        let mut c = ConnectorRec::new("x", "X1", 2);
        c.pins = vec!["1".to_string(), "1".to_string()];
        let idx = ElIndex::build(vec![settings(), (1, ElRecord::encode_connector(&c))]);
        let issues = check(&idx, None);
        assert!(codes(&issues).contains(&"CONN_DUP_PIN".to_string()));
        assert!(!export_allowed(&issues));
    }

    #[test]
    fn a_connector_with_no_pins_blocks_export() {
        let mut c = ConnectorRec::new("x", "X1", 0);
        c.pins.clear();
        let idx = ElIndex::build(vec![settings(), (1, ElRecord::encode_connector(&c))]);
        assert!(codes(&check(&idx, None)).contains(&"CONN_NO_PINS".to_string()));
    }

    #[test]
    fn a_corrupt_record_is_reported_first() {
        // Structure before detail: an unreadable record explains later findings.
        let mut bad = ElRecord::new(KIND_WIRE, "x");
        bad.values[0].1 = crate::codec::ElValue::Int(99);
        let idx = ElIndex::build(vec![(1, bad)]);
        let issues = check(&idx, None);
        assert_eq!(issues[0].code, "BAD_RECORD");
        assert_eq!(issues[0].severity, Severity::Error);
    }

    #[test]
    fn assignments_exclude_children_so_the_bom_does_not_double_count() {
        let mut parent = ElComponentRec::new("p", "relay-coil", "K");
        parent.tag = "K1".to_string();
        parent.mfg = "Siemens".to_string();
        parent.cat = "3RT".to_string();
        let mut child = ElComponentRec::new("c", "contact-no", "K");
        child.tag = "K1".to_string();
        child.is_child = true;
        child.parent_tag = "K1".to_string();
        child.mfg = "Siemens".to_string();
        child.cat = "3RT".to_string();
        let idx = ElIndex::build(vec![
            (1, ElRecord::encode_component(&parent)),
            (2, ElRecord::encode_component(&child)),
        ]);
        let a = assignments(&idx);
        assert_eq!(a.len(), 1, "one device, one BOM row");
        assert_eq!(a[0].item, "K1");
    }

    #[test]
    fn an_unknown_catalogue_part_warns_without_blocking() {
        let mut c = ElComponentRec::new("p", "relay-coil", "K");
        c.tag = "K1".to_string();
        c.mfg = "Nobody".to_string();
        c.cat = "NOSUCH".to_string();
        let idx = ElIndex::build(vec![(1, ElRecord::encode_component(&c))]);
        let catalog = Catalog::sample();
        let issues = check(&idx, Some(&catalog));
        assert!(codes(&issues).contains(&"CATALOG_UNKNOWN".to_string()));
        assert!(export_allowed(&issues));
    }

    #[test]
    fn the_summary_counts_both_severities() {
        let idx = ElIndex::build(vec![settings(), comp(1, "", "K", false, "")]);
        let s = summary(&check(&idx, None));
        assert!(s.contains("error"), "{s}");
        assert_eq!(summary(&[]), "no issues");
    }

    #[test]
    fn the_library_and_the_audit_agree_on_what_a_family_is() {
        // A symbol whose family the audit treats specially must exist, or the
        // rule can never fire on a real drawing.
        let lib = SymbolLibrary::default();
        for family in ["K", "S", "Q", "F"] {
            assert!(
                lib.first_parent(family).is_some(),
                "expects_contacts({family}) has no symbol to apply to"
            );
        }
    }
}
