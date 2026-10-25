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
        "MOUSER" => {
            a.free.remove(0);
            mouser(host, &a);
        }
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

// ── Mouser live search (ported from the wiring-harness plugin) ────────────
//
// The transport seam (`el_core::mouser`) is mock-tested; live search needs
// `MOUSER_API_KEY` in the environment and never touches disk keys. Hits are
// numbered and cached session-side; `EL_CATALOG ADD <n> <tag>` writes the
// pick's manufacturer + part number onto a placed device — the catalog trio
// ACADE requires before a BOM can list it.

static MOUSER_HITS: std::sync::Mutex<Vec<el_core::mouser::PartHit>> =
    std::sync::Mutex::new(Vec::new());

fn mouser(host: &mut dyn HostApi, a: &Args) {
    use el_core::mouser::{self, MouserKey};

    // ADD <n> <tag>: land the numbered pick on a device.
    let words: Vec<String> = a.free.clone();
    if let Some(first) = words.first() {
        if first.eq_ignore_ascii_case("ADD") {
            let (Some(n), Some(tag)) = (words.get(1), words.get(2)) else {
                host.push_error(
                    "EL_CATALOG: ADD needs the hit number and the device — EL_CATALOG ADD 2 K1",
                );
                return;
            };
            let Ok(n) = n.parse::<usize>() else {
                host.push_error("EL_CATALOG: ADD's first argument is the hit number");
                return;
            };
            let hits = MOUSER_HITS.lock().map(|h| h.clone()).unwrap_or_default();
            let Some(hit) = hits.get(n.saturating_sub(1)) else {
                host.push_error(&format!(
                    "EL_CATALOG: no hit #{n} — the last search's hits are 1 … {}",
                    hits.len()
                ));
                return;
            };
            // Assign through the standing ASSIGN machinery: mfg + part number.
            let idx = crate::codec::build_index(host.document());
            if idx.parent_by_tag(tag).is_none() && idx.component_by_tag(tag).is_none() {
                host.push_error(&format!(
                    "EL_CATALOG: nothing tagged {tag:?} is placed — a pick lands on a device"
                ));
                return;
            }
            let cat = if hit.mpn.is_empty() { &hit.mouser_pn } else { &hit.mpn };
            let mut kv: Vec<String> = vec![
                format!("MFG={}", hit.manufacturer),
                format!("CAT={cat}"),
            ];
            if !hit.description.is_empty() {
                // `+` for spaces: the tokenizer splits on whitespace.
                kv.push(format!("DESC={}", hit.description.replace(' ', "+")));
            }
            let refs: Vec<&str> = kv.iter().map(|s| s.as_str()).collect();
            // ASSIGN <tag> MFG=… CAT=…
            let mut call: Vec<&str> = vec!["ASSIGN", tag];
            call.extend(refs);
            crate::cmds::catalog_cmd::run(host, &call);
            return;
        }
    }

    // Search: MOUSER <query>.
    let query = words.join(" ");
    if query.trim().is_empty() {
        host.push_error("EL_CATALOG: MOUSER needs a query — EL_CATALOG MOUSER 24VDC relay");
        return;
    }
    let key = match MouserKey::from_env() {
        Ok(k) => k,
        Err(e) => {
            host.push_error(&format!("EL_CATALOG: {e}"));
            return;
        }
    };
    match mouser::search(&key, &query, 8) {
        Ok(hits) => {
            if let Ok(mut g) = MOUSER_HITS.lock() {
                *g = hits.clone();
            }
            host.push_output(&format!(
                "EL_CATALOG: {} Mouser hit(s) for {query:?} — EL_CATALOG ADD <n> <tag> assigns one",
                hits.len()
            ));
            for (i, h) in hits.iter().enumerate() {
                host.push_output(&format!(
                    "  {:>2}. {} {} — {}",
                    i + 1,
                    h.manufacturer,
                    if h.mpn.is_empty() { &h.mouser_pn } else { &h.mpn },
                    h.description
                ));
            }
            if hits.is_empty() {
                host.push_error("EL_CATALOG: nothing matched — try a broader query");
            }
        }
        Err(e) => host.push_error(&format!("EL_CATALOG: Mouser search failed: {e:?}")),
    }
}

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
