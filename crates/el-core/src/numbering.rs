//! Wire-number allocation.
//!
//! Two modes, matching ACADE:
//!
//! * **Sequential** — a running counter with a per-drawing start value and
//!   increment, walked in drawing order.
//! * **Reference** — the number *is* the line reference of the rung the wire
//!   sits on, with a duplicate suffix (`A`, `B`, …) when several wires share
//!   one rung. This is why a ladder has to exist before reference numbering is
//!   meaningful.
//!
//! Both modes honour *fixed* numbers: a wire the user has pinned keeps its
//! number and its number is reserved so the counter steps over it. That is the
//! difference between a renumber that respects hand-tuned drawing and one that
//! destroys it.
//!
//! The planner is pure — it takes an already-ordered list of wires and returns
//! what to write. Ordering is the caller's job because it depends on where the
//! wires are drawn, which this crate cannot see.

use std::collections::BTreeSet;

use crate::model::{family_of, suffix_for_index, ElSettingsRec, WireNoMode};

/// One wire's inputs to numbering.
#[derive(Debug, Clone, PartialEq)]
pub struct WireNumberInput {
    pub handle: u64,
    /// The number already on the wire, if any.
    pub existing: String,
    /// Pinned by the user: keep it, and reserve the number.
    pub fixed: bool,
    /// Whether the wire's type takes part in numbering at all. A ground bond or
    /// a jumper opts out.
    pub numbered: bool,
    /// The line reference of the rung this wire sits on. `None` when the wire
    /// is not on any ladder rung — which reference mode cannot number.
    pub rung_ref: Option<String>,
}

impl WireNumberInput {
    pub fn new(handle: u64) -> Self {
        WireNumberInput {
            handle,
            existing: String::new(),
            fixed: false,
            numbered: true,
            rung_ref: None,
        }
    }
}

/// What to write, and what could not be numbered.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WireNumberPlan {
    /// `(handle, new number)` for every wire whose number changes.
    pub assignments: Vec<(u64, String)>,
    /// `(handle, reason)` for every wire left alone, so the command can explain
    /// itself instead of silently skipping wires.
    pub skipped: Vec<(u64, String)>,
    pub mode: WireNoMode,
}

impl WireNumberPlan {
    /// How many wires keep the number they already had.
    pub fn unchanged(&self) -> usize {
        self.skipped
            .iter()
            .filter(|(_, r)| r == "already correct")
            .count()
    }

    /// A one-line summary for the command line.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} wire number(s) {}",
            self.assignments.len(),
            if self.assignments.len() == 1 { "assigned" } else { "assigned" }
        );
        if !self.skipped.is_empty() {
            s.push_str(&format!(", {} skipped", self.skipped.len()));
        }
        s.push_str(&format!(" [{} mode]", self.mode.as_str()));
        s
    }
}

/// Plan numbering for `ordered` using the drawing's configured mode.
pub fn plan(ordered: &[WireNumberInput], settings: &ElSettingsRec) -> WireNumberPlan {
    match settings.wireno_mode {
        WireNoMode::Sequential => plan_sequential(ordered, settings),
        WireNoMode::Reference => plan_reference(ordered, settings),
    }
}

/// A running counter, stepping over numbers reserved by fixed wires.
pub fn plan_sequential(ordered: &[WireNumberInput], settings: &ElSettingsRec) -> WireNumberPlan {
    let mut plan = WireNumberPlan {
        mode: WireNoMode::Sequential,
        ..Default::default()
    };

    // Pass 1: reserve what the fixed wires already occupy, so the counter never
    // lands on a hand-picked number.
    let mut reserved: BTreeSet<i32> = BTreeSet::new();
    for w in ordered {
        if w.fixed {
            if let Some(n) = leading_int(&w.existing) {
                reserved.insert(n);
            }
        }
    }

    let mut next = settings.wireno_start;
    let step = settings.wireno_increment.max(1);

    for w in ordered {
        if !w.numbered {
            plan.skipped
                .push((w.handle, "wire type is not numbered".to_string()));
            continue;
        }
        if w.fixed {
            if w.existing.is_empty() {
                plan.skipped.push((
                    w.handle,
                    "marked fixed but carries no number; unfix it to number it".to_string(),
                ));
            } else {
                plan.skipped
                    .push((w.handle, "fixed number kept".to_string()));
            }
            continue;
        }
        while reserved.contains(&next) {
            next += step;
        }
        let candidate = settings.format_wireno(next);
        if candidate == w.existing {
            plan.skipped.push((w.handle, "already correct".to_string()));
        } else {
            plan.assignments.push((w.handle, candidate));
        }
        reserved.insert(next);
        next += step;
    }
    plan
}

/// Reference-based numbering: the number is the rung's line reference, with a
/// suffix when a rung carries more than one wire.
pub fn plan_reference(ordered: &[WireNumberInput], settings: &ElSettingsRec) -> WireNumberPlan {
    let mut plan = WireNumberPlan {
        mode: WireNoMode::Reference,
        ..Default::default()
    };

    // Reserve the references that fixed wires already claim, by (ref, number).
    let mut reserved: BTreeSet<String> = BTreeSet::new();
    for w in ordered {
        if w.fixed && !w.existing.is_empty() {
            reserved.insert(w.existing.clone());
        }
    }

    // Which suffixes are already spent on each reference, so a wire is never
    // given one that a sibling on the same rung already has — whatever the
    // configured suffix list happens to look like.
    let suffixes = settings.suffix_list();
    let mut used_per_ref: Vec<(String, BTreeSet<String>)> = Vec::new();

    for w in ordered {
        if !w.numbered {
            plan.skipped
                .push((w.handle, "wire type is not numbered".to_string()));
            continue;
        }
        if w.fixed {
            if w.existing.is_empty() {
                plan.skipped.push((
                    w.handle,
                    "marked fixed but carries no number; unfix it to number it".to_string(),
                ));
            } else {
                plan.skipped
                    .push((w.handle, "fixed number kept".to_string()));
            }
            continue;
        }
        let Some(rung_ref) = w.rung_ref.as_deref().filter(|r| !r.trim().is_empty()) else {
            plan.skipped.push((
                w.handle,
                "not on a ladder rung, so it has no line reference".to_string(),
            ));
            continue;
        };

        let used = match used_per_ref.iter_mut().find(|(r, _)| r == rung_ref) {
            Some((_, u)) => u,
            None => {
                used_per_ref.push((rung_ref.to_string(), BTreeSet::new()));
                &mut used_per_ref.last_mut().expect("just pushed").1
            }
        };
        let suffix = next_free_suffix(&suffixes, used);
        let candidate = settings.format_wireno_text(rung_ref, &suffix);

        if reserved.contains(&candidate) {
            plan.skipped.push((
                w.handle,
                format!("{candidate} is already used by a fixed wire"),
            ));
            continue;
        }
        if candidate == w.existing {
            used.insert(suffix);
            plan.skipped.push((w.handle, "already correct".to_string()));
            continue;
        }
        used.insert(suffix);
        plan.assignments.push((w.handle, candidate.clone()));
        reserved.insert(candidate);
    }
    plan
}

/// The first suffix for a wire sharing a reference that no sibling already has.
///
/// The configured list is preferred, then the alphabetical generator fills in —
/// because running out of configured suffixes must never produce two wires with
/// the same number, which is exactly the defect a suffix list is there to
/// prevent.
fn next_free_suffix(configured: &[String], used: &BTreeSet<String>) -> String {
    if !used.contains("") {
        return String::new();
    }
    // Configured entries first, in order.
    for s in configured {
        if !s.is_empty() && !used.contains(s) {
            return s.clone();
        }
    }
    // Then walk the alphabet.
    let mut i = 1usize;
    loop {
        let candidate = suffix_for_index(i);
        if !used.contains(&candidate) {
            return candidate;
        }
        i += 1;
        if i > 1000 {
            // Not reachable in practice (would need 1000 wires on one rung),
            // but a bounded loop beats an infinite one.
            return suffix_for_index(used.len() + 1);
        }
    }
}

/// The integer at the start of a wire number, so a fixed `100A` reserves 100.
fn leading_int(s: &str) -> Option<i32> {
    let digits: String = s
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<i32>().ok()
}

/// Order wires the way a drafter renumbers them: top to bottom, then left to
/// right. Positions are the wire's visual midpoint.
pub fn drawing_order(wires: &[(u64, [f64; 2])]) -> Vec<u64> {
    let mut v: Vec<(u64, [f64; 2])> = wires.to_vec();
    v.sort_by(|a, b| {
        // Descending y (top first), then ascending x.
        b.1[1]
            .partial_cmp(&a.1[1])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.1[0]
                    .partial_cmp(&b.1[0])
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    v.into_iter().map(|(h, _)| h).collect()
}

/// Split a wire number into its family and numeric part, for reporting.
pub fn wire_number_family(number: &str) -> String {
    family_of(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq_settings() -> ElSettingsRec {
        let mut s = ElSettingsRec::default();
        s.wireno_mode = WireNoMode::Sequential;
        s.wireno_format = "%N".to_string();
        s.wireno_start = 1;
        s.wireno_increment = 1;
        s
    }

    fn ref_settings() -> ElSettingsRec {
        let mut s = ElSettingsRec::default();
        s.wireno_mode = WireNoMode::Reference;
        s.wireno_format = "%N%X".to_string();
        s
    }

    fn wire(handle: u64, rung: Option<&str>) -> WireNumberInput {
        let mut w = WireNumberInput::new(handle);
        w.rung_ref = rung.map(str::to_string);
        w
    }

    // ── sequential ───────────────────────────────────────────────────────

    #[test]
    fn sequential_numbers_in_order_from_the_start() {
        let s = seq_settings();
        let plan = plan_sequential(&[wire(1, None), wire(2, None), wire(3, None)], &s);
        assert_eq!(
            plan.assignments,
            vec![
                (1, "1".to_string()),
                (2, "2".to_string()),
                (3, "3".to_string())
            ]
        );
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn sequential_honours_start_and_increment() {
        let mut s = seq_settings();
        s.wireno_start = 100;
        s.wireno_increment = 5;
        let plan = plan_sequential(&[wire(1, None), wire(2, None)], &s);
        assert_eq!(
            plan.assignments,
            vec![(1, "100".to_string()), (2, "105".to_string())]
        );
    }

    #[test]
    fn sequential_applies_the_number_format() {
        let mut s = seq_settings();
        s.wireno_format = "W%N".to_string();
        let plan = plan_sequential(&[wire(1, None)], &s);
        assert_eq!(plan.assignments[0].1, "W1");
    }

    #[test]
    fn fixed_wires_keep_their_number_and_reserve_it() {
        // The counter must step *over* a hand-pinned number rather than land on
        // it, or renumbering would create a duplicate.
        let s = seq_settings();
        let mut pinned = wire(1, None);
        pinned.fixed = true;
        pinned.existing = "2".to_string();
        let plan = plan_sequential(&[pinned, wire(2, None), wire(3, None)], &s);
        assert_eq!(
            plan.assignments,
            vec![(2, "1".to_string()), (3, "3".to_string())],
            "wire 2 must skip the reserved 2"
        );
        assert!(plan.skipped.iter().any(|(h, r)| *h == 1 && r == "fixed number kept"));
    }

    #[test]
    fn a_fixed_wire_with_no_number_is_reported_not_guessed() {
        let s = seq_settings();
        let mut pinned = wire(1, None);
        pinned.fixed = true;
        let plan = plan_sequential(&[pinned], &s);
        assert!(plan.assignments.is_empty());
        assert!(plan.skipped[0].1.contains("unfix it"));
    }

    #[test]
    fn unnumbered_wire_types_are_skipped() {
        let s = seq_settings();
        let mut ground = wire(1, None);
        ground.numbered = false;
        let plan = plan_sequential(&[ground, wire(2, None)], &s);
        assert_eq!(plan.assignments, vec![(2, "1".to_string())]);
        assert!(plan.skipped[0].1.contains("not numbered"));
    }

    #[test]
    fn an_unchanged_wire_is_not_rewritten() {
        // Writing the same value would produce a spurious undo step and a
        // spurious "N assigned" count.
        let s = seq_settings();
        let mut w = wire(1, None);
        w.existing = "1".to_string();
        let plan = plan_sequential(&[w, wire(2, None)], &s);
        assert_eq!(plan.assignments, vec![(2, "2".to_string())]);
        assert_eq!(plan.unchanged(), 1);
    }

    #[test]
    fn empty_input_plans_nothing() {
        let plan = plan_sequential(&[], &seq_settings());
        assert!(plan.assignments.is_empty());
        assert!(plan.skipped.is_empty());
    }

    // ── reference ────────────────────────────────────────────────────────

    #[test]
    fn reference_numbers_come_from_the_rung() {
        let s = ref_settings();
        let plan = plan_reference(&[wire(1, Some("100")), wire(2, Some("101"))], &s);
        assert_eq!(
            plan.assignments,
            vec![(1, "100".to_string()), (2, "101".to_string())]
        );
    }

    #[test]
    fn several_wires_on_one_rung_get_suffixes() {
        let s = ref_settings();
        let plan = plan_reference(
            &[wire(1, Some("100")), wire(2, Some("100")), wire(3, Some("100"))],
            &s,
        );
        assert_eq!(
            plan.assignments,
            vec![
                (1, "100".to_string()),
                (2, "100A".to_string()),
                (3, "100B".to_string())
            ]
        );
    }

    #[test]
    fn suffix_resets_per_reference() {
        let s = ref_settings();
        let plan = plan_reference(
            &[wire(1, Some("100")), wire(2, Some("100")), wire(3, Some("101"))],
            &s,
        );
        assert_eq!(plan.assignments[2].1, "101", "a new rung starts unsuffixed");
    }

    #[test]
    fn the_configured_suffix_list_is_used() {
        let mut s = ref_settings();
        s.tag_suffixes = "X,Y".to_string();
        let plan = plan_reference(&[wire(1, Some("100")), wire(2, Some("100"))], &s);
        assert_eq!(plan.assignments[1].1, "100X");
    }

    #[test]
    fn more_wires_than_suffixes_falls_back_to_the_alphabet() {
        // Running out of configured suffixes must not produce duplicates.
        let mut s = ref_settings();
        s.tag_suffixes = "X".to_string();
        let plan = plan_reference(
            &[wire(1, Some("100")), wire(2, Some("100")), wire(3, Some("100"))],
            &s,
        );
        let nums: Vec<&str> = plan.assignments.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(nums, ["100", "100X", "100A"]);
        let uniq: BTreeSet<&str> = nums.iter().copied().collect();
        assert_eq!(uniq.len(), nums.len(), "suffixes must stay unique");
    }

    #[test]
    fn a_wire_off_the_ladder_is_reported_in_reference_mode() {
        let s = ref_settings();
        let plan = plan_reference(&[wire(1, None), wire(2, Some("100"))], &s);
        assert_eq!(plan.assignments, vec![(2, "100".to_string())]);
        assert!(plan.skipped[0].1.contains("no line reference"));
    }

    #[test]
    fn a_non_numeric_reference_still_numbers() {
        // Sheet-prefixed references are ordinary; the format's %N takes text.
        let s = ref_settings();
        let plan = plan_reference(&[wire(1, Some("2A00"))], &s);
        assert_eq!(plan.assignments[0].1, "2A00");
    }

    #[test]
    fn reference_mode_respects_the_number_format() {
        let mut s = ref_settings();
        s.wireno_format = "W%N%X".to_string();
        let plan = plan_reference(&[wire(1, Some("100")), wire(2, Some("100"))], &s);
        assert_eq!(plan.assignments[0].1, "W100");
        assert_eq!(plan.assignments[1].1, "W100A");
    }

    #[test]
    fn fixed_references_are_not_reissued() {
        let s = ref_settings();
        let mut pinned = wire(1, Some("100"));
        pinned.fixed = true;
        pinned.existing = "100".to_string();
        let plan = plan_reference(&[pinned, wire(2, Some("100"))], &s);
        // Wire 2 would want bare `100`, which is taken, so it is reported.
        assert!(plan.assignments.is_empty());
        assert!(plan.skipped.iter().any(|(h, _)| *h == 2));
    }

    #[test]
    fn plan_dispatches_on_the_settings_mode() {
        let seq = plan(&[wire(1, Some("100"))], &seq_settings());
        assert_eq!(seq.mode, WireNoMode::Sequential);
        assert_eq!(seq.assignments[0].1, "1", "sequential ignores the rung");

        let r = plan(&[wire(1, Some("100"))], &ref_settings());
        assert_eq!(r.mode, WireNoMode::Reference);
        assert_eq!(r.assignments[0].1, "100");
    }

    // ── helpers ──────────────────────────────────────────────────────────

    #[test]
    fn drawing_order_is_top_to_bottom_then_left_to_right() {
        let ordered = drawing_order(&[
            (1, [50.0, 10.0]),  // bottom
            (2, [10.0, 100.0]), // top row, left
            (3, [90.0, 100.0]), // top row, right
            (4, [10.0, 50.0]),  // middle
        ]);
        assert_eq!(ordered, vec![2, 3, 4, 1]);
    }

    #[test]
    fn drawing_order_is_stable_for_identical_positions() {
        let a = drawing_order(&[(1, [0.0, 0.0]), (2, [0.0, 0.0])]);
        let b = drawing_order(&[(1, [0.0, 0.0]), (2, [0.0, 0.0])]);
        assert_eq!(a, b, "the same input must not reorder between runs");
    }

    #[test]
    fn leading_int_reads_a_suffixed_number() {
        assert_eq!(leading_int("100A"), Some(100));
        assert_eq!(leading_int("W7"), Some(7));
        assert_eq!(leading_int("ABC"), None);
    }

    #[test]
    fn summary_mentions_mode_and_counts() {
        let s = seq_settings();
        let plan = plan_sequential(&[wire(1, None)], &s);
        let sum = plan.summary();
        assert!(sum.contains("1 wire number"), "{sum}");
        assert!(sum.contains("[S mode]"), "{sum}");
    }
}
