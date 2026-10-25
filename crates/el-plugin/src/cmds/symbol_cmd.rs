//! `EL_SYMBOL` — register a symbol the user drew.
//!
//! Without this the library is closed: a drafter whose part is not in the built-in
//! set has no way to add one, which the research flagged as a hard adoption
//! blocker. AutoCAD Electrical's answer is Symbol Builder; the plug-in
//! equivalent is to take geometry the user has already drawn and capture it as a
//! definition.
//!
//! Captured geometry is stored **relative to a picked base point**, in the
//! library's local coordinates. That is what makes a captured symbol rotatable
//! and placeable anywhere, rather than a set of absolute coordinates that only
//! works where it was drawn.

use ocs_plugin_api::host::codec::EntityType;
use ocs_plugin_api::host::HostApi;

use el_core::geom::Pt;
use el_core::symbol::{GlyphPrim, PinDef, SymbolDef};

use crate::cmds::common::Args;
use crate::geometry::polyline_points;
use crate::library;

/// How far apart two line endpoints may be and still be treated as one pin.
const PIN_MERGE_MM: f64 = 0.1;

/// Run `EL_SYMBOL <name> FAMILY=<code> [DESC=] [CHILD] [CONTACT=NO|NC] [BASE=x,y]
/// [PINS=a,b] | EL_SYMBOL DEL <name>`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let words: Vec<String> = a
        .free
        .iter()
        .filter(|t| !t.contains(','))
        .cloned()
        .collect();

    if words.first().is_some_and(|w| w.eq_ignore_ascii_case("DEL")) {
        delete(host, words.get(1).map(String::as_str));
        return;
    }

    let Some(name) = words.first().cloned() else {
        host.push_error(
            "EL_SYMBOL: name the symbol, e.g. EL_SYMBOL my-coil FAMILY=K (then pick the geometry \
             and the base point), or EL_SYMBOL DEL <name>",
        );
        return;
    };
    let Some(family) = a.get("FAMILY").map(str::to_string) else {
        host.push_error(
            "EL_SYMBOL: FAMILY=<code> is required — it is the %F a tag format expands, so a symbol \
             without one could never be tagged",
        );
        return;
    };

    // A base point may be given outright, which is what a scripted call does; the
    // interactive path picks it.
    let base = a.first_coord().map(|c| [c[0], c[1], 0.0]).unwrap_or([0.0, 0.0, 0.0]);
    let pins: Vec<String> = a
        .get("PINS")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    register(host, &name, &family, base, pins, &mut a);
}

/// Turn what the caller asked for into a definition, then save it.
fn register(
    host: &mut dyn HostApi,
    name: &str,
    family: &str,
    base: [f64; 3],
    pin_names: Vec<String>,
    a: &mut Args,
) {
    let capture = crate::hostops::Capture::take(host);

    // Every drawn entity that is not already an electrical record is fair game:
    // the point of this command is to capture geometry the user drew with the
    // host's own tools.
    let mut glyph: Vec<GlyphPrim> = Vec::new();
    let mut source_handles: Vec<u64> = Vec::new();

    for entity in host.document().entities() {
        if crate::codec::record_of(entity).is_some() {
            continue; // already an electrical object, not raw geometry
        }
        let handle = entity.common().handle.value();
        match entity {
            EntityType::Line(l) => {
                glyph.push(GlyphPrim::Line {
                    a: rel([l.start.x, l.start.y], base),
                    b: rel([l.end.x, l.end.y], base),
                });
                source_handles.push(handle);
            }
            EntityType::LwPolyline(_) => {
                if let Some(pts) = polyline_points(entity) {
                    let closed = matches!(entity, EntityType::LwPolyline(p) if p.is_closed);
                    glyph.push(GlyphPrim::Polyline {
                        points: pts.iter().map(|p| rel([p[0], p[1]], base)).collect(),
                        closed,
                    });
                    source_handles.push(handle);
                }
            }
            EntityType::Circle(c) => {
                glyph.push(GlyphPrim::Circle {
                    center: rel([c.center.x, c.center.y], base),
                    radius: c.radius,
                });
                source_handles.push(handle);
            }
            EntityType::MText(m) => {
                glyph.push(GlyphPrim::Text {
                    at: rel([m.insertion_point.x, m.insertion_point.y], base),
                    height: m.height,
                    text: m.value.clone(),
                });
                source_handles.push(handle);
            }
            _ => {}
        }
    }

    if glyph.is_empty() {
        host.push_error(
            "EL_SYMBOL: found no geometry to capture — draw the symbol with the host's own tools \
             first (LINE, PLINE, CIRCLE, TEXT), then run EL_SYMBOL",
        );
        return;
    }

    // Connection points: the named pins are placed at the geometry's free ends,
    // which is where a wire would attach.
    let pins = match derive_pins(&glyph, &pin_names) {
        Ok(p) => p,
        Err(e) => {
            host.push_error(&format!("EL_SYMBOL: {e}"));
            return;
        }
    };

    let contact = a
        .get("CONTACT")
        .and_then(el_core::symbol::ContactState::parse)
        .unwrap_or(el_core::symbol::ContactState::None);
    // A child symbol belongs to another device. A pushbutton carrying a contact
    // is *not* a child: it is a device in its own right. So this is an explicit
    // flag rather than something inferred from the contact state.
    let is_child = a.flag("CHILD").unwrap_or(false);

    let def = SymbolDef {
        name: name.to_string(),
        family: family.to_string(),
        description: a
            .get("DESC")
            .map(str::to_string)
            .unwrap_or_else(|| format!("{name} (captured)")),
        is_child,
        contact,
        pins,
        glyph,
        default_ratings: a
            .get("RATING")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        tag_offset: [-2.0, 5.0],
    };

    if let Err(e) = def.validate() {
        host.push_error(&format!("EL_SYMBOL: {e}"));
        return;
    }

    let mut lib = match library::load(host) {
        Ok(l) => l,
        Err(e) => {
            host.push_error(&format!("EL_SYMBOL: {e}"));
            return;
        }
    };
    let replaced = lib.upsert(def.clone());
    match library::save(host, &lib) {
        Ok(path) => {
            host.push_output(&format!(
                "EL_SYMBOL: {} {} ({} primitive(s), {} pin(s)) → {}",
                if replaced { "updated" } else { "added" },
                def.name,
                def.glyph.len(),
                def.pins.len(),
                path.display()
            ));
            host.push_output(&format!(
                "  pins: {}",
                def.pins
                    .iter()
                    .map(|p| format!("{}@({}, {})", p.name, el_core::fmt_mm(p.at[0]), el_core::fmt_mm(p.at[1])))
                    .collect::<Vec<_>>()
                    .join("  ")
            ));
            host.push_info("  place it with EL_COMPONENT <name> — reload first if the library was already open");
            let _ = source_handles;
            let _ = capture;
        }
        Err(e) => host.push_error(&format!("EL_SYMBOL: {e}")),
    }
}

fn rel(p: [f64; 2], base: [f64; 3]) -> [f64; 2] {
    [p[0] - base[0], p[1] - base[1]]
}

/// Place the named pins on the geometry's free ends.
///
/// An endpoint is a connection point when it touches **nothing else's interior**.
/// That one rule covers the shapes people actually draw: a motor's leads end on
/// its circle and a resistor's leads end on its rectangle, so in both cases only
/// the outer ends remain — which is exactly where a wire attaches. A T-junction
/// survives, because the shared point is an endpoint of both strokes rather than
/// part of one's middle.
///
/// Inferred pins are auto-named only when exactly two ends remain, the
/// unambiguous two-terminal case. Otherwise the caller names them: silently
/// producing a symbol with no pins, or guessing which of four ends are
/// connections, would both be worse than an error.
fn derive_pins(glyph: &[GlyphPrim], names: &[String]) -> Result<Vec<PinDef>, String> {
    let mut ends: Vec<Pt> = Vec::new();
    let mut touch = |p: Pt| {
        if !ends.iter().any(|e| el_core::geom::dist(*e, p) <= PIN_MERGE_MM) {
            ends.push(p);
        }
    };
    for prim in glyph {
        match prim {
            GlyphPrim::Line { a, b } => {
                touch(*a);
                touch(*b);
            }
            GlyphPrim::Polyline { points, closed } => {
                if !*closed {
                    if let (Some(f), Some(l)) = (points.first(), points.last()) {
                        touch(*f);
                        touch(*l);
                    }
                }
            }
            GlyphPrim::Circle { .. } | GlyphPrim::Text { .. } => {}
        }
    }

    // Drop the ends that merely stop part-way along another stroke.
    let free: Vec<Pt> = ends
        .into_iter()
        .filter(|p| !glyph.iter().any(|prim| lies_on_interior(*p, prim)))
        .collect();

    if names.is_empty() {
        if free.len() == 2 {
            return Ok(vec![
                PinDef { name: "1".to_string(), at: free[0] },
                PinDef { name: "2".to_string(), at: free[1] },
            ]);
        }
        return Err(format!(
            "the geometry has {} free end(s), so the pins cannot be inferred — pass PINS=<a,b,...> \
             with one name per connection point",
            free.len()
        ));
    }

    if free.len() < names.len() {
        return Err(format!(
            "{} pin name(s) were given but the geometry has only {} free end(s) — a pin must sit \
             where a wire can reach it",
            names.len(),
            free.len()
        ));
    }
    Ok(names
        .iter()
        .zip(free.iter())
        .map(|(name, at)| PinDef { name: name.clone(), at: *at })
        .collect())
}

/// Does `p` lie on a primitive's *interior* rather than at one of its ends?
///
/// The distinction is the whole rule: a stroke that stops at another stroke's
/// middle is a connection, while a stroke that stops part-way along another is
/// buried inside a shape.
fn lies_on_interior(p: Pt, prim: &GlyphPrim) -> bool {
    const TOL: f64 = PIN_MERGE_MM;
    match prim {
        GlyphPrim::Line { a, b } => {
            el_core::geom::dist_point_to_segment(p, *a, *b) <= TOL
                && el_core::geom::dist(p, *a) > TOL
                && el_core::geom::dist(p, *b) > TOL
        }
        GlyphPrim::Polyline { points, .. } => {
            let n = points.len();
            if n < 2 {
                return false;
            }
            // A closed loop's last edge is implicit, so it must be tested too or
            // a lead ending on the closing edge would look like a connection.
            let last = if points.first() == points.last() { n - 2 } else { n - 1 };
            for i in 0..=last {
                let a = points[i];
                let b = points[(i + 1) % n];
                if el_core::geom::dist_point_to_segment(p, a, b) <= TOL
                    && el_core::geom::dist(p, a) > TOL
                    && el_core::geom::dist(p, b) > TOL
                {
                    return true;
                }
            }
            false
        }
        GlyphPrim::Circle { center, radius } => {
            (el_core::geom::dist(p, *center) - radius).abs() <= TOL
        }
        GlyphPrim::Text { .. } => false,
    }
}

fn delete(host: &mut dyn HostApi, name: Option<&str>) {
    let Some(name) = name else {
        host.push_error("EL_SYMBOL DEL: name the symbol to remove");
        return;
    };
    let mut lib = match library::load(host) {
        Ok(l) => l,
        Err(e) => {
            host.push_error(&format!("EL_SYMBOL DEL: {e}"));
            return;
        }
    };
    if !lib.remove(name) {
        host.push_error(&format!("EL_SYMBOL DEL: no symbol named {name:?}"));
        return;
    }
    match library::save(host, &lib) {
        Ok(path) => host.push_output(&format!(
            "EL_SYMBOL DEL: removed {name}; {} symbol(s) remain in {}",
            lib.symbols.len(),
            path.display()
        )),
        Err(e) => host.push_error(&format!("EL_SYMBOL DEL: {e}")),
    }
}




#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: [f64; 2], b: [f64; 2]) -> GlyphPrim {
        GlyphPrim::Line { a, b }
    }

    #[test]
    fn two_ends_become_pins_1_and_2_by_default() {
        let glyph = vec![line([-5.0, 0.0], [5.0, 0.0])];
        let pins = derive_pins(&glyph, &[]).unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].name, "1");
        assert_eq!(pins[1].name, "2");
    }

    #[test]
    fn named_pins_take_the_free_ends() {
        let glyph = vec![line([-5.0, 0.0], [5.0, 0.0])];
        let names = ["A1".to_string(), "A2".to_string()];
        let pins = derive_pins(&glyph, &names).unwrap();
        assert_eq!(pins[0].name, "A1");
        assert_eq!(pins[1].name, "A2");
    }

    #[test]
    fn coincident_ends_merge_into_one_pin() {
        // Two stubs meeting at a point is one connection, not two: the shared
        // middle point is deduplicated, leaving three distinct ends.
        let glyph = vec![
            line([-5.0, 0.0], [0.0, 0.0]),
            line([0.0, 0.0], [5.0, 0.0]),
        ];
        let err = derive_pins(&glyph, &[]).unwrap_err();
        assert!(err.contains("3 free end"), "got: {err}");

        // Naming them resolves it, and the middle pin is a real connection.
        let names = ["1".to_string(), "2".to_string(), "3".to_string()];
        let pins = derive_pins(&glyph, &names).unwrap();
        assert_eq!(pins.len(), 3);
        assert_eq!(pins[1].at, [0.0, 0.0], "the T-junction is the shared end");
    }

    #[test]
    fn a_glyph_with_three_free_ends_must_have_its_pins_named() {
        // Three strokes meeting at a point leave three connection points, and
        // nothing in the geometry says which the user meant — so guessing would
        // be arbitrary.
        let glyph = vec![
            line([-5.0, 0.0], [0.0, 0.0]),
            line([0.0, 0.0], [5.0, 0.0]),
            line([0.0, 0.0], [0.0, 5.0]),
        ];
        assert!(derive_pins(&glyph, &[]).is_err(), "must not guess");
        let names = ["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(derive_pins(&glyph, &names).unwrap().len(), 3);
    }

    #[test]
    fn an_open_polyline_is_auto_named_from_its_two_ends() {
        // An L-shaped stroke is a perfectly ordinary captured symbol.
        let glyph = vec![GlyphPrim::Polyline {
            points: vec![[-5.0, 0.0], [0.0, 0.0], [0.0, 5.0]],
            closed: false,
        }];
        let pins = derive_pins(&glyph, &[]).unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].at, [-5.0, 0.0]);
        assert_eq!(pins[1].at, [0.0, 5.0]);
    }

    #[test]
    fn a_two_terminal_glyph_is_named_automatically() {
        // A motor: a circle with a lead at each pole.
        let glyph = vec![
            GlyphPrim::Circle {
                center: [0.0, 0.0],
                radius: 4.0,
            },
            line([0.0, 4.0], [0.0, 5.0]),
            line([0.0, -4.0], [0.0, -5.0]),
        ];
        let pins = derive_pins(&glyph, &[]).unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0].name, "1");
        assert_eq!(pins[1].name, "2");
    }

    #[test]
    fn more_pin_names_than_ends_is_refused() {
        // A pin must sit where a wire can reach it; inventing a position would
        // put a connection point in mid-air.
        let glyph = vec![line([-5.0, 0.0], [5.0, 0.0])];
        let err = derive_pins(&glyph, &["1".into(), "2".into(), "3".into()]).unwrap_err();
        assert!(err.contains("only 2 free end"), "{err}");
    }

    #[test]
    fn a_shape_with_no_free_ends_must_have_its_pins_named() {
        // A closed outline and a circle have no connection points at all, so
        // there is nothing to infer and the caller has to say what connects.
        let closed = vec![GlyphPrim::Polyline {
            points: vec![[0.0, 0.0], [5.0, 0.0], [5.0, 5.0], [0.0, 5.0]],
            closed: true,
        }];
        assert!(derive_pins(&closed, &[]).is_err());

        let round = vec![
            GlyphPrim::Circle {
                center: [0.0, 0.0],
                radius: 3.0,
            },
            GlyphPrim::Text {
                at: [0.0, 0.0],
                height: 3.0,
                text: "M".to_string(),
            },
        ];
        assert!(derive_pins(&round, &[]).is_err());
    }

    #[test]
    fn a_lead_ending_on_a_shape_is_not_a_connection_point() {
        // The property that makes enclosure symbols work: a motor's or a
        // resistor's leads end on the body, and only their outer ends connect.
        let glyph = vec![
            GlyphPrim::Polyline {
                points: vec![[-3.5, -2.0], [3.5, -2.0], [3.5, 2.0], [-3.5, 2.0]],
                closed: true,
            },
            line([-5.0, 0.0], [-3.5, 0.0]),
            line([3.5, 0.0], [5.0, 0.0]),
        ];
        let pins = derive_pins(&glyph, &[]).unwrap();
        assert_eq!(pins.len(), 2, "got {pins:?}");
        let xs: Vec<f64> = pins.iter().map(|p| p.at[0]).collect();
        assert!(
            xs.contains(&-5.0) && xs.contains(&5.0),
            "the outer ends are the pins, got {xs:?}"
        );
    }

    #[test]
    fn relative_coordinates_are_measured_from_the_base() {
        assert_eq!(rel([100.0, 200.0], [90.0, 190.0, 0.0]), [10.0, 10.0]);
    }
}
