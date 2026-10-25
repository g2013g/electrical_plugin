//! `EL_CATALOG` — the parts catalogue, and the assignments that resolve against
//! it.
//!
//! A bill of materials is only as good as the part data behind it, so this is
//! where a device's manufacturer and part number are chosen and where the gaps
//! are found. `EL_CATALOG ASSIGN` writes both halves at once: a part number with
//! no manufacturer cannot be ordered, which is why the report treats that as an
//! error rather than a warning.

use ocs_plugin_api::host::HostApi;

use el_core::catalog::Catalog;

use crate::cmds::common::Args;
use crate::library;

/// Run `EL_CATALOG [LIST|SEARCH <text>|FAMILY <code>] | ASSIGN <item> MFG= CAT= |
/// CLEAR <item> | PATH`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let action = a
        .first_word()
        .map(|w| w.to_ascii_uppercase())
        .unwrap_or_else(|| "LIST".to_string());

    match action.as_str() {
        "LIST" | "SHOW" => list(host, &a),
        "SEARCH" | "FIND" => search(host, &a),
        "FAMILY" => family(host, &a),
        "ASSIGN" | "SET" => assign(host, &mut a),
        "CLEAR" | "UNASSIGN" => clear_assignment(host, &a),
        "PATH" => {
            host.push_output(&format!(
                "EL_CATALOG: the catalogue lives in the symbol library — {}",
                library::describe(host)
            ));
        }
        other => host.push_error(&format!(
            "EL_CATALOG: unknown action {other:?} — try LIST, SEARCH, FAMILY, ASSIGN, CLEAR or PATH"
        )),
    }
}

/// The catalogue in effect.
///
/// The built-in sample is returned when the library file has no catalogue
/// section, so a drawing can resolve parts before anyone authors one — and an
/// unreadable library is reported rather than silently replaced.
fn load(host: &mut dyn HostApi) -> Option<Catalog> {
    match library::load(host) {
        Ok(lib) => {
            if lib.catalog.is_empty() {
                Some(Catalog::sample())
            } else {
                Some(lib.catalog.clone())
            }
        }
        Err(e) => {
            host.push_error(&format!("EL_CATALOG: {e}"));
            None
        }
    }
}

fn list(host: &mut dyn HostApi, a: &Args) {
    let Some(catalog) = load(host) else {
        return;
    };
    // An optional free-text term: `EL_CATALOG LIST relay` is how a user narrows
    // without remembering the SEARCH verb.
    let term = a.free.iter().skip(1).find(|t| !t.contains(',')).cloned();
    let shown = match &term {
        Some(t) => catalog.search(t),
        None => catalog.parts.iter().collect(),
    };

    host.push_output(&format!(
        "EL_CATALOG — {} part(s){}",
        catalog.len(),
        match &term {
            Some(t) => format!(", showing {shown_len} matching {t:?}", shown_len = shown.len()),
            None => String::new(),
        }
    ));
    for p in shown.iter().take(40) {
        host.push_output(&format!(
            "  {:<12} {:<20} {:<8} {}",
            p.mfg,
            p.cat,
            p.families.join(","),
            p.description
        ));
    }
    if shown.len() > 40 {
        host.push_output(&format!("  … and {} more", shown.len() - 40));
    }
}

fn search(host: &mut dyn HostApi, a: &Args) {
    let Some(catalog) = load(host) else {
        return;
    };
    let query = a
        .free
        .iter()
        .skip(1)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if query.trim().is_empty() {
        host.push_error("EL_CATALOG SEARCH: what are you looking for? e.g. EL_CATALOG SEARCH relay");
        return;
    }
    let found = catalog.search(&query);
    if found.is_empty() {
        host.push_error(&format!("EL_CATALOG: nothing matches {query:?}"));
        return;
    }
    host.push_output(&format!("EL_CATALOG — {} match(es) for {query:?}", found.len()));
    for p in found.iter().take(20) {
        host.push_output(&format!(
            "  {:<12} {:<20} {}",
            p.mfg, p.cat, p.description
        ));
        if !p.ratings.is_empty() {
            host.push_output(&format!("      ratings: {}", p.ratings.join(", ")));
        }
    }
}

fn family(host: &mut dyn HostApi, a: &Args) {
    let Some(catalog) = load(host) else {
        return;
    };
    let Some(code) = a.free.iter().skip(1).find(|t| !t.contains(',')).cloned() else {
        host.push_error("EL_CATALOG FAMILY: name a family code, e.g. EL_CATALOG FAMILY K");
        return;
    };
    let found = catalog.for_family(&code);
    if found.is_empty() {
        host.push_error(&format!("EL_CATALOG: no part is catalogued for family {code:?}"));
        return;
    }
    host.push_output(&format!(
        "EL_CATALOG — {} part(s) usable for family {code}",
        found.len()
    ));
    for p in found {
        host.push_output(&format!(
            "  {:<12} {:<20} {}{}",
            p.mfg,
            p.cat,
            p.description,
            if p.families.is_empty() {
                " (unrestricted)"
            } else {
                ""
            }
        ));
    }
}

/// Assign a part to a device, terminal or cable.
///
/// A component's part number *is* its `cat`, so the triple written here is
/// manufacturer, catalogue number and assembly code — the same three ACADE
/// resolves a BOM line from.
fn assign(host: &mut dyn HostApi, a: &mut Args) {
    let Some(item) = a
        .free
        .iter()
        .skip(1)
        .find(|t| !t.contains(','))
        .cloned()
    else {
        host.push_error(
            "EL_CATALOG ASSIGN: name the item, e.g. EL_CATALOG ASSIGN K1 MFG=Generic \
             CAT=RLY-24DC-2CO",
        );
        return;
    };

    let catalog = load(host);
    let idx = crate::codec::build_index(host.document());

    // A device is the common case; terminals and cables are named the same way.
    if let Some(comp) = idx.parent_by_tag(&item) {
        let mut rec = comp.rec.clone();
        apply_catalog(&mut rec.mfg, &mut rec.cat, &mut rec.assy, a);
        // Filling the description from the catalogue is what makes a BOM readable
        // without anyone typing it twice.
        if rec.description.is_empty() {
            if let Some(p) = catalog
                .as_ref()
                .and_then(|c| c.get(&rec.mfg, &rec.cat))
            {
                rec.description = p.description.clone();
                if rec.ratings.is_empty() {
                    rec.ratings = p.ratings.clone();
                }
            }
        }
        let record = el_core::codec::ElRecord::encode_component(&rec);
        host.push_undo("EL_CATALOG ASSIGN");
        for h in idx.handles_for_el_id(&rec.el_id) {
            crate::codec::write_el(host, ocs_plugin_api::host::codec::Handle::new(*h), &record);
        }
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!(
            "EL_CATALOG: {} is {}/{}",
            rec.tag, rec.mfg, rec.cat
        ));
        report_unknown(host, &catalog, &rec.mfg, &rec.cat, &rec.family);
        return;
    }

    // A terminal carries a single part number, so `CAT=` and `PART=` are the
    // same field for it.
    let terminal = idx
        .terminals
        .values()
        .find(|t| t.rec.designator().eq_ignore_ascii_case(&item));
    if let Some(t) = terminal {
        let mut rec = t.rec.clone();
        let mut mfg = String::new();
        let mut cat = rec.part_number.clone();
        apply_catalog(&mut mfg, &mut cat, &mut rec.part_number, a);
        if !cat.is_empty() {
            rec.part_number = cat;
        }
        let record = el_core::codec::ElRecord::encode_terminal(&rec);
        host.push_undo("EL_CATALOG ASSIGN");
        for h in idx.handles_for_el_id(&rec.el_id) {
            crate::codec::write_el(host, ocs_plugin_api::host::codec::Handle::new(*h), &record);
        }
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!(
            "EL_CATALOG: terminal {} is part {}",
            rec.designator(),
            rec.part_number
        ));
        return;
    }

    // A cable, by tag — assigned on its parent marker.
    if let Some(c) = idx.cable_parent(&item) {
        let mut rec = c.rec.clone();
        if let Some(v) = a.get("MFG") {
            rec.mfg = v.to_string();
        }
        if let Some(v) = a.get("CAT") {
            rec.cat = v.to_string();
        }
        if let Some(v) = a.get("PART") {
            rec.part_number = v.to_string();
        }
        if let Some(v) = a.get("DESC") {
            rec.description = v.to_string();
        }
        if let Some(v) = a.get("GAUGE") {
            rec.gauge = v.to_string();
        }
        let record = el_core::codec::ElRecord::encode_cable(&rec);
        host.push_undo("EL_CATALOG ASSIGN");
        for h in idx.handles_for_el_id(&rec.el_id) {
            crate::codec::write_el(host, ocs_plugin_api::host::codec::Handle::new(*h), &record);
        }
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!(
            "EL_CATALOG: cable {} is {}/{}",
            rec.tag, rec.mfg, rec.cat
        ));
        return;
    }

    host.push_error(&format!(
        "EL_CATALOG: no device, terminal or cable matches {item:?} — a device is named by its tag \
         (K1), a terminal by its designator (X1:1), a cable by its tag (W1)"
    ));
}

fn apply_catalog(mfg: &mut String, cat: &mut String, part: &mut String, a: &Args) {
    if let Some(v) = a.get("MFG") {
        *mfg = v.to_string();
    }
    if let Some(v) = a.get("CAT") {
        *cat = v.to_string();
    }
    if let Some(v) = a.get("PART") {
        *part = v.to_string();
    }
    // `CAT` alone is common shorthand for the part number.
    if cat.is_empty() && part.is_empty() {
        if let Some(v) = a.get("PN") {
            *cat = v.to_string();
        }
    }
}

/// Say when an assigned part is not in the catalogue, or is the wrong family.
///
/// A warning rather than a refusal: the part may be real and simply not
/// catalogued yet, and refusing would stop a drafter entering a part they hold in
/// their hand.
fn report_unknown(
    host: &mut dyn HostApi,
    catalog: &Option<Catalog>,
    mfg: &str,
    cat: &str,
    family: &str,
) {
    let Some(c) = catalog else {
        return;
    };
    if cat.is_empty() {
        return;
    }
    match c.get(mfg, cat) {
        None => host.push_info(&format!(
            "  note: {mfg}/{cat} is not in the catalogue, so the BOM will list it as-is"
        )),
        Some(p) if !p.suits_family(family) && !family.is_empty() => host.push_error(&format!(
            "  warning: {mfg}/{cat} is not catalogued for family {family} — EL_REPORT \
             MISSINGCATALOG lists this"
        )),
        Some(_) => {}
    }
}

fn clear_assignment(host: &mut dyn HostApi, a: &Args) {
    let Some(item) = a
        .free
        .iter()
        .skip(1)
        .find(|t| !t.contains(','))
        .cloned()
    else {
        host.push_error("EL_CATALOG CLEAR: name the item to unassign");
        return;
    };
    let idx = crate::codec::build_index(host.document());
    let Some(comp) = idx.parent_by_tag(&item) else {
        host.push_error(&format!("EL_CATALOG CLEAR: no device tagged {item:?}"));
        return;
    };
    let mut rec = comp.rec.clone();
    rec.mfg = String::new();
    rec.cat = String::new();
    rec.assy = String::new();
    let record = el_core::codec::ElRecord::encode_component(&rec);
    host.push_undo("EL_CATALOG CLEAR");
    for h in idx.handles_for_el_id(&rec.el_id) {
        crate::codec::write_el(host, ocs_plugin_api::host::codec::Handle::new(*h), &record);
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_CATALOG: cleared the part for {}", rec.tag));
}
