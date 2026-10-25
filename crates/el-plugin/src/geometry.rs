//! Entity builders for electrical symbols.
//!
//! **Why symbols are primitives, not blocks.** A schematic symbol is naturally
//! a block reference with attributes. The host's `add_entity` accepts an
//! `EntityType::Insert`, but an insert only renders if the *block definition*
//! exists in the document, and the plugin contract exposes no way to create
//! one (there is no `AddBlockRecord` request at API 5). An insert with no
//! definition draws nothing. So every symbol here is built from primitives —
//! `LwPolyline`, `Circle`, `Line`, `MText` — with the record carried as XDATA
//! on the carrier entity. `probe.rs` tests whether that ever changes.
//!
//! All XDATA is embedded in the entity *before* it is handed to the host, so a
//! symbol and its data commit atomically: there is no window in which a symbol
//! exists without its record.

use ocs_plugin_api::host::codec::entities::mtext::AttachmentPoint;
use ocs_plugin_api::host::codec::entities::EntityCommon;
use ocs_plugin_api::host::codec::types::{Vector2, Vector3};
use ocs_plugin_api::host::codec::{Circle, Color, EntityType, Line, LwPolyline, MText, Point};

use el_core::codec::ElRecord;
use el_core::model::{
    color_to_aci, LadderRec, LABEL_HEIGHT, LAYER_LADDER, LAYER_WIRENO, META_LAYER,
    RUNG_REF_HEIGHT,
};
use el_core::symbol::{rotate, GlyphPrim, SymbolDef};

/// Attach a record's XDATA to an entity under construction.
pub(crate) fn attach(record: &ElRecord, common: &mut EntityCommon) {
    common.extended_data.upsert_record(crate::codec::to_xdata(record));
}

/// A text label anchored above `at`.
#[allow(dead_code)] // consumed by the Phase 1 placement commands
pub fn label(text: &str, at: [f64; 3], height: f64, layer: &str) -> MText {
    let mut m = MText::new();
    m.value = text.to_string();
    m.insertion_point = Vector3::new(at[0], at[1] + height * 1.4, at[2]);
    m.height = height;
    m.style = "Standard".to_string();
    m.attachment_point = AttachmentPoint::BottomCenter;
    // MText::new() defaults the wrap width to 10 units, which wraps every
    // label at its spaces. DXF width 0 means "no wrapping".
    m.rectangle_width = 0.0;
    m.common.layer = layer.to_string();
    m
}


/// The invisible carrier any metadata record (settings, ladder, report) lives
/// on: a point on the meta layer, far from the drawing content so it neither
/// prints nor gets picked by accident.
pub fn meta_carrier(record: &ElRecord) -> EntityType {
    let mut point = Point::at(Vector3::new(0.0, 0.0, 0.0));
    point.common.layer = META_LAYER.to_string();
    attach(record, &mut point.common);
    EntityType::Point(point)
}

/// The anchor of a carrier entity.
///
/// By convention a carrier's *first vertex* is its anchor (for a polyline
/// glyph), so an attach point is recoverable from the drawn geometry alone —
/// which matters because the record never stores coordinates.
pub fn carrier_anchor(entity: &EntityType) -> Option<[f64; 3]> {
    match entity {
        EntityType::Circle(c) => Some([c.center.x, c.center.y, c.center.z]),
        EntityType::Point(p) => Some([p.location.x, p.location.y, p.location.z]),
        EntityType::LwPolyline(pl) => pl
            .vertices
            .first()
            .map(|v| [v.location.x, v.location.y, pl.elevation]),
        EntityType::Line(l) => Some([l.start.x, l.start.y, l.start.z]),
        EntityType::Insert(ins) => Some([ins.insert_point.x, ins.insert_point.y, ins.insert_point.z]),
        _ => None,
    }
}

// ── wires ────────────────────────────────────────────────────────────────

/// Build a wire polyline with its record attached, coloured by conductor code.
///
/// The record is embedded before the entity reaches the host, so a wire and its
/// data commit together with no window in which one exists without the other.
pub fn wire_polyline(record: &ElRecord, points: &[[f64; 3]], layer: &str) -> EntityType {
    let verts: Vec<Vector2> = points.iter().map(|p| Vector2::new(p[0], p[1])).collect();
    let mut pl = LwPolyline::from_points(verts);
    pl.common.layer = layer.to_string();
    if let Ok(wire) = record.decode_wire() {
        if let Some(aci) = color_to_aci(&wire.color) {
            pl.common.color = Color::Index(aci);
        }
    }
    pl.elevation = points.first().map(|p| p[2]).unwrap_or(0.0);
    attach(record, &mut pl.common);
    EntityType::LwPolyline(pl)
}

/// The polyline vertex list of a wire, as `[x, y, z]`, or `None` for anything
/// that is not a drawn polyline.
pub fn polyline_points(entity: &EntityType) -> Option<Vec<[f64; 3]>> {
    match entity {
        EntityType::LwPolyline(pl) => Some(
            pl.vertices
                .iter()
                .map(|v| [v.location.x, v.location.y, pl.elevation])
                .collect(),
        ),
        _ => None,
    }
}

/// A gapped wire drawn as its runs.
///
/// Every run carries the record — the same rule a ladder's parts follow — so
/// the index folds the runs back into one wire no matter which one a user
/// picks, and any consumer that thinks in whole wires reads them through
/// `wire_of_handle`.
pub fn wire_runs_entities(
    record: &ElRecord,
    runs: &[Vec<[f64; 3]>],
    layer: &str,
) -> Vec<EntityType> {
    runs.iter()
        .filter(|r| r.len() >= 2)
        .map(|r| wire_polyline(record, r, layer))
        .collect()
}


/// A wire-number label, placed above the wire's midpoint.
pub fn wire_number_label(text: &str, at: [f64; 3]) -> EntityType {
    EntityType::MText(Box::new(label(text, at, RUNG_REF_HEIGHT, LAYER_WIRENO)))
}

// ── ladder ───────────────────────────────────────────────────────────────

/// Build a ladder: the bus verticals, the rungs, and a line reference per rung.
///
/// **Every drawn part carries the record.** A ladder is a frame, not a line, and
/// the plugin contract offers no way to group entities, so the shared `el_id` in
/// each part's record *is* the grouping: it lets the index fold the parts back
/// into one object, and lets the ladder be redrawn or deleted as a unit no
/// matter which part a user picks. The alternative — a record on one "carrier"
/// plus anonymous companions — would leave the companions unfindable.
///
/// Returns the parts in draw order; `None` when the settings produce nothing.
pub fn ladder_entities(record: &ElRecord, rec: &LadderRec) -> Vec<EntityType> {
    let y_top = rec.origin_y;
    let y_bottom = rec.last_rung_y();
    let x_left = rec.origin_x;
    let x_right = rec.right_x();

    let mut parts: Vec<EntityType> = Vec::new();

    // Verticals frame the ladder; three-phase adds the middle phase bus.
    if rec.draw_bus {
        let mut xs = vec![x_left, x_right];
        if rec.phase >= 3 {
            xs.push((x_left + x_right) / 2.0);
        }
        for x in xs {
            let mut line = Line::from_coords(x, y_top, 0.0, x, y_bottom, 0.0);
            line.common.layer = LAYER_LADDER.to_string();
            parts.push(EntityType::Line(line));
        }
    }

    for i in 0..rec.rung_count.max(1) as usize {
        let y = rec.rung_y(i);
        if rec.draw_rungs {
            let mut line = Line::from_coords(x_left, y, 0.0, x_right, y, 0.0);
            line.common.layer = LAYER_LADDER.to_string();
            parts.push(EntityType::Line(line));
        }
        if rec.show_references {
            parts.push(EntityType::MText(Box::new(label(
                &rec.rung_reference(i),
                [x_right + 2.0, y - RUNG_REF_HEIGHT * 0.5, 0.0],
                RUNG_REF_HEIGHT,
                LAYER_LADDER,
            ))));
        }
    }

    for part in &mut parts {
        attach(record, part.common_mut());
    }
    parts
}



/// Translate an entity by a 3D offset.
#[allow(dead_code)] // consumed by the Phase 1 placement commands
pub fn translate_entity(entity: &mut EntityType, delta: [f64; 3]) {
    match entity {
        EntityType::Point(p) => {
            p.location.x += delta[0];
            p.location.y += delta[1];
            p.location.z += delta[2];
        }
        EntityType::Circle(c) => {
            c.center.x += delta[0];
            c.center.y += delta[1];
            c.center.z += delta[2];
        }
        EntityType::LwPolyline(pl) => {
            for v in &mut pl.vertices {
                v.location.x += delta[0];
                v.location.y += delta[1];
            }
            pl.elevation += delta[2];
        }
        EntityType::MText(m) => {
            m.insertion_point.x += delta[0];
            m.insertion_point.y += delta[1];
            m.insertion_point.z += delta[2];
        }
        EntityType::Line(l) => {
            l.start.x += delta[0];
            l.start.y += delta[1];
            l.start.z += delta[2];
            l.end.x += delta[0];
            l.end.y += delta[1];
            l.end.z += delta[2];
        }
        _ => {}
    }
}

/// Rotate a 2D offset `[dx, dy]` by `angle_rad` counter-clockwise.
#[allow(dead_code)] // consumed by the Phase 1 placement commands
pub fn rotate_vec(v: [f64; 2], angle_rad: f64) -> [f64; 2] {
    if angle_rad.abs() < 1e-9 {
        return v;
    }
    let (s, c) = angle_rad.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c]
}

// ── components ───────────────────────────────────────────────────────────

/// The layer component glyphs are drawn on.
pub const LAYER_COMPONENTS: &str = el_core::model::LAYER_COMPONENTS;

/// Text height for cross-reference annotations, mm.
pub const XREF_HEIGHT: f64 = 2.0;

/// Build a placed component: its glyph, its reference designator, and its
/// record on **every** part.
///
/// Every part carries the record for the same reason a ladder's does — the
/// contract offers no way to group entities, so the shared `el_id` *is* the
/// grouping. A symbol is a coil, a label and possibly a dozen primitives, and it
/// must move, retag and delete as one object.
pub fn component_entities(
    record: &ElRecord,
    def: &SymbolDef,
    tag: &str,
    origin: [f64; 3],
    rot_rad: f64,
) -> Vec<EntityType> {
    let anchor = [origin[0], origin[1]];
    let mut parts: Vec<EntityType> = Vec::new();

    for prim in &def.glyph {
        match prim {
            GlyphPrim::Line { a, b } => {
                let a = rotate(*a, rot_rad);
                let b = rotate(*b, rot_rad);
                let mut line = Line::from_coords(
                    anchor[0] + a[0],
                    anchor[1] + a[1],
                    origin[2],
                    anchor[0] + b[0],
                    anchor[1] + b[1],
                    origin[2],
                );
                line.common.layer = LAYER_COMPONENTS.to_string();
                parts.push(EntityType::Line(line));
            }
            GlyphPrim::Polyline { points, closed } => {
                let verts: Vec<Vector2> = points
                    .iter()
                    .map(|p| {
                        let r = rotate(*p, rot_rad);
                        Vector2::new(anchor[0] + r[0], anchor[1] + r[1])
                    })
                    .collect();
                let mut pl = LwPolyline::from_points(verts);
                pl.is_closed = *closed;
                pl.elevation = origin[2];
                pl.common.layer = LAYER_COMPONENTS.to_string();
                parts.push(EntityType::LwPolyline(pl));
            }
            GlyphPrim::Circle { center, radius } => {
                let c = rotate(*center, rot_rad);
                let mut circle = Circle::from_center_radius(
                    Vector3::new(anchor[0] + c[0], anchor[1] + c[1], origin[2]),
                    *radius,
                );
                circle.common.layer = LAYER_COMPONENTS.to_string();
                parts.push(EntityType::Circle(circle));
            }
            GlyphPrim::Text { at, height, text } => {
                // In-glyph text (the `M` in a motor) turns with the symbol and is
                // centred on its point rather than annotated above it.
                let r = rotate(*at, rot_rad);
                let mut m = label(
                    text,
                    [anchor[0] + r[0], anchor[1] + r[1] - height * 0.7, origin[2]],
                    *height,
                    LAYER_COMPONENTS,
                );
                m.rotation = rot_rad;
                parts.push(EntityType::MText(Box::new(m)));
            }
        }
    }

    if !tag.is_empty() {
        let t = def.tag_at(anchor, rot_rad);
        parts.push(EntityType::MText(Box::new(label(
            tag,
            [t[0], t[1], origin[2]],
            LABEL_HEIGHT,
            el_core::model::LAYER_TAGS,
        ))));
    }

    for part in &mut parts {
        attach(record, part.common_mut());
    }
    parts
}

/// A component's connection point in the drawing. Used by wire attachment and
/// cross-reference leaders once components and wires are related.
#[allow(dead_code)] // consumed when wires attach to pins
pub fn component_pin_point(
    def: &SymbolDef,
    origin: [f64; 3],
    rot_rad: f64,
    pin: &str,
) -> Option<[f64; 3]> {
    let p = def.pin_at(pin, [origin[0], origin[1]], rot_rad)?;
    Some([p[0], p[1], origin[2]])
}

/// The nearest pin to a point, so a wire dropped on a symbol attaches to the pin
/// the user meant rather than to the shell.
#[allow(dead_code)] // consumed when wires attach to pins
pub fn nearest_pin(
    def: &SymbolDef,
    origin: [f64; 3],
    rot_rad: f64,
    at: [f64; 3],
) -> Option<(String, [f64; 3], f64)> {
    def.pins
        .iter()
        .filter_map(|p| {
            let abs = component_pin_point(def, origin, rot_rad, &p.name)?;
            let d = el_core::geom::dist([at[0], at[1]], [abs[0], abs[1]]);
            Some((p.name.clone(), abs, d))
        })
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
}

/// A cross-reference annotation: a small text note beside a device.
pub fn xref_label(text: &str, at: [f64; 3]) -> EntityType {
    EntityType::MText(Box::new(label(text, at, XREF_HEIGHT, el_core::model::LAYER_XREF)))
}

// ── terminals ────────────────────────────────────────────────────────────

/// The layer terminal glyphs are drawn on.
pub const LAYER_TERMINALS: &str = el_core::model::LAYER_TERMINALS;

/// The layer cable markers are drawn on.
pub const LAYER_CABLES: &str = el_core::model::LAYER_CABLES;
pub const LAYER_SIGNALS: &str = el_core::model::LAYER_SIGNALS;

/// Half-width of a terminal block glyph, mm. One rung at the default spacing.
const TERMINAL_HALF_W: f64 = 5.0;
/// Half-height of a terminal block glyph, mm.
const TERMINAL_HALF_H: f64 = 3.0;

/// Build a terminal block: a square outline with a connection point at each end.
///
/// The outline is the IEC terminal symbol — a plain square — and the two ends are
/// where wires land. `Ground`, `Fused` and `Disconnect` classes add the mark the
/// standard gives them, so a strip is readable without consulting a schedule.
pub fn terminal_entities(
    record: &ElRecord,
    designator: &str,
    at: [f64; 3],
    rot_rad: f64,
    kind: el_core::model::TerminalKind,
) -> Vec<EntityType> {
    use el_core::model::TerminalKind;
    let anchor = [at[0], at[1]];
    let mut parts: Vec<EntityType> = Vec::new();

    let local = |p: [f64; 2]| {
        let r = rotate(p, rot_rad);
        Vector2::new(anchor[0] + r[0], anchor[1] + r[1])
    };

    // The body.
    let corners = vec![
        local([-TERMINAL_HALF_W, TERMINAL_HALF_H]),
        local([TERMINAL_HALF_W, TERMINAL_HALF_H]),
        local([TERMINAL_HALF_W, -TERMINAL_HALF_H]),
        local([-TERMINAL_HALF_W, -TERMINAL_HALF_H]),
    ];
    let mut body = LwPolyline::from_points(corners);
    body.is_closed = true;
    body.elevation = at[2];
    body.common.layer = LAYER_TERMINALS.to_string();
    parts.push(EntityType::LwPolyline(body));

    // The leads, which are the connection points.
    for x in [TERMINAL_HALF_W, -TERMINAL_HALF_W] {
        let sign = if x > 0.0 { 1.0 } else { -1.0 };
        let a = local([x, 0.0]);
        let b = local([x + sign * 2.5, 0.0]);
        let mut line = Line::from_coords(a.x, a.y, at[2], b.x, b.y, at[2]);
        line.common.layer = LAYER_TERMINALS.to_string();
        parts.push(EntityType::Line(line));
    }

    // The class mark inside the body.
    match kind {
        TerminalKind::Ground => {
            // The earth bars, shortened inward.
            for (i, w) in [2.6f64, 1.6, 0.6].iter().enumerate() {
                let y = 1.2 - i as f64 * 1.0;
                let a = local([-w, y]);
                let b = local([*w, y]);
                let mut line = Line::from_coords(a.x, a.y, at[2], b.x, b.y, at[2]);
                line.common.layer = LAYER_TERMINALS.to_string();
                parts.push(EntityType::Line(line));
            }
        }
        TerminalKind::Fused => {
            // A fuse link across the block.
            let a = local([-2.4, 0.0]);
            let b = local([2.4, 0.0]);
            let mut line = Line::from_coords(a.x, a.y, at[2], b.x, b.y, at[2]);
            line.common.layer = LAYER_TERMINALS.to_string();
            parts.push(EntityType::Line(line));
            let rect = vec![
                local([-1.4, 0.9]),
                local([1.4, 0.9]),
                local([1.4, -0.9]),
                local([-1.4, -0.9]),
            ];
            let mut r = LwPolyline::from_points(rect);
            r.is_closed = true;
            r.elevation = at[2];
            r.common.layer = LAYER_TERMINALS.to_string();
            parts.push(EntityType::LwPolyline(r));
        }
        TerminalKind::Disconnect => {
            // An open blade.
            let a = local([-2.4, 0.0]);
            let b = local([-0.6, 0.0]);
            let mut l1 = Line::from_coords(a.x, a.y, at[2], b.x, b.y, at[2]);
            l1.common.layer = LAYER_TERMINALS.to_string();
            parts.push(EntityType::Line(l1));
            let c = local([0.8, 0.0]);
            let d = local([2.4, 0.0]);
            let mut l2 = Line::from_coords(c.x, c.y, at[2], d.x, d.y, at[2]);
            l2.common.layer = LAYER_TERMINALS.to_string();
            parts.push(EntityType::Line(l2));
            let e = local([-0.6, 0.0]);
            let f = local([1.0, 1.5]);
            let mut blade = Line::from_coords(e.x, e.y, at[2], f.x, f.y, at[2]);
            blade.common.layer = LAYER_TERMINALS.to_string();
            parts.push(EntityType::Line(blade));
        }
        TerminalKind::Multi => {
            // A second level, drawn as an offset inner box.
            let rect = vec![
                local([-3.4, 1.6]),
                local([3.4, 1.6]),
                local([3.4, -1.6]),
                local([-3.4, -1.6]),
            ];
            let mut r = LwPolyline::from_points(rect);
            r.is_closed = true;
            r.elevation = at[2];
            r.common.layer = LAYER_TERMINALS.to_string();
            parts.push(EntityType::LwPolyline(r));
        }
        TerminalKind::Test => {
            let c = local([0.0, 0.0]);
            let mut circle = Circle::from_center_radius(Vector3::new(c.x, c.y, at[2]), 1.0);
            circle.common.layer = LAYER_TERMINALS.to_string();
            parts.push(EntityType::Circle(circle));
        }
        TerminalKind::Screw | TerminalKind::Spring => {}
    }

    if !designator.is_empty() {
        let t = local([0.0, TERMINAL_HALF_H + LABEL_HEIGHT * 0.6]);
        parts.push(EntityType::MText(Box::new(label(
            designator,
            [t.x, t.y, at[2]],
            LABEL_HEIGHT,
            LAYER_TERMINALS,
        ))));
    }

    for part in &mut parts {
        attach(record, part.common_mut());
    }
    parts
}

/// Where a wire attaches to a terminal.
pub fn terminal_pin_point(at: [f64; 3], rot_rad: f64, right: bool) -> [f64; 3] {
    let sign = if right { 1.0 } else { -1.0 };
    let r = rotate([sign * (TERMINAL_HALF_W + 2.5), 0.0], rot_rad);
    [at[0] + r[0], at[1] + r[1], at[2]]
}

// ── cable markers ────────────────────────────────────────────────────────

/// Build a cable marker: a flagged tag showing the cable and its conductor.
///
/// The marker's whole job is to be readable on a crowded sheet, so it is a filled
/// flag with the tag on it — and its colour is the conductor's, which is how a
/// drafter tells two conductors of one cable apart at a glance.
pub fn cable_marker_entities(
    record: &ElRecord,
    text: &str,
    at: [f64; 3],
    rot_rad: f64,
    color: &str,
) -> Vec<EntityType> {
    let anchor = [at[0], at[1]];
    let half_h = 2.5;
    let half_w = 5.0;
    let local = |p: [f64; 2]| {
        let r = rotate(p, rot_rad);
        Vector2::new(anchor[0] + r[0], anchor[1] + r[1])
    };

    let corners = vec![
        local([-half_w, half_h]),
        local([half_w, half_h]),
        local([half_w, -half_h]),
        local([-half_w, -half_h]),
    ];
    let mut body = LwPolyline::from_points(corners);
    body.is_closed = true;
    body.elevation = at[2];
    body.common.layer = LAYER_CABLES.to_string();
    if let Some(aci) = color_to_aci(color) {
        body.common.color = Color::Index(aci);
    }
    attach(record, &mut body.common);

    let mut parts = vec![EntityType::LwPolyline(body)];

    if !text.is_empty() {
        let t = local([0.0, -half_h * 0.5]);
        let mut m = label(text, [t.x, t.y, at[2]], LABEL_HEIGHT, LAYER_CABLES);
        m.rotation = rot_rad;
        let mut e = EntityType::MText(Box::new(m));
        attach(record, e.common_mut());
        parts.push(e);
    }
    parts
}

// ── signals ──────────────────────────────────────────────────────────────

/// Build a signal arrow: a triangle naming its direction, a tail along the
/// wire it anchors to, and a label.
///
/// **Every drawn part carries the record** — the same rule a component's parts
/// follow — so the arrow is one object to pick, move and delete no matter
/// which part of it a user grabs. `note` is the cross-sheet pairing the
/// project register supplies (`→ 02`, `← 01`); it is computed by the caller at
/// render time so a sync can refresh labels without touching records.
pub fn signal_entities(
    record: &ElRecord,
    rec: &el_core::model::SignalRec,
    at: [f64; 3],
    rot_rad: f64,
    note: &str,
) -> Vec<EntityType> {
    use el_core::model::SignalDirection;
    let anchor = [at[0], at[1]];
    let local = |p: [f64; 2]| {
        let r = rotate(p, rot_rad);
        Vector2::new(anchor[0] + r[0], anchor[1] + r[1])
    };

    // A source arrow points away from the sheet (drawn pointing +x at rest);
    // a destination arrow points into it. The tail sits behind the point so
    // the arrowhead lands exactly on the wire end.
    let (tip, back1, back2, tail) = match rec.direction {
        SignalDirection::Source => ([4.0, 0.0], [0.0, 1.8], [0.0, -1.8], [-6.0, 0.0]),
        SignalDirection::Dest => ([-4.0, 0.0], [0.0, 1.8], [0.0, -1.8], [6.0, 0.0]),
    };
    let head = vec![local(tip), local(back1), local(back2)];
    let mut tri = LwPolyline::from_points(head);
    tri.is_closed = true;
    tri.elevation = at[2];
    tri.common.layer = LAYER_SIGNALS.to_string();
    tri.common.color = Color::Index(1);
    attach(record, &mut tri.common);

    let mut tail_line = LwPolyline::from_points(vec![local(tail), local([0.0, 0.0])]);
    tail_line.elevation = at[2];
    tail_line.common.layer = LAYER_SIGNALS.to_string();
    tail_line.common.color = Color::Index(1);
    attach(record, &mut tail_line.common);

    let mut parts = vec![EntityType::LwPolyline(tri), EntityType::LwPolyline(tail_line)];

    let text = match note.is_empty() {
        true => rec.name.clone(),
        false => format!("{} {note}", rec.name),
    };
    if !text.is_empty() {
        let t = local(match rec.direction {
            SignalDirection::Source => [-2.0, 2.6],
            SignalDirection::Dest => [2.0, 2.6],
        });
        let mut m = label(&text, [t.x, t.y, at[2]], LABEL_HEIGHT, LAYER_SIGNALS);
        m.rotation = rot_rad;
        let mut e = EntityType::MText(Box::new(m));
        attach(record, e.common_mut());
        parts.push(e);
    }
    parts
}

// ── connectors ───────────────────────────────────────────────────────────

/// Build a parametric connector: a body outline, a pin stub per pin, and the
/// pin numbers.
///
/// The pins are *generated*, so the glyph is derived from the record rather than
/// stored — which is what makes changing the pin count a record edit rather than
/// a redraw.
pub fn connector_entities(
    record: &ElRecord,
    rec: &el_core::model::ConnectorRec,
    at: [f64; 3],
    rot_rad: f64,
) -> Vec<EntityType> {
    let anchor = [at[0], at[1]];
    let mut parts: Vec<EntityType> = Vec::new();
    let local = |p: [f64; 2]| {
        let r = rotate(p, rot_rad);
        Vector2::new(anchor[0] + r[0], anchor[1] + r[1])
    };

    // The body encloses the pin field with a margin.
    let e = rec.extent();
    let margin = rec.pin_spacing_mm * 0.6;
    let corners = vec![
        local([e[0] - margin, e[3] + margin]),
        local([e[2] + margin, e[3] + margin]),
        local([e[2] + margin, e[1] - margin]),
        local([e[0] - margin, e[1] - margin]),
    ];
    let mut body = LwPolyline::from_points(corners);
    body.is_closed = true;
    body.elevation = at[2];
    body.common.layer = LAYER_COMPONENTS.to_string();
    parts.push(EntityType::LwPolyline(body));

    // A stub and a number per pin, so the connector can actually be wired.
    let stub = rec.pin_spacing_mm * 0.8;
    for (i, name) in rec.pins.iter().enumerate() {
        let off = rec.pin_offset(i);
        let from = local(off);
        let to = local([off[0], off[1] + stub]);
        let mut line = Line::from_coords(from.x, from.y, at[2], to.x, to.y, at[2]);
        line.common.layer = LAYER_COMPONENTS.to_string();
        parts.push(EntityType::Line(line));

        let num = local([off[0], off[1] - LABEL_HEIGHT * 0.9]);
        parts.push(EntityType::MText(Box::new(label(
            name,
            [num.x, num.y, at[2]],
            LABEL_HEIGHT * 0.9,
            LAYER_COMPONENTS,
        ))));
    }

    if !rec.ref_des.is_empty() {
        let t = local([e[0], e[3] + margin + LABEL_HEIGHT * 0.8]);
        parts.push(EntityType::MText(Box::new(label(
            &rec.ref_des,
            [t.x, t.y, at[2]],
            LABEL_HEIGHT,
            el_core::model::LAYER_TAGS,
        ))));
    }

    for part in &mut parts {
        attach(record, part.common_mut());
    }
    parts
}

// ── report tables ────────────────────────────────────────────────────────

/// The layer an on-canvas report table is drawn on.
pub const LAYER_TABLE: &str = el_core::model::LAYER_TABLE;

/// Character advance as a fraction of text height, for a monospace-ish outline
/// font. Approximate on purpose: the table only has to line up with itself.
const TABLE_CHAR_W: f64 = 0.62;

/// Build an on-canvas table from a report layout.
///
/// Rendered as text plus rules rather than a host `TABLE` entity, for the same
/// reason symbols are primitives: the drawing carries everything it needs, and a
/// consumer that does not know about table entities still sees the numbers.
pub fn table_entities(
    layout: &el_core::tables::TableLayout,
    at: [f64; 3],
    text_height: f64,
    layer: &str,
) -> Vec<EntityType> {
    let mut parts: Vec<EntityType> = Vec::new();
    let char_w = text_height * TABLE_CHAR_W;
    let row_pitch = text_height * 1.7;

    // Column x offsets, left to right.
    let mut col_x: Vec<f64> = Vec::with_capacity(layout.col_count());
    let mut x = 0.0;
    for w in &layout.col_widths {
        col_x.push(x);
        x += (*w as f64 + 3.0) * char_w;
    }
    let total_w = x;

    let mut y = at[1];

    if !layout.title.is_empty() {
        parts.push(EntityType::MText(Box::new(label(
            &layout.title,
            [at[0], y, at[2]],
            text_height * 1.2,
            layer,
        ))));
        y -= row_pitch * 1.4;
    }

    // Header.
    let header_y = y;
    for (c, cell) in (0..layout.col_count()).enumerate().map(|(c, _)| (c, layout.header(c))) {
        parts.push(EntityType::MText(Box::new(label(
            cell.trim_end(),
            [at[0] + col_x[c], y, at[2]],
            text_height,
            layer,
        ))));
    }
    y -= row_pitch * 0.6;
    parts.push(rule(at[0], y, at[0] + total_w, y, at[2], layer));
    y -= row_pitch * 0.5;

    for r in 0..layout.row_count() {
        for c in 0..layout.col_count() {
            let cell = layout.cell(r, c);
            parts.push(EntityType::MText(Box::new(label(
                cell.trim_end(),
                [at[0] + col_x[c], y, at[2]],
                text_height,
                layer,
            ))));
        }
        y -= row_pitch;
    }

    // Column rules, drawn after the rows so they bound the whole table.
    let bottom = y + row_pitch * 0.3;
    for cx in &col_x {
        parts.push(rule(
            at[0] + cx - char_w,
            header_y + row_pitch * 0.4,
            at[0] + cx - char_w,
            bottom,
            at[2],
            layer,
        ));
    }
    parts.push(rule(
        at[0] + total_w,
        header_y + row_pitch * 0.4,
        at[0] + total_w,
        bottom,
        at[2],
        layer,
    ));

    parts
}

fn rule(x1: f64, y1: f64, x2: f64, y2: f64, z: f64, layer: &str) -> EntityType {
    let mut line = Line::from_coords(x1, y1, z, x2, y2, z);
    line.common.layer = layer.to_string();
    EntityType::Line(line)
}

/// The drawn height of a table, so a caller can place the next one below it.
pub fn table_height(layout: &el_core::tables::TableLayout, text_height: f64) -> f64 {
    let rows = layout.row_count() as f64 + if layout.title.is_empty() { 1.5 } else { 3.0 };
    rows * text_height * 1.7
}

#[cfg(test)]
mod tests {
    use super::*;
    use el_core::model::{ElSettingsRec, KIND_SETTINGS};

    #[test]
    fn rotate_vec_is_identity_at_zero() {
        assert_eq!(rotate_vec([3.0, 4.0], 0.0), [3.0, 4.0]);
    }

    #[test]
    fn rotate_vec_quarter_turn_maps_x_to_y() {
        let r = rotate_vec([1.0, 0.0], std::f64::consts::FRAC_PI_2);
        assert!((r[0]).abs() < 1e-9);
        assert!((r[1] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn carrier_anchor_reads_a_point() {
        let e = EntityType::Point(Point::at(Vector3::new(5.0, 6.0, 0.0)));
        assert_eq!(carrier_anchor(&e), Some([5.0, 6.0, 0.0]));
    }

    #[test]
    fn translate_moves_a_point() {
        let mut e = EntityType::Point(Point::at(Vector3::new(1.0, 2.0, 0.0)));
        translate_entity(&mut e, [10.0, -1.0, 0.0]);
        assert_eq!(carrier_anchor(&e), Some([11.0, 1.0, 0.0]));
    }

    #[test]
    fn meta_carrier_carries_its_record_on_the_meta_layer() {
        let rec = ElRecord::encode_settings(&ElSettingsRec::default());
        let e = meta_carrier(&rec);
        assert_eq!(e.common().layer, META_LAYER);
        let decoded = crate::codec::record_of(&e).expect("record embedded before handoff");
        assert_eq!(decoded.kind(), Some(KIND_SETTINGS));
    }

    #[test]
    fn label_disables_wrapping() {
        // A wrapped label is one of the most visible possible defects: every
        // ref-des containing a space would break onto two lines.
        let m = label("K1 CONTACT", [0.0, 0.0, 0.0], 2.5, "EL_TAGS");
        assert_eq!(m.rectangle_width, 0.0);
        assert_eq!(m.style, "Standard");
    }
}
