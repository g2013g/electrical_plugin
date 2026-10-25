//! `EL_PANEL` and `EL_BALLOON` — the panel layout (textbook CH8).
//!
//! A **footprint** is the physical outline of a device on the panel, linked
//! to its schematic component **by tag**: the panel BOM carries the
//! schematic's catalog data over that link, and the audit says (never
//! errors) when a footprint has no schematic partner. Footprints get
//! sequential **item numbers**; a **balloon** — a circled item number with a
//! leader to the body — is a *pure render* of the footprint record, so
//! renumbering or re-placing keeps every balloon and leader correct.

use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::{FootprintRec, LAYER_BALLOONS};
use el_core::ElIndex;

use crate::cmds::common::{remove_entities_on_layer, remove_handles, Args};
use crate::geometry::{balloon_entities, footprint_entities};
use crate::hostops::parse_handle;

/// Run `EL_PANEL <tag> [x,y] [W= H= ITEM= DESC= MFG= CAT=] | NUMBER [START=n]
/// | LIST | CLEAR <tag|ALL>`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());

    match a.first_word().map(|w| w.to_ascii_uppercase()).as_deref() {
        Some("LIST") => {
            list(host, &idx);
            return;
        }
        Some("NUMBER") | Some("RENUMBER") => {
            renumber(host, &idx, &mut a);
            return;
        }
        Some("CLEAR") => {
            a.free.remove(0);
            clear(host, &idx, &a);
            return;
        }
        _ => {}
    }

    let Some(tag) = a.first_word().map(|s| s.to_string()) else {
        host.push_error(
            "EL_PANEL: tag the device — EL_PANEL K1 40,40 (or LIST, NUMBER, CLEAR)",
        );
        return;
    };
    a.free.remove(0);
    let at = a.free.iter().find_map(|t| crate::cmds::common::parse_coord_token(t));
    let Some(at) = at else {
        host.push_error("EL_PANEL: where? give a point — EL_PANEL K1 40,40");
        return;
    };

    // Replacing an existing footprint keeps its item number unless ITEM= says
    // otherwise, so re-placing to move or resize never renumbers the BOM.
    let existing = idx.footprint_by_tag(&tag).cloned();
    let mut rec = match &existing {
        Some(f) => f.rec.clone(),
        None => FootprintRec::new(&el_core::new_el_id(), &tag),
    };
    rec.origin_x = at[0];
    rec.origin_y = at[1];
    if let Some(w) = a.number("W") {
        if w <= 0.0 {
            host.push_error("EL_PANEL: W= must be positive");
            return;
        }
        rec.width_mm = w;
    }
    if let Some(h) = a.number("H") {
        if h <= 0.0 {
            host.push_error("EL_PANEL: H= must be positive");
            return;
        }
        rec.height_mm = h;
    }
    if let Some(v) = a.get("DESC") {
        rec.description = v.to_string();
    }
    if let Some(v) = a.get("MFG") {
        rec.mfg = v.to_string();
    }
    if let Some(v) = a.get("CAT") {
        rec.cat = v.to_string();
    }
    if let Some(v) = a.integer("ITEM") {
        if v < 0 {
            host.push_error("EL_PANEL: ITEM= must be zero or positive");
            return;
        }
        rec.item_number = v;
    }
    if existing.is_none() && a.integer("ITEM").is_none() {
        // Auto-assign the next free item number, ACADE's balloon-start model.
        rec.item_number = next_item(&idx);
    }

    host.push_undo("EL_PANEL");
    if let Some(old) = &existing {
        remove_handles(host, idx.handles_for_el_id(&old.rec.el_id), "EL_PANEL (re-placing)");
    }
    let record = ElRecord::encode_footprint(&rec);
    let parts = footprint_entities(&record, &rec);
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();

    let linked = idx.parent_by_tag(&tag).is_some() || idx.component_by_tag(&tag).is_some();
    host.push_output(&format!(
        "EL_PANEL: {} — {}×{}mm at ({}, {}), item {}{}",
        rec.tag,
        el_core::fmt_mm(rec.width_mm),
        el_core::fmt_mm(rec.height_mm),
        el_core::fmt_mm(rec.origin_x),
        el_core::fmt_mm(rec.origin_y),
        match rec.item_number {
            n if n > 0 => n.to_string(),
            _ => "—".to_string(),
        },
        if linked { String::new() } else { " (no schematic component with this tag)".to_string() },
    ));
    // A ballooned footprint that moved needs its balloon redrawn — the
    // re-placed record patched into the pre-write view, never a re-read.
    if rec.ballooned {
        redraw_balloons(host, &balloon_view(&idx, std::slice::from_ref(&rec)));
    }
}

fn next_item(idx: &ElIndex) -> i32 {
    let mut n = 1;
    while idx.footprints.values().any(|f| f.rec.item_number == n) {
        n += 1;
    }
    n
}

fn list(host: &mut dyn HostApi, idx: &ElIndex) {
    if idx.footprints.is_empty() {
        host.push_output("EL_PANEL: no footprints — EL_PANEL K1 40,40 places one");
        return;
    }
    host.push_output(&format!(
        "EL_PANEL: {} footprint(s){}",
        idx.footprints.len(),
        match idx.footprints.values().filter(|f| f.rec.ballooned).count() {
            0 => String::new(),
            n => format!(" ({n} ballooned)"),
        }
    ));
    let mut fps: Vec<&el_core::index::IndexedFootprint> = idx.footprints.values().collect();
    fps.sort_by_key(|f| f.rec.item_number);
    for f in fps {
        let linked = idx.parent_by_tag(&f.rec.tag).is_some();
        host.push_output(&format!(
            "  item {:>3}  {} — {}×{}mm{}{}",
            match f.rec.item_number {
                n if n > 0 => n,
                _ => 0,
            },
            f.rec.tag,
            el_core::fmt_mm(f.rec.width_mm),
            el_core::fmt_mm(f.rec.height_mm),
            if f.rec.ballooned { " ○" } else { "" },
            if linked { String::new() } else { "  (no schematic)".to_string() },
        ));
    }
}

/// Renumber every footprint sequentially in placement order (left to right,
/// top to bottom — the way a panel is read), then refresh the balloons.
fn renumber(host: &mut dyn HostApi, idx: &ElIndex, a: &mut Args) {
    if idx.footprints.is_empty() {
        host.push_error("EL_PANEL: no footprints to number");
        return;
    }
    let start = a.integer("START").unwrap_or(1);
    if start <= 0 {
        host.push_error("EL_PANEL: START must be positive");
        return;
    }
    let mut fps: Vec<el_core::index::IndexedFootprint> =
        idx.footprints.values().cloned().collect();
    fps.sort_by(|x, y| {
        x.rec
            .origin_x
            .partial_cmp(&y.rec.origin_x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(y.rec.origin_y.partial_cmp(&x.rec.origin_y).unwrap_or(std::cmp::Ordering::Equal))
    });

    host.push_undo("EL_PANEL NUMBER");
    let mut n = start;
    let mut updated = Vec::new();
    for f in &fps {
        let mut rec = f.rec.clone();
        rec.item_number = n;
        let record = ElRecord::encode_footprint(&rec);
        for h in idx.handles_for_el_id(&rec.el_id) {
            crate::codec::write_el(
                host,
                ocs_plugin_api::host::codec::Handle::new(*h),
                &record,
            );
        }
        updated.push(rec);
        n += 1;
    }
    // The balloons read item numbers off the records just written, so the
    // redraw runs off `updated` — the index predates these writes.
    redraw_balloons(host, &updated);
    // And the glyphs themselves (the catalog text corners) are rebuilt from
    // the fresh records.
    for rec in &updated {
        let record = ElRecord::encode_footprint(rec);
        let handles: Vec<u64> = idx.handles_for_el_id(&rec.el_id).to_vec();
        if !handles.is_empty() {
            remove_handles(host, &handles, "EL_PANEL NUMBER (re-render)");
            let parts = footprint_entities(&record, rec);
            host.add_entities(parts);
        }
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_PANEL: {} footprint(s) numbered {} … {}",
        updated.len(),
        start,
        n - 1
    ));
}

fn clear(host: &mut dyn HostApi, idx: &ElIndex, a: &Args) {
    let all = a.free.iter().any(|t| t.eq_ignore_ascii_case("ALL"));
    let targets: Vec<el_core::index::IndexedFootprint> = if all {
        idx.footprints.values().cloned().collect()
    } else {
        match a.free.first() {
            Some(t) => match resolve_footprint(idx, t) {
                Some(f) => vec![f],
                None => {
                    host.push_error(&format!(
                        "EL_PANEL: no footprint tagged {t:?} — EL_PANEL LIST shows them"
                    ));
                    return;
                }
            },
            None => {
                host.push_error("EL_PANEL: CLEAR needs a tag or ALL");
                return;
            }
        }
    };
    if targets.is_empty() {
        host.push_output("EL_PANEL: nothing to clear");
        return;
    }
    host.push_undo("EL_PANEL CLEAR");
    let any_ballooned = targets.iter().any(|f| f.rec.ballooned);
    for f in &targets {
        remove_handles(host, idx.handles_for_el_id(&f.rec.el_id), "EL_PANEL CLEAR");
    }
    if any_ballooned {
        remove_entities_on_layer(host, LAYER_BALLOONS);
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_PANEL: {} footprint(s) removed", targets.len()));
}

/// Run `EL_BALLOON <tag|ALL> | CLEAR`.
pub fn run_balloon(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());
    if idx.footprints.is_empty() {
        host.push_error("EL_BALLOON: no footprints to balloon — EL_PANEL places them");
        return;
    }

    let clearing = a.free.iter().any(|t| t.eq_ignore_ascii_case("CLEAR"));
    let all = a.free.iter().any(|t| t.eq_ignore_ascii_case("ALL"));
    a.free.retain(|t| !t.eq_ignore_ascii_case("CLEAR") && !t.eq_ignore_ascii_case("ALL"));

    if clearing {
        host.push_undo("EL_BALLOON CLEAR");
        let mut cleared = 0usize;
        for f in idx.footprints.values() {
            if !f.rec.ballooned {
                continue;
            }
            let mut rec = f.rec.clone();
            rec.ballooned = false;
            let record = ElRecord::encode_footprint(&rec);
            for h in idx.handles_for_el_id(&rec.el_id) {
                crate::codec::write_el(host, ocs_plugin_api::host::codec::Handle::new(*h), &record);
            }
            cleared += 1;
        }
        remove_entities_on_layer(host, LAYER_BALLOONS);
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!("EL_BALLOON: {cleared} balloon(s) removed"));
        return;
    }

    // Which footprints to balloon; ALL takes everything not yet ballooned.
    let targets: Vec<el_core::index::IndexedFootprint> = if all {
        idx.footprints.values().cloned().collect()
    } else {
        match a.free.first() {
            Some(t) => match resolve_footprint(&idx, t) {
                Some(f) => vec![f],
                None => {
                    host.push_error(&format!(
                        "EL_BALLOON: no footprint tagged {t:?} — a balloon marks a footprint"
                    ));
                    return;
                }
            },
            None => {
                host.push_error("EL_BALLOON: tag a footprint — EL_BALLOON K1 (or ALL, CLEAR)");
                return;
            }
        }
    };

    host.push_undo("EL_BALLOON");
    let mut ballooned = 0usize;
    let mut updated: Vec<FootprintRec> = Vec::new();
    for f in &targets {
        let mut rec = f.rec.clone();
        if !rec.ballooned {
            rec.ballooned = true;
            // An unnumbered footprint gets its item number now: a "?" balloon
            // would be the only thing in the drawing that lies.
            if rec.item_number <= 0 {
                rec.item_number = next_item(&idx);
            }
            ballooned += 1;
        }
        let record = ElRecord::encode_footprint(&rec);
        for h in idx.handles_for_el_id(&rec.el_id) {
            crate::codec::write_el(host, ocs_plugin_api::host::codec::Handle::new(*h), &record);
        }
        updated.push(rec);
    }
    if ballooned == 0 {
        host.push_output("EL_BALLOON: already ballooned");
        return;
    }
    redraw_balloons(host, &balloon_view(&idx, &updated));
    host.push_output(&format!(
        "EL_BALLOON: {ballooned} balloon(s) drawn — item numbers keyed to EL_PANEL NUMBER"
    ));
}

/// Rebuild the whole balloon layer from records: every ballooned footprint
/// gets a circle + leader at a default position clear of the body.
///
/// The records are passed in, never re-read: a record written moments ago is
/// not yet in this dispatch's document snapshot, so a redraw that re-read the
/// index would still see the old flags and draw nothing.
fn redraw_balloons(host: &mut dyn HostApi, recs: &[FootprintRec]) {
    remove_entities_on_layer(host, LAYER_BALLOONS);
    let mut parts = Vec::new();
    for rec in recs {
        if !rec.ballooned {
            continue;
        }
        let record = ElRecord::encode_footprint(rec);
        // Above the top-right corner — the convention a panel drafter reads.
        let at = [
            rec.origin_x + rec.width_mm + 8.0,
            rec.origin_y + rec.height_mm + 6.0,
            0.0,
        ];
        parts.extend(balloon_entities(&record, rec, at));
    }
    if !parts.is_empty() {
        host.add_entities(parts);
    }
}

/// Every footprint's record as the index knows it, with `patched` replacing
/// its tag's entry — the caller's just-written view without a re-read.
fn balloon_view(idx: &ElIndex, patched: &[FootprintRec]) -> Vec<FootprintRec> {
    idx.footprints
        .values()
        .map(|f| f.rec.clone())
        .map(|mut r| {
            if let Some(p) = patched.iter().find(|p| p.el_id == r.el_id) {
                r = p.clone();
            }
            r
        })
        .collect()
}

/// Resolve a footprint by tag or the 0x handle of any of its parts.
pub fn resolve_footprint(
    idx: &ElIndex,
    token: &str,
) -> Option<el_core::index::IndexedFootprint> {
    if let Some(f) = idx.footprint_by_tag(token) {
        return Some(f.clone());
    }
    parse_handle(token)
        .and_then(|h| idx.footprint_of_handle(h.value()))
        .cloned()
}
