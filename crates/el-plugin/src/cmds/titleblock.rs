//! `EL_TITLEBLOCK` — the drawing frame and title table (textbook CH12).
//!
//! ACADE's title-block update pushes drawing properties into the block's
//! mapped attributes; the plugin's equivalent follows the pure-render rule:
//! sheet / drawing / installation / location / revision are **read from the
//! settings record** and the `OF n` sheet count from the project register, so
//! `EL_TITLEBLOCK UPDATE` after `EL_SETTINGS SHEET=02` is the whole workflow.
//! Only the placement and the fields a settings record cannot carry (title
//! text, drawn-by, date, approvals) live in the block's own record.

use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::geom::Pt;
use el_core::model::{LAYER_TITLEBLOCK, TitleBlockRec};
use el_core::ElIndex;

use crate::cmds::common::{remove_handles, Args};
use crate::geometry::{polyline_points, titleblock_entities};
use crate::hostops::DocEntityExt;

/// Run `EL_TITLEBLOCK [x,y] [TITLE= DRAWN= DATE= APPROVED= SHEETS=]
/// | UPDATE | CLEAR | SHOW`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());

    match a.first_word().map(|w| w.to_ascii_uppercase()).as_deref() {
        Some("UPDATE") | Some("REFRESH") => {
            update(host, &idx);
            return;
        }
        Some("CLEAR") => {
            clear(host, &idx);
            return;
        }
        Some("SHOW") | Some("LIST") => {
            show(host, &idx);
            return;
        }
        _ => {}
    }

    let existing = idx.title_block.clone();
    let mut rec = match &existing {
        Some(t) => t.rec.clone(),
        None => TitleBlockRec::new(&el_core::new_el_id()),
    };
    if let Some(v) = a.get("TITLE") {
        rec.title = v.to_string();
    }
    if let Some(v) = a.get("DRAWN") {
        rec.drawn_by = v.to_string();
    }
    if let Some(v) = a.get("DATE") {
        rec.date = v.to_string();
    }
    if let Some(v) = a.get("APPROVED") {
        rec.approved = v.to_string();
    }
    if let Some(v) = a.integer("SHEETS") {
        if v >= 0 {
            rec.sheet_of = v;
        }
    }
    if let Some(pt) = a
        .free
        .iter()
        .find_map(|t| crate::cmds::common::parse_coord_token(t))
    {
        rec.origin_x = pt[0];
        rec.origin_y = pt[1];
    }

    host.push_undo("EL_TITLEBLOCK");
    if let Some(old) = &existing {
        remove_handles(
            host,
            idx.handles_for_el_id(&old.rec.el_id),
            "EL_TITLEBLOCK (replacing)",
        );
    }
    render(host, &idx, &rec);
    host.bump_geometry();
    host.set_dirty();
    let s = idx.settings_or_default();
    host.push_output(&format!(
        "EL_TITLEBLOCK: frame + title table{} — sheet {}, fields read live from EL_SETTINGS \
         (UPDATE re-renders after a change)",
        if existing.is_some() { " (replaced)" } else { "" },
        if s.sheet.is_empty() { "—" } else { &s.sheet },
    ));
}

/// Re-render the block from the records as they now stand — the command that
/// runs after any settings or project change.
fn update(host: &mut dyn HostApi, idx: &ElIndex) {
    let Some(existing) = idx.title_block.as_ref() else {
        host.push_error("EL_TITLEBLOCK: nothing to update — place one first");
        return;
    };
    let rec = existing.rec.clone();
    host.push_undo("EL_TITLEBLOCK UPDATE");
    remove_handles(
        host,
        idx.handles_for_el_id(&rec.el_id),
        "EL_TITLEBLOCK UPDATE",
    );
    // The frame follows the drawing's extents, which may themselves have
    // grown since placement — so the re-render reads the live index.
    let live = crate::codec::build_index(host.document());
    render(host, &live, &rec);
    host.bump_geometry();
    host.set_dirty();
    host.push_output("EL_TITLEBLOCK: re-rendered from the current settings and drawing extents");
}

fn clear(host: &mut dyn HostApi, idx: &ElIndex) {
    let Some(existing) = idx.title_block.as_ref() else {
        host.push_output("EL_TITLEBLOCK: nothing to clear");
        return;
    };
    host.push_undo("EL_TITLEBLOCK CLEAR");
    remove_handles(
        host,
        idx.handles_for_el_id(&existing.rec.el_id),
        "EL_TITLEBLOCK CLEAR",
    );
    host.bump_geometry();
    host.set_dirty();
    host.push_output("EL_TITLEBLOCK: removed");
}

fn show(host: &mut dyn HostApi, idx: &ElIndex) {
    let s = idx.settings_or_default();
    let (t, sheets_of) = match idx.title_block.as_ref() {
        Some(t) => (t.rec.clone(), sheet_count(host, t.rec.sheet_of)),
        None => {
            host.push_output("EL_TITLEBLOCK: none placed — EL_TITLEBLOCK TITLE=Main+Line draws one");
            return;
        }
    };
    host.push_output("EL_TITLEBLOCK:");
    host.push_output(&format!("  title    : {}", dash(&t.title)));
    host.push_output(&format!("  drawing  : {}", dash(&s.drawing)));
    host.push_output(&format!(
        "  sheet    : {} of {sheets_of}",
        dash(&s.sheet)
    ));
    host.push_output(&format!("  revision : {}", s.revision));
    host.push_output(&format!("  inst/loc : {} / {}", dash(&s.installation), dash(&s.location)));
    host.push_output(&format!(
        "  drawn    : {} {}",
        dash(&t.drawn_by),
        dash(&t.date)
    ));
    host.push_output(&format!("  approved : {}", dash(&t.approved)));
}

fn dash(s: &str) -> String {
    if s.is_empty() { "—".to_string() } else { s.to_string() }
}

/// `OF n`: the project register's drawing count when there is one, else the
/// record's override, else 1.
fn sheet_count(host: &dyn HostApi, override_n: i32) -> i32 {
    if let Ok(Some(p)) = crate::cmds::project::load(host) {
        if !p.drawings.is_empty() {
            return p.drawings.len() as i32;
        }
    }
    if override_n >= 1 {
        return override_n;
    }
    1
}

/// The drawing's extents across every electrical object, `[min_x, min_y,
/// max_x, max_y]`, so the frame follows the drawing.
pub fn drawing_extent(host: &dyn HostApi, idx: &ElIndex) -> Option<[f64; 4]> {
    let mut pts: Vec<Pt> = Vec::new();
    for l in idx.ladders.values() {
        let (x0, y0) = (l.rec.origin_x, l.rec.origin_y);
        pts.push([x0, y0]);
        pts.push([
            x0 + l.rec.width_mm,
            y0 - (l.rec.rung_count.max(1) as f64) * l.rec.rung_spacing_mm,
        ]);
    }
    for c in idx.components.values() {
        pts.push(c.rec.origin());
        pts.push([c.rec.origin_x, c.rec.origin_y + 6.0]);
    }
    for t in idx.terminals.values() {
        pts.push([t.rec.origin_x - 8.0, t.rec.origin_y]);
        pts.push([t.rec.origin_x + 8.0, t.rec.origin_y]);
    }
    for m in idx.plcs.values() {
        let [w, h] = m.rec.body_mm();
        pts.push([m.rec.origin_x - w / 2.0, m.rec.origin_y - h / 2.0]);
        pts.push([m.rec.origin_x + w / 2.0, m.rec.origin_y + h / 2.0]);
    }
    for f in idx.footprints.values() {
        pts.push([f.rec.origin_x, f.rec.origin_y]);
        pts.push([f.rec.origin_x + f.rec.width_mm, f.rec.origin_y + f.rec.height_mm]);
    }
    // Wire geometry lives in the entities; read both ends of every run.
    for w in idx.wires.values() {
        if let Some(entity) = host.document().get_entity_owned(
            ocs_plugin_api::host::codec::Handle::new(w.handle),
        ) {
            if let Some(p) = polyline_points(&entity) {
                for q in p {
                    pts.push([q[0], q[1]]);
                }
            }
        }
    }
    if pts.is_empty() {
        return None;
    }
    Some(el_core::geom::bbox(&pts))
}

/// Draw the block: frame from the extents (or a default A4 landscape when
/// the drawing is empty), title table bottom-right.
fn render(host: &mut dyn HostApi, idx: &ElIndex, rec: &TitleBlockRec) {
    let settings = idx.settings_or_default();
    let extent = drawing_extent(host, idx).unwrap_or([0.0, 0.0, 297.0, 210.0]);
    let sheets = sheet_count(host, rec.sheet_of);
    let record = ElRecord::encode_titleblock(rec);
    let parts = titleblock_entities(&record, rec, &settings, extent, sheets);
    host.add_entities(parts);
}
