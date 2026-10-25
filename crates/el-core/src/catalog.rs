//! The parts catalogue.
//!
//! A bill of materials is only as good as the part data behind it, and ACADE's
//! answer is a per-project Microsoft Access database. That is not portable to a
//! plugin, so the catalogue here is a JSON file — and, deliberately, the *same*
//! file as the symbol library, so one artifact describes both the shapes and the
//! parts a project uses.
//!
//! What the model has to get right is the distinction ACADE draws between a part
//! and a *catalog assignment*: several devices can share a manufacturer and part
//! number while differing in ratings, which is why the catalogue indexes by
//! `(manufacturer, catalog)` and why a missing assignment is a report rather
//! than an error.

use serde::{Deserialize, Serialize};

/// One catalogue entry: a part a device can be assigned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartEntry {
    pub mfg: String,
    pub cat: String,
    /// The ref-des families this part may be used for. An empty list means any,
    /// which is what a generic terminal block or a length of wire wants.
    #[serde(default)]
    pub families: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub ratings: Vec<String>,
    /// The symbol to place for this part, when it implies one.
    #[serde(default)]
    pub symbol: String,
    /// Manufacturer's drawing reference, for a datasheet lookup.
    #[serde(default)]
    pub drawing: String,
    /// A per-unit price, for a costing column. Absent is not zero.
    #[serde(default)]
    pub price: Option<f64>,
}

impl PartEntry {
    pub fn new(mfg: &str, cat: &str, description: &str) -> Self {
        PartEntry {
            mfg: mfg.to_string(),
            cat: cat.to_string(),
            families: Vec::new(),
            description: description.to_string(),
            ratings: Vec::new(),
            symbol: String::new(),
            drawing: String::new(),
            price: None,
        }
    }

    /// A part key that a report and a BOM row are grouped by.
    ///
    /// Manufacturer *and* catalogue number, because two vendors' `3RT` are not
    /// the same part — grouping on the number alone would merge them.
    pub fn key(&self) -> String {
        format!("{}\u{0}{}", self.mfg, self.cat)
    }

    /// Does this part apply to a device of `family`? An unrestricted part
    /// applies to anything.
    pub fn suits_family(&self, family: &str) -> bool {
        self.families.is_empty()
            || self
                .families
                .iter()
                .any(|f| f.eq_ignore_ascii_case(family))
    }
}

/// A searchable set of parts.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub parts: Vec<PartEntry>,
}

impl Catalog {
    /// A small starter catalogue, so a drawing has something to resolve against
    /// before anyone authors one.
    ///
    /// Deliberately generic: this is a demonstration set, and inventing detailed
    /// ratings for real vendors' parts would be worse than useless in a report.
    pub fn sample() -> Self {
        let mut parts = Vec::new();
        let mut add = |mfg: &str, cat: &str, desc: &str, families: &[&str], ratings: &[&str]| {
            let mut p = PartEntry::new(mfg, cat, desc);
            p.families = families.iter().map(|s| s.to_string()).collect();
            p.ratings = ratings.iter().map(|s| s.to_string()).collect();
            parts.push(p);
        };

        add("Generic", "RLY-24DC-2CO", "Relay, 24 VDC coil, 2 changeover", &["K"], &["24VDC", "8A"]);
        add("Generic", "CONT-24DC-4NO", "Contactor, 24 VDC coil, 4 NO", &["K"], &["24VDC", "9A"]);
        add("Generic", "MCB-1P-C10", "Miniature circuit breaker, 1P, C10", &["Q"], &["230VAC", "10A"]);
        add("Generic", "FUSE-10x38-4A", "Fuse, 10×38 mm, 4 A", &["F"], &["500VAC", "4A"]);
        add("Generic", "OL-1-4A", "Thermal overload, 1–4 A", &["F"], &["4A"]);
        add("Generic", "XT-2.5", "Terminal block, screw, 2.5 mm²", &["X"], &["800V", "24A"]);
        add("Generic", "XT-2.5-PE", "Terminal block, protective earth, 2.5 mm²", &["X"], &["800V"]);
        add("Generic", "MTR-0.75-3PH", "Motor, 0.75 kW, three-phase", &["M"], &["400VAC", "0.75kW"]);
        add("Generic", "LAMP-24-GN", "Indicator lamp, 24 V, green", &["H"], &["24VDC"]);
        add("Generic", "PB-NO-GN", "Pushbutton, NO, green", &["S"], &["24VDC"]);
        add("Generic", "PSU-24-5", "Power supply, 24 VDC 5 A", &["G"], &["230VAC", "24VDC"]);
        add("Generic", "CBL-4x1.5", "Cable, 4 × 1.5 mm²", &[], &["600V"]);
        Catalog { parts }
    }

    pub fn len(&self) -> usize {
        self.parts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// The part for a manufacturer and catalogue number.
    pub fn get(&self, mfg: &str, cat: &str) -> Option<&PartEntry> {
        self.parts.iter().find(|p| {
            p.cat.eq_ignore_ascii_case(cat) && (mfg.is_empty() || p.mfg.eq_ignore_ascii_case(mfg))
        })
    }

    /// The first part matching a catalogue number, whatever the manufacturer.
    pub fn by_cat(&self, cat: &str) -> Option<&PartEntry> {
        self.parts
            .iter()
            .find(|p| p.cat.eq_ignore_ascii_case(cat))
    }

    /// Parts that may be used for a family.
    ///
    /// Parts catalogued *for* the family come first, then unrestricted ones —
    /// both belong in the list, because a generic terminal block applies
    /// anywhere, but the order is what stops a lookup for a terminal from
    /// leading with a length of cable.
    pub fn for_family(&self, family: &str) -> Vec<&PartEntry> {
        let mut explicit: Vec<&PartEntry> = self
            .parts
            .iter()
            .filter(|p| {
                !p.families.is_empty()
                    && p.families.iter().any(|f| f.eq_ignore_ascii_case(family))
            })
            .collect();
        explicit.sort_by(|a, b| a.cat.cmp(&b.cat));
        let mut generic: Vec<&PartEntry> = self
            .parts
            .iter()
            .filter(|p| p.families.is_empty())
            .collect();
        generic.sort_by(|a, b| a.cat.cmp(&b.cat));
        explicit.extend(generic);
        explicit
    }

    /// Free-text search over manufacturer, catalogue number and description.
    ///
    /// Every whitespace-separated term must match somewhere, so `24v relay`
    /// narrows rather than widens — which is what a lookup box should do.
    pub fn search(&self, query: &str) -> Vec<&PartEntry> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|t| t.to_ascii_lowercase())
            .collect();
        if terms.is_empty() {
            return self.parts.iter().collect();
        }
        self.parts
            .iter()
            .filter(|p| {
                let hay = format!(
                    "{} {} {} {}",
                    p.mfg,
                    p.cat,
                    p.description,
                    p.ratings.join(" ")
                )
                .to_ascii_lowercase();
                terms.iter().all(|t| hay.contains(t))
            })
            .collect()
    }

    /// Insert or replace by `(mfg, cat)`, returning whether it replaced.
    pub fn upsert(&mut self, part: PartEntry) -> bool {
        match self
            .parts
            .iter_mut()
            .find(|p| p.key() == part.key() && !p.cat.is_empty())
        {
            Some(slot) => {
                *slot = part;
                true
            }
            None => {
                self.parts.push(part);
                false
            }
        }
    }

    pub fn remove(&mut self, mfg: &str, cat: &str) -> bool {
        let before = self.parts.len();
        self.parts
            .retain(|p| !(p.cat.eq_ignore_ascii_case(cat) && p.mfg.eq_ignore_ascii_case(mfg)));
        self.parts.len() != before
    }

    /// Distinct manufacturers, sorted — the first thing a lookup dialog lists.
    pub fn manufacturers(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .parts
            .iter()
            .map(|p| p.mfg.clone())
            .filter(|m| !m.is_empty())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<Catalog>(text).map_err(|e| format!("{e}"))
    }
}

/// A device's catalogue assignment, as a report needs to see it.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    /// What the device is (a tag, a terminal designator, a cable tag).
    pub item: String,
    pub mfg: String,
    pub cat: String,
    /// The family it was assigned for, so a mismatch can be reported.
    pub family: String,
}

impl Assignment {
    /// Is the device missing its catalogue data?
    ///
    /// Both fields are required: a part number with no manufacturer cannot be
    /// ordered, which is the whole point of the BOM.
    pub fn is_incomplete(&self) -> bool {
        self.cat.trim().is_empty() || self.mfg.trim().is_empty()
    }
}

/// Every assignment in a drawing that is missing catalogue data.
pub fn missing_catalog(assignments: &[Assignment]) -> Vec<&Assignment> {
    let mut out: Vec<&Assignment> = assignments
        .iter()
        .filter(|a| a.is_incomplete())
        .collect();
    out.sort_by(|a, b| crate::refdes_cmp(&a.item, &b.item));
    out
}

/// Assignments whose part exists but is not catalogued for their family.
///
/// Worth reporting separately from "no catalogue data": the part is there, it is
/// simply the wrong kind for the device, which is a different mistake with a
/// different fix.
pub fn wrong_family(assignments: &[Assignment], catalog: &Catalog) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for a in assignments {
        if a.is_incomplete() || a.family.is_empty() {
            continue;
        }
        if let Some(part) = catalog.get(&a.mfg, &a.cat) {
            if !part.suits_family(&a.family) {
                out.push((a.item.clone(), format!("{}/{}", a.mfg, a.cat)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_catalogue_is_usable() {
        let c = Catalog::sample();
        assert!(c.len() >= 10);
        assert!(c.get("Generic", "XT-2.5").is_some());
        assert!(c.get("Nobody", "XT-2.5").is_none());
    }

    #[test]
    fn a_part_key_separates_vendors() {
        // Two vendors' identically numbered parts are different parts; grouping
        // a BOM on the number alone would merge them.
        let a = PartEntry::new("VendorA", "3RT", "x");
        let b = PartEntry::new("VendorB", "3RT", "x");
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn search_requires_every_term() {
        let c = Catalog::sample();
        assert!(!c.search("relay").is_empty());
        assert!(!c.search("relay 24vdc").is_empty(), "both terms appear");
        assert!(c.search("relay 400vac").is_empty(), "the second term does not");
        assert_eq!(c.search("").len(), c.len(), "an empty query lists everything");
    }

    #[test]
    fn search_is_case_insensitive_and_covers_ratings() {
        let c = Catalog::sample();
        assert!(!c.search("MCB-1p").is_empty());
        assert!(!c.search("10a").is_empty(), "ratings are searchable");
    }

    #[test]
    fn family_lookup_respects_restrictions() {
        let c = Catalog::sample();
        let terminals = c.for_family("X");
        // The two terminals catalogued for X, then the unrestricted cable.
        assert_eq!(terminals.len(), 3);
        assert!(terminals[0].families.contains(&"X".to_string()));
        assert!(terminals[1].families.contains(&"X".to_string()));
        assert!(
            terminals[2].families.is_empty(),
            "an unrestricted part is offered last, not first"
        );
        // A part with no family restriction suits anything.
        let cable = c.get("Generic", "CBL-4x1.5").unwrap();
        assert!(cable.suits_family("K"));
        assert!(cable.suits_family("anything"));
        assert!(c.for_family("K").len() >= 2);
    }

    #[test]
    fn upsert_and_remove() {
        let mut c = Catalog::sample();
        let n = c.len();
        let mut p = PartEntry::new("VendorA", "ZZ-1", "a new part");
        p.price = Some(12.5);
        assert!(!c.upsert(p.clone()), "a new key appends");
        assert_eq!(c.len(), n + 1);
        p.description = "revised".to_string();
        assert!(c.upsert(p), "an existing key replaces");
        assert_eq!(c.len(), n + 1);
        assert_eq!(c.get("VendorA", "ZZ-1").unwrap().description, "revised");
        assert!(c.remove("VendorA", "ZZ-1"));
        assert!(!c.remove("VendorA", "ZZ-1"));
    }

    #[test]
    fn manufacturers_are_deduped_and_sorted() {
        let c = Catalog::sample();
        let m = c.manufacturers();
        assert_eq!(m, vec!["Generic".to_string()]);
    }

    #[test]
    fn json_round_trips() {
        let c = Catalog::sample();
        let back = Catalog::from_json(&c.to_json()).unwrap();
        assert_eq!(back, c);
        assert!(Catalog::from_json("{ not json").is_err());
    }

    #[test]
    fn a_hand_written_catalogue_can_omit_everything_optional() {
        let text = r#"{"parts":[{"mfg":"V","cat":"C1"}]}"#;
        let c = Catalog::from_json(text).unwrap();
        let p = c.by_cat("C1").unwrap();
        assert!(p.description.is_empty());
        assert!(p.families.is_empty());
        assert_eq!(p.price, None, "an absent price is not a free part");
        assert!(p.suits_family("K"));
    }

    #[test]
    fn incompleteness_needs_both_fields() {
        // A part number with no manufacturer cannot be ordered.
        let a = Assignment {
            item: "K1".into(),
            mfg: "Siemens".into(),
            cat: String::new(),
            family: "K".into(),
        };
        assert!(a.is_incomplete());
        let b = Assignment {
            item: "K1".into(),
            mfg: String::new(),
            cat: "3RT".into(),
            family: "K".into(),
        };
        assert!(b.is_incomplete());
        let c = Assignment {
            item: "K1".into(),
            mfg: "Siemens".into(),
            cat: "3RT".into(),
            family: "K".into(),
        };
        assert!(!c.is_incomplete());
    }

    #[test]
    fn missing_catalog_listing_is_sorted() {
        let mk = |item: &str, cat: &str| Assignment {
            item: item.into(),
            mfg: if cat.is_empty() { String::new() } else { "M".into() },
            cat: cat.into(),
            family: "K".into(),
        };
        let list = vec![mk("K10", ""), mk("K2", "C"), mk("K1", "")];
        let missing = missing_catalog(&list);
        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0].item, "K1", "natural order, so K1 precedes K10");
        assert_eq!(missing[1].item, "K10");
    }

    #[test]
    fn a_part_catalogued_for_another_family_is_reported_separately() {
        // The part exists but is the wrong kind, which is a different mistake
        // from having no part at all.
        let c = Catalog::sample();
        let list = vec![
            Assignment {
                item: "K1".into(),
                mfg: "Generic".into(),
                cat: "MCB-1P-C10".into(),
                family: "K".into(),
            },
            Assignment {
                item: "Q1".into(),
                mfg: "Generic".into(),
                cat: "MCB-1P-C10".into(),
                family: "Q".into(),
            },
            Assignment {
                item: "K2".into(),
                mfg: String::new(),
                cat: String::new(),
                family: "K".into(),
            },
        ];
        let wrong = wrong_family(&list, &c);
        assert_eq!(wrong.len(), 1);
        assert_eq!(wrong[0].0, "K1", "a breaker assigned to a relay");
    }

    #[test]
    fn a_part_absent_from_the_catalogue_is_not_a_family_mismatch() {
        // It is simply unknown; the BOM will show it, and reporting it as the
        // wrong family too would double-count one problem.
        let c = Catalog::sample();
        let list = vec![Assignment {
            item: "K1".into(),
            mfg: "Nobody".into(),
            cat: "NOSUCH".into(),
            family: "K".into(),
        }];
        assert!(wrong_family(&list, &c).is_empty());
    }
}
