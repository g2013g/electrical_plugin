//! The project register — the multi-drawing sidecar (`el-project.json`).
//!
//! One drawing is one sheet in this plugin, so everything cross-sheet lives
//! here: which drawings make up the project, each one's sheet number, and the
//! **signal register** — which sheet a named signal's source arrow sits on and
//! which sheets receive it. A drawing reads the register to label its signal
//! arrows with the sheet they pair with, without the other drawing being open.
//!
//! The file travels **beside the drawings** (the drawing's own folder), the
//! same way the project-local symbol library does: a project is a folder you
//! copy as a unit. Like the library, it is only ever created on an explicit
//! command — a drawing without a project is the normal state of a fresh sheet,
//! not a defect.
//!
//! Host-free and serde-backed, like the symbol library.

use serde::{Deserialize, Serialize};

/// The sidecar's file name, beside the drawing.
pub const PROJECT_FILE_NAME: &str = "el-project.json";

/// One drawing in the project — one sheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDrawing {
    /// The sheet number (`"01"`), what `%S` expands to and what a paired
    /// signal arrow labels itself with.
    pub sheet: String,
    /// A short drawing name (`"MAIN"`), for the drawing list.
    #[serde(default)]
    pub name: String,
    /// The drawing file's path, as the project folder spells it. Informational
    /// for now: the host opens one document at a time.
    #[serde(default)]
    pub path: String,
}

/// The cross-sheet register of one named signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalEntry {
    pub name: String,
    /// The sheet the signal's source arrow sits on, once that sheet has synced.
    #[serde(default)]
    pub source_sheet: String,
    /// The sheets the signal arrives on.
    #[serde(default)]
    pub dest_sheets: Vec<String>,
}

/// One sheet's **extract**: the device tags and wire numbers its sync
/// registered, so cross-sheet collisions are visible without opening the
/// other drawing. This is the offline stand-in for ACADE's project-wide
/// update/retag — the register cannot read another drawing, but each sheet
/// can declare what it holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetExtract {
    pub sheet: String,
    /// Parent device tags on the sheet. A child contact shares its parent's
    /// tag by design, so only parents are extracted — counting children would
    /// report every correctly-linked contact as a collision.
    #[serde(default)]
    pub tags: Vec<String>,
    /// The sheet's wire numbers.
    #[serde(default)]
    pub wire_numbers: Vec<String>,
}

/// The project file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub drawings: Vec<ProjectDrawing>,
    #[serde(default)]
    pub signals: Vec<SignalEntry>,
    #[serde(default)]
    pub extracts: Vec<SheetExtract>,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            version: 1,
            name: String::new(),
            drawings: Vec::new(),
            signals: Vec::new(),
            extracts: Vec::new(),
        }
    }
}

impl Project {
    pub fn new(name: &str) -> Self {
        Project {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<Project>(text).map_err(|e| format!("{e}"))
    }

    /// This drawing's row, found by sheet number.
    pub fn drawing_by_sheet(&self, sheet: &str) -> Option<&ProjectDrawing> {
        self.drawings.iter().find(|d| d.sheet == sheet)
    }

    /// Insert or update one drawing's row, matched by sheet number.
    pub fn upsert_drawing(&mut self, drawing: ProjectDrawing) {
        match self.drawings.iter_mut().find(|d| d.sheet == drawing.sheet) {
            Some(d) => *d = drawing,
            None => self.drawings.push(drawing),
        }
    }

    /// The register entry for a signal name, if any sheet has synced it.
    pub fn signal(&self, name: &str) -> Option<&SignalEntry> {
        self.signals.iter().find(|s| s.name == name)
    }

    /// One sheet's extract, if it has synced.
    pub fn extract(&self, sheet: &str) -> Option<&SheetExtract> {
        self.extracts.iter().find(|e| e.sheet == sheet)
    }

    /// Fold one sheet's arrow into the register.
    ///
    /// A source sets the entry's source sheet; a destination is appended to
    /// the destination sheets (once — a sheet can hold one arrow per signal).
    /// Sync is an upsert, never a clear: a signal another sheet registered is
    /// not this sheet's to delete.
    pub fn sync_signal(&mut self, sheet: &str, name: &str, is_source: bool) {
        let entry = match self.signals.iter_mut().find(|s| s.name == name) {
            Some(s) => s,
            None => {
                self.signals.push(SignalEntry {
                    name: name.to_string(),
                    source_sheet: String::new(),
                    dest_sheets: Vec::new(),
                });
                self.signals.last_mut().unwrap()
            }
        };
        if is_source {
            entry.source_sheet = sheet.to_string();
        } else if !entry.dest_sheets.iter().any(|s| s == sheet) {
            entry.dest_sheets.push(sheet.to_string());
        }
    }

    /// Where this sheet pairs with `name`'s register entry, as the short
    /// annotation an arrow label carries — the other side's sheet, or nothing
    /// when the register does not know yet.
    pub fn pairing_note(&self, name: &str, is_source: bool) -> Option<String> {
        let entry = self.signal(name)?;
        if is_source {
            match entry.dest_sheets.first() {
                Some(s) => Some(format!("→ {s}")),
                None => None,
            }
        } else {
            match entry.source_sheet.is_empty() {
                true => None,
                false => Some(format!("← {}", entry.source_sheet)),
            }
        }
    }

    /// Insert or update one sheet's extract, matched by sheet number.
    pub fn sync_extract(&mut self, sheet: &str, tags: Vec<String>, wire_numbers: Vec<String>) {
        let extract = SheetExtract {
            sheet: sheet.to_string(),
            tags,
            wire_numbers,
        };
        match self.extracts.iter_mut().find(|e| e.sheet == sheet) {
            Some(e) => *e = extract,
            None => self.extracts.push(extract),
        }
    }

    /// Device tags carried by more than one sheet: `(tag, sheets)`, sorted.
    ///
    /// A collision is a real defect — the same relay numbered K1 on two sheets
    /// is two relays that every cross-reference and bill of materials will
    /// conflate. (An installation/location qualifier refines this in ACADE;
    /// the register flags the plain case first.)
    pub fn tag_conflicts(&self) -> Vec<(String, Vec<String>)> {
        Self::collisions(self.extracts.iter().map(|e| (&e.sheet, &e.tags)))
    }

    /// Wire numbers carried by more than one sheet, same shape as
    /// [`Self::tag_conflicts`].
    pub fn wire_number_conflicts(&self) -> Vec<(String, Vec<String>)> {
        Self::collisions(
            self.extracts
                .iter()
                .map(|e| (&e.sheet, &e.wire_numbers)),
        )
    }

    /// Values shared by more than one sheet, from `(sheet, values)` pairs.
    fn collisions<'a, I>(extracts: I) -> Vec<(String, Vec<String>)>
    where
        I: Iterator<Item = (&'a String, &'a Vec<String>)>,
    {
        let mut seen: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
        for (sheet, values) in extracts {
            for v in values {
                seen.entry(v.as_str()).or_default().push(sheet.clone());
            }
        }
        seen.into_iter()
            .filter(|(_, sheets)| sheets.len() > 1)
            .map(|(v, sheets)| (v.to_string(), sheets))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_keeps_everything() {
        let mut p = Project::new("Panel-01");
        p.upsert_drawing(ProjectDrawing {
            sheet: "01".to_string(),
            name: "MAIN".to_string(),
            path: "main.dwg".to_string(),
        });
        p.sync_signal("01", "STOP", true);
        p.sync_signal("02", "STOP", false);
        let text = p.to_json();
        let back = Project::from_json(&text).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn an_older_file_without_new_fields_opens() {
        // Additive evolution: a register written before a new optional field
        // must still open.
        let text = r#"{"version":1,"name":"P","drawings":[],"signals":[]}"#;
        let p = Project::from_json(text).unwrap();
        assert_eq!(p.name, "P");
    }

    #[test]
    fn upsert_matches_by_sheet_number() {
        let mut p = Project::new("P");
        p.upsert_drawing(ProjectDrawing {
            sheet: "01".into(),
            name: "OLD".into(),
            path: String::new(),
        });
        p.upsert_drawing(ProjectDrawing {
            sheet: "01".into(),
            name: "NEW".into(),
            path: "a.dwg".into(),
        });
        assert_eq!(p.drawings.len(), 1);
        assert_eq!(p.drawing_by_sheet("01").unwrap().name, "NEW");
    }

    #[test]
    fn sync_is_an_upsert_and_dests_accumulate_once() {
        let mut p = Project::new("P");
        p.sync_signal("01", "STOP", true);
        p.sync_signal("02", "STOP", false);
        p.sync_signal("02", "STOP", false);
        p.sync_signal("03", "STOP", false);
        assert_eq!(p.signals.len(), 1);
        assert_eq!(p.signal("STOP").unwrap().source_sheet, "01");
        assert_eq!(p.signal("STOP").unwrap().dest_sheets, vec!["02", "03"]);
    }

    #[test]
    fn pairing_notes_name_the_other_side_only_when_known() {
        let mut p = Project::new("P");
        p.sync_signal("01", "STOP", true);
        // The source is registered, the destination is not yet.
        assert_eq!(p.pairing_note("STOP", true), None);
        p.sync_signal("02", "STOP", false);
        assert_eq!(p.pairing_note("STOP", true).as_deref(), Some("→ 02"));
        assert_eq!(p.pairing_note("STOP", false).as_deref(), Some("← 01"));
        // An unknown signal pairs with nothing.
        assert_eq!(p.pairing_note("NOPE", false), None);
    }

    #[test]
    fn extracts_upsert_by_sheet_and_conflicts_name_every_sheet() {
        let mut p = Project::new("P");
        p.sync_extract("01", vec!["K1".into(), "Q1".into()], vec!["W1".into(), "W2".into()]);
        p.sync_extract("02", vec!["K2".into()], vec!["W3".into()]);
        // A re-sync replaces, not appends.
        p.sync_extract("02", vec!["K1".into()], vec!["W2".into(), "W4".into()]);
        assert_eq!(p.extracts.len(), 2);
        assert_eq!(p.extract("02").unwrap().tags, vec!["K1".to_string()]);

        // K1 and W2 now live on both sheets; everything else is clean.
        let tags = p.tag_conflicts();
        assert_eq!(tags, vec![("K1".to_string(), vec!["01".to_string(), "02".to_string()])]);
        let wires = p.wire_number_conflicts();
        assert_eq!(
            wires,
            vec![("W2".to_string(), vec!["01".to_string(), "02".to_string()])]
        );
    }

    #[test]
    fn an_older_register_without_extracts_opens_and_reports_clean() {
        let p = Project::from_json(r#"{"version":1,"name":"P","drawings":[],"signals":[]}"#).unwrap();
        assert!(p.tag_conflicts().is_empty());
        assert!(p.wire_number_conflicts().is_empty());
    }
}
