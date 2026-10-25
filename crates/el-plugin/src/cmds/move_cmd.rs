//! `EL_MOVE`, `EL_ROTATE` and `EL_REATTACH` — moving objects without breaking
//! their wiring.
//!
//! A device, a terminal and a connector all have pins, and wires name the pins they
//! reach. Moving one therefore has two halves that must happen together: the glyph
//! and the recorded placement move, and *every wire attached to it is re-seated*.
//! Doing only the first is how a drawing ends up with wires that claim to reach a
//! pin they visibly do not touch.
//!
//! Glyphs are rebuilt rather than translated, because a symbol is emitted from its
//! definition and rebuild-from-record is the one path that cannot drift. It is the
//! same rule ladders, terminals, wire numbers and cross-references already follow.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::{ConnectorRec, ElComponentRec, SignalRec, TerminalRec};
use el_core::symbol::SymbolLibrary;
use el_core::ElIndex;

use crate::cmds::attach::{self, Placement};
use crate::cmds::common::Args;
use crate::hostops::Capture;
use crate::library;
use crate::state;

/// Run `EL_MOVE <target> <x,y> | DX= DY= [ROT=]` and `EL_ROTATE <target> [deg]`.
pub fn run(host: &mut dyn HostApi, args: &[&str], rotate: bool) {
    let mut a = Args::parse(args);

    let Some(token) = a.first_word().map(str::to_string) else {
        host.push_error(if rotate {
            "EL_ROTATE: name what to rotate, e.g. EL_ROTATE K1 90"
        } else {
            "EL_MOVE: name what to move, e.g. EL_MOVE K1 100,200"
        });
        return;
    };
    a.free.retain(|f| f != &token);

    let capture = Capture::take(host);
    let target = match resolve(&capture, &token) {
        Ok(t) => t,
        Err(e) => {
            host.push_error(&format!("{}: {e}", if rotate { "EL_ROTATE" } else { "EL_MOVE" }));
            return;
        }
    };

    let Ok(lib) = library::load(host) else {
        host.push_error("EL_MOVE: cannot read the symbol library, so the object cannot be rebuilt");
        return;
    };

    if rotate {
        let degrees = a
            .number("ROT")
            .or_else(|| a.free.iter().find_map(|t| t.parse::<f64>().ok()))
            .unwrap_or(90.0);
        transform(host, &capture.index, &lib, target, None, Some(degrees), &mut a);
    } else {
        let delta = match resolve_delta(&capture.index, &mut a, &target) {
            Ok(d) => d,
            Err(e) => {
                host.push_error(&format!("EL_MOVE: {e}"));
                return;
            }
        };
        let rot = a.number("ROT");
        transform(host, &capture.index, &lib, target, Some(delta), rot, &mut a);
    }
    let _ = &mut a;
}

/// Run `EL_REATTACH` — re-seat every wire onto the pin it names.
pub fn run_reattach(host: &mut dyn HostApi, _args: &[&str]) {
    let idx = crate::codec::build_index(host.document());
    let Ok(lib) = library::load(host) else {
        host.push_error("EL_REATTACH: cannot read the symbol library");
        return;
    };

    let placements = all_placements(&idx, &lib);
    if placements.is_empty() {
        host.push_error("EL_REATTACH: this drawing has nothing to re-seat onto");
        return;
    }

    host.push_undo("EL_REATTACH");
    let (moved, skipped) = attach::reseat_wires(host, &idx, &placements);
    host.bump_geometry();
    host.set_dirty();

    if moved == 0 && skipped == 0 {
        host.push_output("EL_REATTACH: every wire end is already on its pin");
        return;
    }
    host.push_output(&format!("EL_REATTACH: {moved} wire end(s) re-seated"));
    if skipped > 0 {
        host.push_error(&format!(
            "EL_REATTACH: {skipped} wire(s) name a pin that no longer exists, so they were left \
             alone — run EL_AUDIT to list them"
        ));
    }
}

/// What an object is, so it can be rebuilt and re-seated.
#[derive(Debug, Clone)]
pub enum Target {
    Component(ElComponentRec),
    Terminal(TerminalRec),
    Connector(ConnectorRec),
    Signal(SignalRec),
    Plc(el_core::model::PlcRec),
    Ladder(String),
    Wire(u64),
}

/// Resolve a name to the object behind it.
pub fn resolve(capture: &Capture, token: &str) -> Result<Target, String> {
    if let Some(comp) = capture
        .index
        .parent_by_tag(token)
        .or_else(|| capture.index.component_by_tag(token))
    {
        return Ok(Target::Component(comp.rec.clone()));
    }
    if let Some(t) = capture
        .index
        .terminals
        .values()
        .find(|t| t.rec.designator().eq_ignore_ascii_case(token) || t.rec.tag_strip == token)
    {
        return Ok(Target::Terminal(t.rec.clone()));
    }
    if let Some(x) = capture
        .index
        .connectors_ordered()
        .into_iter()
        .find(|x| x.rec.ref_des.eq_ignore_ascii_case(token))
    {
        return Ok(Target::Connector(x.rec.clone()));
    }
    if let Some(s) = capture
        .index
        .signals
        .values()
        .find(|s| s.rec.name.eq_ignore_ascii_case(token))
    {
        return Ok(Target::Signal(s.rec.clone()));
    }
    if let Some(m) = capture.index.plc_by_tag(token) {
        return Ok(Target::Plc(m.rec.clone()));
    }
    if let Some(l) = capture.index.ladder_by_el_id(token) {
        return Ok(Target::Ladder(l.el_id.clone()));
    }
    if let Some(w) = capture.index.wire_by_ref(token) {
        return Ok(Target::Wire(w.handle));
    }
    if let Some(h) = crate::hostops::parse_handle(token) {
        let path = classify_handle(&capture.index, h.value())?;
        return Ok(path);
    }
    Err(format!(
        "nothing matches {token:?} — a device is named by its tag (K1), a terminal by its \
         designator (X1:1), a connector by its tag (X1), a wire by its number (W1)"
    ))
}

fn classify_handle(idx: &ElIndex, handle: u64) -> Result<Target, String> {
    if let Some(c) = idx.component_of_handle(handle) {
        return Ok(Target::Component(c.rec.clone()));
    }
    if let Some(t) = idx.terminal_of_handle(handle) {
        return Ok(Target::Terminal(t.rec.clone()));
    }
    if let Some(x) = idx.connector_of_handle(handle) {
        return Ok(Target::Connector(x.rec.clone()));
    }
    if let Some(s) = idx.signal_of_handle(handle) {
        return Ok(Target::Signal(s.rec.clone()));
    }
    if let Some(m) = idx.plc_of_handle(handle) {
        return Ok(Target::Plc(m.rec.clone()));
    }
    if idx.wires.contains_key(&handle) {
        return Ok(Target::Wire(handle));
    }
    Err(format!("handle {handle:#x} is not an electrical object"))
}

/// The delta to move by: an absolute coordinate, or `DX=`/`DY=`.
///
/// An absolute coordinate is turned into a delta against the object's own anchor,
/// which is what makes `EL_MOVE K1 100,200` mean "put K1 at 100,200" rather than
/// "shift it by 100,200".
fn resolve_delta(idx: &ElIndex, a: &mut Args, target: &Target) -> Result<[f64; 3], String> {
    let dx = a.number("DX");
    let dy = a.number("DY");
    if dx.is_some() || dy.is_some() {
        return Ok([dx.unwrap_or(0.0), dy.unwrap_or(0.0), 0.0]);
    }
    let Some(abs) = a.first_coord() else {
        return Err(
            "where to? give a coordinate (EL_MOVE K1 100,200) or an offset (EL_MOVE K1 DX=10)"
                .to_string(),
        );
    };
    let base = anchor_of(idx, target)?;
    Ok([abs[0] - base[0], abs[1] - base[1], 0.0])
}

/// The drawn anchor of a target.
pub fn anchor_of(idx: &ElIndex, target: &Target) -> Result<[f64; 2], String> {
    match target {
        Target::Component(r) => Ok(r.origin()),
        Target::Terminal(r) => Ok([r.origin_x, r.origin_y]),
        Target::Connector(r) => Ok([r.origin_x, r.origin_y]),
        Target::Signal(r) => Ok([r.origin_x, r.origin_y]),
        Target::Plc(r) => Ok([r.origin_x, r.origin_y]),
        Target::Ladder(id) => idx
            .ladder_by_el_id(id)
            .map(|l| [l.rec.origin_x, l.rec.origin_y])
            .ok_or_else(|| format!("ladder {id} is gone")),
        Target::Wire(h) => idx
            .wires
            .get(h)
            .map(|_| [0.0, 0.0])
            .ok_or_else(|| format!("wire 0x{h:X} is gone")),
    }
}

/// Move and/or rotate one object, then re-seat what it is wired to.
fn transform(
    host: &mut dyn HostApi,
    idx: &ElIndex,
    lib: &SymbolLibrary,
    target: Target,
    delta: Option<[f64; 3]>,
    rotation: Option<f64>,
    a: &mut Args,
) {
    let base = match anchor_of(idx, &target) {
        Ok(b) => b,
        Err(e) => {
            host.push_error(&format!("EL_MOVE: {e}"));
            return;
        }
    };
    let delta = delta.unwrap_or([0.0, 0.0, 0.0]);
    let new_origin = [base[0] + delta[0], base[1] + delta[1]];

    // The parts to remove, the record to write, the glyph to emit, and the
    // placement the wires are re-seated against.
    let (el_id, parts, record, placement, what) = match &target {
        Target::Component(rec) => {
            let Some(def) = lib.get(&rec.symbol) else {
                host.push_error(&format!(
                    "EL_MOVE: {} uses symbol {:?}, which is not in the library — reload with \
                     EL_LIB RELOAD",
                    rec.tag, rec.symbol
                ));
                return;
            };
            let mut rec = rec.clone();
            rec.origin_x = new_origin[0];
            rec.origin_y = new_origin[1];
            if let Some(r) = rotation {
                rec.rotation_deg = r;
            }
            apply_keys(&mut rec, a);
            let record = ElRecord::encode_component(&rec);
            let parts = crate::geometry::component_entities(
                &record,
                def,
                &rec.tag,
                [rec.origin_x, rec.origin_y, 0.0],
                rec.rotation_rad(),
            );
            let placement = Placement {
                node_id: rec.tag.clone(),
                origin: rec.origin(),
                rotation_deg: rec.rotation_deg,
                kind: attach::PlacementKind::Component(def.clone()),
            };
            let what = format!("{} to ({}, {})", rec.tag, el_core::fmt_mm(rec.origin_x), el_core::fmt_mm(rec.origin_y));
            (rec.el_id.clone(), parts, record, placement, what)
        }
        Target::Terminal(rec) => {
            let mut rec = rec.clone();
            rec.origin_x = new_origin[0];
            rec.origin_y = new_origin[1];
            if let Some(r) = rotation {
                rec.rotation_deg = r;
            }
            if let Some(v) = a.get("NO") {
                rec.number = v.to_string();
            }
            if let Some(v) = a.get("PART") {
                rec.part_number = v.to_string();
            }
            let record = ElRecord::encode_terminal(&rec);
            let parts = crate::geometry::terminal_entities(
                &record,
                &rec.designator(),
                [rec.origin_x, rec.origin_y, 0.0],
                rec.rotation_deg.to_radians(),
                rec.kind,
            );
            let placement = attach::terminal_placement(&rec);
            let what = format!(
                "{} to ({}, {})",
                rec.designator(),
                el_core::fmt_mm(rec.origin_x),
                el_core::fmt_mm(rec.origin_y)
            );
            (rec.el_id.clone(), parts, record, placement, what)
        }
        Target::Connector(rec) => {
            let mut rec = rec.clone();
            rec.origin_x = new_origin[0];
            rec.origin_y = new_origin[1];
            if let Some(r) = rotation {
                rec.rotation_deg = r;
            }
            let record = ElRecord::encode_connector(&rec);
            let parts = crate::geometry::connector_entities(
                &record,
                &rec,
                [rec.origin_x, rec.origin_y, 0.0],
                rec.rotation_deg.to_radians(),
            );
            let placement = attach::connector_placement(&rec);
            let what = format!(
                "{} to ({}, {})",
                rec.ref_des,
                el_core::fmt_mm(rec.origin_x),
                el_core::fmt_mm(rec.origin_y)
            );
            (rec.el_id.clone(), parts, record, placement, what)
        }
        Target::Plc(rec) => {
            let mut rec = rec.clone();
            rec.origin_x = new_origin[0];
            rec.origin_y = new_origin[1];
            let record = ElRecord::encode_plc(&rec);
            let parts = crate::geometry::plc_entities(&record, &rec);
            let placement = Placement {
                node_id: rec.tag.clone(),
                origin: [rec.origin_x, rec.origin_y],
                rotation_deg: 0.0,
                kind: attach::PlacementKind::Plc(rec.clone()),
            };
            let what = format!(
                "PLC {} to ({}, {})",
                rec.tag,
                el_core::fmt_mm(rec.origin_x),
                el_core::fmt_mm(rec.origin_y)
            );
            (rec.el_id.clone(), parts, record, placement, what)
        }
        Target::Signal(rec) => {
            let mut rec = rec.clone();
            rec.origin_x = new_origin[0];
            rec.origin_y = new_origin[1];
            let record = ElRecord::encode_signal(&rec);
            let note = crate::cmds::project::load(host)
                .ok()
                .flatten()
                .and_then(|p| {
                    p.pairing_note(&rec.name, rec.direction == el_core::model::SignalDirection::Source)
                })
                .unwrap_or_default();
            let parts = crate::geometry::signal_entities(
                &record,
                &rec,
                [rec.origin_x, rec.origin_y, 0.0],
                0.0,
                &note,
            );
            let placement = Placement {
                node_id: String::new(),
                origin: [rec.origin_x, rec.origin_y],
                rotation_deg: 0.0,
                kind: attach::PlacementKind::Terminal,
            };
            let what = format!(
                "signal {} to ({}, {})",
                rec.name,
                el_core::fmt_mm(rec.origin_x),
                el_core::fmt_mm(rec.origin_y)
            );
            (rec.el_id.clone(), parts, record, placement, what)
        }
        Target::Ladder(id) => {
            let Some(ladder) = idx.ladder_by_el_id(id) else {
                host.push_error("EL_MOVE: the ladder is gone");
                return;
            };
            let mut rec = ladder.rec.clone();
            rec.origin_x = new_origin[0];
            rec.origin_y = new_origin[1];
            let record = ElRecord::encode_ladder(&rec);
            let parts = crate::geometry::ladder_entities(&record, &rec);
            // A ladder has no pins, so there is nothing to re-seat onto it — and
            // moving one does not move the devices drawn on its rungs.
            let placement = Placement {
                node_id: String::new(),
                origin: [rec.origin_x, rec.origin_y],
                rotation_deg: 0.0,
                kind: attach::PlacementKind::Terminal,
            };
            let what = format!(
                "ladder {} to ({}, {})",
                rec.el_id,
                el_core::fmt_mm(rec.origin_x),
                el_core::fmt_mm(rec.origin_y)
            );
            (rec.el_id.clone(), parts, record, placement, what)
        }
        Target::Wire(handle) => {
            // A wire has no pins of its own; moving one means moving a whole end,
            // which is what re-seating or re-drawing is for.
            host.push_error(&format!(
                "EL_MOVE: wire 0x{handle:X} has no pins to move with it — move the device it \
                 reaches instead, or re-draw the wire"
            ));
            return;
        }
    };

    let linked = attach::wires_touching(idx, &placement.node_id);

    host.push_undo(if rotation.is_some() { "EL_ROTATE" } else { "EL_MOVE" });

    // Remove the old glyph, write the placement, emit the new glyph.
    for h in idx.handles_for_el_id(&el_id) {
        host.remove_entity(Handle::new(*h));
    }
    host.add_entities(parts);
    for h in idx.handles_for_el_id(&el_id) {
        crate::codec::write_el(host, Handle::new(*h), &record);
    }

    // Then re-seat, which is the half that keeps the wiring truthful.
    let (moved, skipped) = attach::reseat_wires(host, idx, std::slice::from_ref(&placement));

    host.bump_geometry();
    host.set_dirty();

    host.push_output(&format!(
        "{}: {what}{}",
        if rotation.is_some() { "EL_ROTATE" } else { "EL_MOVE" },
        if linked == 0 {
            String::new()
        } else {
            format!("; {moved} of {linked} attached wire(s) re-seated")
        }
    ));
    if skipped > 0 {
        host.push_error(&format!(
            "{skipped} wire(s) name a pin that does not exist on this object — run EL_AUDIT"
        ));
    }
    if linked > 0 {
        state::set_last_target(idx.handles_for_el_id(&el_id).first().copied().unwrap_or(0));
    }
}

/// Apply the catalogue and identity keys `EL_MOVE` also accepts, so a move and an
/// edit are one command when it saves a round trip.
fn apply_keys(rec: &mut ElComponentRec, a: &Args) {
    if let Some(v) = a.get("DESC") {
        rec.description = v.to_string();
    }
    if let Some(v) = a.get("MFG") {
        rec.mfg = v.to_string();
    }
    if let Some(v) = a.get("CAT") {
        rec.cat = v.to_string();
    }
    if let Some(v) = a.get("LOC") {
        rec.location = v.to_string();
    }
}

/// Every placement in a drawing, for `EL_REATTACH`.
fn all_placements(idx: &ElIndex, lib: &SymbolLibrary) -> Vec<Placement> {
    let mut out: Vec<Placement> = Vec::new();
    for c in idx.components.values() {
        if let Some(p) = attach::component_placement(&c.rec, lib) {
            if !p.node_id.is_empty() {
                out.push(p);
            }
        }
    }
    for t in idx.terminals.values() {
        if !t.rec.designator().is_empty() {
            out.push(attach::terminal_placement(&t.rec));
        }
    }
    for x in idx.connectors.values() {
        if !x.rec.ref_des.is_empty() {
            out.push(attach::connector_placement(&x.rec));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use el_core::model::{ElSettingsRec, ElWireRec};

    fn index_with_component() -> ElIndex {
        let mut c = ElComponentRec::new("cid", "relay-coil", "K");
        c.tag = "K1".to_string();
        c.origin_x = 60.0;
        c.origin_y = 200.0;
        ElIndex::build(vec![
            (900, ElRecord::encode_settings(&ElSettingsRec::default())),
            (1, ElRecord::encode_component(&c)),
            (2, ElRecord::encode_wire(&ElWireRec::new("wid", "K1", "X1"))),
        ])
    }

    #[test]
    fn a_target_resolves_by_tag_designator_and_handle() {
        let idx = index_with_component();
        let capture = Capture {
            index: idx.clone(),
            ..Default::default()
        };
        assert!(matches!(resolve(&capture, "K1").unwrap(), Target::Component(_)));
        assert!(matches!(resolve(&capture, "0x1").unwrap(), Target::Component(_)));
        assert!(resolve(&capture, "nope").is_err());
    }

    #[test]
    fn the_anchor_of_a_component_is_its_recorded_origin() {
        let idx = index_with_component();
        let target = Target::Component(
            idx.component_of_handle(1).unwrap().rec.clone(),
        );
        assert_eq!(anchor_of(&idx, &target).unwrap(), [60.0, 200.0]);
    }

    #[test]
    fn wires_touching_counts_the_attachment() {
        let idx = index_with_component();
        assert_eq!(attach::wires_touching(&idx, "K1"), 1);
        assert_eq!(attach::wires_touching(&idx, "X1"), 1);
    }

    #[test]
    fn a_rotation_moves_the_anchor_not_at_all() {
        // Rotating about the anchor leaves the anchor where it was, which is what
        // makes a rotation safe to apply to a wired device.
        let idx = index_with_component();
        let mut rec = idx.component_of_handle(1).unwrap().rec.clone();
        let before = rec.origin();
        rec.rotation_deg = 90.0;
        assert_eq!(rec.origin(), before);
    }
}
