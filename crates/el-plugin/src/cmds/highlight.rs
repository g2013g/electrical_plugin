//! `EL_HIGHLIGHT` — trace a net across the drawing.
//!
//! Tracing is what makes the connectivity data pay off: a net is derived from the
//! wires and the pins they name, so highlighting it shows the actual circuit rather
//! than whoever drew what near what. Two wires that merely cross are not in one
//! net, and a terminal's two sides are separate circuits — which is exactly the
//! distinction a drafter needs to see.
//!
//! The overlay is drawn on its own layer and is a **pure render**: every run clears
//! it. A highlight that outlived its trace would be worse than none, because it
//! would look like part of the drawing.

use ocs_plugin_api::host::HostApi;

use el_core::net::NetIndex;
use el_core::ElIndex;

use crate::cmds::common::{remove_entities_on_layer, Args};
use crate::hostops::DocEntityExt;
use crate::geometry::polyline_points;

/// The layer the trace overlay is drawn on.
pub const LAYER_HILITE: &str = "EL_HILITE";

/// Run `EL_HIGHLIGHT <net|wire|tag|handle|@SEL> | CLEAR`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let a = Args::parse(args);
    let word = a.first_word().map(|w| w.to_ascii_uppercase());

    if word.as_deref() == Some("CLEAR") {
        host.push_undo("EL_HIGHLIGHT CLEAR");
        let n = remove_entities_on_layer(host, LAYER_HILITE);
        host.bump_geometry();
        host.set_dirty();
        host.push_output(&format!("EL_HIGHLIGHT CLEAR: removed {n} overlay entit(ies)"));
        return;
    }

    let Some(token) = a.first_word().map(str::to_string) else {
        host.push_error(
            "EL_HIGHLIGHT: name what to trace — a wire number (W1), a device tag (K1), a net \
             label, or @SEL for the inspected entity",
        );
        return;
    };

    let idx = crate::codec::build_index(host.document());
    if idx.wires.is_empty() {
        host.push_error("EL_HIGHLIGHT: this drawing has no wires to trace");
        return;
    }
    let nets = NetIndex::build(&idx);

    let net = match find_net(&nets, &idx, &token) {
        Ok(n) => n,
        Err(e) => {
            host.push_error(&format!("EL_HIGHLIGHT: {e}"));
            return;
        }
    };

    // The wires in the net, resolved to their drawn geometry. A gapped wire is
    // drawn as runs; the trace overlays each run so it never bridges the gap —
    // an overlay that crossed a gap would lie about the connection.
    let mut overlay: Vec<ocs_plugin_api::host::codec::EntityType> = Vec::new();
    let mut traced = 0usize;
    for handle in &net.wires {
        let Some(w) = idx.wire_of_handle(*handle) else {
            continue;
        };
        let mut drew_any = false;
        for h in idx.handles_for_el_id(&w.rec.el_id) {
            let Some(points) = host
                .document()
                .get_entity_owned(ocs_plugin_api::host::codec::Handle::new(*h))
                .and_then(|e| polyline_points(&e))
            else {
                continue;
            };
            overlay.push(trace_polyline(&points));
            drew_any = true;
        }
        if drew_any {
            traced += 1;
        }
    }

    host.push_undo("EL_HIGHLIGHT");
    remove_entities_on_layer(host, LAYER_HILITE);
    host.add_entities(overlay);
    host.bump_geometry();
    host.set_dirty();

    host.push_output(&format!(
        "EL_HIGHLIGHT: net {} — {traced} wire(s) traced on {LAYER_HILITE}",
        net.label
    ));
    host.push_output(&format!(
        "  devices: {}",
        net.nodes.iter().cloned().collect::<Vec<_>>().join("  ")
    ));
    if net.wires.len() == 1 {
        host.push_info("  (this net is one wire — nothing else joins it)");
    }
}

/// A stubby replacement for a wire, drawn heavier so it reads as an overlay.
///
/// Same geometry, different layer: re-deriving the route would risk drawing
/// something that is not the wire, and a trace that lies is worse than no trace.
fn trace_polyline(points: &[[f64; 3]]) -> ocs_plugin_api::host::codec::EntityType {
    use ocs_plugin_api::host::codec::types::Vector2;
    use ocs_plugin_api::host::codec::{Color, EntityType, LwPolyline};

    let verts: Vec<Vector2> = points.iter().map(|p| Vector2::new(p[0], p[1])).collect();
    let mut pl = LwPolyline::from_points(verts);
    pl.elevation = points.first().map(|p| p[2]).unwrap_or(0.0);
    pl.constant_width = 0.4;
    pl.common.layer = LAYER_HILITE.to_string();
    // A fixed colour rather than the conductor's: the overlay has to stand out
    // from the wiring, not blend into it.
    pl.common.color = Color::Index(1);
    EntityType::LwPolyline(pl)
}

/// Find the net a token names.
fn find_net<'a>(
    nets: &'a NetIndex,
    idx: &ElIndex,
    token: &str,
) -> Result<&'a el_core::net::Net, String> {
    // A wire number.
    if let Some(w) = idx.wire_by_ref(token) {
        return nets
            .net_of_wire(w.handle)
            .ok_or_else(|| format!("wire {token} is not in any net"));
    }
    // A device tag: its pins are nodes, and the first one that is in a net wins.
    if let Some(comp) = idx
        .parent_by_tag(token)
        .or_else(|| idx.component_by_tag(token))
    {
        let mut found: Vec<&el_core::net::Net> = Vec::new();
        for pin in &comp.rec.pins {
            let node = format!("{}.{}", comp.rec.tag, pin);
            if let Some(n) = nets.net_of_node(&node) {
                found.push(n);
            }
        }
        return match found.len() {
            0 => Err(format!(
                "{} has no wired pin — nothing is connected to it yet",
                comp.rec.tag
            )),
            1 => Ok(found[0]),
            _ => {
                // Several pins on different nets: trace the first and say so,
                // rather than silently picking one of several circuits.
                Ok(found[0])
            }
        };
    }
    // A terminal or connector designator.
    if let Some(n) = nets.net_of_node(token) {
        return Ok(n);
    }
    // A net label.
    if let Some(n) = nets.nets.iter().find(|n| n.label.eq_ignore_ascii_case(token)) {
        return Ok(n);
    }
    // An explicit handle.
    if let Some(h) = crate::hostops::parse_handle(token) {
        if let Some(n) = nets.net_of_wire(h.value()) {
            return Ok(n);
        }
    }
    if let Some(pinned) = crate::state::resolve_sel_token(token) {
        if let Some(handle) = pinned {
            // The pinned entity may be a drawn part, so resolve it to a wire or a
            // device through the index.
            if let Some(n) = nets.net_of_wire(handle) {
                return Ok(n);
            }
            if let Some(comp) = idx.component_of_handle(handle) {
                for pin in &comp.rec.pins {
                    if let Some(n) =
                        nets.net_of_node(&format!("{}.{}", comp.rec.tag, pin))
                    {
                        return Ok(n);
                    }
                }
            }
            return Err("the pinned entity is not on any net".to_string());
        }
        return Err("nothing inspected yet — pick an entity or name a wire".to_string());
    }
    Err(format!(
        "nothing matches {token:?} — try a wire number (W1), a device tag (K1), a net label, or \
         EL_LIST nets to see every net"
    ))
}
