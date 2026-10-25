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
}

impl Default for Project {
    fn default() -> Self {
        Project {
            version: 1,
            name: String::new(),
            drawings: Vec::new(),
            signals: Vec::new(),
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
}
