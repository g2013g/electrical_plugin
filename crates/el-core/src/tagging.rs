//! Component tagging and cross-referencing.
//!
//! Tagging mirrors wire numbering, with one addition that matters: **counter
//! per family**. `K1`, `K2` and `Q1` are independent sequences, because a drafter
//! reads `Q1` as "the first breaker", not as "the third device". Reference mode
//! applies here too — a device on rung 104 is `K104`, with a suffix when two
//! devices share a rung.
//!
//! Cross-referencing is the reason a component record exists at all: a coil
//! without its contacts listed is unusable, and a contact that does not name its
//! coil is a dead end. Both directions are derived from the records, never
//! stored, so they cannot go stale.

use std::collections::BTreeMap;

use crate::index::ElIndex;
use crate::model::{suffix_for_index, ElSettingsRec, TagMode};

/// One component's inputs to tagging.
#[derive(Debug, Clone, PartialEq)]
pub struct TagInput {
    pub handle: u64,
    pub existing: String,
    /// Pinned by the user: keep it, and reserve the number.
    pub fixed: bool,
    /// The ref-des family this device belongs to (`K`, `Q`, …).
    pub family: String,
    /// A child shares its parent's tag, so it is never allocated one.
    pub is_child: bool,
    /// The line reference of the rung it sits on, for reference mode.
    pub rung_ref: Option<String>,
}

impl TagInput {
    pub fn new(handle: u64, family: &str) -> Self {
        TagInput {
            handle,
            existing: String::new(),
            fixed: false,
            family: family.to_string(),
            is_child: false,
            rung_ref: None,
        }
    }
}

/// What to write, and what could not be tagged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagPlan {
    pub assignments: Vec<(u64, String)>,
    pub skipped: Vec<(u64, String)>,
    pub mode: TagMode,
}

impl TagPlan {
    pub fn summary(&self) -> String {
        let mut s = format!("{} tag(s) assigned", self.assignments.len());
        if !self.skipped.is_empty() {
            s.push_str(&format!(", {} skipped", self.skipped.len()));
        }
        s.push_str(&format!(" [{} mode]", self.mode.as_str()));
        s
    }

    /// The distinct reasons a component was left alone, for a report.
    pub fn skip_reasons(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for (_, reason) in &self.skipped {
            if reason == "already correct" || reason == "fixed number kept" {
                continue;
            }
            if !out.contains(reason) {
                out.push(reason.clone());
            }
        }
        out
    }
}

/// Plan tags for `ordered` using the drawing's configured mode.
pub fn plan_tags(ordered: &[TagInput], settings: &ElSettingsRec) -> TagPlan {
    match settings.tag_mode {
        TagMode::Sequential => plan_sequential(ordered, settings),
        TagMode::Reference => plan_reference(ordered, settings),
    }
}

/// A running counter **per family**, stepping over pinned tags.
pub fn plan_sequential(ordered: &[TagInput], settings: &ElSettingsRec) -> TagPlan {
    let mut plan = TagPlan {
        mode: TagMode::Sequential,
        ..Default::default()
    };

    // Reserve what pinned tags already occupy, per family.
    let mut reserved: BTreeMap<String, Vec<i32>> = BTreeMap::new();
    for t in ordered {
        if t.fixed {
            if let Some(n) = leading_int(&t.existing) {
                reserved.entry(t.family.clone()).or_default().push(n);
            }
        }
    }

    let mut next: BTreeMap<String, i32> = BTreeMap::new();

    for t in ordered {
        if t.is_child {
            plan.skipped.push((
                t.handle,
                "a child shares its parent's tag".to_string(),
            ));
            continue;
        }
        if t.family.trim().is_empty() {
            plan.skipped.push((
                t.handle,
                "no family code, so it has no tag format to expand".to_string(),
            ));
            continue;
        }
        if t.fixed {
            if t.existing.is_empty() {
                plan.skipped.push((
                    t.handle,
                    "marked fixed but carries no tag; unfix it to tag it".to_string(),
                ));
            } else {
                plan.skipped
                    .push((t.handle, "fixed number kept".to_string()));
            }
            continue;
        }

        let start = settings.tag_start;
        let counter = next.entry(t.family.clone()).or_insert(start);
        while reserved
            .get(&t.family)
            .is_some_and(|v| v.contains(counter))
        {
            *counter += 1;
        }
        let candidate = settings.format_tag(&t.family, *counter);

        if candidate == t.existing {
            plan.skipped.push((t.handle, "already correct".to_string()));
        } else {
            plan.assignments.push((t.handle, candidate));
        }
        reserved
            .entry(t.family.clone())
            .or_default()
            .push(*counter);
        *counter += 1;
    }
    plan
}

/// Reference mode: the tag number *is* the rung's line reference.
pub fn plan_reference(ordered: &[TagInput], settings: &ElSettingsRec) -> TagPlan {
    let mut plan = TagPlan {
        mode: TagMode::Reference,
        ..Default::default()
    };

    let mut reserved: Vec<String> = ordered
        .iter()
        .filter(|t| t.fixed && !t.existing.is_empty())
        .map(|t| t.existing.clone())
        .collect();
    let suffixes = settings.suffix_list();
    // Suffixes are spent per (family, reference) pair: two contacts of the same
    // relay on one rung are the *same* device and must not become K104A.
    let mut used: Vec<(String, std::collections::BTreeSet<String>)> = Vec::new();

    for t in ordered {
        if t.is_child {
            plan.skipped
                .push((t.handle, "a child shares its parent's tag".to_string()));
            continue;
        }
        if t.family.trim().is_empty() {
            plan.skipped.push((
                t.handle,
                "no family code, so it has no tag format to expand".to_string(),
            ));
            continue;
        }
        if t.fixed {
            if t.existing.is_empty() {
                plan.skipped.push((
                    t.handle,
                    "marked fixed but carries no tag; unfix it to tag it".to_string(),
                ));
            } else {
                plan.skipped
                    .push((t.handle, "fixed number kept".to_string()));
            }
            continue;
        }
        let Some(rung_ref) = t.rung_ref.as_deref().filter(|r| !r.trim().is_empty()) else {
            plan.skipped.push((
                t.handle,
                "not on a ladder rung, so it has no line reference".to_string(),
            ));
            continue;
        };

        let key = format!("{}\u{0}{rung_ref}", t.family);
        let spent = match used.iter_mut().find(|(k, _)| *k == key) {
            Some((_, u)) => u,
            None => {
                used.push((key, std::collections::BTreeSet::new()));
                &mut used.last_mut().expect("just pushed").1
            }
        };

        // Try the bare reference, then each suffix in turn. The loop matters:
        // when the bare reference is taken by a device elsewhere in the drawing
        // (a pinned tag, or an earlier group), the allocator has to move on to a
        // suffix rather than declare the wire unnumberable.
        let mut chosen: Option<(String, String)> = None;
        for suffix in candidate_suffixes(&suffixes, spent) {
            // The tag's number slot takes the line reference as *text*, so a
            // non-numeric reference (`2A04`) needs no character substitution.
            let candidate = format!("{}{suffix}", settings.format_tag_text(&t.family, rung_ref));
            if reserved.contains(&candidate) {
                continue;
            }
            chosen = Some((suffix, candidate));
            break;
        }

        let Some((suffix, candidate)) = chosen else {
            plan.skipped.push((
                t.handle,
                format!("every tag for reference {rung_ref} is already taken"),
            ));
            continue;
        };

        if candidate == t.existing {
            spent.insert(suffix);
            plan.skipped.push((t.handle, "already correct".to_string()));
            continue;
        }
        spent.insert(suffix);
        plan.assignments.push((t.handle, candidate.clone()));
        reserved.push(candidate);
    }
    plan
}

/// Suffixes to try, in order, for a device sharing a line reference.
///
/// The bare reference comes first, then the configured list, then the alphabet —
/// because running out of configured suffixes must never produce two devices
/// with one tag. Entries already spent on this reference are dropped here.
fn candidate_suffixes(
    configured: &[String],
    spent: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let mut out: Vec<String> = vec![String::new()];
    for s in configured {
        if !s.is_empty() && !out.contains(s) {
            out.push(s.clone());
        }
    }
    // Bounded: a rung with more devices than this wants a different scheme, not
    // an unbounded list.
    for i in 1..=256usize {
        let candidate = suffix_for_index(i);
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out.retain(|s| !spent.contains(s));
    out
}

/// The integer at the start of a tag, so a pinned `K12A` reserves 12.
fn leading_int(s: &str) -> Option<i32> {
    let digits: String = s
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<i32>().ok()
}

/// The next free tag for a family, given everything already placed.
///
/// A planner cannot see the drawing, so a caller allocating one tag at a time
/// must hand it the existing tags as reservations. Without that, every new
/// device in a family starts from `TAG_START` and the drawing fills up with
/// duplicates — which is exactly what an index-free allocator does.
pub fn next_tag(
    idx: &ElIndex,
    family: &str,
    settings: &ElSettingsRec,
    rung_ref: Option<&str>,
) -> Option<String> {
    let mut inputs: Vec<TagInput> = Vec::new();

    for comp in idx.components.values() {
        if comp.rec.tag.is_empty() {
            continue;
        }
        // A child is reserved too: it shares its parent's tag, and in reference
        // mode the parent's own reservation is what matters.
        inputs.push(TagInput {
            handle: comp.primary(),
            existing: comp.rec.tag.clone(),
            fixed: true,
            family: if comp.rec.family.is_empty() {
                crate::model::family_of(&comp.rec.tag)
            } else {
                comp.rec.family.clone()
            },
            is_child: comp.rec.is_child,
            rung_ref: None,
        });
    }

    const PROBE: u64 = u64::MAX;
    inputs.push(TagInput {
        handle: PROBE,
        existing: String::new(),
        fixed: false,
        family: family.to_string(),
        is_child: false,
        rung_ref: rung_ref.map(str::to_string),
    });

    plan_tags(&inputs, settings)
        .assignments
        .into_iter()
        .find(|(h, _)| *h == PROBE)
        .map(|(_, tag)| tag)
}

// ── cross-references ─────────────────────────────────────────────────────

/// One device's cross-reference, resolved in both directions.
#[derive(Debug, Clone, PartialEq)]
pub struct XrefEntry {
    pub tag: String,
    pub family: String,
    /// The line reference of the rung the device sits on, if it is on one.
    pub rung_ref: Option<String>,
    /// A contact or auxiliary element belonging to another device.
    pub is_child: bool,
    /// The parent's tag, for a child.
    pub parent_tag: String,
}

/// A parent and the children that reference it.
#[derive(Debug, Clone, PartialEq)]
pub struct XrefGroup {
    pub parent_tag: String,
    pub family: String,
    pub parent_ref: Option<String>,
    /// Children, in tag order.
    pub children: Vec<XrefEntry>,
}

impl XrefGroup {
    /// The text a parent shows: where its contacts live.
    ///
    /// One contact prints its reference bare; several are comma-separated. A
    /// child with no reference is reported rather than skipped, because a
    /// missing entry is more likely a second rung that has not been tagged than
    /// a device that genuinely has no location.
    pub fn parent_text(&self) -> String {
        if self.children.is_empty() {
            return String::new();
        }
        let refs: Vec<String> = self
            .children
            .iter()
            .map(|c| {
                c.rung_ref
                    .clone()
                    .unwrap_or_else(|| "—".to_string())
            })
            .collect();
        refs.join(",")
    }

    /// The text a child shows: where its parent is.
    pub fn child_text(&self) -> String {
        match (&self.parent_ref, self.children.len()) {
            (Some(r), _) => r.clone(),
            (None, 0) => String::new(),
            (None, _) => "—".to_string(),
        }
    }
}

/// Build every cross-reference group in a drawing.
///
/// `positions` pairs a component handle with the line reference of the rung it
/// sits on, which only the caller can determine — it needs geometry.
pub fn build_xrefs(idx: &ElIndex, positions: &[(u64, Option<String>)]) -> Vec<XrefGroup> {
    let ref_of = |handle: u64| -> Option<String> {
        positions
            .iter()
            .find(|(h, _)| *h == handle)
            .and_then(|(_, r)| r.clone())
    };

    let mut groups: BTreeMap<String, XrefGroup> = BTreeMap::new();

    // Pass 1: a placed *parent* is a group. Children are not: a child shares its
    // parent's tag, so giving it a group of its own would produce two groups
    // with one name and split the very list the drafter needs to read.
    for comp in idx.components.values() {
        if comp.rec.is_child || comp.rec.tag.is_empty() {
            continue;
        }
        groups.insert(
            comp.rec.tag.clone(),
            XrefGroup {
                parent_tag: comp.rec.tag.clone(),
                family: comp.rec.family.clone(),
                parent_ref: ref_of(comp.primary()),
                children: Vec::new(),
            },
        );
    }

    // Pass 2: attach children to their parents, creating a placeholder group
    // when the parent is not placed in this drawing.
    for comp in idx.components.values() {
        if !comp.rec.is_child || comp.rec.parent_tag.is_empty() {
            continue;
        }
        let parent = comp.rec.parent_tag.clone();
        let group = groups.entry(parent.clone()).or_insert_with(|| XrefGroup {
            parent_tag: parent.clone(),
            family: comp.rec.family.clone(),
            parent_ref: None,
            children: Vec::new(),
        });
        group.children.push(XrefEntry {
            tag: comp.rec.tag.clone(),
            family: comp.rec.family.clone(),
            rung_ref: ref_of(comp.primary()),
            is_child: true,
            parent_tag: parent,
        });
    }

    let mut out: Vec<XrefGroup> = groups.into_values().collect();
    for g in &mut out {
        g.children.sort_by(|a, b| crate::refdes_cmp(&a.tag, &b.tag));
    }
    out.sort_by(|a, b| crate::refdes_cmp(&a.parent_tag, &b.parent_tag));
    out
}

/// Children whose parent is not placed in this drawing.
pub fn unlinked_children(idx: &ElIndex) -> Vec<(String, String)> {
    idx.orphan_children()
        .into_iter()
        .map(|c| (c.rec.tag.clone(), c.rec.parent_tag.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ElRecord;
    use crate::model::ElComponentRec;

    fn seq() -> ElSettingsRec {
        let mut s = ElSettingsRec::default();
        s.tag_mode = TagMode::Sequential;
        s.tag_format = "%F%N".to_string();
        s.tag_start = 1;
        s
    }

    fn reference() -> ElSettingsRec {
        let mut s = ElSettingsRec::default();
        s.tag_mode = TagMode::Reference;
        s.tag_format = "%F%N".to_string();
        s
    }

    fn dev(handle: u64, family: &str, rung: Option<&str>) -> TagInput {
        let mut t = TagInput::new(handle, family);
        t.rung_ref = rung.map(str::to_string);
        t
    }

    fn component(handle: u64, tag: &str, family: &str, is_child: bool, parent: &str) -> (u64, ElRecord) {
        let mut c = ElComponentRec::new(&format!("id{handle}"), "sym", family);
        c.tag = tag.to_string();
        c.is_child = is_child;
        c.parent_tag = parent.to_string();
        (handle, ElRecord::encode_component(&c))
    }

    // ── sequential ───────────────────────────────────────────────────────

    #[test]
    fn counters_run_per_family() {
        // The whole point: Q1 is the first breaker, not the third device.
        let s = seq();
        let plan = plan_sequential(
            &[dev(1, "K", None), dev(2, "Q", None), dev(3, "K", None), dev(4, "Q", None)],
            &s,
        );
        assert_eq!(
            plan.assignments,
            vec![
                (1, "K1".to_string()),
                (2, "Q1".to_string()),
                (3, "K2".to_string()),
                (4, "Q2".to_string())
            ]
        );
    }

    #[test]
    fn tag_start_is_respected_per_family() {
        let mut s = seq();
        s.tag_start = 10;
        let plan = plan_sequential(&[dev(1, "K", None), dev(2, "K", None)], &s);
        assert_eq!(plan.assignments[0].1, "K10");
        assert_eq!(plan.assignments[1].1, "K11");
    }

    #[test]
    fn a_children_is_never_allocated_a_tag() {
        let s = seq();
        let mut child = dev(1, "K", None);
        child.is_child = true;
        let plan = plan_sequential(&[child, dev(2, "K", None)], &s);
        assert_eq!(plan.assignments, vec![(2, "K1".to_string())]);
        assert!(plan.skipped[0].1.contains("shares its parent"));
    }

    #[test]
    fn a_pinned_tag_reserves_its_number() {
        let s = seq();
        let mut pinned = dev(1, "K", None);
        pinned.fixed = true;
        pinned.existing = "K2".to_string();
        let plan = plan_sequential(&[pinned, dev(2, "K", None), dev(3, "K", None)], &s);
        assert_eq!(
            plan.assignments,
            vec![(2, "K1".to_string()), (3, "K3".to_string())],
            "the counter must step over the pinned K2"
        );
    }

    #[test]
    fn a_family_with_no_code_is_reported() {
        // Without a family there is no %F to expand, so the tag would be
        // meaningless rather than merely wrong.
        let s = seq();
        let plan = plan_sequential(&[dev(1, "", None)], &s);
        assert!(plan.assignments.is_empty());
        assert!(plan.skipped[0].1.contains("no family code"));
    }

    #[test]
    fn fixed_without_a_tag_is_reported() {
        let s = seq();
        let mut pinned = dev(1, "K", None);
        pinned.fixed = true;
        let plan = plan_sequential(&[pinned], &s);
        assert!(plan.skipped[0].1.contains("unfix it"));
    }

    #[test]
    fn an_unchanged_tag_is_not_rewritten() {
        let s = seq();
        let mut t = dev(1, "K", None);
        t.existing = "K1".to_string();
        let plan = plan_sequential(&[t, dev(2, "K", None)], &s);
        assert_eq!(plan.assignments, vec![(2, "K2".to_string())]);
        assert_eq!(plan.skipped.len(), 1);
    }

    // ── reference ────────────────────────────────────────────────────────

    #[test]
    fn reference_mode_takes_the_rung() {
        let s = reference();
        let plan = plan_reference(&[dev(1, "K", Some("104")), dev(2, "Q", Some("105"))], &s);
        assert_eq!(
            plan.assignments,
            vec![(1, "K104".to_string()), (2, "Q105".to_string())]
        );
    }

    #[test]
    fn two_devices_of_one_family_on_a_rung_get_suffixes() {
        let s = reference();
        let plan = plan_reference(
            &[dev(1, "K", Some("104")), dev(2, "K", Some("104"))],
            &s,
        );
        assert_eq!(plan.assignments[0].1, "K104");
        assert_eq!(plan.assignments[1].1, "K104A");
    }

    #[test]
    fn suffixes_are_spent_per_family_not_per_rung() {
        // A relay and a breaker on one rung are different devices: K104 and
        // Q104, not Q104A.
        let s = reference();
        let plan = plan_reference(
            &[dev(1, "K", Some("104")), dev(2, "Q", Some("104"))],
            &s,
        );
        assert_eq!(plan.assignments[0].1, "K104");
        assert_eq!(plan.assignments[1].1, "Q104", "a different family is a different device");
    }

    #[test]
    fn more_devices_than_suffixes_stays_unique() {
        let mut s = reference();
        s.tag_suffixes = "X".to_string();
        let plan = plan_reference(
            &[
                dev(1, "K", Some("104")),
                dev(2, "K", Some("104")),
                dev(3, "K", Some("104")),
            ],
            &s,
        );
        let tags: Vec<&str> = plan.assignments.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(tags, ["K104", "K104X", "K104A"]);
        let unique: std::collections::BTreeSet<&&str> = tags.iter().collect();
        assert_eq!(unique.len(), tags.len());
    }

    #[test]
    fn a_device_off_the_ladder_is_reported_in_reference_mode() {
        let s = reference();
        let plan = plan_reference(&[dev(1, "K", None), dev(2, "K", Some("104"))], &s);
        assert_eq!(plan.assignments, vec![(2, "K104".to_string())]);
        assert!(plan.skipped[0].1.contains("no line reference"));
    }

    #[test]
    fn plan_dispatches_on_the_settings_mode() {
        assert_eq!(plan_tags(&[dev(1, "K", Some("104"))], &seq()).assignments[0].1, "K1");
        assert_eq!(
            plan_tags(&[dev(1, "K", Some("104"))], &reference()).assignments[0].1,
            "K104"
        );
    }

    // ── allocation against a drawing ─────────────────────────────────────

    fn placed(handle: u64, tag: &str, family: &str, is_child: bool) -> (u64, ElRecord) {
        component(handle, tag, family, is_child, if is_child { tag } else { "" })
    }

    #[test]
    fn the_next_tag_skips_what_is_already_placed() {
        // The defect this guards: allocating per-placement without looking at
        // the drawing gives every device in a family the same tag.
        let idx = ElIndex::build(vec![placed(1, "K1", "K", false), placed(2, "K2", "K", false)]);
        let s = seq();
        assert_eq!(next_tag(&idx, "K", &s, None).as_deref(), Some("K3"));
    }

    #[test]
    fn families_are_counted_independently() {
        let idx = ElIndex::build(vec![placed(1, "K1", "K", false)]);
        let s = seq();
        assert_eq!(next_tag(&idx, "Q", &s, None).as_deref(), Some("Q1"));
        assert_eq!(next_tag(&idx, "K", &s, None).as_deref(), Some("K2"));
    }

    #[test]
    fn an_empty_drawing_starts_at_the_configured_start() {
        let idx = ElIndex::build(vec![]);
        let mut s = seq();
        s.tag_start = 100;
        assert_eq!(next_tag(&idx, "K", &s, None).as_deref(), Some("K100"));
    }

    #[test]
    fn the_lowest_free_number_in_a_family_is_taken() {
        // With K2 gone the next device is K2 again. Hole-filling rather than
        // max+1 is the deliberate choice: a drawing's device list stays dense,
        // and the same planner backs EL_RETAG, where a dense sequence is the
        // whole point.
        let idx = ElIndex::build(vec![placed(1, "K1", "K", false), placed(3, "K3", "K", false)]);
        let s = seq();
        assert_eq!(next_tag(&idx, "K", &s, None).as_deref(), Some("K2"));
    }

    #[test]
    fn reference_mode_allocates_from_the_rung_and_suffixes_around_occupants() {
        let idx = ElIndex::build(vec![placed(1, "K104", "K", false)]);
        let s = reference();
        assert_eq!(
            next_tag(&idx, "K", &s, Some("104")).as_deref(),
            Some("K104A"),
            "one device already holds K104"
        );
        assert_eq!(next_tag(&idx, "K", &s, Some("105")).as_deref(), Some("K105"));
    }

    #[test]
    fn a_child_does_not_consume_the_next_parent_number() {
        // Two contacts of K1 plus a second coil: the coil is K2, not K3.
        let idx = ElIndex::build(vec![
            placed(1, "K1", "K", false),
            placed(2, "K1", "K", true),
            placed(3, "K1", "K", true),
        ]);
        let s = seq();
        assert_eq!(next_tag(&idx, "K", &s, None).as_deref(), Some("K2"));
    }

    // ── cross-references ─────────────────────────────────────────────────

    #[test]
    fn a_parent_lists_its_childrens_references() {
        let idx = ElIndex::build(vec![
            component(1, "K1", "K", false, ""),
            component(2, "K1", "K", true, "K1"),
            component(3, "K1", "K", true, "K1"),
        ]);
        let groups = build_xrefs(&idx, &[(1, Some("104".into())), (2, Some("110".into())), (3, Some("112".into()))]);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.parent_tag, "K1");
        assert_eq!(g.parent_ref.as_deref(), Some("104"));
        assert_eq!(g.parent_text(), "110,112");
        assert_eq!(g.child_text(), "104", "a child points back at its parent");
    }

    #[test]
    fn a_parent_with_no_children_has_empty_text() {
        let idx = ElIndex::build(vec![component(1, "K1", "K", false, "")]);
        let groups = build_xrefs(&idx, &[(1, Some("100".into()))]);
        assert_eq!(groups[0].parent_text(), "", "nothing to list");
        assert!(groups[0].children.is_empty());
    }

    #[test]
    fn a_child_whose_parent_is_absent_still_appears() {
        // The group is created with a "—" reference rather than dropped: a
        // contact with no coil is exactly what a drafter must be told about.
        let idx = ElIndex::build(vec![component(2, "K9", "K", true, "K9")]);
        let groups = build_xrefs(&idx, &[(2, Some("110".into()))]);
        assert_eq!(groups.len(), 1);
        assert!(groups[0].parent_ref.is_none());
        assert_eq!(groups[0].child_text(), "—");
        assert_eq!(groups[0].parent_text(), "110");
    }

    #[test]
    fn a_child_with_no_rung_is_shown_not_skipped() {
        let idx = ElIndex::build(vec![
            component(1, "K1", "K", false, ""),
            component(2, "K1", "K", true, "K1"),
        ]);
        let groups = build_xrefs(&idx, &[(1, Some("104".into())), (2, None)]);
        assert_eq!(groups[0].parent_text(), "—", "a missing location is visible");
    }

    #[test]
    fn groups_are_sorted_by_tag() {
        let idx = ElIndex::build(vec![
            component(1, "Q1", "Q", false, ""),
            component(2, "K10", "K", false, ""),
            component(3, "K2", "K", false, ""),
        ]);
        let groups = build_xrefs(&idx, &[]);
        let tags: Vec<&str> = groups.iter().map(|g| g.parent_tag.as_str()).collect();
        assert_eq!(tags, ["K2", "K10", "Q1"], "natural order, so K2 precedes K10");
    }

    #[test]
    fn a_component_with_no_tag_is_not_a_group() {
        let idx = ElIndex::build(vec![component(1, "", "K", false, "")]);
        assert!(build_xrefs(&idx, &[]).is_empty());
    }

    #[test]
    fn unlinked_children_are_listed() {
        let idx = ElIndex::build(vec![
            component(1, "K1", "K", false, ""),
            component(2, "K1", "K", true, "K1"),
            component(3, "K9", "K", true, "K9"),
        ]);
        let orphans = unlinked_children(&idx);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0], ("K9".to_string(), "K9".to_string()));
    }

    #[test]
    fn lead_int_reads_a_suffixed_tag() {
        assert_eq!(leading_int("K12A"), Some(12));
        assert_eq!(leading_int("Q1"), Some(1));
        assert_eq!(leading_int("K"), None);
    }

    #[test]
    fn summary_mentions_the_mode() {
        let plan = plan_sequential(&[dev(1, "K", None)], &seq());
        let s = plan.summary();
        assert!(s.contains("1 tag(s) assigned"), "{s}");
        assert!(s.contains("[S mode]"), "{s}");
    }
}
