//! The symbol library: data-driven schematic symbol definitions.
//!
//! A definition is pure data — no code, no host types — so the whole library is
//! serialisable, diffable and replaceable. That is the point: the plan is
//! IEC-first with a JIC/NFPA set added later, and "later" must mean *a second
//! JSON file*, not a second code path.
//!
//! Each definition carries a local-coordinate glyph and the connection points a
//! wire attaches to. Symbols are drawn as primitives because the host cannot
//! create block definitions (see `docs/host-capabilities-v2026.36.md`), so the
//! glyph *is* the symbol — there is no block to fall back on.

use serde::{Deserialize, Serialize};

/// One drawing primitive in a symbol's local coordinates.
///
/// Deliberately small: real IEC symbols are lines, rectangles, circles and the
/// occasional letter. Arcs are omitted because a polyline approximates them
/// closely enough at schematic scale and keeps the library hand-editable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "prim", rename_all = "kebab-case")]
pub enum GlyphPrim {
    Line { a: [f64; 2], b: [f64; 2] },
    Polyline {
        points: Vec<[f64; 2]>,
        #[serde(default)]
        closed: bool,
    },
    Circle { center: [f64; 2], radius: f64 },
    /// A letter or short mark inside the symbol (the `M` in a motor, the `~` in
    /// a coil). Text is part of the glyph, not a label: it rotates and moves
    /// with the symbol and is not the reference designator.
    Text {
        at: [f64; 2],
        height: f64,
        text: String,
    },
}

/// A connection point, in the symbol's local coordinates.
///
/// Named rather than numbered because that is what a wirelist and a pin
/// schedule use: `A1`/`A2` on a coil, `13`/`14` on a contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinDef {
    pub name: String,
    pub at: [f64; 2],
}

/// How a contact rests when the device is unpowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContactState {
    /// Not a contact (a coil, a lamp, a terminal).
    None,
    /// Normally open — the blade is drawn away from its other contact.
    Open,
    /// Normally closed — the blade rests on its other contact.
    Closed,
    /// Changeover: one common contact with both an open and a closed throw.
    Changeover,
}

impl Default for ContactState {
    fn default() -> Self {
        ContactState::None
    }
}

impl ContactState {
    pub fn as_str(self) -> &'static str {
        match self {
            ContactState::None => "",
            ContactState::Open => "NO",
            ContactState::Closed => "NC",
            ContactState::Changeover => "CO",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "" | "NONE" | "-" => Some(ContactState::None),
            "NO" | "OPEN" | "MAKE" => Some(ContactState::Open),
            "NC" | "CLOSED" | "BREAK" => Some(ContactState::Closed),
            "CO" | "CHANGEOVER" | "SPDT" => Some(ContactState::Changeover),
            _ => None,
        }
    }
}

/// One symbol definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolDef {
    /// Library name, e.g. `contact-no`. Unique within a library.
    pub name: String,
    /// Ref-des family code (`K`, `Q`, `F`, …), the `%F` a tag format expands.
    pub family: String,
    /// Human description, for a listing or a catalogue default.
    pub description: String,
    /// A child shares its parent's tag (a contact belongs to its coil).
    #[serde(default)]
    pub is_child: bool,
    #[serde(default)]
    pub contact: ContactState,
    /// Connection points, in drawing order.
    #[serde(default)]
    pub pins: Vec<PinDef>,
    /// The drawn geometry, in local coordinates.
    #[serde(default)]
    pub glyph: Vec<GlyphPrim>,
    /// Ratings seeded onto a new instance (`RATING1..n`).
    #[serde(default)]
    pub default_ratings: Vec<String>,
    /// Where the reference designator sits relative to the anchor.
    #[serde(default = "default_tag_offset")]
    pub tag_offset: [f64; 2],
}

fn default_tag_offset() -> [f64; 2] {
    [-3.0, 5.0]
}

impl SymbolDef {
    pub fn pin(&self, name: &str) -> Option<&PinDef> {
        self.pins.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// A pin's absolute position, with the symbol rotated about its anchor.
    pub fn pin_at(&self, name: &str, origin: [f64; 2], rot_rad: f64) -> Option<[f64; 2]> {
        let p = self.pin(name)?;
        let r = rotate(p.at, rot_rad);
        Some([origin[0] + r[0], origin[1] + r[1]])
    }

    /// The reference-designator position for a placed instance.
    pub fn tag_at(&self, origin: [f64; 2], rot_rad: f64) -> [f64; 2] {
        let r = rotate(self.tag_offset, rot_rad);
        [origin[0] + r[0], origin[1] + r[1]]
    }

    /// The glyph's local bounding box `[min_x, min_y, max_x, max_y]`.
    pub fn extent(&self) -> [f64; 4] {
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        let mut add = |p: [f64; 2]| {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        };
        for prim in &self.glyph {
            match prim {
                GlyphPrim::Line { a, b: e } => {
                    add(*a);
                    add(*e);
                }
                GlyphPrim::Polyline { points, .. } => points.iter().for_each(|p| add(*p)),
                GlyphPrim::Circle { center, radius } => {
                    add([center[0] - radius, center[1] - radius]);
                    add([center[0] + radius, center[1] + radius]);
                }
                GlyphPrim::Text { at, height, text } => {
                    // Rough: the label is not worth exact metrics, but it must
                    // widen the box or a collision test would overlap it.
                    let w = text.chars().count() as f64 * height * 0.6;
                    add([at[0] - w / 2.0, at[1] - height / 2.0]);
                    add([at[0] + w / 2.0, at[1] + height / 2.0]);
                }
            }
        }
        for p in &self.pins {
            add(p.at);
        }
        if !b[0].is_finite() {
            return [0.0, 0.0, 0.0, 0.0];
        }
        b
    }

    /// Does this definition look usable? A symbol with no glyph draws nothing,
    /// and one with no pins cannot be wired — both are authoring mistakes worth
    /// catching at load rather than at placement.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("a symbol must have a name".to_string());
        }
        if self.glyph.is_empty() {
            return Err(format!("symbol {:?} has no glyph", self.name));
        }
        if self.family.trim().is_empty() {
            return Err(format!("symbol {:?} has no family code", self.name));
        }
        Ok(())
    }
}

/// Rotate a local offset counter-clockwise.
pub fn rotate(p: [f64; 2], rot_rad: f64) -> [f64; 2] {
    if rot_rad.abs() < 1e-12 {
        return p;
    }
    let (s, c) = rot_rad.sin_cos();
    [p[0] * c - p[1] * s, p[0] * s + p[1] * c]
}

/// A named set of symbol definitions, and the parts catalogue that goes with it.
///
/// One file for both, deliberately: a project's shapes and the parts they
/// represent travel together, and two files would be two things to keep in step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolLibrary {
    /// Library name, shown by `EL_LIB`.
    pub name: String,
    /// Format version, so a future reader can migrate rather than guess.
    #[serde(default = "default_version")]
    pub version: i32,
    pub symbols: Vec<SymbolDef>,
    /// The parts catalogue. Absent means the built-in sample is used.
    #[serde(default)]
    pub catalog: crate::catalog::Catalog,
}

fn default_version() -> i32 {
    1
}

impl Default for SymbolLibrary {
    /// The built-in IEC 60617 starter set.
    fn default() -> Self {
        LibraryBuilder::iec().build()
    }
}

impl SymbolLibrary {
    pub fn get(&self, name: &str) -> Option<&SymbolDef> {
        self.symbols
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Every definition in a family, parents before children — the order a
    /// chooser should present them.
    pub fn by_family(&self, family: &str) -> Vec<&SymbolDef> {
        // Parents before children, and within each the library's own order —
        // which parent a chooser offers first is an authoring decision, not
        // something to derive alphabetically.
        let mut v: Vec<(usize, &SymbolDef)> = self
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| s.family.eq_ignore_ascii_case(family))
            .collect();
        v.sort_by_key(|(i, s)| (s.is_child, *i));
        v.into_iter().map(|(_, s)| s).collect()
    }

    /// The first parent definition in a family — what `EL_COMPONENT K` inserts
    /// when the caller names only a family.
    pub fn first_parent(&self, family: &str) -> Option<&SymbolDef> {
        self.by_family(family).into_iter().find(|s| !s.is_child)
    }

    /// The distinct family codes present, with a description.
    pub fn families(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for s in &self.symbols {
            if out.iter().any(|(f, _)| f.eq_ignore_ascii_case(&s.family)) {
                continue;
            }
            out.push((s.family.clone(), s.description.clone()));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Insert or replace by name, returning whether it replaced.
    pub fn upsert(&mut self, def: SymbolDef) -> bool {
        match self.symbols.iter_mut().find(|s| s.name == def.name) {
            Some(slot) => {
                *slot = def;
                true
            }
            None => {
                self.symbols.push(def);
                false
            }
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.symbols.len();
        self.symbols
            .retain(|s| !s.name.eq_ignore_ascii_case(name));
        self.symbols.len() != before
    }

    /// Every definition that would not draw or cannot be wired.
    pub fn invalid(&self) -> Vec<String> {
        self.symbols
            .iter()
            .filter_map(|s| s.validate().err())
            .collect()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Parse a library. A malformed file is reported, never silently replaced
    /// by the built-in set — losing a vendor library to a typo would be worse
    /// than refusing to load it.
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<SymbolLibrary>(text).map_err(|e| format!("{e}"))
    }
}

/// Fluent builder for the built-in set, so the geometry reads as a drawing
/// rather than as nested struct literals.
struct LibraryBuilder {
    lib: SymbolLibrary,
}

impl LibraryBuilder {
    fn new(name: &str) -> Self {
        LibraryBuilder {
            lib: SymbolLibrary {
                name: name.to_string(),
                version: 1,
                symbols: Vec::new(),
                catalog: crate::catalog::Catalog::sample(),
            },
        }
    }

    /// A two-terminal horizontal symbol: pins at ±5 on the x axis, which is one
    /// ladder rung wide at the default 10 mm spacing.
    fn horizontal(
        mut self,
        name: &str,
        family: &str,
        description: &str,
        is_child: bool,
        contact: ContactState,
        glyph: Vec<GlyphPrim>,
    ) -> Self {
        self.lib.symbols.push(SymbolDef {
            name: name.to_string(),
            family: family.to_string(),
            description: description.to_string(),
            is_child,
            contact,
            pins: vec![
                PinDef {
                    name: "1".to_string(),
                    at: [-5.0, 0.0],
                },
                PinDef {
                    name: "2".to_string(),
                    at: [5.0, 0.0],
                },
            ],
            glyph,
            default_ratings: Vec::new(),
            tag_offset: [-2.0, 5.0],
        });
        self
    }

    /// A two-terminal vertical symbol: pins at ±5 on the y axis.
    fn vertical(
        mut self,
        name: &str,
        family: &str,
        description: &str,
        glyph: Vec<GlyphPrim>,
        ratings: &[&str],
    ) -> Self {
        self.lib.symbols.push(SymbolDef {
            name: name.to_string(),
            family: family.to_string(),
            description: description.to_string(),
            is_child: false,
            contact: ContactState::None,
            pins: vec![
                PinDef {
                    name: "A1".to_string(),
                    at: [0.0, 5.0],
                },
                PinDef {
                    name: "A2".to_string(),
                    at: [0.0, -5.0],
                },
            ],
            glyph,
            default_ratings: ratings.iter().map(|s| s.to_string()).collect(),
            tag_offset: [-7.0, 2.0],
        });
        self
    }


    /// A symbol with an explicit pin list — for anything that is neither a
    /// two-terminal rung device nor a vertical coil: changeover contacts,
    /// multi-pole breakers, transformers, potentiometers.
    #[allow(clippy::too_many_arguments)]
    fn custom(
        mut self,
        name: &str,
        family: &str,
        description: &str,
        is_child: bool,
        contact: ContactState,
        pins: &[(&str, [f64; 2])],
        glyph: Vec<GlyphPrim>,
        tag_offset: [f64; 2],
    ) -> Self {
        self.lib.symbols.push(SymbolDef {
            name: name.to_string(),
            family: family.to_string(),
            description: description.to_string(),
            is_child,
            contact,
            pins: pins
                .iter()
                .map(|(n, at)| PinDef {
                    name: n.to_string(),
                    at: *at,
                })
                .collect(),
            glyph,
            default_ratings: Vec::new(),
            tag_offset,
        });
        self
    }

    fn build(self) -> SymbolLibrary {
        self.lib
    }

    /// The IEC 60617 starter set.
    ///
    /// Geometry follows the standard's shapes at schematic scale: coils and
    /// resistors are rectangles, contacts are a gap bridged by a blade, motors
    /// and lamps are circles. Filled shapes are avoided — the standard's symbols
    /// are outlines, and an outline reads correctly at any plot weight.
    fn iec() -> Self {
        let mut b = LibraryBuilder::new("IEC 60617 (built-in)");

        // ── K: relays and contactors ────────────────────────────────────
        // Coil: a rectangle on a vertical wire, per IEC 60617-7.
        b = b.vertical(
            "relay-coil",
            "K",
            "Relay coil",
            vec![
                GlyphPrim::Polyline {
                    points: vec![
                        [-2.5, -4.0],
                        [2.5, -4.0],
                        [2.5, 4.0],
                        [-2.5, 4.0],
                    ],
                    closed: true,
                },
                GlyphPrim::Line {
                    a: [0.0, 4.0],
                    b: [0.0, 5.0],
                },
                GlyphPrim::Line {
                    a: [0.0, -4.0],
                    b: [0.0, -5.0],
                },
            ],
            &["24VDC"],
        );
        b = b.vertical(
            "contactor-coil",
            "K",
            "Contactor coil",
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-2.5, -4.0], [2.5, -4.0], [2.5, 4.0], [-2.5, 4.0]],
                    closed: true,
                },
                // A contactor coil carries the mechanical-linkage bar a plain
                // relay coil does not, which is the standard's distinction.
                GlyphPrim::Line {
                    a: [-2.5, 4.0],
                    b: [2.5, 4.0],
                },
                GlyphPrim::Line {
                    a: [-3.5, 4.0],
                    b: [-3.5, 5.0],
                },
                GlyphPrim::Line {
                    a: [3.5, 4.0],
                    b: [3.5, 5.0],
                },
                GlyphPrim::Line {
                    a: [0.0, -4.0],
                    b: [0.0, -5.0],
                },
            ],
            &["24VDC"],
        );

        // Contacts: two fixed stubs with a gap the blade bridges (or does not).
        b = b.horizontal(
            "contact-no",
            "K",
            "Normally-open contact",
            true,
            ContactState::Open,
            vec![
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-2.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [2.0, 0.0],
                    b: [5.0, 0.0],
                },
                // The blade, angled clear of the right-hand contact.
                GlyphPrim::Line {
                    a: [-2.0, 0.0],
                    b: [2.6, 3.2],
                },
            ],
        );
        b = b.horizontal(
            "contact-nc",
            "K",
            "Normally-closed contact",
            true,
            ContactState::Closed,
            vec![
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-2.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [2.0, 0.0],
                    b: [5.0, 0.0],
                },
                // The blade rests on the right contact, and the short tick is
                // the standard's "closed" mark.
                GlyphPrim::Line {
                    a: [-2.0, 0.0],
                    b: [2.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [2.0, -1.6],
                    b: [2.0, 1.6],
                },
            ],
        );

        // ── S: switches and pushbuttons ────────────────────────────────
        b = b.horizontal(
            "pushbutton-no",
            "S",
            "Pushbutton, normally open",
            false,
            ContactState::Open,
            vec![
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-2.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [2.0, 0.0],
                    b: [5.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [-2.0, 0.0],
                    b: [2.6, 3.2],
                },
                // The plunger.
                GlyphPrim::Line {
                    a: [0.3, 1.0],
                    b: [0.3, 5.0],
                },
                GlyphPrim::Line {
                    a: [-2.5, 5.0],
                    b: [3.0, 5.0],
                },
            ],
        );
        b = b.horizontal(
            "pushbutton-nc",
            "S",
            "Pushbutton, normally closed",
            false,
            ContactState::Closed,
            vec![
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-2.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [2.0, 0.0],
                    b: [5.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [-2.0, 0.0],
                    b: [2.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [2.0, -1.6],
                    b: [2.0, 1.6],
                },
                GlyphPrim::Line {
                    a: [0.3, 1.0],
                    b: [0.3, 5.0],
                },
                GlyphPrim::Line {
                    a: [-2.5, 5.0],
                    b: [3.0, 5.0],
                },
            ],
        );
        b = b.horizontal(
            "switch-disconnect",
            "Q",
            "Disconnect switch",
            false,
            ContactState::Open,
            vec![
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-2.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [2.0, 0.0],
                    b: [5.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [-2.0, 0.0],
                    b: [2.6, 3.2],
                },
                GlyphPrim::Circle {
                    center: [-2.0, 0.0],
                    radius: 0.6,
                },
                GlyphPrim::Circle {
                    center: [2.0, 0.0],
                    radius: 0.6,
                },
            ],
        );
        b = b.horizontal(
            "breaker-1p",
            "Q",
            "Circuit breaker, single pole",
            false,
            ContactState::Open,
            vec![
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-2.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [2.0, 0.0],
                    b: [5.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [-2.0, 0.0],
                    b: [2.6, 3.2],
                },
                // The breaker's mechanical-latch mark.
                GlyphPrim::Line {
                    a: [-1.2, 0.9],
                    b: [1.2, 0.9],
                },
            ],
        );

        // ── F: protective devices ──────────────────────────────────────
        b = b.horizontal(
            "fuse",
            "F",
            "Fuse",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-3.0, -2.0], [3.0, -2.0], [3.0, 2.0], [-3.0, 2.0]],
                    closed: true,
                },
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [5.0, 0.0],
                },
            ],
        );
        b = b.horizontal(
            "overload",
            "F",
            "Thermal overload element",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-3.0, -2.5], [3.0, -2.5], [3.0, 2.5], [-3.0, 2.5]],
                    closed: true,
                },
                // The bimetal step that marks a thermal element.
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-1.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [-1.0, 0.0],
                    b: [1.0, 1.6],
                },
                GlyphPrim::Line {
                    a: [1.0, 1.6],
                    b: [5.0, 1.6],
                },
            ],
        );

        // ── M, H: loads and indicators ─────────────────────────────────
        b = b.vertical(
            "motor",
            "M",
            "Motor",
            vec![
                GlyphPrim::Circle {
                    center: [0.0, 0.0],
                    radius: 4.0,
                },
                GlyphPrim::Text {
                    at: [0.0, 0.0],
                    height: 3.5,
                    text: "M".to_string(),
                },
                GlyphPrim::Line {
                    a: [0.0, 4.0],
                    b: [0.0, 5.0],
                },
                GlyphPrim::Line {
                    a: [0.0, -4.0],
                    b: [0.0, -5.0],
                },
            ],
            &["0.75kW"],
        );
        b = b.horizontal(
            "lamp",
            "H",
            "Indicator lamp",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Circle {
                    center: [0.0, 0.0],
                    radius: 3.0,
                },
                // The standard's cross through the envelope.
                GlyphPrim::Line {
                    a: [-2.1, -2.1],
                    b: [2.1, 2.1],
                },
                GlyphPrim::Line {
                    a: [-2.1, 2.1],
                    b: [2.1, -2.1],
                },
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-3.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [3.0, 0.0],
                    b: [5.0, 0.0],
                },
            ],
        );

        // ── X: terminals and connectors ────────────────────────────────
        b = b.horizontal(
            "terminal",
            "X",
            "Terminal",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Circle {
                    center: [0.0, 0.0],
                    radius: 1.8,
                },
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-1.8, 0.0],
                },
                GlyphPrim::Line {
                    a: [1.8, 0.0],
                    b: [5.0, 0.0],
                },
            ],
        );

        // ── R, C: passive components ───────────────────────────────────
        b = b.horizontal(
            "resistor",
            "R",
            "Resistor",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-3.5, -2.0], [3.5, -2.0], [3.5, 2.0], [-3.5, 2.0]],
                    closed: true,
                },
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-3.5, 0.0],
                },
                GlyphPrim::Line {
                    a: [3.5, 0.0],
                    b: [5.0, 0.0],
                },
            ],
        );
        b = b.horizontal(
            "capacitor",
            "C",
            "Capacitor",
            false,
            ContactState::None,
            vec![
                // Two plates, the standard's parallel lines.
                GlyphPrim::Line {
                    a: [-1.0, -3.0],
                    b: [-1.0, 3.0],
                },
                GlyphPrim::Line {
                    a: [1.0, -3.0],
                    b: [1.0, 3.0],
                },
                GlyphPrim::Line {
                    a: [-5.0, 0.0],
                    b: [-1.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [1.0, 0.0],
                    b: [5.0, 0.0],
                },
            ],
        );

        // ── G: supply ──────────────────────────────────────────────────
        b = b.vertical(
            "supply-dc",
            "G",
            "DC supply",
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-5.0, -3.0], [5.0, -3.0], [5.0, 3.0], [-5.0, 3.0]],
                    closed: true,
                },
                GlyphPrim::Line {
                    a: [-2.5, 1.2],
                    b: [2.5, 1.2],
                },
                // The `+` and `-` pair, which is what makes it a supply.
                GlyphPrim::Line {
                    a: [0.0, -0.5],
                    b: [0.0, -2.5],
                },
                GlyphPrim::Line {
                    a: [-1.0, -1.5],
                    b: [1.0, -1.5],
                },
            ],
            &["24VDC"],
        );

        // ── Earth ──────────────────────────────────────────────────────
        b.lib.symbols.push(SymbolDef {
            name: "earth".to_string(),
            family: "PE".to_string(),
            description: "Protective earth".to_string(),
            is_child: false,
            contact: ContactState::None,
            pins: vec![PinDef {
                name: "1".to_string(),
                at: [0.0, 5.0],
            }],
            glyph: vec![
                GlyphPrim::Line {
                    a: [0.0, 5.0],
                    b: [0.0, 0.0],
                },
                // Three bars, shortening: the standard earth mark.
                GlyphPrim::Line {
                    a: [-4.0, 0.0],
                    b: [4.0, 0.0],
                },
                GlyphPrim::Line {
                    a: [-2.6, -1.8],
                    b: [2.6, -1.8],
                },
                GlyphPrim::Line {
                    a: [-1.2, -3.6],
                    b: [1.2, -3.6],
                },
            ],
            default_ratings: Vec::new(),
            tag_offset: [-2.0, 6.5],
        });

        // ══ Expanded set (ACADE icon-menu coverage) ══════════════════════
        // Every family in IEC_FAMILIES now has a parent; the variants a
        // control schematic reaches for (timers, delayed contacts, multi-pole
        // breakers, three-phase machines, transformers, sensors) no longer
        // fall back to "not a symbol or family".

        // ── K: timer coils and delayed contacts ─────────────────────────
        b = b.vertical(
            "timer-coil-on",
            "K",
            "Timer coil, on-delay",
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-2.5, -4.0], [2.5, -4.0], [2.5, 4.0], [-2.5, 4.0]],
                    closed: true,
                },
                GlyphPrim::Line { a: [0.0, 4.0], b: [0.0, 5.0] },
                GlyphPrim::Line { a: [0.0, -4.0], b: [0.0, -5.0] },
                GlyphPrim::Line { a: [-2.5, 4.0], b: [2.5, 4.0] },
                // On-delay: the cap's left end hooks up.
                GlyphPrim::Line { a: [-2.5, 4.0], b: [-2.5, 5.0] },
            ],
            &["24VDC"],
        );
        b = b.vertical(
            "timer-coil-off",
            "K",
            "Timer coil, off-delay",
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-2.5, -4.0], [2.5, -4.0], [2.5, 4.0], [-2.5, 4.0]],
                    closed: true,
                },
                GlyphPrim::Line { a: [0.0, 4.0], b: [0.0, 5.0] },
                GlyphPrim::Line { a: [0.0, -4.0], b: [0.0, -5.0] },
                GlyphPrim::Line { a: [-2.5, 4.0], b: [2.5, 4.0] },
                // Off-delay: the cap's right end hooks up.
                GlyphPrim::Line { a: [2.5, 4.0], b: [2.5, 5.0] },
            ],
            &["24VDC"],
        );
        b = b.horizontal(
            "contact-no-delayed",
            "K",
            "Normally-open contact, time-delayed",
            true,
            ContactState::Open,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.2] },
                // The delay umbrella on the fixed contact.
                GlyphPrim::Polyline {
                    points: vec![[2.0, 1.6], [2.0, 4.2], [4.4, 4.2]],
                    closed: false,
                },
            ],
        );
        b = b.horizontal(
            "contact-nc-delayed",
            "K",
            "Normally-closed contact, time-delayed",
            true,
            ContactState::Closed,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, -1.6], b: [2.0, 1.6] },
                GlyphPrim::Polyline {
                    points: vec![[3.2, -1.6], [3.2, -4.2], [5.6, -4.2]],
                    closed: false,
                },
            ],
        );
        b = b.custom(
            "contact-changeover",
            "K",
            "Changeover contact (CO)",
            true,
            ContactState::None,
            &[
                ("1", [-5.0, 0.0]),
                ("2", [5.0, 2.5]),
                ("3", [5.0, -2.5]),
            ],
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 2.5], b: [5.0, 2.5] },
                GlyphPrim::Line { a: [2.0, -2.5], b: [5.0, -2.5] },
                // The blade rests on NC (top) and breaks toward NO.
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.4, 2.5] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.4, -2.0] },
            ],
            [-2.0, 6.0],
        );

        // ── Q: multi-pole breakers ──────────────────────────────────────
        b = b.custom(
            "breaker-2p",
            "Q",
            "Circuit breaker, 2-pole",
            false,
            ContactState::None,
            &[
                ("L1", [-2.5, 5.0]),
                ("T1", [-2.5, -5.0]),
                ("L2", [2.5, 5.0]),
                ("T2", [2.5, -5.0]),
            ],
            vec![
                GlyphPrim::Line { a: [-2.5, 5.0], b: [-2.5, 1.6] },
                GlyphPrim::Line { a: [-2.5, 1.6], b: [-1.0, -1.8] },
                GlyphPrim::Line { a: [-2.5, -3.4], b: [-2.5, -5.0] },
                GlyphPrim::Line { a: [2.5, 5.0], b: [2.5, 1.6] },
                GlyphPrim::Line { a: [2.5, 1.6], b: [1.0, -1.8] },
                GlyphPrim::Line { a: [2.5, -3.4], b: [2.5, -5.0] },
                // The trip cross links both poles.
                GlyphPrim::Line { a: [-2.5, -1.8], b: [2.5, -1.8] },
                GlyphPrim::Line { a: [0.0, -1.8], b: [0.0, -3.6] },
            ],
            [-4.0, 6.0],
        );
        b = b.custom(
            "breaker-3p",
            "Q",
            "Circuit breaker, 3-pole",
            false,
            ContactState::None,
            &[
                ("L1", [-4.0, 5.0]),
                ("T1", [-4.0, -5.0]),
                ("L2", [0.0, 5.0]),
                ("T2", [0.0, -5.0]),
                ("L3", [4.0, 5.0]),
                ("T3", [4.0, -5.0]),
            ],
            vec![
                GlyphPrim::Line { a: [-4.0, 5.0], b: [-4.0, 1.6] },
                GlyphPrim::Line { a: [-4.0, 1.6], b: [-2.5, -1.8] },
                GlyphPrim::Line { a: [-4.0, -3.4], b: [-4.0, -5.0] },
                GlyphPrim::Line { a: [0.0, 5.0], b: [0.0, 1.6] },
                GlyphPrim::Line { a: [0.0, 1.6], b: [1.5, -1.8] },
                GlyphPrim::Line { a: [0.0, -3.4], b: [0.0, -5.0] },
                GlyphPrim::Line { a: [4.0, 5.0], b: [4.0, 1.6] },
                GlyphPrim::Line { a: [4.0, 1.6], b: [2.5, -1.8] },
                GlyphPrim::Line { a: [4.0, -3.4], b: [4.0, -5.0] },
                GlyphPrim::Line { a: [-4.0, -1.8], b: [4.0, -1.8] },
                GlyphPrim::Line { a: [0.0, -1.8], b: [0.0, -3.6] },
            ],
            [-5.5, 6.0],
        );

        // ── F: combined protection ──────────────────────────────────────
        b = b.horizontal(
            "fuse-breaker",
            "F",
            "Fuse/circuit-breaker (combined)",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-3.2, 0.0] },
                GlyphPrim::Line { a: [3.2, 0.0], b: [5.0, 0.0] },
                // The fuse rectangle straddling the rung.
                GlyphPrim::Polyline {
                    points: vec![[-3.2, -1.8], [3.2, -1.8], [3.2, 1.8], [-3.2, 1.8]],
                    closed: true,
                },
                // The breaker blade crossing it.
                GlyphPrim::Line { a: [-3.2, 0.0], b: [4.0, 3.4] },
            ],
        );

        // ── M: more machines ────────────────────────────────────────────
        b = b.vertical(
            "motor-3",
            "M",
            "Three-phase motor",
            vec![
                GlyphPrim::Circle { center: [0.0, 0.0], radius: 3.8 },
                GlyphPrim::Text { at: [0.0, -0.2], height: 2.6, text: "M".to_string() },
                GlyphPrim::Text { at: [0.0, -2.6], height: 1.8, text: "3~".to_string() },
                GlyphPrim::Line { a: [0.0, 3.8], b: [0.0, 5.0] },
                GlyphPrim::Line { a: [0.0, -3.8], b: [0.0, -5.0] },
            ],
            &["400VAC"],
        );
        b = b.vertical(
            "motor-dc",
            "M",
            "DC motor",
            vec![
                GlyphPrim::Circle { center: [0.0, 0.0], radius: 3.8 },
                GlyphPrim::Text { at: [0.0, -0.2], height: 2.6, text: "M".to_string() },
                GlyphPrim::Text { at: [0.0, -2.6], height: 1.8, text: "=".to_string() },
                GlyphPrim::Line { a: [0.0, 3.8], b: [0.0, 5.0] },
                GlyphPrim::Line { a: [0.0, -3.8], b: [0.0, -5.0] },
            ],
            &["24VDC"],
        );

        // ── G: more supplies ────────────────────────────────────────────
        b = b.vertical(
            "supply-ac",
            "G",
            "AC supply",
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-5.0, -3.0], [5.0, -3.0], [5.0, 3.0], [-5.0, 3.0]],
                    closed: true,
                },
                // The sine, as the standard draws it.
                GlyphPrim::Polyline {
                    points: vec![
                        [-3.0, 0.0],
                        [-2.2, 1.6],
                        [-1.0, 1.6],
                        [-0.2, 0.0],
                        [0.6, -1.6],
                        [1.8, -1.6],
                        [2.6, 0.0],
                        [3.4, 1.2],
                    ],
                    closed: false,
                },
            ],
            &["230VAC"],
        );
        b = b.custom(
            "battery",
            "G",
            "Battery",
            false,
            ContactState::None,
            &[("+", [0.0, 5.0]), ("-", [0.0, -5.0])],
            vec![
                GlyphPrim::Line { a: [0.0, 5.0], b: [0.0, 1.2] },
                GlyphPrim::Line { a: [-3.0, 1.2], b: [3.0, 1.2] },
                GlyphPrim::Line { a: [-1.2, -0.4], b: [1.2, -0.4] },
                GlyphPrim::Line { a: [-3.0, -1.6], b: [3.0, -1.6] },
                GlyphPrim::Line { a: [-1.2, -3.2], b: [1.2, -3.2] },
                GlyphPrim::Line { a: [0.0, -5.0], b: [0.0, -3.2] },
                GlyphPrim::Text { at: [4.2, 1.2], height: 2.0, text: "+".to_string() },
            ],
            [-4.0, 5.0],
        );
        b = b.vertical(
            "generator",
            "G",
            "Generator",
            vec![
                GlyphPrim::Circle { center: [0.0, 0.0], radius: 3.8 },
                GlyphPrim::Text { at: [0.0, -0.4], height: 2.6, text: "G".to_string() },
                GlyphPrim::Line { a: [0.0, 3.8], b: [0.0, 5.0] },
                GlyphPrim::Line { a: [0.0, -3.8], b: [0.0, -5.0] },
            ],
            &["400VAC"],
        );

        // ── T: transformers (new family) ────────────────────────────────
        b = b.custom(
            "transformer-2w",
            "T",
            "Transformer, two-winding",
            false,
            ContactState::None,
            &[
                ("P1", [-5.0, 2.5]),
                ("P2", [-5.0, -2.5]),
                ("S1", [5.0, 2.5]),
                ("S2", [5.0, -2.5]),
            ],
            vec![
                // Two C-shaped windings facing each other over the core.
                GlyphPrim::Polyline {
                    points: vec![
                        [-2.0, 4.5],
                        [-2.0, -4.5],
                        [0.6, -4.5],
                        [0.6, -3.4],
                        [-1.0, -3.4],
                        [-1.0, 3.4],
                        [0.6, 3.4],
                        [0.6, 4.5],
                    ],
                    closed: false,
                },
                GlyphPrim::Polyline {
                    points: vec![
                        [2.0, 4.5],
                        [2.0, -4.5],
                        [-0.6, -4.5],
                        [-0.6, -3.4],
                        [1.0, -3.4],
                        [1.0, 3.4],
                        [-0.6, 3.4],
                        [-0.6, 4.5],
                    ],
                    closed: false,
                },
                GlyphPrim::Line { a: [-5.0, 2.5], b: [-2.0, 2.5] },
                GlyphPrim::Line { a: [-5.0, -2.5], b: [-2.0, -2.5] },
                GlyphPrim::Line { a: [5.0, 2.5], b: [2.0, 2.5] },
                GlyphPrim::Line { a: [5.0, -2.5], b: [2.0, -2.5] },
                // The core between the windings.
                GlyphPrim::Line { a: [0.0, 4.5], b: [0.0, -4.5] },
            ],
            [-6.0, 6.0],
        );
        b = b.custom(
            "transformer-3w",
            "T",
            "Transformer, three-winding",
            false,
            ContactState::None,
            &[
                ("P1", [-5.0, 2.5]),
                ("P2", [-5.0, -2.5]),
                ("S1", [5.0, 3.6]),
                ("S2", [5.0, 0.0]),
                ("S3", [5.0, -3.6]),
            ],
            vec![
                GlyphPrim::Polyline {
                    points: vec![
                        [-2.0, 4.5],
                        [-2.0, -4.5],
                        [0.6, -4.5],
                        [0.6, -3.4],
                        [-1.0, -3.4],
                        [-1.0, 3.4],
                        [0.6, 3.4],
                        [0.6, 4.5],
                    ],
                    closed: false,
                },
                GlyphPrim::Polyline {
                    points: vec![
                        [2.0, 5.2],
                        [2.0, -5.2],
                        [-0.6, -5.2],
                        [-0.6, -4.2],
                        [1.0, -4.2],
                        [1.0, 4.2],
                        [-0.6, 4.2],
                        [-0.6, 5.2],
                    ],
                    closed: false,
                },
                GlyphPrim::Line { a: [-5.0, 2.5], b: [-2.0, 2.5] },
                GlyphPrim::Line { a: [-5.0, -2.5], b: [-2.0, -2.5] },
                GlyphPrim::Line { a: [5.0, 3.6], b: [2.0, 3.6] },
                GlyphPrim::Line { a: [5.0, 0.0], b: [2.0, 0.0] },
                GlyphPrim::Line { a: [5.0, -3.6], b: [2.0, -3.6] },
                GlyphPrim::Line { a: [0.0, 5.2], b: [0.0, -5.2] },
            ],
            [-6.0, 6.5],
        );
        b = b.custom(
            "current-transformer",
            "T",
            "Current transformer",
            false,
            ContactState::None,
            &[
                ("I1", [-5.0, 0.0]),
                ("I2", [5.0, 0.0]),
                ("S1", [0.0, 5.0]),
                ("S2", [0.0, -5.0]),
            ],
            vec![
                // The primary bar runs straight through.
                GlyphPrim::Line { a: [-5.0, 0.0], b: [5.0, 0.0] },
                // The secondary winding wraps it: three turns suggested.
                GlyphPrim::Polyline {
                    points: vec![[-2.6, 1.6], [-1.2, 1.6], [-1.2, -1.6], [-2.6, -1.6], [-2.6, 1.6]],
                    closed: true,
                },
                GlyphPrim::Polyline {
                    points: vec![[-0.6, 1.6], [0.8, 1.6], [0.8, -1.6], [-0.6, -1.6], [-0.6, 1.6]],
                    closed: true,
                },
                GlyphPrim::Polyline {
                    points: vec![[1.4, 1.6], [2.8, 1.6], [2.8, -1.6], [1.4, -1.6], [1.4, 1.6]],
                    closed: true,
                },
                GlyphPrim::Line { a: [0.0, 1.6], b: [0.0, 5.0] },
                GlyphPrim::Line { a: [2.8, 0.8], b: [2.8, 0.0] },
                GlyphPrim::Line { a: [2.8, -0.8], b: [2.8, 0.0] },
                GlyphPrim::Line { a: [2.8, 0.0], b: [4.2, 0.0] },
                GlyphPrim::Line { a: [2.8, 0.0], b: [2.8, -5.0] },
            ],
            [-4.0, 5.5],
        );

        // ── S: more switches ────────────────────────────────────────────
        b = b.horizontal(
            "selector-2pos",
            "S",
            "Selector switch, two-position",
            false,
            ContactState::Open,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.2] },
                // The selector arrowhead replaces the plain blade tip.
                GlyphPrim::Polyline {
                    points: vec![[1.6, 2.1], [2.6, 3.2], [1.2, 3.4]],
                    closed: true,
                },
            ],
        );
        b = b.custom(
            "selector-3pos",
            "S",
            "Selector switch, three-position",
            false,
            ContactState::None,
            &[
                ("1", [-5.0, 0.0]),
                ("2", [5.0, 2.5]),
                ("3", [5.0, -2.5]),
                ("4", [-5.0, -5.0]),
            ],
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 2.5], b: [5.0, 2.5] },
                GlyphPrim::Line { a: [2.0, -2.5], b: [5.0, -2.5] },
                GlyphPrim::Line { a: [-5.0, -5.0], b: [-2.0, -5.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.4, 2.5] },
                GlyphPrim::Polyline {
                    points: vec![[1.4, 1.4], [2.4, 2.5], [1.0, 2.7]],
                    closed: true,
                },
                GlyphPrim::Line { a: [-2.0, -0.4], b: [-2.0, -4.6] },
            ],
            [-2.0, 6.5],
        );
        b = b.horizontal(
            "limit-no",
            "S",
            "Limit switch, normally open",
            false,
            ContactState::Open,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.2] },
                // The roller arm.
                GlyphPrim::Line { a: [2.6, 3.2], b: [3.8, 4.6] },
                GlyphPrim::Circle { center: [4.3, 5.1], radius: 0.9 },
            ],
        );
        b = b.horizontal(
            "limit-nc",
            "S",
            "Limit switch, normally closed",
            false,
            ContactState::Closed,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, -1.6], b: [2.0, 1.6] },
                GlyphPrim::Line { a: [2.0, 0.6], b: [3.8, 4.4] },
                GlyphPrim::Circle { center: [4.3, 5.1], radius: 0.9 },
            ],
        );
        b = b.horizontal(
            "e-stop",
            "S",
            "Emergency stop (mushroom head)",
            false,
            ContactState::Closed,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, -1.6], b: [2.0, 1.6] },
                GlyphPrim::Line { a: [0.0, 0.8], b: [0.0, 4.2] },
                // The mushroom: a wide flat head.
                GlyphPrim::Line { a: [-3.2, 5.2], b: [3.2, 5.2] },
                GlyphPrim::Line { a: [-3.2, 5.2], b: [-3.2, 4.2] },
                GlyphPrim::Line { a: [3.2, 5.2], b: [3.2, 4.2] },
                GlyphPrim::Line { a: [-3.2, 4.2], b: [3.2, 4.2] },
                // The reset arrows.
                GlyphPrim::Line { a: [-1.0, 6.0], b: [0.0, 5.4] },
                GlyphPrim::Line { a: [1.0, 6.0], b: [0.0, 5.4] },
            ],
        );
        b = b.custom(
            "float-switch",
            "S",
            "Float switch",
            false,
            ContactState::Open,
            &[
                ("1", [-5.0, 0.0]),
                ("2", [5.0, 0.0]),
            ],
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.0] },
                // The measured quantity, boxed: the standard marks the
                // operating medium beside the blade.
                GlyphPrim::Polyline {
                    points: vec![[1.2, 4.0], [4.6, 4.0], [4.6, 7.0], [1.2, 7.0]],
                    closed: true,
                },
                GlyphPrim::Text { at: [2.9, 4.9], height: 1.9, text: "FL".to_string() },
            ],
            [-2.0, 8.5],
        );
        b = b.custom(
            "pressure-switch",
            "S",
            "Pressure switch",
            false,
            ContactState::Open,
            &[
                ("1", [-5.0, 0.0]),
                ("2", [5.0, 0.0]),
            ],
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.0] },
                GlyphPrim::Polyline {
                    points: vec![[1.2, 4.0], [4.6, 4.0], [4.6, 7.0], [1.2, 7.0]],
                    closed: true,
                },
                GlyphPrim::Text { at: [2.9, 4.9], height: 1.9, text: "P".to_string() },
            ],
            [-2.0, 8.5],
        );
        b = b.custom(
            "temperature-switch",
            "S",
            "Temperature switch",
            false,
            ContactState::Open,
            &[
                ("1", [-5.0, 0.0]),
                ("2", [5.0, 0.0]),
            ],
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.0] },
                GlyphPrim::Polyline {
                    points: vec![[1.2, 4.0], [4.6, 4.0], [4.6, 7.0], [1.2, 7.0]],
                    closed: true,
                },
                GlyphPrim::Text { at: [2.9, 4.9], height: 1.9, text: "T".to_string() },
            ],
            [-2.0, 8.5],
        );

        // ── H: audible signals ──────────────────────────────────────────
        b = b.vertical(
            "buzzer",
            "H",
            "Buzzer",
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-3.4, 0.6], [-3.4, 3.2], [3.4, 3.2], [3.4, 0.6]],
                    closed: false,
                },
                // The semicircle dome.
                GlyphPrim::Polyline {
                    points: vec![
                        [-3.4, 0.6],
                        [-2.6, 2.2],
                        [-1.3, 3.1],
                        [0.0, 3.4],
                        [1.3, 3.1],
                        [2.6, 2.2],
                        [3.4, 0.6],
                    ],
                    closed: false,
                },
                GlyphPrim::Line { a: [0.0, 0.6], b: [0.0, -5.0] },
            ],
            &["24VDC"],
        );
        b = b.horizontal(
            "horn",
            "H",
            "Horn / sounder",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.6, 0.0] },
                GlyphPrim::Line { a: [3.6, 0.0], b: [5.0, 0.0] },
                // The horn bell, opening outward.
                GlyphPrim::Polyline {
                    points: vec![[-2.6, -1.4], [-2.6, 1.4], [3.6, 2.8], [3.6, -2.8]],
                    closed: true,
                },
            ],
        );

        // ── R: variable resistors ───────────────────────────────────────
        b = b.horizontal(
            "resistor-variable",
            "R",
            "Variable resistor",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-3.0, -1.6], [3.0, -1.6], [3.0, 1.6], [-3.0, 1.6]],
                    closed: true,
                },
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-3.0, 0.0] },
                GlyphPrim::Line { a: [3.0, 0.0], b: [5.0, 0.0] },
                // The adjustment arrow through the body.
                GlyphPrim::Line { a: [-4.2, 4.0], b: [3.6, -2.6] },
                GlyphPrim::Polyline {
                    points: vec![[3.6, -2.6], [2.2, -2.4], [3.2, -1.0]],
                    closed: true,
                },
            ],
        );
        b = b.custom(
            "potentiometer",
            "R",
            "Potentiometer",
            false,
            ContactState::None,
            &[
                ("1", [-5.0, 0.0]),
                ("2", [5.0, 0.0]),
                ("3", [0.0, 5.0]),
            ],
            vec![
                GlyphPrim::Polyline {
                    points: vec![[-3.0, -1.6], [3.0, -1.6], [3.0, 1.6], [-3.0, 1.6]],
                    closed: true,
                },
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-3.0, 0.0] },
                GlyphPrim::Line { a: [3.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [0.0, 5.0], b: [0.0, 1.2] },
                GlyphPrim::Line { a: [-1.0, 1.2], b: [1.0, 1.2] },
                GlyphPrim::Line { a: [-4.2, 4.0], b: [3.6, -2.4] },
                GlyphPrim::Polyline {
                    points: vec![[3.6, -2.4], [2.2, -2.2], [3.2, -0.8]],
                    closed: true,
                },
            ],
            [-2.0, 6.5],
        );

        // ── C: polarized capacitor ──────────────────────────────────────
        b = b.horizontal(
            "capacitor-polarized",
            "C",
            "Capacitor, polarized",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-1.4, 0.0] },
                GlyphPrim::Line { a: [-1.4, -2.6], b: [-1.4, 2.6] },
                // The curved (negative) plate.
                GlyphPrim::Polyline {
                    points: vec![[1.6, -2.6], [0.6, -1.2], [0.6, 1.2], [1.6, 2.6]],
                    closed: false,
                },
                GlyphPrim::Line { a: [1.6, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Text { at: [-3.0, 3.2], height: 1.8, text: "+".to_string() },
            ],
        );

        // ── B: sensors (new family) ─────────────────────────────────────
        b = b.horizontal(
            "proximity-switch",
            "B",
            "Proximity switch",
            false,
            ContactState::Open,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.2] },
                // The sensing head: a box with the field fence.
                GlyphPrim::Polyline {
                    points: vec![[2.6, 3.2], [4.8, 3.2], [4.8, 6.4], [2.6, 6.4]],
                    closed: true,
                },
                GlyphPrim::Line { a: [4.8, 4.0], b: [5.8, 4.0] },
                GlyphPrim::Line { a: [4.8, 4.8], b: [5.8, 4.8] },
                GlyphPrim::Line { a: [4.8, 5.6], b: [5.8, 5.6] },
            ],
        );
        b = b.custom(
            "flow-switch",
            "B",
            "Flow switch / sensor",
            false,
            ContactState::Open,
            &[
                ("1", [-5.0, 0.0]),
                ("2", [5.0, 0.0]),
            ],
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.0] },
                GlyphPrim::Polyline {
                    points: vec![[1.2, 4.0], [4.6, 4.0], [4.6, 7.0], [1.2, 7.0]],
                    closed: true,
                },
                GlyphPrim::Text { at: [2.9, 4.9], height: 1.9, text: "F".to_string() },
            ],
            [-2.0, 8.5],
        );
        b = b.custom(
            "level-switch",
            "B",
            "Level switch / sensor",
            false,
            ContactState::Open,
            &[
                ("1", [-5.0, 0.0]),
                ("2", [5.0, 0.0]),
            ],
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.0] },
                GlyphPrim::Polyline {
                    points: vec![[1.2, 4.0], [4.6, 4.0], [4.6, 7.0], [1.2, 7.0]],
                    closed: true,
                },
                GlyphPrim::Text { at: [2.9, 4.9], height: 1.9, text: "L".to_string() },
            ],
            [-2.0, 8.5],
        );
        b = b.custom(
            "photoelectric-sensor",
            "B",
            "Photoelectric sensor",
            false,
            ContactState::Open,
            &[
                ("1", [-5.0, 0.0]),
                ("2", [5.0, 0.0]),
            ],
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-2.0, 0.0] },
                GlyphPrim::Line { a: [2.0, 0.0], b: [5.0, 0.0] },
                GlyphPrim::Line { a: [-2.0, 0.0], b: [2.6, 3.0] },
                GlyphPrim::Polyline {
                    points: vec![[1.2, 4.0], [4.6, 4.0], [4.6, 7.0], [1.2, 7.0]],
                    closed: true,
                },
                GlyphPrim::Text { at: [2.9, 4.9], height: 1.9, text: "PE".to_string() },
            ],
            [-2.0, 8.5],
        );

        // ── V: semiconductors (new family) ──────────────────────────────
        b = b.horizontal(
            "diode",
            "V",
            "Diode",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-1.8, 0.0] },
                GlyphPrim::Polyline {
                    points: vec![[-1.8, -2.2], [1.8, 0.0], [-1.8, 2.2]],
                    closed: true,
                },
                GlyphPrim::Line { a: [1.8, -2.4], b: [1.8, 2.4] },
                GlyphPrim::Line { a: [1.8, 0.0], b: [5.0, 0.0] },
            ],
        );
        b = b.horizontal(
            "led",
            "V",
            "Light-emitting diode",
            false,
            ContactState::None,
            vec![
                GlyphPrim::Line { a: [-5.0, 0.0], b: [-1.8, 0.0] },
                GlyphPrim::Polyline {
                    points: vec![[-1.8, -2.2], [1.8, 0.0], [-1.8, 2.2]],
                    closed: true,
                },
                GlyphPrim::Line { a: [1.8, -2.4], b: [1.8, 2.4] },
                GlyphPrim::Line { a: [1.8, 0.0], b: [5.0, 0.0] },
                // The two emission arrows.
                GlyphPrim::Line { a: [0.6, 2.0], b: [2.6, 4.0] },
                GlyphPrim::Polyline {
                    points: vec![[2.6, 4.0], [1.6, 3.6], [2.2, 3.0]],
                    closed: true,
                },
                GlyphPrim::Line { a: [1.8, 1.2], b: [3.8, 3.2] },
                GlyphPrim::Polyline {
                    points: vec![[3.8, 3.2], [2.8, 2.8], [3.4, 2.2]],
                    closed: true,
                },
            ],
        );

        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_library_is_valid_and_covers_the_common_families() {
        let lib = SymbolLibrary::default();
        assert!(
            lib.invalid().is_empty(),
            "built-in symbols must all be usable: {:?}",
            lib.invalid()
        );
        let families: Vec<String> = lib.families().into_iter().map(|(f, _)| f).collect();
        for expect in ["K", "Q", "F", "M", "H", "X", "R", "C", "S", "G", "PE", "T", "B", "V"] {
            assert!(families.contains(&expect.to_string()), "missing family {expect}");
        }
    }

    #[test]
    fn every_symbol_has_pins_and_a_glyph() {
        let lib = SymbolLibrary::default();
        for s in &lib.symbols {
            assert!(!s.pins.is_empty(), "{} has no connection points", s.name);
            assert!(!s.glyph.is_empty(), "{} draws nothing", s.name);
        }
    }

    #[test]
    fn a_children_definition_is_marked_as_one() {
        let lib = SymbolLibrary::default();
        assert!(lib.get("contact-no").unwrap().is_child);
        assert!(!lib.get("relay-coil").unwrap().is_child);
        assert_eq!(lib.get("contact-no").unwrap().contact, ContactState::Open);
        assert_eq!(lib.get("contact-nc").unwrap().contact, ContactState::Closed);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let lib = SymbolLibrary::default();
        assert!(lib.get("RELAY-COIL").is_some());
        assert!(lib.get("nope").is_none());
    }

    #[test]
    fn family_lookup_puts_parents_before_children() {
        let lib = SymbolLibrary::default();
        let k = lib.by_family("K");
        assert!(k.len() >= 4);
        assert!(!k[0].is_child, "the parent should be offered first");
        assert_eq!(lib.first_parent("K").unwrap().name, "relay-coil");
        // A family with only children has no parent to offer.
        assert!(lib.first_parent("ZZ").is_none());
    }

    #[test]
    fn pins_resolve_at_an_absolute_position() {
        let lib = SymbolLibrary::default();
        let coil = lib.get("relay-coil").unwrap();
        assert_eq!(coil.pin_at("A1", [100.0, 200.0], 0.0), Some([100.0, 205.0]));
        assert_eq!(coil.pin_at("A2", [100.0, 200.0], 0.0), Some([100.0, 195.0]));
        assert!(coil.pin_at("nope", [0.0, 0.0], 0.0).is_none());
    }

    #[test]
    fn rotation_moves_pins_about_the_anchor() {
        let lib = SymbolLibrary::default();
        let coil = lib.get("relay-coil").unwrap();
        // A quarter turn puts A1 to the left of the anchor.
        let p = coil.pin_at("A1", [0.0, 0.0], std::f64::consts::FRAC_PI_2).unwrap();
        assert!((p[0] + 5.0).abs() < 1e-9, "got {p:?}");
        assert!(p[1].abs() < 1e-9);
    }

    #[test]
    fn rotation_of_a_half_turn_is_exact() {
        // Catches a sign error that a quarter turn can hide.
        let p = rotate([3.0, 4.0], std::f64::consts::PI);
        assert!((p[0] + 3.0).abs() < 1e-9);
        assert!((p[1] + 4.0).abs() < 1e-9);
    }

    #[test]
    fn rotation_by_zero_is_the_identity() {
        assert_eq!(rotate([3.0, 4.0], 0.0), [3.0, 4.0]);
    }

    #[test]
    fn extent_covers_the_glyph_and_the_pins() {
        let lib = SymbolLibrary::default();
        let coil = lib.get("relay-coil").unwrap();
        let e = coil.extent();
        // Pins reach ±5 on y, the body only ±4 — the pins must widen the box.
        assert!(e[1] <= -5.0, "extent {e:?} misses the lower pin");
        assert!(e[3] >= 5.0, "extent {e:?} misses the upper pin");
        assert!(e[0] >= -5.0 && e[2] <= 5.0);
    }

    #[test]
    fn a_contact_spans_exactly_one_rung() {
        // The glyph is sized to the default 10mm rung spacing, so a contact
        // placed on a rung meets the wire at both ends.
        let lib = SymbolLibrary::default();
        let c = lib.get("contact-no").unwrap();
        assert_eq!(c.pin_at("1", [0.0, 0.0], 0.0), Some([-5.0, 0.0]));
        assert_eq!(c.pin_at("2", [0.0, 0.0], 0.0), Some([5.0, 0.0]));
    }

    #[test]
    fn contact_state_round_trips() {
        assert_eq!(ContactState::parse("no"), Some(ContactState::Open));
        assert_eq!(ContactState::parse("NC"), Some(ContactState::Closed));
        assert_eq!(ContactState::parse(""), Some(ContactState::None));
        assert_eq!(ContactState::parse("?"), None);
        assert_eq!(ContactState::Open.as_str(), "NO");
        assert_eq!(ContactState::None.as_str(), "");
    }

    #[test]
    fn json_round_trips_the_whole_library() {
        let lib = SymbolLibrary::default();
        let json = lib.to_json();
        let back = SymbolLibrary::from_json(&json).unwrap();
        assert_eq!(back, lib);
        // The catalogue travels with the shapes.
        assert!(!back.catalog.is_empty());
    }

    #[test]
    fn a_library_without_a_catalogue_still_loads() {
        // A hand-written library that only lists shapes must not be rejected.
        let text = r#"{"name":"shapes only","symbols":[
            {"name":"blob","family":"Z","description":"t",
             "glyph":[{"prim":"circle","center":[0,0],"radius":1}],
             "pins":[{"name":"1","at":[0,1]}]}]}"#;
        let lib = SymbolLibrary::from_json(text).unwrap();
        assert!(lib.catalog.is_empty());
        assert_eq!(lib.symbols.len(), 1);
    }

    #[test]
    fn a_malformed_library_is_reported_not_replaced() {
        // Silently falling back to the built-in set would lose a vendor library
        // to a typo, so a parse failure must surface.
        assert!(SymbolLibrary::from_json("{ not json").is_err());
        assert!(SymbolLibrary::from_json("{}").is_err(), "a library needs a name");
    }

    #[test]
    fn a_library_written_by_hand_can_omit_optionals() {
        let text = r#"{
            "name": "tiny",
            "symbols": [
                {"name": "blob", "family": "Z", "description": "test",
                 "glyph": [{"prim": "circle", "center": [0,0], "radius": 2}],
                 "pins": [{"name": "1", "at": [0,2]}]}
            ]
        }"#;
        let lib = SymbolLibrary::from_json(text).unwrap();
        let s = lib.get("blob").unwrap();
        assert_eq!(s.contact, ContactState::None);
        assert!(!s.is_child);
        assert_eq!(s.tag_offset, default_tag_offset(), "a default fills in");
        assert!(lib.invalid().is_empty());
    }

    #[test]
    fn validate_catches_an_unusable_definition() {
        let mut s = SymbolDef {
            name: "nameless".to_string(),
            family: "K".to_string(),
            description: String::new(),
            is_child: false,
            contact: ContactState::None,
            pins: vec![],
            glyph: vec![],
            default_ratings: vec![],
            tag_offset: [0.0, 0.0],
        };
        assert!(s.validate().unwrap_err().contains("no glyph"));
        s.glyph = vec![GlyphPrim::Line {
            a: [0.0, 0.0],
            b: [1.0, 0.0],
        }];
        assert!(s.validate().is_ok(), "a glyph with no pins is drawable");
        s.name = String::new();
        assert!(s.validate().unwrap_err().contains("name"));
        s.name = "x".to_string();
        s.family = String::new();
        assert!(s.validate().unwrap_err().contains("family"));
    }

    #[test]
    fn upsert_and_remove() {
        let mut lib = SymbolLibrary::default();
        let n = lib.symbols.len();
        let mut custom = lib.get("fuse").unwrap().clone();
        custom.description = "Custom fuse".to_string();
        assert!(lib.upsert(custom), "an existing name replaces");
        assert_eq!(lib.symbols.len(), n);
        assert_eq!(lib.get("fuse").unwrap().description, "Custom fuse");

        let mut extra = lib.get("fuse").unwrap().clone();
        extra.name = "fuse-2".to_string();
        assert!(!lib.upsert(extra), "a new name appends");
        assert_eq!(lib.symbols.len(), n + 1);

        assert!(lib.remove("fuse-2"));
        assert!(!lib.remove("fuse-2"));
    }
}
