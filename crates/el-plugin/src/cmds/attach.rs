//! Re-seating wire ends onto their connection points.
//!
//! A wire records *which* pin it connects to (`from_id` + `from_pin`) and is drawn
//! as a polyline. Those two facts can disagree: move a device and the record still
//! names the pin correctly while the drawn end is left behind. Re-seating closes
//! that gap, and it is why moving a device here does not break every wire attached
//! to it.
//!
//! The placement is passed in rather than re-read, because after a move the
//! plugin's document snapshot still holds the *old* position — a re-read would
//! re-seat the wires onto where the device used to be.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::HostApi;

use el_core::model::{ConnectorRec, TerminalRec};
use el_core::symbol::SymbolDef;
use el_core::ElIndex;

use crate::geometry::polyline_points;
use crate::hostops::DocEntityExt;

/// What kind of object a placement is, so its pin positions can be computed.
#[derive(Debug, Clone)]
pub enum PlacementKind {
    Component(SymbolDef),
    Terminal,
    Connector(ConnectorRec),
}

/// Where an object now sits, and what it is.
#[derive(Debug, Clone)]
pub struct Placement {
    /// The name a wire would use to refer to this object: a tag, or a terminal
    /// strip, or a connector's designator.
    pub node_id: String,
    pub origin: [f64; 2],
    /// Rotation in degrees.
    pub rotation_deg: f64,
    pub kind: PlacementKind,
}

impl Placement {
    /// The connection point a wire naming `pin` should reach.
    ///
    /// `end` picks which connection of a two-terminal object is meant: a terminal
    /// has a left and a right, and a wire must reach the one it was drawn to.
    pub fn pin_point(&self, pin: &str, end: bool) -> Option<[f64; 3]> {
        match &self.kind {
            PlacementKind::Component(def) => {
                let name = if pin.is_empty() {
                    def.pins.first().map(|p| p.name.as_str())?
                } else {
                    pin
                };
                let p = def.pin_at(name, self.origin, self.rotation_deg.to_radians())?;
                Some([p[0], p[1], 0.0])
            }
            PlacementKind::Terminal => {
                let (s, c) = self.rotation_deg.to_radians().sin_cos();
                // The leads match `geometry::terminal_entities`.
                let sign = if end { 1.0 } else { -1.0 };
                let (dx, dy) = (sign * 7.5, 0.0);
                Some([
                    self.origin[0] + dx * c - dy * s,
                    self.origin[1] + dx * s + dy * c,
                    0.0,
                ])
            }
            PlacementKind::Connector(rec) => rec
                .pin_position(pin, self.origin, self.rotation_deg.to_radians())
                .map(|p| [p[0], p[1], 0.0])
                .or_else(|| {
                    // No pin named: the connector's first pin is the honest
                    // default, as it is for a device.
                    let first = rec.pins.first()?;
                    rec.pin_position(first, self.origin, self.rotation_deg.to_radians())
                        .map(|p| [p[0], p[1], 0.0])
                }),
        }
    }
}

/// Re-seat every wire end attached to one of `placements`.
///
/// Returns how many wire ends were moved. A wire whose named pin no longer exists
/// is left alone and counted as skipped, because moving it to a guessed point
/// would hide the real problem.
pub fn reseat_wires(
    host: &mut dyn HostApi,
    idx: &ElIndex,
    placements: &[Placement],
) -> (usize, usize) {
    let mut moved = 0usize;
    let mut skipped = 0usize;

    for wire in idx.wires.values() {
        // A gapped wire is several runs sharing one record; its ends live at
        // the fold of those runs, not on any one entity.
        let multi = idx.handles_for_el_id(&wire.rec.el_id).len() > 1;
        let read = if multi {
            crate::cmds::wiregap::folded_points(host, idx, wire)
        } else {
            host.document()
                .get_entity_owned(Handle::new(wire.handle))
                .and_then(|e| polyline_points(&e))
        };
        let mut points = match read {
            Some(p) if p.len() >= 2 => p,
            _ => continue,
        };

        let mut touched = false;
        let mut failed = false;
        let mut used_slot: Option<usize> = None;

        // The `from` end is the first vertex, the `to` end the last — except
        // on a folded path, which is canonically ordered, so the end that
        // belongs to a pin is the boundary vertex nearest it.
        let last = points.len() - 1;
        for (which_from, node_id, pin) in [
            (true, &wire.rec.from_id, &wire.rec.from_pin),
            (false, &wire.rec.to_id, &wire.rec.to_pin),
        ] {
            let Some(placement) = placements.iter().find(|p| &p.node_id == node_id) else {
                continue;
            };
            match placement.pin_point(pin, !which_from) {
                Some(pt) => {
                    let mut slot = if multi {
                        let d0 = el_core::geom::dist([points[0][0], points[0][1]], [pt[0], pt[1]]);
                        let dl =
                            el_core::geom::dist([points[last][0], points[last][1]], [pt[0], pt[1]]);
                        if d0 <= dl {
                            0
                        } else {
                            last
                        }
                    } else if which_from {
                        0
                    } else {
                        last
                    };
                    // Both ends of a very short wire can land on one slot;
                    // the second taker moves to the other end.
                    if used_slot == Some(slot) {
                        slot = if slot == 0 { last } else { 0 };
                    }
                    used_slot = Some(slot);
                    if el_core::geom::dist([points[slot][0], points[slot][1]], [pt[0], pt[1]])
                        > 1e-9
                    {
                        points[slot] = pt;
                        touched = true;
                    }
                }
                None => failed = true,
            }
        }

        if failed {
            skipped += 1;
            continue;
        }
        if !touched {
            continue;
        }

        if multi {
            // Collapse to one polyline carrying the record; the gap refresh
            // below re-splits every gapped wire against the crossings as
            // they are now.
            if crate::cmds::wiregap::write_single(host, idx, &wire.rec, &points) {
                moved += 1;
            }
        } else {
            // The record is unchanged — only the drawn end moves — so the polyline is
            // rewritten in place and its XDATA is left as it stands.
            if let Some(entity) = host.document().get_entity_owned(Handle::new(wire.handle)) {
                let mut entity = entity;
                if set_points(&mut entity, &points) {
                    host.update_entity(entity);
                    moved += 1;
                }
            }
        }
    }

    // Runs are a render: once ends have moved, every gapped wire is re-split
    // against the crossings as they are now.
    if moved > 0 {
        crate::cmds::wiregap::refresh_gapped(host, idx);
    }
    (moved, skipped)
}

fn set_points(
    entity: &mut ocs_plugin_api::host::codec::EntityType,
    points: &[[f64; 3]],
) -> bool {
    use ocs_plugin_api::host::codec::types::Vector2;
    use ocs_plugin_api::host::codec::entities::LwVertex;
    use ocs_plugin_api::host::codec::EntityType;
    match entity {
        EntityType::LwPolyline(pl) => {
            pl.vertices = points
                .iter()
                .map(|p| LwVertex::new(Vector2::new(p[0], p[1])))
                .collect();
            if let Some(first) = points.first() {
                pl.elevation = first[2];
            }
            true
        }
        _ => false,
    }
}

/// The placement a component records, with its definition from the library.
pub fn component_placement(
    rec: &el_core::model::ElComponentRec,
    lib: &el_core::symbol::SymbolLibrary,
) -> Option<Placement> {
    let def = lib.get(&rec.symbol)?;
    Some(Placement {
        node_id: rec.tag.clone(),
        origin: rec.origin(),
        rotation_deg: rec.rotation_deg,
        kind: PlacementKind::Component(def.clone()),
    })
}

/// The placement a terminal records.
pub fn terminal_placement(rec: &TerminalRec) -> Placement {
    Placement {
        node_id: rec.designator(),
        origin: [rec.origin_x, rec.origin_y],
        rotation_deg: rec.rotation_deg,
        kind: PlacementKind::Terminal,
    }
}

/// The placement a connector records.
pub fn connector_placement(rec: &ConnectorRec) -> Placement {
    Placement {
        node_id: rec.ref_des.clone(),
        origin: [rec.origin_x, rec.origin_y],
        rotation_deg: rec.rotation_deg,
        kind: PlacementKind::Connector(rec.clone()),
    }
}

/// How many wires name `node_id` as one of their ends.
pub fn wires_touching(idx: &ElIndex, node_id: &str) -> usize {
    idx.wires
        .values()
        .filter(|w| w.rec.from_id == node_id || w.rec.to_id == node_id)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use el_core::codec::ElRecord;
    use el_core::model::{ElComponentRec, TerminalKind};
    use el_core::symbol::SymbolLibrary;

    #[test]
    fn a_component_pin_point_follows_its_placement() {
        let lib = SymbolLibrary::default();
        let mut rec = ElComponentRec::new("id", "relay-coil", "K");
        rec.tag = "K1".to_string();
        let p = component_placement(&rec, &lib).unwrap();
        // The coil's pins are at ±5 on the y axis from the anchor.
        assert_eq!(p.pin_point("A1", false), Some([0.0, 5.0, 0.0]));
        assert_eq!(p.pin_point("A2", false), Some([0.0, -5.0, 0.0]));
    }

    #[test]
    fn moving_the_origin_moves_every_pin() {
        let lib = SymbolLibrary::default();
        let mut rec = ElComponentRec::new("id", "relay-coil", "K");
        rec.tag = "K1".to_string();
        rec.origin_x = 100.0;
        rec.origin_y = 200.0;
        let p = component_placement(&rec, &lib).unwrap();
        assert_eq!(p.pin_point("A1", false), Some([100.0, 205.0, 0.0]));
        assert_eq!(p.pin_point("A2", false), Some([100.0, 195.0, 0.0]));
    }

    #[test]
    fn rotation_moves_the_pins_too() {
        let lib = SymbolLibrary::default();
        let mut rec = ElComponentRec::new("id", "relay-coil", "K");
        rec.tag = "K1".to_string();
        rec.rotation_deg = 90.0;
        let p = component_placement(&rec, &lib).unwrap();
        let a1 = p.pin_point("A1", false).unwrap();
        assert!(a1[0] < -4.9 && a1[0] > -5.1, "A1 swings left: {a1:?}");
        assert!(a1[1].abs() < 1e-9, "got {a1:?}");
    }

    #[test]
    fn a_terminal_has_two_ends_and_the_wire_must_reach_the_right_one() {
        let mut rec = TerminalRec::new("id", "X1", TerminalKind::Screw);
        rec.origin_x = 10.0;
        rec.origin_y = 20.0;
        let p = terminal_placement(&rec);
        assert_eq!(p.node_id, "X1");
        assert_eq!(p.pin_point("", false), Some([2.5, 20.0, 0.0]));
        assert_eq!(p.pin_point("", true), Some([17.5, 20.0, 0.0]));
    }

    #[test]
    fn one_terminal_end_is_not_the_other() {
        // The property that keeps a two-sided terminal two circuits.
        let rec = TerminalRec::new("id", "X1", TerminalKind::Screw);
        let p = terminal_placement(&rec);
        assert_ne!(p.pin_point("", false), p.pin_point("", true));
    }

    #[test]
    fn a_connector_pin_point_uses_its_recorded_place() {
        let mut rec = ConnectorRec::new("id", "X1", 3);
        rec.origin_x = 50.0;
        rec.origin_y = 60.0;
        let p = connector_placement(&rec);
        assert_eq!(p.node_id, "X1");
        // The middle pin of three sits on the anchor.
        assert_eq!(p.pin_point("2", false), Some([50.0, 60.0, 0.0]));
        // An unnamed pin falls back to the first.
        assert_eq!(p.pin_point("", false), p.pin_point("1", false));
    }

    #[test]
    fn an_unknown_pin_reports_nothing_rather_than_guessing() {
        // Re-seating to a guessed point would hide the real problem.
        let lib = SymbolLibrary::default();
        let mut rec = ElComponentRec::new("id", "relay-coil", "K");
        rec.tag = "K1".to_string();
        let p = component_placement(&rec, &lib).unwrap();
        assert_eq!(p.pin_point("ZZ", false), None);
    }

    #[test]
    fn a_component_whose_symbol_is_missing_has_no_placement() {
        let lib = SymbolLibrary::default();
        let mut rec = ElComponentRec::new("id", "not-in-the-library", "K");
        rec.tag = "K1".to_string();
        assert!(component_placement(&rec, &lib).is_none());
    }

    #[test]
    fn wires_touching_counts_both_ends() {
        use el_core::model::{ElSettingsRec, ElWireRec};
        let idx = ElIndex::build(vec![
            (900, ElRecord::encode_settings(&ElSettingsRec::default())),
            (1, ElRecord::encode_wire(&ElWireRec::new("a", "K1", "K2"))),
            (2, ElRecord::encode_wire(&ElWireRec::new("b", "K2", "K3"))),
        ]);
        assert_eq!(wires_touching(&idx, "K2"), 2);
        assert_eq!(wires_touching(&idx, "K1"), 1);
        assert_eq!(wires_touching(&idx, "K9"), 0);
    }
}
