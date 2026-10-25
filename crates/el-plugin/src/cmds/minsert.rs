//! `EL_MINSERT` — multiple insert (textbook CH5): one symbol, placed N times
//! down consecutive rungs, each with its own allocated tag.
//!
//! ACADE's multiple insert keeps clicking; here the count is stated and the
//! devices land on successive rungs of the ladder (or spaced by `DY=` without
//! one). It is the batch form of `EL_COMPONENT` — same record, same tagging,
//! same everything — which is why a multi-inserted row of pilot lamps is
//! immediately numberable, reportable and cross-referenced like any hand
//! placement.

use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::LadderRec;
use el_core::tagging;

use crate::cmds::common::Args;
use crate::cmds::component;
use crate::geometry::component_entities;

/// Run `EL_MINSERT <symbol|family> <count> [REF=<rung>] [x,y] [DY=<mm>] [KEY=VALUE …]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let lib = match crate::library::load(host) {
        Ok(l) => l,
        Err(e) => {
            host.push_error(&format!("EL_MINSERT: {e}"));
            return;
        }
    };

    let words: Vec<String> = a.free.clone();
    let Some(symbol_token) = words.first().cloned() else {
        host.push_error("EL_MINSERT: name the symbol and a count — EL_MINSERT lamp 4 REF=101");
        return;
    };
    let count: usize = match words.get(1).and_then(|t| t.parse().ok()) {
        Some(n) if n > 0 => n,
        _ => {
            host.push_error("EL_MINSERT: the count is a positive integer — EL_MINSERT lamp 4");
            return;
        }
    };

    // The symbol: by name, else the family's first parent (as EL_COMPONENT).
    let def = if let Some(d) = lib.get(&symbol_token) {
        d.clone()
    } else if let Some(d) = lib.first_parent(&el_core::model::family_of(&symbol_token)) {
        d.clone()
    } else {
        host.push_error(&format!(
            "EL_MINSERT: {symbol_token:?} is not a symbol or family — EL_LIB FAMILIES lists them"
        ));
        return;
    };
    if def.is_child {
        host.push_error(&format!(
            "EL_MINSERT: {} is a child symbol — children belong to a placed parent, so they do \
             not multi-insert",
            def.name
        ));
        return;
    }

    let idx = crate::codec::build_index(host.document());

    // Where: an explicit point, else rungs starting at REF= (default the
    // first rung) on the ladder; without a ladder, DY= spacing from the point.
    let explicit_pt = a
        .free
        .iter()
        .find_map(|t| crate::cmds::common::parse_coord_token(t));
    let (ladder, mut rung_index): (Option<LadderRec>, usize) = match explicit_pt {
        Some(_) => {
            let l = idx
                .ladders_ordered()
                .into_iter()
                .next()
                .map(|l| l.rec.clone());
            match (l, a.get("REF")) {
                (Some(l), Some(r)) => {
                    let i = l
                        .references()
                        .iter()
                        .position(|x| x.eq_ignore_ascii_case(r))
                        .unwrap_or(0);
                    (Some(l), i)
                }
                (Some(l), None) => (Some(l), 0),
                (None, _) => (None, 0),
            }
        }
        None => match idx
            .ladders_ordered()
            .into_iter()
            .next()
            .map(|l| l.rec.clone())
        {
            Some(l) => {
                let i = a
                    .get("REF")
                    .and_then(|r| {
                        l.references()
                            .iter()
                            .position(|x| x.eq_ignore_ascii_case(r))
                    })
                    .unwrap_or(0);
                (Some(l), i)
            }
            None => {
                host.push_error(
                    "EL_MINSERT: no ladder to run down — place one (EL_LADDER), or anchor with a \
                     point and DY= spacing",
                );
                return;
            }
        },
    };
    let dy = a.number("DY");

    host.push_undo(&format!("EL_MINSERT {} x{count}", def.name));
    let mut placed: Vec<String> = Vec::new();
    for _ in 0..count {
        // Fresh index per placement: tags must not collide with what just
        // landed (the same rule the circuit builder follows).
        let idx = crate::codec::build_index(host.document());
        let settings = idx.settings_or_default();

        let at = match &ladder {
            Some(l) => [
                explicit_pt.map(|p| p[0]).unwrap_or(l.origin_x + 60.0),
                l.rung_y(rung_index),
                0.0,
            ],
            None => {
                let base = explicit_pt.unwrap_or([40.0, 200.0, 0.0]);
                [base[0], base[1] + dy.unwrap_or(15.0) * placed.len() as f64, 0.0]
            }
        };

        let mut rec = component::build_record(&def, None, false, "", &mut a);
        rec.origin_x = at[0];
        rec.origin_y = at[1];
        if rec.tag.is_empty() {
            let rung_ref = idx.rung_reference_at([at[0], at[1]], 20.0);
            match tagging::next_tag(&idx, &rec.family, &settings, rung_ref.as_deref()) {
                Some(tag) => rec.tag = tag,
                None => {
                    host.push_error(&format!(
                        "EL_MINSERT: cannot allocate a {} tag — check TAG_FORMAT",
                        rec.family
                    ));
                    return;
                }
            }
        }
        let record = ElRecord::encode_component(&rec);
        let parts = component_entities(&record, &def, &rec.tag, at, 0.0);
        host.add_entities(parts);
        placed.push(rec.tag.clone());
        rung_index += 1;
    }

    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_MINSERT: {} x{count} — {}",
        def.name,
        placed.join(", ")
    ));
}
