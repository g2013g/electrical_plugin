//! `EL_PROJECT` — the cross-sheet register (`el-project.json`).
//!
//! One drawing is one sheet, so everything that spans sheets lives in this
//! sidecar: the drawing list with each sheet's number, and the signal register
//! that lets a destination arrow on sheet 02 label itself with the sheet its
//! source lives on — without that sheet being open.
//!
//! The file lives **beside the drawings**, so a project is a folder that moves
//! as a unit. It is only ever created by an explicit `EL_PROJECT NEW`: a
//! drawing without a project is the normal state of a fresh sheet, not a
//! defect, and nothing implicitly creates one.

use ocs_plugin_api::host::HostApi;

use el_core::project::{Project, ProjectDrawing, PROJECT_FILE_NAME};

use crate::cmds::common::Args;
use crate::library::drawing_dir;

/// Load the project sidecar beside the drawing, if there is one.
///
/// `Ok(None)` = no project file, the normal state; `Err` = a file is there
/// but unreadable, which must be reported rather than silently ignored.
pub fn load(host: &dyn HostApi) -> Result<Option<Project>, String> {
    let Some(dir) = drawing_dir(host) else {
        return Ok(None);
    };
    let path = dir.join(PROJECT_FILE_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Project::from_json(&text).map(Some)
}

/// Write the project sidecar beside the drawing.
///
/// An unsaved drawing has no folder to put it in, which is a hard stop: a
/// project file in an incidental working directory would strand the project.
pub fn save(host: &dyn HostApi, project: &Project) -> Result<std::path::PathBuf, String> {
    let Some(dir) = drawing_dir(host) else {
        return Err(
            "the drawing has not been saved yet — save it into the project folder first"
                .to_string(),
        );
    };
    let path = dir.join(PROJECT_FILE_NAME);
    std::fs::write(&path, project.to_json()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The path the project file would take, whether or not it exists.
pub fn path_of(host: &dyn HostApi) -> Option<std::path::PathBuf> {
    drawing_dir(host).map(|dir| dir.join(PROJECT_FILE_NAME))
}

/// This drawing's identity, from its settings record.
fn this_drawing(_host: &dyn HostApi, idx: &el_core::ElIndex) -> ProjectDrawing {
    let s = idx.settings_or_default();
    ProjectDrawing {
        sheet: s.sheet.clone(),
        name: s.drawing.clone(),
        path: String::new(),
    }
}

/// Run `EL_PROJECT [NEW <name> | SET [SHEET=n NAME=x ADD=path] | SYNC | PATH]`.
///
/// Bare prints the register. The drawing's own row comes from its settings
/// (`EL_SETTINGS SHEET= DRAWING=`), so a sheet renumbering is a settings edit
/// followed by `EL_PROJECT SET`, not a hand-edit of the JSON.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());
    let action = a.first_word().map(|w| w.to_ascii_uppercase());

    let existing = match load(host) {
        Ok(p) => p,
        Err(e) => {
            host.push_error(&format!("EL_PROJECT: the project file is unreadable: {e}"));
            return;
        }
    };

    match action.as_deref() {
        None | Some("SHOW") | Some("LIST") => show(host, existing.as_ref()),
        Some("NEW") => new_project(host, &idx, &a, existing),
        Some("SET") => set(host, &idx, &a, existing),
        Some("SYNC") => sync(host, &idx, existing),
        Some("PATH") => match path_of(host) {
            Some(p) => host.push_output(&format!(
                "EL_PROJECT: {}{}",
                p.display(),
                if p.exists() { "" } else { " (no file there yet)" }
            )),
            None => host.push_error(
                "EL_PROJECT: the drawing has not been saved, so there is no folder for a project",
            ),
        },
        Some(other) => host.push_error(&format!(
            "EL_PROJECT: unknown action {other:?} — try NEW, SET, SYNC, PATH, or bare to show"
        )),
    }
}

fn show(host: &mut dyn HostApi, project: Option<&Project>) {
    let Some(p) = project else {
        host.push_output(&format!(
            "EL_PROJECT: no project — run EL_PROJECT NEW <name> (file would be beside the drawing)"
        ));
        return;
    };
    host.push_output(&format!(
        "EL_PROJECT {:?} — {} drawing(s), {} registered signal(s)",
        p.name,
        p.drawings.len(),
        p.signals.len()
    ));
    for d in &p.drawings {
        host.push_output(&format!(
            "  sheet {} {}{}",
            dash(&d.sheet),
            dash(&d.name),
            if d.path.is_empty() {
                String::new()
            } else {
                format!(" ({})", d.path)
            }
        ));
    }
    for s in &p.signals {
        host.push_output(&format!(
            "  signal {} — source {} dest {}",
            dash(&s.name),
            dash(&s.source_sheet),
            if s.dest_sheets.is_empty() {
                "—".to_string()
            } else {
                s.dest_sheets.join(",")
            }
        ));
    }
}

fn dash(s: &str) -> String {
    if s.is_empty() {
        "—".to_string()
    } else {
        s.to_string()
    }
}

fn new_project(
    host: &mut dyn HostApi,
    idx: &el_core::ElIndex,
    a: &Args,
    existing: Option<Project>,
) {
    if let Some(p) = &existing {
        host.push_error(&format!(
            "EL_PROJECT: a project {:?} already exists — EL_PROJECT SET to change it",
            p.name
        ));
        return;
    }
    let Some(name) = a.free.get(1).map(|s| s.to_string()) else {
        host.push_error("EL_PROJECT: name it — EL_PROJECT NEW Panel-01");
        return;
    };
    let mut project = Project::new(&name);
    project.upsert_drawing(this_drawing(host, idx));
    match save(host, &project) {
        Ok(p) => host.push_output(&format!(
            "EL_PROJECT: {:?} created at {} (this drawing is sheet {})",
            name,
            p.display(),
            dash(&idx.settings_or_default().sheet)
        )),
        Err(e) => host.push_error(&format!("EL_PROJECT: {e}")),
    }
}

fn set(host: &mut dyn HostApi, idx: &el_core::ElIndex, a: &Args, existing: Option<Project>) {
    let Some(mut project) = existing else {
        host.push_error("EL_PROJECT: no project to set — EL_PROJECT NEW first");
        return;
    };
    // Registering another drawing is ADD=<path> with its own SHEET=/NAME=;
    // without ADD= the keys describe *this* drawing, taken from its settings.
    let mut drawing = match a.get("ADD") {
        Some(path) => ProjectDrawing {
            sheet: a.get("SHEET").unwrap_or("").to_string(),
            name: a.get("NAME").unwrap_or("").to_string(),
            path: path.to_string(),
        },
        None => {
            let mut d = this_drawing(host, idx);
            if let Some(s) = a.get("SHEET") {
                d.sheet = s.to_string();
            }
            if let Some(n) = a.get("NAME") {
                d.name = n.to_string();
            }
            d
        }
    };
    if drawing.sheet.is_empty() {
        host.push_error("EL_PROJECT: a drawing row needs a sheet — EL_SETTINGS SHEET=01 or EL_PROJECT SET SHEET=01");
        return;
    }
    project.upsert_drawing(drawing);
    match save(host, &project) {
        Ok(p) => host.push_output(&format!("EL_PROJECT: drawing list updated ({})", p.display())),
        Err(e) => host.push_error(&format!("EL_PROJECT: {e}")),
    }
}

/// Fold this drawing's arrows into the register and refresh every arrow's
/// label, which is where the pairing becomes visible on the sheet.
fn sync(host: &mut dyn HostApi, idx: &el_core::ElIndex, existing: Option<Project>) {
    let Some(mut project) = existing else {
        host.push_error("EL_PROJECT: no project to sync with — EL_PROJECT NEW first");
        return;
    };
    project.upsert_drawing(this_drawing(host, idx));
    let sheet = idx.settings_or_default().sheet.clone();
    if sheet.is_empty() {
        host.push_error(
            "EL_PROJECT: this drawing has no sheet number — EL_SETTINGS SHEET=01 first, \
             or a paired arrow cannot say where it goes",
        );
        return;
    }
    let count = idx.signals.len();
    for s in idx.signals.values() {
        if s.rec.name.is_empty() {
            continue;
        }
        project.sync_signal(&sheet, &s.rec.name, s.rec.direction == el_core::model::SignalDirection::Source);
    }
    match save(host, &project) {
        Ok(p) => host.push_output(&format!(
            "EL_PROJECT: {count} arrow(s) from sheet {sheet} registered ({})",
            p.display()
        )),
        Err(e) => {
            host.push_error(&format!("EL_PROJECT: {e}"));
            return;
        }
    }

    // The register just learned where things pair; the arrows are a render, so
    // rebuild them with the fresh notes.
    if count > 0 {
        crate::cmds::signal::redraw_all(host, idx);
    }
}
