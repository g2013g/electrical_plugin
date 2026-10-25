//! `EL_EXPORT WH` — the bridge to the wiring-harness plugin (strategic).
//!
//! A schematic's wires are a harness's wirelist: `EL_EXPORT WH` writes the
//! CSV `WH_IMPORT` consumes — `net,from,from_cav,to,to_cav,gauge,color,type`
//! — one row per fully-connected wire, the net taken from the wire's number
//! (so run `EL_WIRENO` first) or its note. Neither ACADE nor RapidHarness
//! hands a schematic to a harness tool this cleanly; here both ends are ours.
//! The export is gated on the audit like every deliverable: errors block,
//! `FORCE` overrides.

use ocs_plugin_api::host::HostApi;

use crate::cmds::common::Args;

/// Run `EL_EXPORT WH [path.csv] [FORCE]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let a = Args::parse(args);
    let force = a.free.iter().any(|t| t.eq_ignore_ascii_case("FORCE"));
    let idx = crate::codec::build_index(host.document());

    // The audit gate, the same rule the reports follow.
    let issues = el_core::drc::check(&idx, None);
    if !el_core::drc::export_allowed(&issues) && !force {
        host.push_error(&format!(
            "EL_EXPORT: BLOCKED — {}. Fix the errors, or pass FORCE",
            el_core::drc::summary(&issues)
        ));
        return;
    }

    // Only wires that say something survive the bridge: a harness cannot
    // crimp an unnamed end.
    let mut rows: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for w in idx.wires.values() {
        if !w.rec.is_connected() {
            skipped += 1;
            continue;
        }
        let net = if !w.rec.wire_ref.is_empty() {
            &w.rec.wire_ref
        } else {
            &w.rec.note
        };
        if net.is_empty() {
            skipped += 1;
            continue;
        }
        rows.push(format!(
            "{},{},{},{},{},{},{},wire",
            csv(net),
            csv(&w.rec.from_id),
            csv(&w.rec.from_pin),
            csv(&w.rec.to_id),
            csv(&w.rec.to_pin),
            csv(&w.rec.gauge),
            csv(&w.rec.color),
        ));
    }
    if rows.is_empty() {
        host.push_error(
            "EL_EXPORT: nothing to bridge — wires need numbers (EL_WIRENO) or NET= notes, and \
             both ends connected",
        );
        return;
    }

    // The first free word is the format (WH); a second one is the path.
    let target = a
        .free
        .iter()
        .filter(|t| !t.eq_ignore_ascii_case("FORCE"))
        .nth(1)
        .map(|raw| crate::library::output(host, raw))
        .unwrap_or_else(|| crate::library::output(host, "wh-import.csv"));

    let mut text = String::from("net,from,from_cav,to,to_cav,gauge,color,type\n");
    for r in &rows {
        text.push_str(r);
        text.push('\n');
    }
    match std::fs::write(&target, text) {
        Ok(()) => host.push_output(&format!(
            "EL_EXPORT: {} wire(s) bridged to {}{} — WH_IMPORT <file> builds the harness",
            rows.len(),
            target.display(),
            if skipped > 0 {
                format!(" ({skipped} skipped: unnumbered or dangling)")
            } else {
                String::new()
            },
        )),
        Err(e) => host.push_error(&format!("EL_EXPORT: cannot write {}: {e}", target.display())),
    }
}

/// Minimal CSV quoting: a comma or quote in a field quotes and escapes it.
fn csv(s: &str) -> String {
    if s.contains(',') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
