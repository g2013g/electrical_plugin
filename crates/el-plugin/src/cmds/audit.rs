//! `EL_AUDIT` — what is wrong with this drawing.
//!
//! The rules live in `el_core::drc` and gate every report. This command is how a
//! user reads them: grouped by severity, with the counts first, because the
//! question a drafter asks is "can I issue this?" and only then "what is wrong?".
//!
//! Errors block a report; warnings do not. That split is stated in the output, so
//! a warning is never mistaken for a blocker and an error is never mistaken for
//! advice.

use ocs_plugin_api::host::HostApi;

use el_core::catalog::Catalog;
use el_core::drc::{self, Severity};


use crate::cmds::common::Args;
use crate::library;

/// Run `EL_AUDIT [QUIET] [CODES]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let a = Args::parse(args);
    let quiet = a.flag("QUIET").unwrap_or(false);
    let codes = a.flag("CODES").unwrap_or(false);

    let idx = crate::codec::build_index(host.document());
    let catalog: Option<Catalog> = library::load(host).ok().map(|l| {
        if l.catalog.is_empty() {
            Catalog::sample()
        } else {
            l.catalog
        }
    });

    // A drawing with nothing on it is not a clean drawing, it is an empty one —
    // and saying "no issues" for it would be a lie of omission.
    let empty = idx.components.is_empty()
        && idx.wires.is_empty()
        && idx.terminals.is_empty()
        && idx.cables.is_empty()
        && idx.connectors.is_empty()
        && idx.ladders.is_empty();
    if empty {
        host.push_output("EL_AUDIT: nothing to check — this drawing has no electrical objects yet");
        return;
    }

    let issues = drc::check(&idx, catalog.as_ref());

    if codes {
        print_codes(host);
        return;
    }

    let errors = drc::errors(&issues);
    let warnings = drc::warnings(&issues);

    host.push_output(&format!(
        "EL_AUDIT — {}",
        drc::summary(&issues)
    ));

    if !errors.is_empty() {
        host.push_output("  ERRORS (these block EL_REPORT):");
        for i in &errors {
            host.push_error(&format!("    {}: {}", i.code, i.message));
        }
    }
    if !warnings.is_empty() && !quiet {
        host.push_output("  WARNINGS (a report is still truthful):");
        for i in warnings.iter().take(40) {
            host.push_info(&format!("    {}: {}", i.code, i.message));
        }
        if warnings.len() > 40 {
            host.push_info(&format!("    … and {} more", warnings.len() - 40));
        }
    }

    if errors.is_empty() && warnings.is_empty() {
        host.push_output("  nothing wrong: every device is tagged and catalogued, every wire connected");
    }

    // A one-line verdict, so the answer survives even when the detail is long.
    host.push_output(if drc::export_allowed(&issues) {
        "  VERDICT: reportable"
    } else {
        "  VERDICT: NOT reportable until the errors are fixed (or use EL_REPORT FORCE)"
    });

    // Point at the report that lists what is missing, since that is usually the
    // next thing a user wants.
    if issues.iter().any(|i| i.code == "NO_CATALOG" || i.code == "NO_MFG") {
        host.push_info("  run EL_REPORT MISSINGCATALOG for the full list of unorderable devices");
    }
}

/// List the rule codes, so a user can refer to one and a script can rely on it.
fn print_codes(host: &mut dyn HostApi) {
    const CODES: &[(&str, Severity, &str)] = &[
        ("BAD_RECORD", Severity::Error, "a record could not be decoded"),
        ("UNKNOWN_KIND", Severity::Warning, "a record from a newer build"),
        ("DUP_TAG", Severity::Error, "two devices share a tag"),
        ("DUP_WREF", Severity::Error, "two wires share a number"),
        ("NO_SETTINGS", Severity::Warning, "the drawing has no settings record"),
        ("UNTAGGED", Severity::Error, "a device has no tag"),
        ("NO_FAMILY", Severity::Warning, "a device has no family code"),
        ("NO_CATALOG", Severity::Warning, "a device has no part number"),
        ("NO_MFG", Severity::Error, "a part number has no manufacturer"),
        ("NO_PINS", Severity::Warning, "a device has no connection points"),
        ("CHILD_NO_PARENT", Severity::Error, "a child device names no parent"),
        ("PARENT_HAS_PARENT", Severity::Warning, "a parent names a parent"),
        ("ORPHAN_CHILD", Severity::Error, "a child's parent is not placed"),
        ("NO_CONTACTS", Severity::Warning, "a relay-family device has no contacts"),
        ("WIRE_DANGLING", Severity::Error, "a wire has an unconnected end"),
        ("WIRE_UNNUMBERED", Severity::Warning, "a wire has no number"),
        ("WIRE_SELF", Severity::Warning, "a wire connects a device to itself"),
        ("NO_GAUGE", Severity::Warning, "a wire has no gauge"),
        ("TERM_NO_STRIP", Severity::Error, "a terminal has no strip"),
        ("TERM_UNNUMBERED", Severity::Warning, "a terminal has no number"),
        ("TERM_DUP_NUMBER", Severity::Error, "a strip has two terminals with one number"),
        ("TERM_SELF_LINK", Severity::Error, "a terminal links to itself"),
        ("TERM_MULTI_UNLINKED", Severity::Warning, "a multi-level terminal names no level"),
        ("CABLE_NO_TAG", Severity::Error, "a cable marker has no cable"),
        ("CABLE_NO_COLOR", Severity::Warning, "a cable marker has no colour"),
        ("CABLE_NO_PARENT", Severity::Error, "a cable marker's parent is missing"),
        ("CABLE_PARENT_NOT_FIRST", Severity::Warning, "a cable parent is not conductor 1"),
        ("CABLE_DUP_CONDUCTOR", Severity::Error, "two markers claim one conductor"),
        ("CONN_NO_PINS", Severity::Error, "a connector has no pins"),
        ("CONN_SPACING", Severity::Error, "a connector's pin spacing is invalid"),
        ("CONN_DUP_PIN", Severity::Error, "a connector lists a pin twice"),
        ("CATALOG_UNKNOWN", Severity::Warning, "an assigned part is not in the catalogue"),
        ("CATALOG_FAMILY", Severity::Warning, "an assigned part is not for that family"),
    ];

    host.push_output(&format!("EL_AUDIT — {} rule(s)", CODES.len()));
    for (code, severity, desc) in CODES {
        host.push_output(&format!("  {:<22} {:<6} {}", code, severity.as_str(), desc));
    }
    host.push_output(&format!(
        "  {} rule(s) block a report; the rest are advisory",
        CODES.iter().filter(|(_, s, _)| *s == Severity::Error).count()
    ));
}
