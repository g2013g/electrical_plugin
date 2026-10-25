//! `EL_REPORT` — the deliverables: bill of materials, wire lists, terminal
//! plans, and the drawing's own audit.
//!
//! Every report is gated on the audit. A report that exports a confident but
//! wrong number is worse than one that refuses, so an *error* blocks and a
//! warning does not — and `FORCE` is how a user says they know.
//!
//! Output goes two ways, and both come from the same layout: a CSV file, and a
//! table drawn on the sheet. Making them one layout is what stops the printed
//! report and the file from disagreeing.

use ocs_plugin_api::host::HostApi;

use el_core::catalog::Catalog;
use el_core::report::{self, ReportKind};
use el_core::tables::TableLayout;

use crate::cmds::common::Args;
use crate::library;

/// Run `EL_REPORT [kind] [path.csv] [FORCE] [TABLE [x,y]] [HEIGHT=] | ALL | LIST`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);

    let word = a.first_word().map(|w| w.to_ascii_uppercase());
    if word.as_deref() == Some("LIST") {
        list_kinds(host);
        return;
    }

    // `ALL` writes every report as one CSV, which is what a scripted export
    // wants; a named kind is what a person wants.
    let kinds: Vec<ReportKind> = match word.as_deref() {
        Some("ALL") => {
            a.free.retain(|t| !t.eq_ignore_ascii_case("ALL"));
            ReportKind::ALL.to_vec()
        }
        Some(w) => match ReportKind::parse(w) {
            Some(k) => {
                a.free.retain(|t| !t.eq_ignore_ascii_case(w));
                vec![k]
            }
            None => {
                host.push_error(&format!(
                    "EL_REPORT: {w:?} is not a report — try {}",
                    ReportKind::ALL
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                return;
            }
        },
        None => {
            host.push_error(
                "EL_REPORT: name a report — try EL_REPORT BOM, or EL_REPORT LIST to see them all",
            );
            return;
        }
    };

    let force = a.flag("FORCE").unwrap_or(false);
    let to_table = a.flag("TABLE").unwrap_or(false);

    let idx = crate::codec::build_index(host.document());
    if idx.components.is_empty()
        && idx.wires.is_empty()
        && idx.terminals.is_empty()
        && idx.cables.is_empty()
        && idx.connectors.is_empty()
    {
        host.push_error("EL_REPORT: this drawing has nothing to report yet");
        return;
    }

    // The catalogue augments the audit: an unknown part is worth knowing about
    // before a BOM is issued.
    let catalog: Option<Catalog> = library::load(host).ok().map(|l| {
        if l.catalog.is_empty() {
            Catalog::sample()
        } else {
            l.catalog
        }
    });
    let settings = idx.settings_or_default();
    let issues = el_core::run_drc(&idx, catalog.as_ref());

    // Print the findings either way: a report issued *despite* errors should
    // still say what they were.
    for issue in el_core::drc::errors(&issues) {
        host.push_error(&format!("  [{}] {}: {}", issue.severity.as_str(), issue.code, issue.message));
    }
    for issue in el_core::drc::warnings(&issues).iter().take(20) {
        host.push_info(&format!("  [{}] {}: {}", issue.severity.as_str(), issue.code, issue.message));
    }

    if !el_core::drc::export_allowed(&issues) && !force {
        host.push_error(&format!(
            "EL_REPORT: BLOCKED — {}. Fix the errors, or pass FORCE to export anyway",
            el_core::drc::summary(&issues)
        ));
        return;
    }
    if !issues.is_empty() {
        host.push_info(&format!("  ({})", el_core::drc::summary(&issues)));
    }

    // ── build and emit ───────────────────────────────────────────────────
    // The drawing list is the one report that reads beyond this drawing: when
    // a project register exists it names every sheet and any cross-sheet
    // collisions, so the report and `EL_PROJECT SHOW` cannot disagree.
    let project = crate::cmds::project::load(host).ok().flatten();
    let build = |kind: ReportKind| -> TableLayout {
        let layout = report::build(kind, &idx, catalog.as_ref(), &settings);
        match (kind, &project) {
            (ReportKind::DrawingList, Some(p)) => with_project_rows(&layout, p),
            _ => layout,
        }
    };

    if to_table {
        let height = a.number("HEIGHT").unwrap_or(2.5);
        let at = a.first_coord().map(|c| [c[0], c[1], 0.0]);
        let mut layouts: Vec<TableLayout> = Vec::new();
        for kind in &kinds {
            layouts.push(build(*kind));
        }
        draw_tables(host, &layouts, at, height);
        return;
    }

    // One CSV, or one per report for `ALL`.
    let stamp = epoch();
    if kinds.len() == 1 {
        let layout = build(kinds[0]);
        let target = a
            .free
            .iter()
            .find(|t| !t.contains(',') && !t.contains('='))
            .cloned()
            .map(|raw| library::output(host, &raw))
            .unwrap_or_else(|| {
                library::output(host, &el_core::csv::file_name(kinds[0], &stamp))
            });
        write_csv(host, &layout, &target);
        return;
    }

    let dir = library::drawing_dir(host).unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut written = 0usize;
    for kind in &kinds {
        let layout = build(*kind);
        let target = dir.join(el_core::csv::file_name(*kind, &stamp));
        if write_csv_quiet(host, &layout, &target) {
            written += 1;
        }
    }
    host.push_output(&format!(
        "EL_REPORT ALL: {written} of {} report(s) written beside the drawing",
        kinds.len()
    ));
    let _ = &mut a;
}

fn write_csv(host: &mut dyn HostApi, layout: &TableLayout, target: &std::path::Path) {
    let csv = el_core::csv::to_csv(layout);    match std::fs::write(target, &csv) {
        Ok(()) => {
            host.push_output(&format!(
                "{} — {} row(s) written to {}",
                layout.title,
                layout.row_count(),
                target.display()
            ));
            // A table with no rows is a legitimate answer, but it is almost
            // always a surprise, so it is called out.
            if layout.is_empty() {
                host.push_info("  (the report has no rows — nothing matched)");
            }
        }
        Err(e) => host.push_error(&format!(
            "EL_REPORT: cannot write {}: {e}",
            target.display()
        )),
    }
}

fn write_csv_quiet(host: &mut dyn HostApi, layout: &TableLayout, target: &std::path::Path) -> bool {
    match std::fs::write(target, el_core::csv::to_csv(layout)) {
        Ok(()) => {
            host.push_output(&format!(
                "  {} ({} row(s))",
                target.display(),
                layout.row_count()
            ));
            true
        }
        Err(e) => {
            host.push_error(&format!("EL_REPORT: cannot write {}: {e}", target.display()));
            false
        }
    }
}

/// Draw report tables on the sheet, stacked top to bottom.
fn draw_tables(
    host: &mut dyn HostApi,
    layouts: &[TableLayout],
    at: Option<[f64; 3]>,
    height: f64,
) {
    if height <= 0.0 {
        host.push_error("EL_REPORT: HEIGHT must be positive");
        return;
    }
    // No coordinate means "beside the drawing", so a report never lands on top of
    // the wiring it describes.
    let start = at.unwrap_or_else(|| beside_the_drawing(host));
    let layer = crate::geometry::LAYER_TABLE;

    host.push_undo(&format!("EL_REPORT TABLE ({} table(s))", layouts.len()));
    crate::cmds::common::remove_entities_on_layer(host, layer);

    let mut y = start[1];
    let mut total = 0usize;
    for layout in layouts {
        let parts = crate::geometry::table_entities(layout, [start[0], y, start[2]], height, layer);
        total += parts.len();
        host.add_entities(parts);
        y -= crate::geometry::table_height(layout, height) + height * 2.0;
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_REPORT TABLE: {} table(s), {total} entit(ies) on {layer} at ({}, {})",
        layouts.len(),
        el_core::fmt_mm(start[0]),
        el_core::fmt_mm(start[1])
    ));
}

/// A point to the right of everything drawn, so a table never covers the work.
fn beside_the_drawing(host: &dyn HostApi) -> [f64; 3] {
    let mut max_x: f64 = 0.0;
    let mut max_y: f64 = 0.0;
    for e in host.document().entities() {
        if let Some(a) = crate::geometry::carrier_anchor(e) {
            max_x = max_x.max(a[0]);
            max_y = max_y.max(a[1]);
        }
        if let Some(pts) = crate::geometry::polyline_points(e) {
            for p in pts {
                max_x = max_x.max(p[0]);
                max_y = max_y.max(p[1]);
            }
        }
    }
    [max_x + 20.0, max_y, 0.0]
}

fn list_kinds(host: &mut dyn HostApi) {
    host.push_output("EL_REPORT — available reports");
    for k in ReportKind::ALL {
        host.push_output(&format!("  {:<16} {}", k.as_str(), k.title()));
    }
    host.push_output("  add a path to write elsewhere, TABLE to draw on the sheet, FORCE to export despite errors");
}

/// Seconds since the epoch, for a file name that does not collide.
fn epoch() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// Extend a drawing list with the project register's rows.
///
/// The sheet's own metadata stays first; the register adds what it knows —
/// every project drawing, the signal register's size, and the cross-sheet
/// collision verdict — in the same two columns, so the table reads as one.
fn with_project_rows(layout: &TableLayout, p: &el_core::project::Project) -> TableLayout {
    let mut rows = layout.rows.clone();
    if !p.name.is_empty() {
        rows.push(vec!["Project".to_string(), p.name.clone()]);
    }
    for d in &p.drawings {
        let mut line = format!("sheet {}", if d.sheet.is_empty() { "—" } else { &d.sheet });
        if !d.name.is_empty() {
            line.push_str(&format!(" — {}", d.name));
        }
        if !d.path.is_empty() {
            line.push_str(&format!(" ({})", d.path));
        }
        rows.push(vec!["Project drawing".to_string(), line]);
    }
    if !p.signals.is_empty() {
        rows.push(vec![
            "Project signals".to_string(),
            format!("{} registered", p.signals.len()),
        ]);
    }
    let collisions = p.tag_conflicts().len() + p.wire_number_conflicts().len();
    rows.push(vec![
        "Cross-sheet check".to_string(),
        match collisions {
            0 => "clean".to_string(),
            n => format!("{n} collision(s) — EL_PROJECT CHECK"),
        },
    ]);
    let headers: Vec<&str> = layout.headers.iter().map(|h| h.as_str()).collect();
    TableLayout::new(&layout.title, &headers, rows)
}
