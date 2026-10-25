//! Electrical schematic data model.
//!
//! The design mirrors AutoCAD Electrical's three-tier scheme as far as a
//! plugin without a project database can: this module holds the per-drawing
//! settings carrier (ACADE's invisible `WD_M` block, whose ~50 attributes
//! define every tag and wire-number format the rest of the tool reads) and the
//! per-entity records for wires, components, terminals and cables.
//!
//! Every record is persisted as ordered key/value XDATA under [`APPID`], so the
//! data round-trips through DWG/DXF with the drawing rather than living in a
//! side database.

/// XDATA application id. Distinct from the wiring-harness plugin's `OCSWH`:
/// both plugins can be installed at once, and each must only ever see its own
/// records.
pub const APPID: &str = "OCSEL";

/// Current record schema. Bump when a record's meaning changes; `codec.rs` is
/// the migration point.
pub const SCHEMA_VERSION: i32 = 1;

/// Oldest schema this build can still read.
pub const SCHEMA_MIN_SUPPORTED: i32 = 1;

/// Layer for invisible metadata carriers (the settings/ladder/report records).
pub const META_LAYER: &str = "EL_META";

// ── record kinds ─────────────────────────────────────────────────────────

pub const KIND_SETTINGS: &str = "settings";
pub const KIND_LADDER: &str = "ladder";
pub const KIND_WIRETYPES: &str = "wiretypes";
pub const KIND_WIRE: &str = "wire";
pub const KIND_COMPONENT: &str = "component";
pub const KIND_TERMINAL: &str = "terminal";
pub const KIND_CABLE: &str = "cable";
pub const KIND_CONNECTOR: &str = "connector";
pub const KIND_SIGNAL: &str = "signal";
pub const KIND_JUMPER: &str = "jumper";
pub const KIND_STRIPPLOT: &str = "stripplot";
pub const KIND_FOOTPRINT: &str = "footprint";
pub const KIND_PLC: &str = "plc";
pub const KIND_XREF: &str = "xref";

// ── drawing layers ───────────────────────────────────────────────────────
//
// ACADE drives everything off a layer map stored in `WD_M`; we keep the same
// idea with fixed defaults that `EL_SETTINGS` can retarget.

pub const LAYER_LADDER: &str = "EL_LADDER";
pub const LAYER_WIRES: &str = "EL_WIRES";
pub const LAYER_COMPONENTS: &str = "EL_COMPONENTS";
pub const LAYER_WIRENO: &str = "EL_WIRENO";
pub const LAYER_XREF: &str = "EL_XREF";
pub const LAYER_TAGS: &str = "EL_TAGS";
pub const LAYER_TERMINALS: &str = "EL_TERMINALS";
pub const LAYER_CABLES: &str = "EL_CABLES";
pub const LAYER_SIGNALS: &str = "EL_SIGNALS";
pub const LAYER_STRIPS: &str = "EL_STRIPS";
pub const LAYER_PANEL: &str = "EL_PANEL";
pub const LAYER_PLC: &str = "EL_PLC";
pub const LAYER_BALLOONS: &str = "EL_BALLOONS";
pub const LAYER_TABLE: &str = "EL_TABLE";

/// Text height for reference-designator labels, mm.
pub const LABEL_HEIGHT: f64 = 2.5;

/// Line-reference text height on a ladder, mm.
pub const RUNG_REF_HEIGHT: f64 = 2.5;

/// How far a wire must be from a rung, in mm, to be considered *on* it. Also
/// the default tolerance for "is this wire on rung N".
pub const RUNG_TOLERANCE_MM: f64 = 5.0;

/// Width of a drawn wire-crossing gap, mm. ACADE leaves a small break rather
/// than a dot so a crossing never reads as a junction.
pub const GAP_MM: f64 = 1.5;

/// Layer prefix for wire-type layers built from a `%C`/`%S` pattern.
pub const WIRE_LAYER_PATTERN: &str = "EL_WIRE_%C_%S";

// ── numbering modes ──────────────────────────────────────────────────────

/// How automatic tags are assigned (ACADE's `TAGMODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagMode {
    /// A running counter per family, independent of where the symbol sits.
    #[default]
    Sequential,
    /// The tag number comes from the line reference of the rung it sits on,
    /// with a suffix when several devices share one reference.
    Reference,
}

impl TagMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TagMode::Sequential => "S",
            TagMode::Reference => "R",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "S" | "SEQ" | "SEQUENTIAL" => Some(TagMode::Sequential),
            "R" | "REF" | "REFERENCE" => Some(TagMode::Reference),
            _ => None,
        }
    }
}

/// How automatic wire numbers are assigned (ACADE's `WIREMODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WireNoMode {
    /// A running counter with a per-drawing start value.
    #[default]
    Sequential,
    /// The wire number *is* the rung's line reference, suffixed when several
    /// wires share one rung.
    Reference,
}

impl WireNoMode {
    pub fn as_str(self) -> &'static str {
        match self {
            WireNoMode::Sequential => "S",
            WireNoMode::Reference => "R",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "S" | "SEQ" | "SEQUENTIAL" => Some(WireNoMode::Sequential),
            "R" | "REF" | "REFERENCE" => Some(WireNoMode::Reference),
            _ => None,
        }
    }
}

/// Per-wire number type. `Fixed` numbers survive a renumber pass; `Extra`
/// numbers mark wires inside a network that already has a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NumberType {
    #[default]
    Normal,
    Fixed,
    Extra,
}

impl NumberType {
    pub fn as_str(self) -> &'static str {
        match self {
            NumberType::Normal => "NORMAL",
            NumberType::Fixed => "FIXED",
            NumberType::Extra => "EXTRA",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "NORMAL" | "N" => Some(NumberType::Normal),
            "FIXED" | "FIX" | "F" => Some(NumberType::Fixed),
            "EXTRA" | "X" => Some(NumberType::Extra),
            _ => None,
        }
    }
}

// ── format strings ───────────────────────────────────────────────────────

/// The drawing/project values a format string can interpolate.
#[derive(Debug, Clone, Default)]
pub struct FormatContext<'a> {
    pub sheet: &'a str,
    pub drawing: &'a str,
    pub installation: &'a str,
    pub location: &'a str,
    /// The duplicate suffix (`%X`) — `A`, `B`, … when several devices share one
    /// line reference. Empty for a tag.
    pub suffix: &'a str,
}

/// Expand a tag/wire-number format string.
///
/// Recognised tokens, matching ACADE's convention: `%F` family code, `%N` the
/// allocated number, `%X` the duplicate suffix, `%S` sheet, `%D` drawing,
/// `%I` installation, `%L` location. An unknown `%x` is left verbatim so a typo
/// is visible in the output rather than silently swallowed.
pub fn format_refdes(fmt: &str, family: &str, number: i32, ctx: &FormatContext) -> String {
    format_refdes_text(fmt, family, &number.to_string(), ctx)
}

/// As [`format_refdes`], but `%N` expands a string.
///
/// Reference-based wire numbering substitutes a *line reference* for `%N`, and
/// a reference is not necessarily numeric (`2A00` is a perfectly ordinary one),
/// so the number slot has to accept text.
pub fn format_refdes_text(
    fmt: &str,
    family: &str,
    number_text: &str,
    ctx: &FormatContext,
) -> String {
    let mut out = String::with_capacity(fmt.len() + number_text.len() + 8);
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('F') => out.push_str(family),
            Some('N') => out.push_str(number_text),
            Some('X') => out.push_str(ctx.suffix),
            Some('S') => out.push_str(ctx.sheet),
            Some('D') => out.push_str(ctx.drawing),
            Some('I') => out.push_str(ctx.installation),
            Some('L') => out.push_str(ctx.location),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// ACADE requires `%N` in a tag or wire-number format: without it every device
/// in a family collides on the same string. Validation surfaces this as an
/// audit error rather than a silent duplicate.
pub fn format_uses_number(fmt: &str) -> bool {
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' && chars.next() == Some('N') {
            return true;
        }
    }
    false
}

/// The ref-des family code: the leading alphabetic run of a tag (`K12` → `K`,
/// `X1A` → `X`). ACADE seeds this from the symbol's `TAG1` default.
pub fn family_of(tag: &str) -> String {
    tag.chars().take_while(|c| c.is_ascii_alphabetic()).collect()
}

/// Duplicate suffix for the *n*-th device sharing a reference: `0` is the bare
/// reference, `1` → `A`, `26` → `Z`, `27` → `AA`.
pub fn suffix_for_index(index: usize) -> String {
    if index == 0 {
        return String::new();
    }
    let mut n = index;
    let mut out = String::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        out.insert(0, (b'A' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    out
}

// ── terminal record ──────────────────────────────────────────────────────

/// The mechanical class of a terminal block.
///
/// The class matters beyond appearance: a ground terminal is not numbered, a
/// fused one carries a part, and a multi-level one links to its siblings — so
/// the audits and the terminal plan both branch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalKind {
    #[default]
    Screw,
    Spring,
    Ground,
    Fused,
    Disconnect,
    /// A two-level (or deeper) block: `LINKTERM` associates its levels.
    Multi,
    Test,
}

impl TerminalKind {
    pub const ALL: [TerminalKind; 7] = [
        TerminalKind::Screw,
        TerminalKind::Spring,
        TerminalKind::Ground,
        TerminalKind::Fused,
        TerminalKind::Disconnect,
        TerminalKind::Multi,
        TerminalKind::Test,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TerminalKind::Screw => "SCREW",
            TerminalKind::Spring => "SPRING",
            TerminalKind::Ground => "GROUND",
            TerminalKind::Fused => "FUSED",
            TerminalKind::Disconnect => "DISCONNECT",
            TerminalKind::Multi => "MULTI",
            TerminalKind::Test => "TEST",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "SCREW" | "S" => Some(TerminalKind::Screw),
            "SPRING" | "P" => Some(TerminalKind::Spring),
            "GROUND" | "PE" | "G" => Some(TerminalKind::Ground),
            "FUSED" | "F" => Some(TerminalKind::Fused),
            "DISCONNECT" | "D" => Some(TerminalKind::Disconnect),
            "MULTI" | "M" => Some(TerminalKind::Multi),
            "TEST" | "T" => Some(TerminalKind::Test),
            _ => None,
        }
    }
}

/// One terminal block on a strip.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalRec {
    pub el_id: String,
    /// Where the block sits, for the same reason a component records it.
    pub origin_x: f64,
    pub origin_y: f64,
    pub rotation_deg: f64,
    /// The strip this terminal belongs to — ACADE's `TAGSTRIP`, the only
    /// attribute a schematic terminal strictly needs.
    pub tag_strip: String,
    /// The terminal's own number on the strip (`TERM01`). Empty means "derive it
    /// from the strip order", which is what a drafter usually wants.
    pub number: String,
    /// Where it sits in the strip, for ordering. Ties are broken by position.
    pub strip_seq: i32,
    pub kind: TerminalKind,
    pub part_number: String,
    pub description: String,
    /// For a multi-level block: the terminal it is a level of.
    pub linked: String,
    /// The partition group the terminal belongs to (`A`, `B`, `MAIN`…), or
    /// empty for none. A strip divides where the label changes — the drawn
    /// strip diagram and the terminal plan mark the boundary.
    pub partition: String,
    /// Connection points, in drawing order.
    pub pins: Vec<String>,
}

impl TerminalRec {
    /// The two wire connection points, in drawing units: left then right.
    pub fn connection_points(&self) -> [[f64; 2]; 2] {
        let (s, c) = self.rotation_deg.to_radians().sin_cos();
        // The leads run 7.5 mm either side of the anchor, matching `geometry`.
        let mut out = [[0.0, 0.0]; 2];
        for (i, sign) in [-1.0f64, 1.0].iter().enumerate() {
            let (dx, dy) = (sign * 7.5, 0.0);
            out[i] = [
                self.origin_x + dx * c - dy * s,
                self.origin_y + dx * s + dy * c,
            ];
        }
        out
    }

    pub fn new(el_id: &str, tag_strip: &str, kind: TerminalKind) -> Self {
        TerminalRec {
            el_id: el_id.to_string(),
            origin_x: 0.0,
            origin_y: 0.0,
            rotation_deg: 0.0,
            tag_strip: tag_strip.to_string(),
            number: String::new(),
            strip_seq: 0,
            kind,
            part_number: String::new(),
            description: String::new(),
            linked: String::new(),
            partition: String::new(),
            pins: vec!["1".to_string(), "2".to_string()],
        }
    }

    /// The full designator a report and a wirelist use: `X1:1`.
    pub fn designator(&self) -> String {
        if self.tag_strip.is_empty() {
            return String::new();
        }
        if self.number.is_empty() {
            return self.tag_strip.clone();
        }
        format!("{}:{}", self.tag_strip, self.number)
    }
}



// ── PLC module record ────────────────────────────────────────────────────

/// One parametric PLC I/O module (textbook CH10): a box of `di` input points
/// on the left rail and `do` output points on the right, **generated from the
/// counts** — which is what makes a 16-point card one command.
///
/// Points are named `I1..In` / `Q1..Qn` and addressed `BASE + index` (`I0.0`
/// style by default; `BASE=I:1/` gives the rack:slot style). A wire names a
/// point exactly as it names any other pin (`FROM=P1.I3`), so connectivity,
/// reports and audit work through the standing machinery.
#[derive(Debug, Clone, PartialEq)]
pub struct PlcRec {
    pub el_id: String,
    pub tag: String,
    pub origin_x: f64,
    pub origin_y: f64,
    /// Digital input point count.
    pub di: i32,
    /// Digital output point count.
    pub do_: i32,
    /// Address prefix for inputs (default `I0.` → I0.0, I0.1, …).
    pub base_i: String,
    /// Address prefix for outputs (default `Q0.`).
    pub base_q: String,
    pub description: String,
    pub mfg: String,
    pub cat: String,
}

impl PlcRec {
    pub fn new(el_id: &str, tag: &str, di: i32, do_: i32) -> Self {
        PlcRec {
            el_id: el_id.to_string(),
            tag: tag.to_string(),
            origin_x: 0.0,
            origin_y: 0.0,
            di,
            do_,
            base_i: "I0.".to_string(),
            base_q: "Q0.".to_string(),
            description: String::new(),
            mfg: String::new(),
            cat: String::new(),
        }
    }

    /// Every point name, inputs then outputs, in rail order.
    pub fn point_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 1..=self.di.max(0) {
            out.push(format!("I{i}"));
        }
        for i in 1..=self.do_.max(0) {
            out.push(format!("Q{i}"));
        }
        out
    }

    /// The address a point carries (`P1.I3` → `I0.2` — the third input is
    /// index 2, because addresses are zero-based the way a PLC numbers them).
    pub fn address_of(&self, point: &str) -> Option<String> {
        let (rail, n) = point.split_at(1.max(point.len().min(1)));
        let idx: i32 = n.parse().ok()?;
        match rail {
            "I" if idx >= 1 && idx <= self.di => Some(format!("{}{}", self.base_i, idx - 1)),
            "Q" if idx >= 1 && idx <= self.do_ => Some(format!("{}{}", self.base_q, idx - 1)),
            _ => None,
        }
    }

    /// The address label for every point, `(point, address)`.
    pub fn addresses(&self) -> Vec<(String, String)> {
        self.point_names()
            .into_iter()
            .filter_map(|p| self.address_of(&p).map(|a| (p, a)))
            .collect()
    }

    /// A point's pin position in the drawing: inputs on the left rail,
    /// outputs on the right, numbered top to bottom.
    pub fn point_position(&self, point: &str) -> Option<[f64; 2]> {
        const ROW: f64 = 6.0;
        const W: f64 = 40.0;
        let (rail, n) = point.split_at(1.max(point.len().min(1)));
        let idx: i32 = n.parse().ok()?;
        // The taller rail sizes the box; points sit from the top.
        let rows = self.di.max(self.do_).max(1) as f64;
        let top = self.origin_y + rows * ROW / 2.0;
        let y = top - (idx as f64 - 0.5) * ROW;
        match rail {
            "I" if idx >= 1 && idx <= self.di => Some([self.origin_x - W / 2.0, y]),
            "Q" if idx >= 1 && idx <= self.do_ => Some([self.origin_x + W / 2.0, y]),
            _ => None,
        }
    }

    /// Body size `[w, h]` for the drawn module.
    pub fn body_mm(&self) -> [f64; 2] {
        const ROW: f64 = 6.0;
        [40.0, self.di.max(self.do_).max(1) as f64 * ROW]
    }
}

// ── footprint record ─────────────────────────────────────────────────────

/// One panel-layout footprint: the physical outline of a device on the panel,
/// linked to its schematic component **by tag**.
///
/// The link is the whole point of ACADE's panel drawings (CH8): the footprint
/// is placed from the schematic list, carries the device's catalog data, and
/// gets an **item number** that the panel BOM and the balloons key off. A
/// footprint whose tag matches no schematic component still places (a panel
/// may hold items the schematic omits) — the audit reports it as a warning,
/// never an error.
#[derive(Debug, Clone, PartialEq)]
pub struct FootprintRec {
    pub el_id: String,
    /// The schematic device's tag (`K1`); the bi-directional link.
    pub tag: String,
    pub origin_x: f64,
    pub origin_y: f64,
    /// Body size in mm. Defaults come from the family when not stated.
    pub width_mm: f64,
    pub height_mm: f64,
    /// The BOM item number; `0` means "not numbered yet".
    pub item_number: i32,
    pub description: String,
    pub mfg: String,
    pub cat: String,
    /// A balloon is drawn for this footprint. The balloon itself is a pure
    /// render (rebuilt from the record), so this flag — not any drawn entity
    /// — is the truth about whether one exists.
    pub ballooned: bool,
}

impl FootprintRec {
    pub fn new(el_id: &str, tag: &str) -> Self {
        let (w, h) = family_footprint_mm(&family_of(tag));
        FootprintRec {
            el_id: el_id.to_string(),
            tag: tag.to_string(),
            origin_x: 0.0,
            origin_y: 0.0,
            width_mm: w,
            height_mm: h,
            item_number: 0,
            description: String::new(),
            mfg: String::new(),
            cat: String::new(),
            ballooned: false,
        }
    }
}

/// Default panel body size per family — the stand-in for ACADE's footprint
/// lookup table, sized from typical DIN-rail hardware. Unknown families get a
/// generic slot.
pub fn family_footprint_mm(family: &str) -> (f64, f64) {
    match family.to_ascii_uppercase().as_str() {
        "K" => (35.0, 75.0),        // relay / contactor on a DIN rail
        "Q" => (45.0, 90.0),        // breaker
        "F" => (18.0, 80.0),        // fuse holder / overload
        "M" => (90.0, 90.0),        // motor body
        "G" => (60.0, 60.0),        // supply / battery
        "T" => (75.0, 90.0),        // transformer
        "X" => (18.0, 120.0),       // terminal block row
        "S" => (22.0, 22.0),        // pushbutton hole
        "H" => (22.0, 22.0),        // pilot lamp hole
        "R" | "C" | "V" => (18.0, 45.0),
        "B" => (30.0, 45.0),        // sensor / amplifier
        _ => (40.0, 40.0),
    }
}

// ── jumper record ────────────────────────────────────────────────────────

/// A jumper bridging two terminals of one strip into a single circuit.
///
/// This is the electrical core of a terminal strip editor: two jumpered
/// terminals are one node, so nets derived from wires merge across the
/// jumper (`net.rs` unions the designators), while the drawn terminals keep
/// their own numbers and parts. The record names terminals by their numbers
/// on `strip` — never by handle or `el_id`, for the usual reason.
#[derive(Debug, Clone, PartialEq)]
pub struct JumperRec {
    pub el_id: String,
    pub strip: String,
    pub a_number: String,
    pub b_number: String,
}

impl JumperRec {
    pub fn new(el_id: &str, strip: &str, a_number: &str, b_number: &str) -> Self {
        JumperRec {
            el_id: el_id.to_string(),
            strip: strip.to_string(),
            a_number: a_number.to_string(),
            b_number: b_number.to_string(),
        }
    }

    /// The two designators the jumper unites, in record order.
    pub fn designators(&self) -> (String, String) {
        (
            format!("{}:{}", self.strip, self.a_number),
            format!("{}:{}", self.strip, self.b_number),
        )
    }

    /// Does this jumper touch `designator` (`X1:1`)?
    pub fn touches(&self, designator: &str) -> bool {
        let (a, b) = self.designators();
        a == designator || b == designator
    }
}

// ── strip-plot record ────────────────────────────────────────────────────

/// One drawn strip diagram (`EL_STRIP`) — the graphical terminal plan.
///
/// The plot is a **render** of the strip's terminals, jumpers, partitions and
/// wires; this record only carries where it sits and which strip it shows, so
/// re-drawing a strip replaces its plot and deleting it is the standard
/// machinery. One plot per strip: a second draw of the same strip replaces
/// the first.
#[derive(Debug, Clone, PartialEq)]
pub struct StripPlotRec {
    pub el_id: String,
    pub strip: String,
    pub origin_x: f64,
    pub origin_y: f64,
}

impl StripPlotRec {
    pub fn new(el_id: &str, strip: &str) -> Self {
        StripPlotRec {
            el_id: el_id.to_string(),
            strip: strip.to_string(),
            origin_x: 0.0,
            origin_y: 0.0,
        }
    }
}

// ── cable record ─────────────────────────────────────────────────────────

/// One cable marker.
///
/// A cable is a *multi-conductor* object, and ACADE models it as a marker per
/// conductor: the first is the parent carrying the cable's tag and part, and
/// each later one is a child that pulls the parent's tag and takes the next
/// unused conductor colour. That is why this record carries both a conductor
/// index and a parent tag.
#[derive(Debug, Clone, PartialEq)]
pub struct CableRec {
    pub el_id: String,
    /// The cable's designator (`W1`), shared by every marker of the cable.
    pub tag: String,
    /// The parent marker's tag, empty on the parent itself.
    pub parent_tag: String,
    /// Which conductor of the cable this marker shows (1-based).
    pub conductor_index: i32,
    /// The conductor colour this marker carries.
    pub color: String,
    pub gauge: String,
    pub mfg: String,
    pub cat: String,
    pub part_number: String,
    pub description: String,
    /// Source and destination devices, for a cable from/to report.
    pub from_id: String,
    pub to_id: String,
}

impl CableRec {
    pub fn new(el_id: &str, tag: &str) -> Self {
        CableRec {
            el_id: el_id.to_string(),
            tag: tag.to_string(),
            parent_tag: String::new(),
            conductor_index: 1,
            color: String::new(),
            gauge: String::new(),
            mfg: String::new(),
            cat: String::new(),
            part_number: String::new(),
            description: String::new(),
            from_id: String::new(),
            to_id: String::new(),
        }
    }

    /// A parent marker is the one that owns the cable's identity.
    pub fn is_parent(&self) -> bool {
        self.parent_tag.is_empty()
    }
}

// ── signal record ────────────────────────────────────────────────────────

/// Which way a signal arrow points in the drawing's story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignalDirection {
    /// The signal *leaves* this sheet here.
    #[default]
    Source,
    /// The signal *arrives* on this sheet here.
    Dest,
}

impl SignalDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            SignalDirection::Source => "SOURCE",
            SignalDirection::Dest => "DEST",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "SOURCE" | "SRC" | "OUT" | "GOING" => Some(SignalDirection::Source),
            "DEST" | "DESTINATION" | "IN" | "COMING" => Some(SignalDirection::Dest),
            _ => None,
        }
    }
}

/// One signal arrow: an off-sheet continuation marker anchored to a wire end.
///
/// ACADE calls these source/destination arrows. The arrow does not change a
/// wire's connectivity — it *additionally* declares that the signal continues
/// on another sheet. Sheets pair through the project register (`el-core/src/
/// project.rs`): a destination arrow labels itself with the sheet its source
/// lives on, once both sheets have synced.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalRec {
    pub el_id: String,
    /// The signal's name (`STOP`, `E-STOP-2`); the pairing key across sheets.
    pub name: String,
    pub direction: SignalDirection,
    /// Where the arrow sits, in drawing units — recorded because a signal is
    /// drawn as several primitives and its anchor is not recoverable from any
    /// one of them.
    pub origin_x: f64,
    pub origin_y: f64,
    /// The wire the arrow anchors to, by that wire's `el_id` (never a handle).
    pub wire_el_id: String,
    /// Which end of the wire: `false` = the from/first end, `true` = the
    /// to/last end. What lets a re-seat carry the arrow along.
    pub at_to_end: bool,
}

impl SignalRec {
    pub fn new(el_id: &str, name: &str, direction: SignalDirection) -> Self {
        SignalRec {
            el_id: el_id.to_string(),
            name: name.to_string(),
            direction,
            origin_x: 0.0,
            origin_y: 0.0,
            wire_el_id: String::new(),
            at_to_end: false,
        }
    }
}

// ── connector record ─────────────────────────────────────────────────────

/// How a connector is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectorOrientation {
    /// Receptacle and plug facing each other — the default.
    #[default]
    Combo,
    /// Receptacle only.
    Receptacle,
    /// Plug only.
    Plug,
}

impl ConnectorOrientation {
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectorOrientation::Combo => "COMBO",
            ConnectorOrientation::Receptacle => "RECEPTACLE",
            ConnectorOrientation::Plug => "PLUG",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "COMBO" | "BOTH" => Some(ConnectorOrientation::Combo),
            "RECEPTACLE" | "REC" | "R" => Some(ConnectorOrientation::Receptacle),
            "PLUG" | "P" => Some(ConnectorOrientation::Plug),
            _ => None,
        }
    }
}

/// A parametric connector: its pins are *generated* from a list and a spacing,
/// not drawn one by one.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorRec {
    pub el_id: String,
    /// Where the connector sits, for the same reason a component records it.
    pub origin_x: f64,
    pub origin_y: f64,
    pub rotation_deg: f64,
    pub ref_des: String,
    /// The pin list, in drawing order (`1,2,3`, or `A,B,C,GND`). ACADE stores
    /// this as `PINLIST`; the order is the geometry, so it is a list, not a set.
    pub pins: Vec<String>,
    pub pin_spacing_mm: f64,
    pub orientation: ConnectorOrientation,
    /// Pins per row; 0 means a single row.
    pub pins_per_row: i32,
    pub mfg: String,
    pub cat: String,
    pub part_number: String,
    pub description: String,
    pub location: String,
    /// Whether a wire number changes across this connector (a feed-through).
    pub wire_number_change: bool,
}

impl ConnectorRec {
    /// A pin's absolute position, using the connector's recorded placement.
    pub fn pin_point_in_drawing(&self, name: &str) -> Option<[f64; 2]> {
        self.pin_position(
            name,
            [self.origin_x, self.origin_y],
            self.rotation_deg.to_radians(),
        )
    }

    pub fn new(el_id: &str, ref_des: &str, pin_count: usize) -> Self {
        ConnectorRec {
            el_id: el_id.to_string(),
            origin_x: 0.0,
            origin_y: 0.0,
            rotation_deg: 0.0,
            ref_des: ref_des.to_string(),
            pins: (1..=pin_count).map(|i| i.to_string()).collect(),
            pin_spacing_mm: 5.0,
            orientation: ConnectorOrientation::Combo,
            pins_per_row: 0,
            mfg: String::new(),
            cat: String::new(),
            part_number: String::new(),
            description: String::new(),
            location: String::new(),
            wire_number_change: false,
        }
    }

    pub fn pin_count(&self) -> usize {
        self.pins.len()
    }

    pub fn has_pin(&self, name: &str) -> bool {
        self.pins.iter().any(|p| p == name)
    }

    /// How many pins sit in one row.
    pub fn row_len(&self) -> usize {
        if self.pins_per_row > 0 {
            self.pins_per_row as usize
        } else {
            self.pins.len().max(1)
        }
    }

    /// The local offset of the *i*-th pin, in the connector's own coordinates.
    ///
    /// Pins run left to right then wrap; wrapping is what makes a 24-pin
    /// connector readable instead of a metre wide.
    pub fn pin_offset(&self, index: usize) -> [f64; 2] {
        let row_len = self.row_len();
        let row = index / row_len;
        let col = index % row_len;
        // Centre each row on the connector's origin.
        let x = (col as f64 - (row_len as f64 - 1.0) / 2.0) * self.pin_spacing_mm;
        let y = -(row as f64) * self.pin_spacing_mm * 2.0;
        [x, y]
    }

    /// A pin's absolute position.
    pub fn pin_position(&self, name: &str, origin: [f64; 2], rot_rad: f64) -> Option<[f64; 2]> {
        let index = self.pins.iter().position(|p| p == name)?;
        let (s, c) = rot_rad.sin_cos();
        let local = self.pin_offset(index);
        let off = [local[0] * c - local[1] * s, local[0] * s + local[1] * c];
        Some([origin[0] + off[0], origin[1] + off[1]])
    }

    /// Bounding box `[min_x, min_y, max_x, max_y]` in local coordinates.
    pub fn extent(&self) -> [f64; 4] {
        let n = self.pins.len().max(1);
        let row_len = self.row_len();
        let rows = n.div_ceil(row_len.max(1));
        let half_w = (row_len as f64 - 1.0) * self.pin_spacing_mm / 2.0 + self.pin_spacing_mm;
        let half_h = ((rows.max(1) as f64) * self.pin_spacing_mm * 2.0) / 2.0;
        [-half_w, -half_h, half_w, half_h]
    }
}


// ── settings record (the `WD_M` substitute) ──────────────────────────────

/// Per-drawing electrical settings.
///
/// This is the single record every other feature reads its formats and
/// defaults from — the same role ACADE's `WD_M` block plays, and the reason
/// both put it first.
#[derive(Debug, Clone, PartialEq)]
pub struct ElSettingsRec {
    // drawing identity
    pub sheet: String,
    pub drawing: String,
    pub installation: String,
    pub location: String,

    // ladder defaults
    pub ladder_width_mm: f64,
    pub rung_spacing_mm: f64,
    pub rung_increment: i32,
    pub first_rung_ref: String,
    pub rung_count: i32,

    // component tagging
    pub tag_mode: TagMode,
    pub tag_format: String,
    pub tag_start: i32,
    pub tag_suffixes: String,

    // wire numbering
    pub wireno_mode: WireNoMode,
    pub wireno_format: String,
    pub wireno_start: i32,
    pub wireno_increment: i32,

    /// Half-gap each side of a crossing, for wires flagged `gap` (mm). The
    /// crossing itself is where the *other* wire passes through, so the total
    /// visual break is twice this.
    pub wire_gap_mm: f64,

    pub revision: i32,
}

impl Default for ElSettingsRec {
    fn default() -> Self {
        ElSettingsRec {
            sheet: String::new(),
            drawing: String::new(),
            installation: String::new(),
            location: String::new(),
            ladder_width_mm: 170.0,
            rung_spacing_mm: 10.0,
            rung_increment: 1,
            first_rung_ref: "100".to_string(),
            rung_count: 10,
            tag_mode: TagMode::Sequential,
            // `%F` (family) + `%N` (number) is ACADE's default tag format.
            tag_format: "%F%N".to_string(),
            tag_start: 1,
            tag_suffixes: "A,B,C,D,E,F,G,H".to_string(),
            wireno_mode: WireNoMode::Sequential,
            wireno_format: "%N".to_string(),
            wireno_start: 1,
            wireno_increment: 1,
            wire_gap_mm: 0.8,
            revision: 1,
        }
    }
}

impl ElSettingsRec {
    /// The context a format string interpolates against.
    pub fn format_context(&self) -> FormatContext<'_> {
        FormatContext {
            sheet: &self.sheet,
            drawing: &self.drawing,
            installation: &self.installation,
            location: &self.location,
            suffix: "",
        }
    }

    /// The context with a duplicate suffix, for reference-based numbering where
    /// several wires share one line reference.
    pub fn format_context_with_suffix<'a>(&'a self, suffix: &'a str) -> FormatContext<'a> {
        FormatContext {
            suffix,
            ..self.format_context()
        }
    }

    /// Expand this drawing's tag format for one device.
    pub fn format_tag(&self, family: &str, number: i32) -> String {
        format_refdes(&self.tag_format, family, number, &self.format_context())
    }

    /// As [`Self::format_tag`], with the `%N` slot taking text — which is what
    /// reference-based *tagging* needs, since a tag's number is a line reference
    /// and a reference need not be numeric.
    pub fn format_tag_text(&self, family: &str, number_text: &str) -> String {
        format_refdes_text(
            &self.tag_format,
            family,
            number_text,
            &self.format_context(),
        )
    }

    /// Expand this drawing's wire-number format for one wire.
    pub fn format_wireno(&self, number: i32) -> String {
        format_refdes(&self.wireno_format, "", number, &self.format_context())
    }

    /// Expand the wire-number format with a duplicate suffix in `%X`.
    ///
    /// A format without `%X` appends the suffix instead, so a user who edits
    /// the format down to a bare `%N` still gets unique numbers rather than
    /// silent duplicates.
    pub fn format_wireno_suffixed(&self, number: i32, suffix: &str) -> String {
        self.format_wireno_text(&number.to_string(), suffix)
    }

    /// As [`Self::format_wireno_suffixed`], with the `%N` slot taking text —
    /// which is what reference-based numbering needs, since a line reference
    /// need not be numeric.
    pub fn format_wireno_text(&self, number_text: &str, suffix: &str) -> String {
        let out = format_refdes_text(
            &self.wireno_format,
            "",
            number_text,
            &self.format_context_with_suffix(suffix),
        );
        if suffix.is_empty() || self.wireno_format.contains("%X") {
            out
        } else {
            format!("{out}{suffix}")
        }
    }

    /// The duplicate suffixes in order, as configured.
    pub fn suffix_list(&self) -> Vec<String> {
        self.tag_suffixes
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

// ── wire types ───────────────────────────────────────────────────────────

/// One wire type: a named conductor class that maps to a drawing layer.
///
/// In ACADE the layer *is* the wire's identity — connectivity and numbering
/// both key off it — and the layer name is generated from the type's color and
/// size through a pattern (`%C` colour, `%S` size). Same idea here, so a
/// drawing can be reorganised by editing patterns rather than layers.
#[derive(Debug, Clone, PartialEq)]
pub struct WireType {
    pub name: String,
    pub color: String,
    pub size: String,
    pub layer_pattern: String,
    /// Whether wires of this type take part in wire numbering. ACADE lets a
    /// type opt out, which is how jumpers and ground bonds stay unnumbered.
    pub numbered: bool,
}

impl WireType {
    pub fn new(name: &str, color: &str, size: &str) -> Self {
        WireType {
            name: name.to_string(),
            color: color.to_string(),
            size: size.to_string(),
            layer_pattern: WIRE_LAYER_PATTERN.to_string(),
            numbered: true,
        }
    }

    /// The layer this type's wires are drawn on.
    pub fn layer(&self) -> String {
        expand_layer_pattern(&self.layer_pattern, &self.color, &self.size)
    }
}

/// Expand a wire-layer pattern: `%C` → colour, `%S` → size. Unknown tokens are
/// left verbatim so a typo shows up in the layer name instead of vanishing.
pub fn expand_layer_pattern(pattern: &str, color: &str, size: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('C') => out.push_str(color),
            Some('S') => out.push_str(size),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// The drawing's wire types, in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub struct WireTypeTable {
    pub types: Vec<WireType>,
}

impl Default for WireTypeTable {
    fn default() -> Self {
        // A starting set covering what a control drawing actually uses: the
        // ordinary black control wire plus the three that carry meaning.
        WireTypeTable {
            types: vec![
                WireType::new("BLACK", "BK", "14"),
                WireType::new("RED", "RD", "14"),
                WireType::new("BLUE", "BL", "14"),
                WireType::new("GREEN", "GN", "14"),
                // Ground bonds are not numbered.
                {
                    let mut t = WireType::new("GREEN-YELLOW", "GN-YE", "14");
                    t.numbered = false;
                    t
                },
            ],
        }
    }
}

impl WireTypeTable {
    pub fn get(&self, name: &str) -> Option<&WireType> {
        self.types
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut WireType> {
        self.types
            .iter_mut()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// The type that draws on `layer`, if any — this is how a picked wire is
    /// resolved back to its type.
    pub fn by_layer(&self, layer: &str) -> Option<&WireType> {
        self.types.iter().find(|t| t.layer() == layer)
    }

    /// Insert or replace by name, returning whether it replaced.
    pub fn upsert(&mut self, wt: WireType) -> bool {
        match self.get_mut(&wt.name) {
            Some(slot) => {
                *slot = wt;
                true
            }
            None => {
                self.types.push(wt);
                false
            }
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.types.len();
        self.types.retain(|t| !t.name.eq_ignore_ascii_case(name));
        self.types.len() != before
    }

    /// The first type that participates in numbering, used as the default for a
    /// new wire when the drawing has no explicit choice.
    pub fn default_type(&self) -> Option<&WireType> {
        self.types.iter().find(|t| t.numbered)
    }
}

// ── ladder ───────────────────────────────────────────────────────────────

/// A ladder: the vertical bus (or phase buses) plus the numbered horizontal
/// rungs that tag and wire numbering both reference.
///
/// The topmost line reference is ACADE's "master line reference" — the block
/// that carries the ladder's intelligence — while the rest are plain text.
/// Here the whole ladder is one record, so there is no master/plain split to
/// keep in sync.
#[derive(Debug, Clone, PartialEq)]
pub struct LadderRec {
    pub el_id: String,
    /// Left edge of the ladder, mm.
    pub origin_x: f64,
    /// Y of the first (top) rung, mm.
    pub origin_y: f64,
    /// Horizontal span between the verticals, mm.
    pub width_mm: f64,
    /// Vertical pitch between rungs, mm.
    pub rung_spacing_mm: f64,
    pub rung_count: i32,
    /// Line reference of the first rung, e.g. `100`.
    pub first_ref: String,
    /// Added to the reference for each subsequent rung.
    pub rung_increment: i32,
    /// 1 for a single-phase ladder, 3 for a three-phase one.
    pub phase: u8,
    /// Draw the horizontal rung lines (ACADE's "No Rungs" mode leaves them out
    /// and draws only the references).
    pub draw_rungs: bool,
    /// Draw the vertical bus.
    pub draw_bus: bool,
    /// Print the line references beside the rungs.
    pub show_references: bool,
}

impl LadderRec {
    pub fn new(el_id: &str, origin: [f64; 2], width_mm: f64, spacing_mm: f64, count: i32) -> Self {
        LadderRec {
            el_id: el_id.to_string(),
            origin_x: origin[0],
            origin_y: origin[1],
            width_mm,
            rung_spacing_mm: spacing_mm,
            rung_count: count.max(1),
            first_ref: "100".to_string(),
            rung_increment: 1,
            phase: 1,
            draw_rungs: true,
            draw_bus: true,
            show_references: true,
        }
    }

    /// Y of rung `index` (0-based, top rung first).
    pub fn rung_y(&self, index: usize) -> f64 {
        self.origin_y - index as f64 * self.rung_spacing_mm
    }

    /// Y of the bottom rung.
    pub fn last_rung_y(&self) -> f64 {
        self.rung_y((self.rung_count.max(1) - 1) as usize)
    }

    /// The right edge of the ladder.
    pub fn right_x(&self) -> f64 {
        self.origin_x + self.width_mm
    }

    /// The line reference printed on rung `index`.
    ///
    /// The reference keeps any non-numeric prefix and steps its trailing
    /// integer, preserving the digit width so a zero-padded convention survives:
    /// `100` → `100, 101, …` and `2A00` → `2A00, 2A01, …`, not `2A1`.
    pub fn rung_reference(&self, index: usize) -> String {
        let delta = index as i32 * self.rung_increment;
        match split_trailing_int(&self.first_ref) {
            Some((prefix, base, width)) => {
                format!("{prefix}{:0width$}", base + delta, width = width)
            }
            // A reference with no number cannot be stepped; repeat it rather
            // than invent one, so the drawing is visibly wrong instead of
            // quietly plausible.
            None => self.first_ref.clone(),
        }
    }

    /// Every line reference, top to bottom.
    pub fn references(&self) -> Vec<String> {
        (0..self.rung_count.max(1) as usize)
            .map(|i| self.rung_reference(i))
            .collect()
    }

    /// The 0-based index of the rung nearest `y`, or `None` when `y` is not
    /// within `tol` of any rung. This is what makes reference-based numbering
    /// work: a wire's number is the reference of the rung it sits on.
    pub fn nearest_rung(&self, y: f64, tol: f64) -> Option<usize> {
        (0..self.rung_count.max(1) as usize)
            .map(|i| (i, (self.rung_y(i) - y).abs()))
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
    }

    /// The line reference covering `y`, if any.
    pub fn reference_at(&self, y: f64, tol: f64) -> Option<String> {
        self.nearest_rung(y, tol)
            .map(|i| self.rung_reference(i))
    }

    /// Bounding box `[min_x, min_y, max_x, max_y]`.
    pub fn extent(&self) -> [f64; 4] {
        let y0 = self.last_rung_y();
        [
            self.origin_x,
            y0.min(self.origin_y),
            self.right_x(),
            y0.max(self.origin_y),
        ]
    }
}

/// Split a trailing integer off a reference string, returning its leading text,
/// its value, and the digit width: `100` → `("", 100, 3)`, `2A00` →
/// `("2A", 0, 2)`, `A` → `None`.
///
/// The *trailing* run is taken so a sheet-prefixed ladder (`2A00`) steps its
/// numeric tail and keeps its prefix, and the width is returned so a
/// zero-padded convention survives stepping.
fn split_trailing_int(s: &str) -> Option<(String, i32, usize)> {
    let start = s
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map(|(i, _)| i)?;
    let digits = &s[start..];
    let n = digits.parse::<i32>().ok()?;
    Some((s[..start].to_string(), n, digits.len()))
}

// ── colours ──────────────────────────────────────────────────────────────

/// The standard conductor colour codes, in the order a dropdown lists them.
pub const COLOR_CODES: [&str; 10] = ["BK", "WH", "RD", "GN", "BL", "YE", "OR", "BR", "VT", "PK"];

/// Map a conductor colour code to an AutoCAD Color Index.
///
/// Index 7 is AutoCAD's "white or black, whichever contrasts" slot, so both
/// `WH` and `BK` land there — a black wire on a white sheet reads correctly
/// without the user having to pick a different code per background.
/// A code with no single colour (`GN-YE`, a striped conductor) returns `None`,
/// which callers render as ByLayer.
pub fn color_to_aci(color: &str) -> Option<u8> {
    let up = color.trim().to_ascii_uppercase();
    match up.as_str() {
        "RD" | "RED" => Some(1),
        "YE" | "YELLOW" => Some(2),
        "GN" | "GREEN" => Some(3),
        "CY" | "CYAN" => Some(4),
        "BL" | "BLUE" => Some(5),
        "MG" | "MAGENTA" | "VT" | "VIOLET" => Some(6),
        "WH" | "WHITE" | "BK" | "BLACK" => Some(7),
        "GY" | "GRAY" | "GREY" | "DKGRAY" => Some(8),
        "LTGRAY" | "LTGREY" => Some(9),
        "OR" | "ORANGE" | "BR" | "BROWN" => Some(30),
        "PK" | "PINK" => Some(210),
        _ => None,
    }
}

/// The ref-des family codes a device symbol may carry, per IEC 60617. Used to
/// seed the family of a newly placed component.
pub const IEC_FAMILIES: [(&str, &str); 13] = [
    ("K", "relay / contactor"),
    ("Q", "circuit breaker / switch"),
    ("F", "protective device (fuse, overload)"),
    ("M", "motor"),
    ("G", "generator / supply"),
    ("T", "transformer"),
    ("X", "terminal / connector"),
    ("S", "switch / pushbutton"),
    ("H", "indicator / lamp"),
    ("R", "resistor"),
    ("C", "capacitor"),
    ("B", "sensor / transducer"),
    ("V", "semiconductor / diode"),
];

// ── wire record ──────────────────────────────────────────────────────────

/// One drawn wire (a conductor between two connection points).
#[derive(Debug, Clone, PartialEq)]
pub struct ElWireRec {
    pub el_id: String,
    /// The displayed wire number; empty until `EL_WIRENO` assigns one.
    pub wire_ref: String,
    /// Endpoint A: a component/terminal tag or another record's `el_id`.
    pub from_id: String,
    /// Pin name on endpoint A (empty when the node has no pins).
    pub from_pin: String,
    pub to_id: String,
    pub to_pin: String,
    /// Wire type / layer name, empty = the drawing default.
    pub wire_type: String,
    pub gauge: String,
    pub color: String,
    pub number_type: NumberType,
    pub note: String,
    /// Draw this wire with a visual gap where another wire properly crosses it.
    ///
    /// Purely a rendering decision: connectivity is record-based (`from_id`/
    /// `to_id` name pins), so a gap can never change a net. A gapped wire is
    /// drawn as several polylines sharing one `el_id` — the same fold ladders
    /// use — with each run carrying the record.
    pub gap: bool,
}

impl ElWireRec {
    pub fn new(el_id: &str, from_id: &str, to_id: &str) -> Self {
        ElWireRec {
            el_id: el_id.to_string(),
            wire_ref: String::new(),
            from_id: from_id.to_string(),
            from_pin: String::new(),
            to_id: to_id.to_string(),
            to_pin: String::new(),
            wire_type: String::new(),
            gauge: String::new(),
            color: String::new(),
            number_type: NumberType::Normal,
            note: String::new(),
            gap: false,
        }
    }

    /// Both ends named — a wire with a dangling end is an audit finding, not a
    /// crash, so this is a predicate rather than an invariant.
    pub fn is_connected(&self) -> bool {
        !self.from_id.is_empty() && !self.to_id.is_empty()
    }
}

// ── component record ─────────────────────────────────────────────────────

/// One placed schematic component: a parent device or a child contact.
#[derive(Debug, Clone, PartialEq)]
pub struct ElComponentRec {
    pub el_id: String,
    /// The reference designator, e.g. `K1`. Empty until tagged.
    pub tag: String,
    /// Where the symbol's anchor sits, in drawing units.
    ///
    /// Stored because a symbol is drawn as many primitives and its anchor is
    /// therefore not recoverable from any one of them — the same thing a block
    /// insert carries. Every pin position is derived from this, so it is what
    /// makes a wire able to name the pin it lands on.
    pub origin_x: f64,
    pub origin_y: f64,
    /// Rotation about the anchor, in degrees.
    pub rotation_deg: f64,
    /// Pinned by the user: keeps this tag through a drawing-wide retag.
    pub tag_fixed: bool,
    /// Family code (`K`, `Q`, `F`, `X`, …) — the `%F` a tag format expands.
    pub family: String,
    /// Symbol definition name in the library (`relay-coil`, `contact-no`, …).
    pub symbol: String,
    /// A child is a contact or auxiliary element that shares its parent's tag.
    pub is_child: bool,
    /// Parent's tag, for children. Empty for a parent or standalone device.
    pub parent_tag: String,
    pub description: String,
    pub mfg: String,
    pub cat: String,
    pub assy: String,
    /// Rating strings (`RATING1..n`), free-form.
    pub ratings: Vec<String>,
    pub installation: String,
    pub location: String,
    /// Connection pin names, in drawing order.
    pub pins: Vec<String>,
}

impl ElComponentRec {
    /// The anchor as a point.
    pub fn origin(&self) -> [f64; 2] {
        [self.origin_x, self.origin_y]
    }

    /// Rotation in radians.
    pub fn rotation_rad(&self) -> f64 {
        self.rotation_deg.to_radians()
    }

    pub fn new(el_id: &str, symbol: &str, family: &str) -> Self {
        ElComponentRec {
            el_id: el_id.to_string(),
            tag: String::new(),
            origin_x: 0.0,
            origin_y: 0.0,
            rotation_deg: 0.0,
            tag_fixed: false,
            family: family.to_string(),
            symbol: symbol.to_string(),
            is_child: false,
            parent_tag: String::new(),
            description: String::new(),
            mfg: String::new(),
            cat: String::new(),
            assy: String::new(),
            ratings: Vec::new(),
            installation: String::new(),
            location: String::new(),
            pins: Vec::new(),
        }
    }

    /// Has the catalog trio ACADE requires before a BOM can list the part.
    pub fn has_catalog(&self) -> bool {
        !self.cat.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_format_expands_family_and_number() {
        let s = ElSettingsRec::default();
        assert_eq!(s.format_tag("K", 12), "K12");
    }

    #[test]
    fn tag_format_expands_drawing_tokens() {
        let mut s = ElSettingsRec::default();
        s.tag_format = "%S-%F%N".to_string();
        s.sheet = "02".to_string();
        assert_eq!(s.format_tag("Q", 3), "02-Q3");
    }

    #[test]
    fn unknown_format_token_is_left_visible() {
        let ctx = FormatContext::default();
        assert_eq!(format_refdes("%Z%N", "K", 1, &ctx), "%Z1");
    }

    #[test]
    fn unterminated_percent_is_literal() {
        let ctx = FormatContext::default();
        assert_eq!(format_refdes("K%", "K", 1, &ctx), "K%");
    }

    #[test]
    fn format_uses_number_detects_the_mandatory_token() {
        assert!(format_uses_number("%F%N"));
        assert!(format_uses_number("W%N"));
        assert!(!format_uses_number("%F"));
        assert!(!format_uses_number(""));
    }

    #[test]
    fn family_is_the_leading_alpha_run() {
        assert_eq!(family_of("K12"), "K");
        assert_eq!(family_of("X1A"), "X");
        assert_eq!(family_of("12"), "");
    }

    #[test]
    fn suffix_index_walks_the_alphabet_then_doubles() {
        assert_eq!(suffix_for_index(0), "");
        assert_eq!(suffix_for_index(1), "A");
        assert_eq!(suffix_for_index(26), "Z");
        assert_eq!(suffix_for_index(27), "AA");
    }

    #[test]
    fn wire_number_format_uses_the_same_machinery() {
        let mut s = ElSettingsRec::default();
        s.wireno_format = "W%N".to_string();
        assert_eq!(s.format_wireno(42), "W42");
    }

    #[test]
    fn modes_round_trip_through_their_codes() {
        assert_eq!(TagMode::parse("r"), Some(TagMode::Reference));
        assert_eq!(TagMode::parse("sequential"), Some(TagMode::Sequential));
        assert_eq!(TagMode::parse("?"), None);
        assert_eq!(WireNoMode::parse("R"), Some(WireNoMode::Reference));
        assert_eq!(NumberType::parse("fixed"), Some(NumberType::Fixed));
    }

    #[test]
    fn default_settings_match_acade_conventions() {
        let s = ElSettingsRec::default();
        assert_eq!(s.tag_format, "%F%N");
        assert_eq!(s.wireno_format, "%N");
        assert!(format_uses_number(&s.tag_format));
        assert!(format_uses_number(&s.wireno_format));
    }

    #[test]
    fn suffix_list_splits_and_trims() {
        let s = ElSettingsRec::default();
        assert_eq!(s.suffix_list()[0], "A");
        assert_eq!(s.suffix_list().len(), 8);
    }

    #[test]
    fn wire_connection_predicate() {
        let mut w = ElWireRec::new("id", "K1", "");
        assert!(!w.is_connected());
        w.to_id = "K2".to_string();
        assert!(w.is_connected());
    }

    #[test]
    fn component_catalog_predicate() {
        let mut c = ElComponentRec::new("id", "relay-coil", "K");
        assert!(!c.has_catalog());
        c.cat = "NXB-63".to_string();
        assert!(c.has_catalog());
    }

    // ── terminals, cables, connectors ────────────────────────────────────

    #[test]
    fn a_terminal_designator_joins_strip_and_number() {
        let mut t = TerminalRec::new("id", "X1", TerminalKind::Screw);
        assert_eq!(t.designator(), "X1", "no number yet, so the strip alone");
        t.number = "1".to_string();
        assert_eq!(t.designator(), "X1:1");
        t.tag_strip = String::new();
        assert_eq!(t.designator(), "", "a terminal with no strip has no designator");
    }

    #[test]
    fn terminal_kinds_round_trip() {
        for k in TerminalKind::ALL {
            assert_eq!(TerminalKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(TerminalKind::parse("pe"), Some(TerminalKind::Ground));
        assert!(TerminalKind::parse("nonsense").is_none());
    }

    #[test]
    fn a_cable_marker_is_a_parent_until_it_names_one() {
        let mut c = CableRec::new("id", "W1");
        assert!(c.is_parent());
        c.parent_tag = "W1".to_string();
        assert!(!c.is_parent());
    }

    #[test]
    fn connector_pins_are_generated_in_order() {
        let c = ConnectorRec::new("id", "X1", 4);
        assert_eq!(c.pins, ["1", "2", "3", "4"]);
        assert_eq!(c.pin_count(), 4);
        assert!(c.has_pin("3"));
        assert!(!c.has_pin("9"));
    }

    #[test]
    fn connector_pin_offsets_are_centred_and_ordered() {
        let c = ConnectorRec::new("id", "X1", 3);
        let xs: Vec<f64> = (0..3).map(|i| c.pin_offset(i)[0]).collect();
        assert!(xs[0] < xs[1] && xs[1] < xs[2], "pins run left to right: {xs:?}");
        // Centred on the origin: the middle pin sits at x = 0.
        assert!(xs[1].abs() < 1e-9, "got {xs:?}");
        assert!(c.pin_offset(0)[1].abs() < 1e-9, "one row, so no vertical offset");
    }

    #[test]
    fn a_wrapped_connector_keeps_its_pins_readable() {
        // 24 pins in one row would be a metre wide, so wrapping is the point.
        let mut c = ConnectorRec::new("id", "X1", 24);
        c.pins_per_row = 12;
        assert_eq!(c.row_len(), 12);
        assert!(c.pin_offset(0)[1].abs() < 1e-9, "first row on the axis");
        assert!(c.pin_offset(12)[1] < 0.0, "the second row steps down");
        assert!((c.pin_offset(12)[0] - c.pin_offset(0)[0]).abs() < 1e-9,
            "a wrapped row restarts at the same x");
    }

    #[test]
    fn connector_pin_position_places_a_pin_absolutely() {
        let c = ConnectorRec::new("id", "X1", 3);
        // The middle pin of three sits on the anchor.
        assert_eq!(c.pin_position("2", [100.0, 200.0], 0.0), Some([100.0, 200.0]));
        assert_eq!(c.pin_position("nope", [0.0, 0.0], 0.0), None);
    }

    #[test]
    fn connector_rotation_moves_pin_positions() {
        let c = ConnectorRec::new("id", "X1", 3);
        let p = c
            .pin_position("3", [0.0, 0.0], std::f64::consts::FRAC_PI_2)
            .unwrap();
        // The right-hand pin swings to the top.
        assert!(p[0].abs() < 1e-9, "got {p:?}");
        assert!(p[1] > 0.0, "got {p:?}");
    }

    #[test]
    fn connector_extent_grows_with_pins_and_rows() {
        let one_row = ConnectorRec::new("id", "X1", 4);
        let mut two_rows = ConnectorRec::new("id", "X1", 8);
        two_rows.pins_per_row = 4;
        let a = one_row.extent();
        let b = two_rows.extent();
        assert!((a[2] - a[0]) > 0.0, "a connector has width");
        assert!(b[1] < a[1], "two rows extend below one row: {a:?} vs {b:?}");
    }

    #[test]
    fn connector_orientation_round_trips() {
        for o in [
            ConnectorOrientation::Combo,
            ConnectorOrientation::Receptacle,
            ConnectorOrientation::Plug,
        ] {
            assert_eq!(ConnectorOrientation::parse(o.as_str()), Some(o));
        }
        assert!(ConnectorOrientation::parse("?").is_none());
    }

    // ── format suffixes ──────────────────────────────────────────────────

    #[test]
    fn suffix_token_positions_the_duplicate() {
        let mut s = ElSettingsRec::default();
        s.wireno_format = "%N%X".to_string();
        assert_eq!(s.format_wireno_suffixed(100, ""), "100");
        assert_eq!(s.format_wireno_suffixed(100, "A"), "100A");
    }

    #[test]
    fn suffix_is_appended_when_the_format_lacks_the_token() {
        // A user who simplifies the format to `%N` must still get unique
        // numbers, not silent duplicates.
        let mut s = ElSettingsRec::default();
        s.wireno_format = "%N".to_string();
        assert_eq!(s.format_wireno_suffixed(100, "B"), "100B");
    }

    #[test]
    fn suffix_token_can_sit_mid_format() {
        let mut s = ElSettingsRec::default();
        s.wireno_format = "%N%X-1".to_string();
        assert_eq!(s.format_wireno_suffixed(7, "C"), "7C-1");
    }

    // ── wire types ───────────────────────────────────────────────────────

    #[test]
    fn layer_pattern_expands_colour_and_size() {
        assert_eq!(expand_layer_pattern("EL_WIRE_%C_%S", "RD", "14"), "EL_WIRE_RD_14");
    }

    #[test]
    fn layer_pattern_leaves_an_unknown_token_visible() {
        assert_eq!(expand_layer_pattern("EL_%Q", "RD", "14"), "EL_%Q");
        assert_eq!(expand_layer_pattern("EL_%", "RD", "14"), "EL_%");
    }

    #[test]
    fn wire_type_layer_is_derived_not_stored() {
        let mut t = WireType::new("RED", "RD", "14");
        assert_eq!(t.layer(), "EL_WIRE_RD_14");
        // Changing the size moves the layer, which is the point of the pattern.
        t.size = "16".to_string();
        assert_eq!(t.layer(), "EL_WIRE_RD_16");
    }

    #[test]
    fn wire_type_lookup_is_case_insensitive_and_layer_resolvable() {
        let table = WireTypeTable::default();
        assert!(table.get("black").is_some());
        assert_eq!(table.by_layer("EL_WIRE_RD_14").unwrap().name, "RED");
        assert!(table.by_layer("EL_WIRE_NOPE_99").is_none());
    }

    #[test]
    fn wire_type_upsert_reports_whether_it_replaced() {
        let mut table = WireTypeTable::default();
        let n = table.types.len();
        assert!(!table.upsert(WireType::new("ORANGE", "OR", "12")), "new name appends");
        assert_eq!(table.types.len(), n + 1);
        assert!(table.upsert(WireType::new("ORANGE", "OR", "10")), "existing name replaces");
        assert_eq!(table.types.len(), n + 1);
        assert_eq!(table.get("ORANGE").unwrap().size, "10");
    }

    #[test]
    fn wire_type_remove_and_default() {
        let mut table = WireTypeTable::default();
        assert!(table.remove("GREEN"));
        assert!(!table.remove("GREEN"));
        assert_eq!(table.default_type().unwrap().name, "BLACK");
    }

    #[test]
    fn default_table_has_an_unnumbered_ground_type() {
        let table = WireTypeTable::default();
        assert!(table.get("GREEN-YELLOW").unwrap().numbered == false);
    }

    // ── ladder ───────────────────────────────────────────────────────────

    #[test]
    fn rungs_step_downward_from_the_top() {
        let l = LadderRec::new("id", [10.0, 200.0], 170.0, 10.0, 5);
        assert_eq!(l.rung_y(0), 200.0);
        assert_eq!(l.rung_y(1), 190.0);
        assert_eq!(l.last_rung_y(), 160.0);
        assert_eq!(l.right_x(), 180.0);
    }

    #[test]
    fn references_step_by_the_increment() {
        let mut l = LadderRec::new("id", [0.0, 0.0], 100.0, 10.0, 4);
        l.first_ref = "100".to_string();
        assert_eq!(l.references(), ["100", "101", "102", "103"]);
        l.rung_increment = 2;
        assert_eq!(l.references(), ["100", "102", "104", "106"]);
        l.first_ref = "200".to_string();
        assert_eq!(l.references(), ["200", "202", "204", "206"]);
    }

    #[test]
    fn references_keep_a_non_numeric_prefix() {
        let mut l = LadderRec::new("id", [0.0, 0.0], 100.0, 10.0, 3);
        l.first_ref = "2A00".to_string();
        assert_eq!(l.references(), ["2A00", "2A01", "2A02"]);
    }

    #[test]
    fn zero_padded_references_keep_their_width() {
        // A padded convention must survive stepping, and padding is a minimum:
        // growing past the width must not truncate.
        let mut l = LadderRec::new("id", [0.0, 0.0], 100.0, 10.0, 3);
        l.first_ref = "099".to_string();
        assert_eq!(l.references(), ["099", "100", "101"]);
        l.first_ref = "010".to_string();
        assert_eq!(l.references(), ["010", "011", "012"]);
    }

    #[test]
    fn an_unparseable_reference_repeats_rather_than_inventing_one() {
        // Better a visibly wrong drawing than a quietly plausible one.
        let mut l = LadderRec::new("id", [0.0, 0.0], 100.0, 10.0, 3);
        l.first_ref = "REF".to_string();
        assert_eq!(l.references(), ["REF", "REF", "REF"]);
    }

    #[test]
    fn nearest_rung_finds_the_rung_a_wire_sits_on() {
        let mut l = LadderRec::new("id", [10.0, 200.0], 170.0, 10.0, 5);
        l.first_ref = "100".to_string();
        assert_eq!(l.reference_at(200.0, 5.0).as_deref(), Some("100"));
        assert_eq!(l.reference_at(190.5, 5.0).as_deref(), Some("101"));
        assert_eq!(l.reference_at(160.4, 5.0).as_deref(), Some("104"));
        // Between rungs, outside tolerance: not on the ladder.
        assert_eq!(l.reference_at(195.0, 2.0), None);
    }

    #[test]
    fn nearest_rung_picks_the_closer_of_two() {
        let l = LadderRec::new("id", [0.0, 100.0], 50.0, 10.0, 3);
        // 95.5 is 0.5 from rung 100 and 4.5 from 90.
        assert_eq!(l.nearest_rung(95.5, 5.0), Some(0));
        assert_eq!(l.nearest_rung(94.5, 5.0), Some(1));
    }

    #[test]
    fn extent_covers_every_rung() {
        let l = LadderRec::new("id", [10.0, 200.0], 170.0, 10.0, 5);
        assert_eq!(l.extent(), [10.0, 160.0, 180.0, 200.0]);
    }

    #[test]
    fn a_single_rung_ladder_has_a_flat_extent() {
        let l = LadderRec::new("id", [0.0, 50.0], 100.0, 10.0, 1);
        assert_eq!(l.extent(), [0.0, 50.0, 100.0, 50.0]);
    }

    #[test]
    fn rung_count_is_at_least_one() {
        // A zero-rung ladder would divide by nothing and draw nothing.
        let l = LadderRec::new("id", [0.0, 0.0], 100.0, 10.0, 0);
        assert_eq!(l.rung_count, 1);
        assert_eq!(l.references().len(), 1);
    }
}
