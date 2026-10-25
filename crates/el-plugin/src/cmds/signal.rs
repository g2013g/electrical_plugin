//! `EL_SIGNAL` — source/destination arrows for signals that continue on
//! another sheet.
//!
//! An arrow anchors to a **wire end** and declares, additionally to whatever
//! the wire connects, that the signal continues off-sheet. Arrows pair across
//! sheets by name through the project register: after both sheets have run
//! `EL_PROJECT SYNC`, a destination arrow labels itself with the sheet its
//! source lives on — without that sheet being open.
//!
//! The glyph is several primitives sharing one record (the component rule), so
//! the arrow is one pickable, movable, deletable object however many parts it
//! draws as.

use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::geom;
use el_core::model::{SignalDirection, SignalRec};
use el_core::ElIndex;

use crate::cmds::common::{remove_handles, Args};
use crate::geometry::signal_entities;
use crate::hostops::parse_handle;

/// How far a picked point may sit from a wire end and still claim it.
///
/// Wider than a pin snap: an arrow is placed at a sheet boundary where the
/// wire end is the thing being aimed at, not a dense pin field.
const SIGNAL_SNAP_MM: f64 = 5.0;

/// Run `EL_SIGNAL <name> SOURCE|DEST [x,y] [WIRE=<wref>] | LIST | CLEAR <name|ALL>
/// | SET <name> NAME=<new> DIR=SOURCE|DEST`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());

    let action = a
        .first_word()
        .map(|w| w.to_ascii_uppercase())
        .unwrap_or_else(|| "PLACE".to_string());

    match action.as_str() {
        "LIST" => {
            list(host, &idx);
            return;
        }
        "CLEAR" => {
            a.free.remove(0);
            clear(host, &idx, &a);
            return;
        }
        "SET" => {
            a.free.remove(0);
            set(host, &idx, &a);
            return;
        }
        _ => {}
    }

    // Place: `<name> SOURCE|DEST [x,y] [WIRE=<wref>]`.
    let Some(name) = a.free.first().map(|s| s.to_string()) else {
        host.push_error(
            "EL_SIGNAL: name the signal — EL_SIGNAL STOP SOURCE 30,120 (or LIST, CLEAR, SET)",
        );
        return;
    };
    let Some(dir_word) = a.free.get(1).map(|s| s.to_ascii_uppercase()) else {
        host.push_error("EL_SIGNAL: say which way — SOURCE (leaves this sheet) or DEST (arrives)");
        return;
    };
    let Some(direction) = SignalDirection::parse(&dir_word) else {
        host.push_error(&format!(
            "EL_SIGNAL: {dir_word} is not a direction — SOURCE or DEST"
        ));
        return;
    };

    // One arrow per name per direction per sheet, or the pair-up is ambiguous.
    if idx.signals.values().any(|s| {
        s.rec.name.eq_ignore_ascii_case(&name) && s.rec.direction == direction
    }) {
        host.push_error(&format!(
            "EL_SIGNAL: a {direction_str} arrow for {name:?} already exists on this sheet — \
             SET to change it, or CLEAR it first",
            direction_str = direction.as_str()
        ));
        return;
    }

    let at = a
        .free
        .iter()
        .find_map(|t| crate::cmds::common::parse_coord_token(t));
    let wire_token = a.get("WIRE").map(|s| s.to_string());
    let anchor = match resolve_anchor(host, &idx, wire_token.as_deref(), at) {
        Ok(a) => a,
        Err(e) => {
            host.push_error(&format!("EL_SIGNAL: {e}"));
            return;
        }
    };

    let mut rec = SignalRec::new(&el_core::new_el_id(), &name, direction);
    rec.origin_x = anchor.at[0];
    rec.origin_y = anchor.at[1];
    rec.wire_el_id = anchor.wire_el_id;
    rec.at_to_end = anchor.at_to_end;

    host.push_undo("EL_SIGNAL");
    render_one(host, &idx, &rec);
    host.bump_geometry();
    host.set_dirty();

    let pairing = crate::cmds::project::load(host)
        .ok()
        .flatten()
        .and_then(|p| p.pairing_note(&name, direction == SignalDirection::Source));
    let pairing_text = match &pairing {
        Some(note) => format!(" — pairs {note}"),
        None => String::new(),
    };
    host.push_output(&format!(
        "EL_SIGNAL: {name} {dir_str} at ({}, {}) on {}{pairing_text}",
        el_core::fmt_mm(rec.origin_x),
        el_core::fmt_mm(rec.origin_y),
        wire_label(&idx, &rec),
        dir_str = direction.as_str()
    ));
    if pairing.is_none() {
        host.push_info(
            "  pairing shows on the arrow once both sheets have run EL_PROJECT SYNC",
        );
    }
}

/// The wire end an arrow anchors to.
struct Anchor {
    wire_el_id: String,
    at_to_end: bool,
    at: [f64; 3],
}

/// Resolve the anchor: an explicit `WIRE=` snaps to that wire's nearer end;
/// otherwise the nearest wire end within [`SIGNAL_SNAP_MM`] wins.
fn resolve_anchor(
    host: &dyn HostApi,
    idx: &ElIndex,
    wire_token: Option<&str>,
    at: Option<[f64; 3]>,
) -> Result<Anchor, String> {
    let Some(at) = at else {
        return Err(
            "where? give a point near the wire end — EL_SIGNAL STOP SOURCE 30,120".to_string(),
        );
    };

    // WIRE= narrows the candidates to one wire, and the snap is then a
    // deliberate choice of its nearer end, not a proximity test.
    let mut best: Option<(Anchor, f64)> = None;
    for w in idx.wires.values() {
        if let Some(token) = wire_token {
            let matches = w.rec.wire_ref.eq_ignore_ascii_case(token)
                || crate::hostops::parse_handle(token).map(|h| h.value()) == Some(w.handle);
            if !matches {
                continue;
            }
        }
        let Some(pts) = crate::cmds::wiregap::folded_points(host, idx, w) else {
            continue;
        };
        let last = pts.len() - 1;
        for (at_to_end, pt) in [(false, pts[0]), (true, pts[last])] {
            let d = geom::dist([at[0], at[1]], [pt[0], pt[1]]);
            let within = wire_token.is_some() || d <= SIGNAL_SNAP_MM;
            if within && best.as_ref().map(|(_, bd)| d < *bd).unwrap_or(true) {
                best = Some((
                    Anchor {
                        wire_el_id: w.rec.el_id.clone(),
                        at_to_end,
                        at: [pt[0], pt[1], 0.0],
                    },
                    d,
                ));
            }
        }
    }
    match best {
        Some((a, _)) => Ok(a),
        None => match wire_token {
            Some(t) => Err(format!("no wire matches {t:?} — name it by its number (W1)")),
            None => Err(format!(
                "no wire end within {}mm of ({}, {}) — an arrow anchors to a wire end",
                el_core::fmt_mm(SIGNAL_SNAP_MM),
                el_core::fmt_mm(at[0]),
                el_core::fmt_mm(at[1])
            )),
        },
    }
}

fn wire_label(idx: &ElIndex, rec: &SignalRec) -> String {
    idx.wires
        .values()
        .find(|w| w.rec.el_id == rec.wire_el_id)
        .map(|w| {
            if w.rec.wire_ref.is_empty() {
                format!("wire 0x{:X}", w.handle)
            } else {
                format!("wire {}", w.rec.wire_ref)
            }
        })
        .unwrap_or_else(|| "wire (gone)".to_string())
}

/// Draw one arrow: remove whatever is drawn for its `el_id`, add the glyph.
///
/// The pairing note comes from the project register at render time — records
/// never store it, so a sync refreshes labels without rewriting them.
pub(crate) fn render_one(host: &mut dyn HostApi, idx: &ElIndex, rec: &SignalRec) -> usize {
    let note = crate::cmds::project::load(host)
        .ok()
        .flatten()
        .and_then(|p| {
            p.pairing_note(&rec.name, rec.direction == SignalDirection::Source)
        })
        .unwrap_or_default();
    let record = ElRecord::encode_signal(rec);
    remove_handles(
        host,
        idx.handles_for_el_id(&rec.el_id),
        "EL_SIGNAL (re-rendering the arrow)",
    );
    let parts = signal_entities(
        &record,
        rec,
        [rec.origin_x, rec.origin_y, 0.0],
        0.0,
        &note,
    );
    let n = parts.len();
    host.add_entities(parts);
    n
}

/// Re-render every arrow — the label layer rule applied to signals: a render
/// is rebuilt from records and the register, never patched.
pub(crate) fn redraw_all(host: &mut dyn HostApi, idx: &ElIndex) {
    let recs: Vec<SignalRec> = idx.signals.values().map(|s| s.rec.clone()).collect();
    for rec in &recs {
        render_one(host, idx, rec);
    }
    host.bump_geometry();
    host.set_dirty();
}

fn list(host: &mut dyn HostApi, idx: &ElIndex) {
    if idx.signals.is_empty() {
        host.push_output("EL_SIGNAL: no arrows on this sheet — EL_SIGNAL <name> SOURCE|DEST places one");
        return;
    }
    let project = crate::cmds::project::load(host).ok().flatten();
    host.push_output(&format!(
        "EL_SIGNAL: {} arrow(s){}",
        idx.signals.len(),
        match &project {
            Some(_) => String::new(),
            None => " (no project register — pairing unknown)".to_string(),
        }
    ));
    for s in idx.signals.values() {
        let note = project
            .as_ref()
            .and_then(|p| {
                p.pairing_note(&s.rec.name, s.rec.direction == SignalDirection::Source)
            });
        host.push_output(&format!(
            "  {} {} — {}, {}{}",
            dash(&s.rec.name),
            s.rec.direction.as_str(),
            wire_label(idx, &s.rec),
            match s.rec.at_to_end {
                false => "from end",
                true => "to end",
            },
            match note {
                Some(n) => format!(" — pairs {n}"),
                None => String::new(),
            }
        ));
    }
}

fn dash(s: &str) -> String {
    if s.is_empty() { "—".to_string() } else { s.to_string() }
}

fn clear(host: &mut dyn HostApi, idx: &ElIndex, a: &Args) {
    let all = a.free.iter().any(|t| t.eq_ignore_ascii_case("ALL"));
    let targets: Vec<el_core::index::IndexedSignal> = if all {
        idx.signals.values().cloned().collect()
    } else {
        match a.free.first().and_then(|t| resolve_signal(idx, t)) {
            Some(s) => vec![s],
            None => {
                host.push_error("EL_SIGNAL: nothing to clear — name a signal or ALL");
                return;
            }
        }
    };
    if targets.is_empty() {
        host.push_output("EL_SIGNAL: nothing to clear");
        return;
    }
    host.push_undo("EL_SIGNAL CLEAR");
    for s in &targets {
        remove_handles(
            host,
            idx.handles_for_el_id(&s.rec.el_id),
            "EL_SIGNAL CLEAR",
        );
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_SIGNAL: {} arrow(s) removed", targets.len()));
}

fn set(host: &mut dyn HostApi, idx: &ElIndex, a: &Args) {
    let Some(token) = a.free.first().map(|s| s.to_string()) else {
        host.push_error("EL_SIGNAL: SET needs the signal to change — EL_SIGNAL SET STOP NAME=E-STOP");
        return;
    };
    let Some(found) = resolve_signal(idx, &token) else {
        host.push_error(&format!(
            "EL_SIGNAL: no arrow named {token:?} on this sheet — EL_SIGNAL LIST shows what is"
        ));
        return;
    };
    let mut rec = found.rec.clone();
    let mut changed: Vec<String> = Vec::new();
    if let Some(v) = a.get("NAME") {
        rec.name = v.to_string();
        changed.push(format!("NAME={v}"));
    }
    if let Some(v) = a.get("DIR") {
        match SignalDirection::parse(v) {
            Some(d) => {
                rec.direction = d;
                changed.push(format!("DIR={}", d.as_str()));
            }
            None => {
                host.push_error(&format!("EL_SIGNAL: DIR expects SOURCE or DEST, got {v:?}"));
                return;
            }
        }
    }
    if changed.is_empty() {
        host.push_error("EL_SIGNAL: nothing to change — pass NAME= or DIR=");
        return;
    }
    host.push_undo("EL_SIGNAL SET");
    render_one(host, idx, &rec);
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_SIGNAL: {} — {}", found.rec.name, changed.join(", ")));
}

/// Resolve a signal by name or a 0x handle of any of its parts.
pub(crate) fn resolve_signal(idx: &ElIndex, token: &str) -> Option<el_core::index::IndexedSignal> {
    if let Some(s) = idx
        .signals
        .values()
        .find(|s| s.rec.name.eq_ignore_ascii_case(token))
    {
        return Some(s.clone());
    }
    if let Some(h) = parse_handle(token) {
        return idx.signal_of_handle(h.value()).cloned();
    }
    None
}
