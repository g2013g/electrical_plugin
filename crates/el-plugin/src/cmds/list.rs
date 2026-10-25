//! `EL_LIST` — the drawing's inventory.
//!
//! A one-screen answer to "what is on this sheet?", because the first question a
//! drafter asks of an unfamiliar drawing is what it contains and the second is
//! whether it is sound. So the counts come first and the audit verdict last.
//!
//! Every count is derived from the records, so the dashboard cannot disagree with
//! the reports it summarises.

use ocs_plugin_api::host::HostApi;

use el_core::catalog::Catalog;
use el_core::drc;
use el_core::net::NetIndex;
use el_core::ElIndex;

use crate::cmds::common::Args;
use crate::library;

/// Run `EL_LIST [summary | devices | wires | terminals | cables | connectors |
/// nets | qc]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let a = Args::parse(args);
    let section = a
        .first_word()
        .map(|w| w.to_ascii_uppercase())
        .unwrap_or_else(|| "SUMMARY".to_string());

    let idx = crate::codec::build_index(host.document());
    let empty = idx.components.is_empty()
        && idx.wires.is_empty()
        && idx.terminals.is_empty()
        && idx.cables.is_empty()
        && idx.connectors.is_empty()
        && idx.signals.is_empty()
        && idx.footprints.is_empty()
        && idx.plcs.is_empty()
        && idx.ladders.is_empty();
    if empty {
        host.push_output("EL_LIST: this drawing has no electrical objects yet");
        return;
    }

    match section.as_str() {
        "SUMMARY" | "ALL" => summary(host, &idx),
        "DEVICES" | "COMPONENTS" => devices(host, &idx),
        "WIRES" => wires(host, &idx),
        "TERMINALS" => terminals(host, &idx),
        "CABLES" => cables(host, &idx),
        "CONNECTORS" => connectors(host, &idx),
        "NETS" => nets(host, &idx),
        "QC" | "AUDIT" => qc(host, &idx),
        other => host.push_error(&format!(
            "EL_LIST: unknown section {other:?} — try summary, devices, wires, terminals, cables, \
             connectors, nets or qc"
        )),
    }
}

fn summary(host: &mut dyn HostApi, idx: &ElIndex) {
    let nets = NetIndex::build(idx);
    let settings = idx.settings_or_default();

    host.push_output(&format!(
        "EL_LIST — sheet {} drawing {}",
        dash(&settings.sheet),
        dash(&settings.drawing)
    ));
    host.push_output(&format!(
        "  ladders       {} ({} rung reference(s))",
        idx.ladders.len(),
        idx.all_references().len()
    ));

    // A child contact is a device but not a part, so the two counts are separate:
    // a dashboard that conflated them would mislead about the bill of materials.
    let parents = idx.components.values().filter(|c| !c.rec.is_child).count();
    let children = idx.components.len() - parents;
    host.push_output(&format!("  devices       {parents} + {children} contact(s)"));
    let families = family_breakdown(idx);
    if !families.is_empty() {
        host.push_output(&format!("                {families}"));
    }

    let numbered = idx
        .wires
        .values()
        .filter(|w| !w.rec.wire_ref.is_empty())
        .count();
    host.push_output(&format!(
        "  wires         {} ({numbered} numbered, {} not)",
        idx.wires.len(),
        idx.wires.len() - numbered
    ));
    host.push_output(&format!(
        "  nets          {} ({} joining devices, {} stub(s))",
        nets.len(),
        nets.connected_nets().len(),
        nets.stubs().len()
    ));

    let strips = idx.strips();
    let strip_names: Vec<String> = strips.keys().cloned().collect();
    host.push_output(&format!(
        "  terminals     {} on {} strip(s){}",
        idx.terminals.len(),
        strips.len(),
        if strip_names.is_empty() {
            String::new()
        } else {
            format!(" [{}]", strip_names.join(", "))
        }
    ));

    let cable_count = idx.cables.values().filter(|c| c.rec.is_parent()).count();
    host.push_output(&format!(
        "  cables        {cable_count} ({} conductor marker(s))",
        idx.cables.len()
    ));
    host.push_output(&format!("  connectors    {}", idx.connectors.len()));
    if !idx.signals.is_empty() {
        host.push_output(&format!("  signals       {}", idx.signals.len()));
    }
    if !idx.plcs.is_empty() {
        let points: usize = idx
            .plcs
            .values()
            .map(|m| (m.rec.di + m.rec.do_).max(0) as usize)
            .sum();
        host.push_output(&format!(
            "  plc           {} module(s), {} point(s)",
            idx.plcs.len(),
            points
        ));
    }
    if !idx.footprints.is_empty() {
        let ballooned = idx.footprints.values().filter(|f| f.rec.ballooned).count();
        host.push_output(&format!(
            "  panel         {} footprint(s) ({} ballooned)",
            idx.footprints.len(),
            ballooned
        ));
    }

    // Catalogue coverage decides whether a bill of materials can be issued.
    let assignments = drc::assignments(idx);
    let missing = assignments.iter().filter(|a| a.is_incomplete()).count();
    host.push_output(&format!(
        "  catalogue     {}/{} item(s) assigned{}",
        assignments.len() - missing,
        assignments.len(),
        if missing > 0 {
            format!(" — {missing} missing, see EL_REPORT MISSINGCATALOG")
        } else {
            String::new()
        }
    ));

    // The verdict, gated on the same rules the reports use.
    let issues = drc::check(idx, catalog_in_effect(host).as_ref());
    host.push_output(&format!("  audit         {}", drc::summary(&issues)));
    if !drc::export_allowed(&issues) {
        host.push_error(&format!(
            "EL_LIST: this drawing is NOT reportable — {} error(s); run EL_AUDIT for the detail",
            drc::errors(&issues).len()
        ));
    }
}

/// Devices by family, so a glance says what kind of panel this is.
fn family_breakdown(idx: &ElIndex) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for c in idx.components.values() {
        if c.rec.is_child {
            continue;
        }
        let family = if c.rec.family.is_empty() {
            el_core::model::family_of(&c.rec.tag)
        } else {
            c.rec.family.clone()
        };
        if family.is_empty() {
            continue;
        }
        match counts.iter_mut().find(|(f, _)| *f == family) {
            Some((_, n)) => *n += 1,
            None => counts.push((family, 1)),
        }
    }
    counts.sort_by(|a, b| a.0.cmp(&b.0));
    counts
        .into_iter()
        .map(|(f, n)| format!("{f} x{n}"))
        .collect::<Vec<_>>()
        .join("  ")
}

fn devices(host: &mut dyn HostApi, idx: &ElIndex) {
    let mut list: Vec<(String, String, String)> = idx
        .components
        .values()
        .map(|c| {
            (
                if c.rec.tag.is_empty() {
                    format!("0x{:X}", c.primary())
                } else {
                    c.rec.tag.clone()
                },
                c.rec.symbol.clone(),
                if c.rec.is_child {
                    format!("child of {}", c.rec.parent_tag)
                } else if c.rec.cat.is_empty() {
                    "no part".to_string()
                } else {
                    format!("{}/{}", c.rec.mfg, c.rec.cat)
                },
            )
        })
        .collect();
    list.sort_by(|a, b| el_core::refdes_cmp(&a.0, &b.0));

    host.push_output(&format!("EL_LIST devices — {} device(s)", list.len()));
    for (tag, symbol, note) in list {
        host.push_output(&format!("  {tag:<10} {symbol:<18} {note}"));
    }
}

fn wires(host: &mut dyn HostApi, idx: &ElIndex) {
    let mut list: Vec<(String, String, String)> = idx
        .wires
        .values()
        .map(|w| {
            let ends = format!("{} -> {}", dash(&w.rec.from_id), dash(&w.rec.to_id));
            let spec = format!("{} {}", dash(&w.rec.gauge), dash(&w.rec.color));
            (
                if w.rec.wire_ref.is_empty() {
                    format!("0x{:X}", w.handle)
                } else {
                    w.rec.wire_ref.clone()
                },
                ends,
                spec,
            )
        })
        .collect();
    list.sort_by(|a, b| el_core::refdes_cmp(&a.0, &b.0));

    host.push_output(&format!("EL_LIST wires — {} wire(s)", list.len()));
    for (wire, ends, spec) in list {
        host.push_output(&format!("  {wire:<10} {ends:<28} {spec}"));
    }
}

fn terminals(host: &mut dyn HostApi, idx: &ElIndex) {
    let strips = idx.strips();
    let total: usize = strips.values().map(|v| v.len()).sum();
    host.push_output(&format!(
        "EL_LIST terminals — {} strip(s), {total} terminal(s)",
        strips.len()
    ));
    for (strip, list) in strips {
        let jumpers = idx
            .jumpers
            .values()
            .filter(|j| j.rec.strip == strip)
            .count();
        let numbers: Vec<String> = list
            .iter()
            .map(|t| {
                let number = if t.rec.number.is_empty() {
                    dash("")
                } else {
                    t.rec.number.clone()
                };
                // Only a non-standard block is annotated, so an ordinary screw
                // strip reads as just its numbers.
                match t.rec.kind {
                    el_core::model::TerminalKind::Screw => number,
                    k => format!("{number}({})", k.as_str().to_ascii_lowercase()),
                }
            })
            .collect();
        host.push_output(&format!(
            "  {strip}: {}{}",
            numbers.join(" "),
            if jumpers > 0 {
                format!(" ({jumpers} jumper{})", if jumpers == 1 { "" } else { "s" })
            } else {
                String::new()
            }
        ));
    }
}

fn cables(host: &mut dyn HostApi, idx: &ElIndex) {
    let mut tags: Vec<String> = idx
        .cables
        .values()
        .map(|c| c.rec.tag.clone())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort_by(|a, b| el_core::refdes_cmp(a, b));
    tags.dedup();

    host.push_output(&format!("EL_LIST cables — {} cable(s)", tags.len()));
    for tag in tags {
        let markers = idx.cable_markers(&tag);
        let colors: Vec<String> = markers
            .iter()
            .map(|m| {
                if m.rec.color.is_empty() {
                    dash("")
                } else {
                    m.rec.color.clone()
                }
            })
            .collect();
        host.push_output(&format!(
            "  {tag:<8} {} conductor(s)  {}",
            markers.len(),
            colors.join(",")
        ));
    }
}

fn connectors(host: &mut dyn HostApi, idx: &ElIndex) {
    let list = idx.connectors_ordered();
    host.push_output(&format!("EL_LIST connectors — {} connector(s)", list.len()));
    for c in list {
        host.push_output(&format!(
            "  {:<8} {:>3} pin(s) @{}mm  {}",
            dash(&c.rec.ref_des),
            c.rec.pin_count(),
            el_core::fmt_mm(c.rec.pin_spacing_mm),
            c.rec.pins.join(",")
        ));
    }
}

fn nets(host: &mut dyn HostApi, idx: &ElIndex) {
    let nets = NetIndex::build(idx);
    host.push_output(&format!(
        "EL_LIST nets — {} net(s), {} joining more than one device",
        nets.len(),
        nets.connected_nets().len()
    ));
    for n in &nets.nets {
        host.push_output(&format!(
            "  {:<28} {} wire(s)  {}",
            n.label,
            n.wires.len(),
            n.nodes.iter().cloned().collect::<Vec<_>>().join(" ")
        ));
    }
}

fn qc(host: &mut dyn HostApi, idx: &ElIndex) {
    let issues = drc::check(idx, catalog_in_effect(host).as_ref());
    host.push_output(&format!("EL_LIST qc — {}", drc::summary(&issues)));

    // Grouped by code, because a drafter fixes one kind of mistake at a time.
    let mut by_code: Vec<(String, usize, bool)> = Vec::new();
    for i in &issues {
        let blocking = i.severity == drc::Severity::Error;
        match by_code.iter_mut().find(|(c, _, _)| *c == i.code) {
            Some((_, n, _)) => *n += 1,
            None => by_code.push((i.code.clone(), 1, blocking)),
        }
    }
    by_code.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (code, count, blocking) in by_code {
        host.push_output(&format!(
            "  {code:<24} x{count}{}",
            if blocking { "  (blocks reports)" } else { "" }
        ));
    }
    if issues.is_empty() {
        host.push_output("  nothing to fix");
    }
}

/// The catalogue in effect, falling back to the built-in sample.
fn catalog_in_effect(host: &dyn HostApi) -> Option<Catalog> {
    library::load(host).ok().map(|l| {
        if l.catalog.is_empty() {
            Catalog::sample()
        } else {
            l.catalog
        }
    })
}

fn dash(s: &str) -> String {
    if s.trim().is_empty() {
        "\u{2014}".to_string()
    } else {
        s.to_string()
    }
}
