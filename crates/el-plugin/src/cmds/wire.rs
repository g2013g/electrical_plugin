//! `EL_WIRE` — draw a wire between two points.
//!
//! A wire is one `LwPolyline` carrying its record, so a placement is a single
//! entity and a single undo step — no deferred writes needed. New wires are
//! deliberately **unnumbered**: `EL_WIRENO` assigns numbers, and keeping the two
//! apart is what makes a renumber an explicit act rather than a side effect of
//! drawing.

use ocs_plugin_api::host::{CommandStep, HostApi, InteractiveCommand};

use el_core::codec::ElRecord;
use el_core::geom;
use el_core::model::{ElWireRec, LAYER_WIRES, RUNG_TOLERANCE_MM};
use el_core::symbol::SymbolLibrary;
use el_core::ElIndex;

use crate::cmds::common::Args;
use crate::geometry::wire_polyline;
use crate::hostops::Capture;
use crate::state::{self, PendingLine};

/// Run `EL_WIRE [x1,y1 x2,y2 | FROM=node TO=node] [TYPE=] [GAUGE=] [COLOR=] [NET=]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());

    // Two coordinates place the wire outright.
    let coords: Vec<[f64; 3]> = a
        .free
        .iter()
        .filter_map(|t| crate::cmds::common::parse_coord_token(t))
        .collect();
    if coords.len() >= 2 {
        place(host, &idx, coords[0], coords[1], &mut a);
        return;
    }

    // `FROM=node TO=node` resolves through the connectivity index, which is how
    // a wire is drawn between two placed components.
    if let (Some(from), Some(to)) = (a.get("FROM"), a.get("TO")) {
        let Ok(lib) = crate::library::load(host) else {
            host.push_error("EL_WIRE: cannot read the symbol library, so a named node cannot be resolved");
            return;
        };
        let from_pos = match resolve_node(&idx, &lib, from, false) {
            Ok(p) => p,
            Err(e) => {
                host.push_error(&format!("EL_WIRE: FROM={from}: {e}"));
                return;
            }
        };
        let to_pos = match resolve_node(&idx, &lib, to, true) {
            Ok(p) => p,
            Err(e) => {
                host.push_error(&format!("EL_WIRE: TO={to}: {e}"));
                return;
            }
        };
        let (fid, fpin) = split_node(from);
        let (tid, tpin) = split_node(to);
        place_between(host, &idx, from_pos, to_pos, &mut a, &fid, &fpin, &tid, &tpin);
        return;
    }

    if coords.len() == 1 {
        host.push_error(
            "EL_WIRE: one point is not a wire — give two, or run EL_WIRE bare and pick both ends",
        );
        return;
    }

    start_interactive(host);
}

/// Resolve a node reference to the drawn point a wire should reach.
///
/// A node is a device pin (`K1.A1`), a terminal connection (`X1:1`), a connector
/// pin (`X1.A`), or a bare tag. The position comes from the symbol's recorded
/// placement plus its library definition, which is why a placed symbol stores
/// where it went: a symbol drawn as primitives has no recoverable anchor.
///
/// `end` selects which connection of a two-terminal object is meant — `false` for
/// the left/first, `true` for the right/second — because a terminal has two ends
/// and a wire must reach the right one.
pub fn resolve_node(
    idx: &ElIndex,
    lib: &SymbolLibrary,
    name: &str,
    end: bool,
) -> Result<[f64; 3], String> {
    // `K1.A1` and `X1.1` split into a node and a pin; a terminal uses `X1:1`.
    let (node, pin) = match name.rsplit_once('.') {
        Some((n, p)) if !n.is_empty() && !p.is_empty() => (n, Some(p.to_string())),
        _ => (name, None),
    };
    let (node, pin) = match node.rsplit_once(':') {
        Some((n, p)) if !n.is_empty() && !p.is_empty() => (n, Some(p.to_string())),
        _ => (node, pin),
    };

    // A device.
    if let Some(comp) = idx.parent_by_tag(node).or_else(|| idx.component_by_tag(node)) {
        let def = lib.get(&comp.rec.symbol).ok_or_else(|| {
            format!(
                "{} uses symbol {:?}, which is not in the library — reload with EL_LIB RELOAD",
                if comp.rec.tag.is_empty() { node } else { &comp.rec.tag },
                comp.rec.symbol
            )
        })?;
        let origin = [comp.rec.origin_x, comp.rec.origin_y];
        match pin.as_deref() {
            Some(p) => def
                .pin_at(p, origin, comp.rec.rotation_rad())
                .map(|pt| [pt[0], pt[1], 0.0])
                .ok_or_else(|| {
                    format!(
                        "{} has no pin {p:?} — its pins are {}",
                        comp.rec.tag,
                        comp.rec.pins.join(", ")
                    )
                }),
            None => {
                // No pin named: the symbol's first pin is the sensible default,
                // because a two-terminal device drawn on a run joins at one end.
                let first = comp
                    .rec
                    .pins
                    .first()
                    .cloned()
                    .or_else(|| def.pins.first().map(|p| p.name.clone()))
                    .ok_or_else(|| format!("{} has no connection points", comp.rec.tag))?;
                def.pin_at(&first, origin, comp.rec.rotation_rad())
                    .map(|pt| [pt[0], pt[1], 0.0])
                    .ok_or_else(|| format!("{} has no pin {first:?}", comp.rec.tag))
            }
        }
    } else if let Some(t) = idx
        .terminals
        .values()
        .find(|t| t.rec.tag_strip == node || t.rec.designator() == name)
    {
        let pts = t.rec.connection_points();
        let pt = if end { pts[1] } else { pts[0] };
        Ok([pt[0], pt[1], 0.0])
    } else if let Some(x) = idx
        .connectors_ordered()
        .into_iter()
        .find(|x| x.rec.ref_des.eq_ignore_ascii_case(node))
    {
        let wanted = pin.clone().unwrap_or_else(|| {
            x.rec.pins.first().cloned().unwrap_or_default()
        });
        x.rec
            .pin_point_in_drawing(&wanted)
            .map(|pt| [pt[0], pt[1], 0.0])
            .ok_or_else(|| {
                format!(
                    "connector {} has no pin {wanted:?} — its pins are {}",
                    x.rec.ref_des,
                    x.rec.pins.join(",")
                )
            })
    } else if let Some(m) = idx.plcs.values().find(|m| m.rec.tag.eq_ignore_ascii_case(node)) {
        let wanted = pin.clone().unwrap_or_default();
        let point = if wanted.is_empty() {
            m.rec.point_names().into_iter().next()
        } else {
            Some(wanted)
        };
        match point.as_deref().and_then(|p| m.rec.point_position(p)) {
            Some(pt) => Ok([pt[0], pt[1], 0.0]),
            None => Err(format!(
                "PLC {} has no point {:?} — its points are {}",
                m.rec.tag,
                point.unwrap_or_default(),
                m.rec.point_names().join(", ")
            )),
        }
    } else if idx.wire_by_ref(node).is_some() {
        Err(format!("{node} is a wire number, not a connection point"))
    } else {
        Err(format!(
            "no device, terminal or connector matches {node:?} — a device is named by its tag \
             (K1) or a pin on it (K1.A1), a terminal by its designator (X1:1), a connector by its \
             tag (X1)"
        ))
    }
}

/// The nearest connection point to a drawn position, within `tol`.
///
/// This is what lets a wire drawn by eye still record what it reached, so the
/// from/to report says something instead of "unconnected".
pub fn nearest_node(
    idx: &ElIndex,
    lib: &SymbolLibrary,
    at: [f64; 3],
    tol: f64,
) -> Option<(String, [f64; 3], f64)> {
    let mut best: Option<(String, [f64; 3], f64)> = None;
    let mut consider = |name: String, pt: [f64; 3]| {
        let d = el_core::geom::dist([at[0], at[1]], [pt[0], pt[1]]);
        if d > tol {
            return;
        }
        if best.as_ref().map(|(_, _, bd)| d < *bd).unwrap_or(true) {
            best = Some((name, pt, d));
        }
    };

    for comp in idx.components.values() {
        let Some(def) = lib.get(&comp.rec.symbol) else {
            continue;
        };
        if comp.rec.tag.is_empty() {
            continue;
        }
        for p in &def.pins {
            if let Some(pt) = def.pin_at(&p.name, comp.rec.origin(), comp.rec.rotation_rad()) {
                consider(format!("{}.{}", comp.rec.tag, p.name), [pt[0], pt[1], 0.0]);
            }
        }
    }
    for t in idx.terminals.values() {
        if t.rec.tag_strip.is_empty() {
            continue;
        }
        // The designator is the node's identity; which side is reached follows
        // from which end of the wire this is, not from the name.
        for pt in t.rec.connection_points().iter() {
            consider(t.rec.designator(), [pt[0], pt[1], 0.0]);
        }
    }
    for x in idx.connectors.values() {
        if x.rec.ref_des.is_empty() {
            continue;
        }
        for p in &x.rec.pins {
            if let Some(pt) = x.rec.pin_point_in_drawing(p) {
                consider(format!("{}.{}", x.rec.ref_des, p), [pt[0], pt[1], 0.0]);
            }
        }
    }
    for m in idx.plcs.values() {
        if m.rec.tag.is_empty() {
            continue;
        }
        for p in &m.rec.point_names() {
            if let Some(pt) = m.rec.point_position(p) {
                consider(format!("{}.{}", m.rec.tag, p), [pt[0], pt[1], 0.0]);
            }
        }
    }
    best
}

/// Split a resolved node name into the record's `(id, pin)` pair.
///
/// Only a **dot** separates a pin from its device, because a device pin is
/// `K1.A1`. A colon separates a terminal's strip from its own number (`X1:1`), and
/// that whole designator is the node's identity: two terminals on strip X1 are
/// different connections, so splitting there would make them indistinguishable —
/// and the wire would then never match the terminal it was drawn to.
///
/// Which of a terminal's two sides is meant is not carried here; it follows from
/// which *end* of the wire this is, which the caller knows.
pub fn split_node(name: &str) -> (String, String) {
    if let Some((n, p)) = name.rsplit_once('.') {
        if !n.is_empty() && !p.is_empty() {
            return (n.to_string(), p.to_string());
        }
    }
    (name.to_string(), String::new())
}

/// Place a wire between two absolute points, recording what it connects.
#[allow(clippy::too_many_arguments)]
fn place_between(
    host: &mut dyn HostApi,
    idx: &ElIndex,
    from: [f64; 3],
    to: [f64; 3],
    a: &mut Args,
    from_id: &str,
    from_pin: &str,
    to_id: &str,
    to_pin: &str,
) {
    place_inner(host, idx, from, to, a, Some((from_id, from_pin, to_id, to_pin)));
}

/// Place a wire between two absolute points.
fn place(host: &mut dyn HostApi, idx: &ElIndex, from: [f64; 3], to: [f64; 3], a: &mut Args) {
    place_inner(host, idx, from, to, a, None);
}

fn place_inner(
    host: &mut dyn HostApi,
    idx: &ElIndex,
    from: [f64; 3],
    to: [f64; 3],
    a: &mut Args,
    nodes: Option<(&str, &str, &str, &str)>,
) {
    let s = idx.settings_or_default();

    // Snap to the dominant axis when close to it: a schematic reads as
    // orthogonal runs, and a hand-picked second point is rarely exactly so.
    let snapped = geom::snap_to_ortho(
        [from[0], from[1]],
        [to[0], to[1]],
        geom::ORTHO_TOL_DEG,
    );
    let to = [snapped[0], snapped[1], to[2]];

    if geom::dist([from[0], from[1]], [to[0], to[1]]) < 1e-6 {
        host.push_error("EL_WIRE: the two ends are the same point");
        return;
    }

    let mut rec = ElWireRec::new(&el_core::new_el_id(), "", "");
    // A wire placed between two named nodes records them, which is what makes the
    // from/to report say something other than "unconnected".
    if let Some((fid, fpin, tid, tpin)) = nodes {
        rec.from_id = fid.to_string();
        rec.from_pin = fpin.to_string();
        rec.to_id = tid.to_string();
        rec.to_pin = tpin.to_string();
    }
    if let Some(t) = a.get("TYPE") {
        rec.wire_type = t.to_string();
    }
    if let Some(g) = a.get("GAUGE") {
        rec.gauge = g.to_string();
    }
    if let Some(c) = a.get("COLOR") {
        rec.color = c.to_string();
    }
    // `NET=` is the harness's name for a circuit label; accept it as the note so
    // a wirelist import can carry circuit names.
    if let Some(n) = a.get("NET") {
        rec.note = n.to_string();
    }

    // A wire's defaults come from the type table, so an explicit TYPE picks up
    // its gauge and colour without the caller repeating them.
    let layer = if rec.wire_type.is_empty() {
        LAYER_WIRES.to_string()
    } else {
        idx.wire_layer_for(&rec.wire_type)
    };
    if rec.color.is_empty() {
        // The table is owned, so bind it before borrowing a type out of it.
        let table = idx.wire_types_or_default();
        if let Some(t) = table
            .get(&rec.wire_type)
            .or_else(|| table.by_layer(&layer))
        {
            rec.color = t.color.clone();
            if rec.gauge.is_empty() {
                rec.gauge = t.size.clone();
            }
        }
    }

    let record = ElRecord::encode_wire(&rec);
    let entity = wire_polyline(&record, &[from, to], &layer);

    host.push_undo("EL_WIRE");
    host.add_entity(entity);
    host.bump_geometry();
    host.set_dirty();

    let length = geom::dist([from[0], from[1]], [to[0], to[1]]);
    let rung = idx
        .rung_reference_at([(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0], RUNG_TOLERANCE_MM * 2.0)
        .unwrap_or_else(|| "—".to_string());

    host.push_output(&format!(
        "EL_WIRE: {} → {}, {}mm on {}, rung {}",
        fmt_pt(from),
        fmt_pt(to),
        el_core::fmt_mm(length),
        layer,
        rung
    ));
    host.push_info(&format!(
        "  unnumbered by design — {} to assign numbers",
        if s.wireno_mode == el_core::model::WireNoMode::Reference {
            "run EL_WIRENO (reference mode: numbers come from the rung)"
        } else {
            "run EL_WIRENO"
        }
    ));
}

fn fmt_pt(p: [f64; 3]) -> String {
    format!("({}, {})", el_core::fmt_mm(p[0]), el_core::fmt_mm(p[1]))
}

/// Interactive two-point placement.
fn start_interactive(host: &mut dyn HostApi) {
    // Everything the callback needs, taken while the host is real: the callback
    // runs against `NullHost`, where `document()` panics.
    let capture = Capture::take(host);
    let s = capture.index.settings_or_default();
    host.push_info(&format!(
        "EL_WIRE: pick the start point ({} rung(s) in this drawing)",
        capture.index.ladders.len()
    ));
    let _ = s;
    // The library is needed to resolve a picked end to a pin, and the callback
    // cannot read it.
    let lib = crate::library::load(host).unwrap_or_default();
    host.start_interactive(Box::new(WireCmd {
        capture,
        lib,
        start: None,
        placed: false,
    }));
}

/// How close a wire end must be to a connection point to be attached to it.
///
/// Generous enough to catch a hand-placed end, tight enough that two adjacent
/// terminals are never confused for one another.
pub const NODE_SNAP_MM: f64 = 2.5;

struct WireCmd {
    capture: Capture,
    lib: SymbolLibrary,
    start: Option<[f64; 3]>,
    placed: bool,
}

impl InteractiveCommand for WireCmd {
    fn prompt(&self) -> String {
        if self.start.is_some() {
            "EL_WIRE: pick the end point".to_string()
        } else {
            "EL_WIRE: pick the start point".to_string()
        }
    }

    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep {
        if self.placed {
            return CommandStep::Done;
        }
        // Snapping the first point onto an existing ladder rung is what makes a
        // wire land *on* a rung, which reference numbering depends on.
        let pt = snap_start(&self.capture, pt);

        match self.start {
            None => {
                self.start = Some(pt);
                CommandStep::NeedPoint
            }
            Some(from) => {
                self.placed = true;
                let snapped = geom::snap_to_ortho(
                    [from[0], from[1]],
                    [pt[0], pt[1]],
                    geom::ORTHO_TOL_DEG,
                );
                let to = [snapped[0], snapped[1], pt[2]];
                if geom::dist([from[0], from[1]], [to[0], to[1]]) < 1e-6 {
                    state::emit(PendingLine::Error(
                        "EL_WIRE: start and end are the same point — nothing drawn".to_string(),
                    ));
                    return CommandStep::Done;
                }

                let idx = &self.capture.index;
                let mut rec = ElWireRec::new(&el_core::new_el_id(), "", "");
                // A picked end resolves to the nearest connection point, so a wire
                // drawn by eye still records what it reached.
                if let Some((name, _, _)) =
                    nearest_node(idx, &self.lib, from, NODE_SNAP_MM)
                {
                    let (id, pin) = split_node(&name);
                    rec.from_id = id;
                    rec.from_pin = pin;
                }
                if let Some((name, _, _)) = nearest_node(idx, &self.lib, to, NODE_SNAP_MM) {
                    let (id, pin) = split_node(&name);
                    rec.to_id = id;
                    rec.to_pin = pin;
                }
                if let Some(t) = idx.wire_types_or_default().default_type() {
                    rec.wire_type = t.name.clone();
                    rec.color = t.color.clone();
                    rec.gauge = t.size.clone();
                }
                let layer = if rec.wire_type.is_empty() {
                    LAYER_WIRES.to_string()
                } else {
                    idx.wire_layer_for(&rec.wire_type)
                };
                let record = ElRecord::encode_wire(&rec);
                let entity = wire_polyline(&record, &[from, to], &layer);

                let length = geom::dist([from[0], from[1]], [to[0], to[1]]);
                let mid = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
                let rung = idx
                    .rung_reference_at(mid, RUNG_TOLERANCE_MM * 2.0)
                    .unwrap_or_else(|| "—".to_string());

                // One entity, so a plain commit is right: the host wraps it in
                // its own undo step and there is nothing to defer.
                state::emit(PendingLine::Output(format!(
                    "EL_WIRE: {} → {}, {}mm on {}, rung {}; unnumbered — run EL_WIRENO",
                    fmt_pt(from),
                    fmt_pt(to),
                    el_core::fmt_mm(length),
                    layer,
                    rung
                )));
                CommandStep::CommitAndEnd(entity)
            }
        }
    }

    fn on_enter(&mut self) -> CommandStep {
        CommandStep::Cancel
    }
}

/// Pull a picked start point onto a rung if it is near one, so a wire drawn by
/// eye still lands on the rung that will number it.
fn snap_start(capture: &Capture, pt: [f64; 3]) -> [f64; 3] {
    let Some(ladder) = capture
        .index
        .ladder_at([pt[0], pt[1]], RUNG_TOLERANCE_MM)
    else {
        return pt;
    };
    let Some(index) = ladder.rec.nearest_rung(pt[1], RUNG_TOLERANCE_MM) else {
        return pt;
    };
    [pt[0], ladder.rec.rung_y(index), pt[2]]
}

