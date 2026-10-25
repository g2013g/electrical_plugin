//! `EL_STRIP` — the drawn strip diagram, the visual half of the terminal
//! strip editor.
//!
//! One box per terminal with its number inside, the wires that reach it
//! above, its partition group below — jumper bridges arcing between bridged
//! cells and doubled lines where a partition divides. Like every drawn layer
//! here it is a **render**: the plot record carries only which strip it shows
//! and where it sits, so re-drawing a strip replaces its diagram and the
//! drawing can never disagree with the records it renders.

use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::StripPlotRec;
use el_core::ElIndex;

use crate::cmds::common::{remove_handles, Args};
use crate::geometry::strip_plot_entities;
use crate::hostops::parse_handle;

/// Run `EL_STRIP [<strip>] [x,y] | CLEAR [<strip>|ALL] | LIST`.
///
/// Bare or bare-with-coordinate draws the strip when the drawing has exactly
/// one, which is the common case; `EL_STRIP LIST` says what is drawn.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());

    let clearing = a.free.iter().any(|t| t.eq_ignore_ascii_case("CLEAR"));
    let listing = a.free.iter().any(|t| t.eq_ignore_ascii_case("LIST"));
    a.free
        .retain(|t| !t.eq_ignore_ascii_case("CLEAR") && !t.eq_ignore_ascii_case("LIST"));

    if listing {
        list_plots(host, &idx);
        return;
    }

    // Which strip: the named one, or the only one there is.
    let named = a.free.first().cloned();
    let strip = match &named {
        Some(s) => s.clone(),
        None => match single_strip(&idx) {
            Some(s) => s,
            None => {
                host.push_error(
                    "EL_STRIP: name the strip — EL_STRIP X1 40,80 (or CLEAR, LIST)",
                );
                return;
            }
        },
    };

    if clearing {
        clear_plots(host, &idx, named.as_deref());
        return;
    }

    if !idx
        .terminals
        .values()
        .any(|t| t.rec.tag_strip.eq_ignore_ascii_case(&strip))
    {
        host.push_error(&format!(
            "EL_STRIP: no terminals on strip {strip:?} — EL_TERMINAL places them"
        ));
        return;
    }

    // Where: an explicit point, or to the right of the strip's drawn extent —
    // the diagram is a schedule of the strip, so it reads beside it.
    let at = match a.first_coord() {
        Some(c) => [c[0], c[1], 0.0],
        None => {
            let mut max_x = f64::NEG_INFINITY;
            let mut max_y = f64::NEG_INFINITY;
            for t in idx.terminals.values() {
                if t.rec.tag_strip.eq_ignore_ascii_case(&strip) {
                    max_x = max_x.max(t.rec.origin_x);
                    max_y = max_y.max(t.rec.origin_y);
                }
            }
            if max_x.is_infinite() {
                [40.0, 40.0, 0.0]
            } else {
                [max_x + 25.0, max_y, 0.0]
            }
        }
    };

    let cells = build_cells(&idx, &strip);
    if cells.is_empty() {
        host.push_error(&format!("EL_STRIP: strip {strip:?} has no terminals"));
        return;
    }

    // One plot per strip: the previous diagram of this strip goes first.
    let old: Vec<u64> = idx
        .strip_plots
        .values()
        .filter(|sp| sp.rec.strip.eq_ignore_ascii_case(&strip))
        .flat_map(|sp| idx.handles_for_el_id(&sp.rec.el_id).to_vec())
        .collect();

    let mut rec = StripPlotRec::new(&el_core::new_el_id(), &strip);
    rec.origin_x = at[0];
    rec.origin_y = at[1];
    let record = ElRecord::encode_stripplot(&rec);

    host.push_undo("EL_STRIP");
    if !old.is_empty() {
        remove_handles(host, &old, "EL_STRIP (replacing the diagram)");
    }
    let parts = strip_plot_entities(&record, &cells, at);
    let n = parts.len();
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_STRIP: {strip} drawn as {} cell(s), {} entity(ies){}",
        cells.len(),
        n,
        if old.is_empty() {
            String::new()
        } else {
            format!(" (previous diagram replaced)")
        }
    ));
}

fn single_strip(idx: &ElIndex) -> Option<String> {
    let mut strips: Vec<String> = idx
        .terminals
        .values()
        .map(|t| t.rec.tag_strip.clone())
        .filter(|s| !s.is_empty())
        .collect();
    strips.sort();
    strips.dedup();
    if strips.len() == 1 {
        strips.pop()
    } else {
        None
    }
}

/// The strip's cells, in drawn order, with everything the render needs
/// resolved from the records: jumpers, partitions, wires.
fn build_cells(idx: &ElIndex, strip: &str) -> Vec<crate::geometry::StripCell> {
    let mut terminals: Vec<&el_core::index::IndexedTerminal> = idx
        .terminals
        .values()
        .filter(|t| t.rec.tag_strip.eq_ignore_ascii_case(strip))
        .collect();
    // The order a strip is read: left to right, then top to bottom — the same
    // rule EL_TERMINAL NUMBER uses, so the diagram and the numbers agree.
    terminals.sort_by(|a, b| {
        a.rec.origin_x
            .partial_cmp(&b.rec.origin_x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.rec.origin_y.partial_cmp(&a.rec.origin_y).unwrap_or(std::cmp::Ordering::Equal))
    });

    // Wires per terminal designator.
    let mut wires: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for w in idx.wires.values() {
        for (node, _) in [(&w.rec.from_id, true), (&w.rec.to_id, false)] {
            if node.trim().is_empty() {
                continue;
            }
            let entry = wires.entry(node.clone()).or_default();
            if w.rec.wire_ref.is_empty() {
                if !entry.contains(&"?".to_string()) {
                    entry.push("?".to_string());
                }
            } else if !entry.contains(&w.rec.wire_ref) {
                entry.push(w.rec.wire_ref.clone());
            }
        }
    }

    let jumpers: Vec<&el_core::index::IndexedJumper> = idx
        .jumpers
        .values()
        .filter(|j| j.rec.strip.eq_ignore_ascii_case(strip))
        .collect();

    let mut cells: Vec<crate::geometry::StripCell> = Vec::new();
    for t in &terminals {
        cells.push(crate::geometry::StripCell {
            number: t.rec.number.clone(),
            wires: wires.get(&t.rec.designator()).cloned().unwrap_or_default(),
            partition: t.rec.partition.clone(),
            jumper_right: false,
            divider_right: false,
        });
    }
    for i in 0..cells.len() {
        if i + 1 >= cells.len() {
            break;
        }
        let (a, b) = (cells[i].number.clone(), cells[i + 1].number.clone());
        cells[i].jumper_right = jumpers
            .iter()
            .any(|j| j.rec.a_number == a && j.rec.b_number == b)
            || jumpers
                .iter()
                .any(|j| j.rec.a_number == b && j.rec.b_number == a);
        cells[i].divider_right = cells[i].partition != cells[i + 1].partition;
    }
    cells
}

fn list_plots(host: &mut dyn HostApi, idx: &ElIndex) {
    if idx.strip_plots.is_empty() {
        host.push_output("EL_STRIP: no diagrams drawn — EL_STRIP X1 40,80 draws one");
        return;
    }
    host.push_output(&format!(
        "EL_STRIP: {} diagram(s) drawn",
        idx.strip_plots.len()
    ));
    for sp in idx.strip_plots.values() {
        host.push_output(&format!(
            "  strip {} at ({}, {})",
            sp.rec.strip,
            el_core::fmt_mm(sp.rec.origin_x),
            el_core::fmt_mm(sp.rec.origin_y)
        ));
    }
}

fn clear_plots(host: &mut dyn HostApi, idx: &ElIndex, strip: Option<&str>) {
    let targets: Vec<&el_core::index::IndexedStripPlot> = match strip {
        None => idx.strip_plots.values().collect(),
        Some(s) => idx
            .strip_plots
            .values()
            .filter(|sp| sp.rec.strip.eq_ignore_ascii_case(s))
            .collect(),
    };
    if targets.is_empty() {
        host.push_output("EL_STRIP: nothing to clear");
        return;
    }
    host.push_undo("EL_STRIP CLEAR");
    for sp in &targets {
        remove_handles(host, idx.handles_for_el_id(&sp.rec.el_id), "EL_STRIP CLEAR");
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_STRIP: {} diagram(s) removed", targets.len()));
}

/// Resolve a strip plot by name or the 0x handle of any of its parts.
#[allow(dead_code)]
fn resolve_plot(idx: &ElIndex, token: &str) -> Option<el_core::index::IndexedStripPlot> {
    if let Some(sp) = idx
        .strip_plots
        .values()
        .find(|sp| sp.rec.strip.eq_ignore_ascii_case(token))
    {
        return Some(sp.clone());
    }
    parse_handle(token).and_then(|h| idx.strip_plot_of_handle(h.value())).cloned()
}
