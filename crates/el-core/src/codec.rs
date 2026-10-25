//! Neutral XDATA record codec (schema 1).
//!
//! Electrical records are stored as ordered key/value pairs. The plugin crate
//! converts [`ElValue`] to/from the host's `XDataValue` (a `Str` maps to DXF
//! group code 1000, `Real` to 1040, `Int` to 1070), so this crate stays free of
//! any host dependency.
//!
//! Encoding rules, matching the sibling wiring-harness plugin so both behave
//! predictably under the same host:
//! - `SCHEMA` is always the first key, `ELID` the second, `KIND` the third.
//! - Unknown keys are preserved on decode (forward compatibility).
//! - `Bool` maps to `Int` 0/1 (classic XDATA has no boolean).
//! - A repeated key carries an ordered list — this is how a component's pin
//!   names and ratings are stored, since a plain map could not preserve order.
//! - All lengths are millimetres regardless of drawing units.

use crate::model::*;

/// A single typed value in an electrical record.
#[derive(Debug, Clone, PartialEq)]
pub enum ElValue {
    Str(String),
    Real(f64),
    Int(i32),
}

impl ElValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ElValue::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_real(&self) -> Option<f64> {
        match self {
            ElValue::Real(r) => Some(*r),
            ElValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i32> {
        match self {
            ElValue::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ElValue::Int(0) => Some(false),
            ElValue::Int(_) => Some(true),
            _ => None,
        }
    }
}

/// An ordered key/value record (one entity's XDATA under `OCSEL`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ElRecord {
    pub values: Vec<(String, ElValue)>,
}

/// Decode failure: wrong schema or structurally invalid record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub String);

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OCSEL record decode: {}", self.0)
    }
}
impl std::error::Error for DecodeError {}

impl ElRecord {
    pub fn new(kind: &str, el_id: &str) -> Self {
        ElRecord {
            values: vec![
                ("SCHEMA".to_string(), ElValue::Int(SCHEMA_VERSION)),
                ("ELID".to_string(), ElValue::Str(el_id.to_string())),
                ("KIND".to_string(), ElValue::Str(kind.to_string())),
            ],
        }
    }

    pub fn put(&mut self, key: &str, value: ElValue) {
        self.values.push((key.to_string(), value));
    }

    pub fn put_str(&mut self, key: &str, v: &str) {
        self.put(key, ElValue::Str(v.to_string()));
    }

    pub fn put_real(&mut self, key: &str, v: f64) {
        self.put(key, ElValue::Real(v));
    }

    pub fn put_int(&mut self, key: &str, v: i32) {
        self.put(key, ElValue::Int(v));
    }

    pub fn put_bool(&mut self, key: &str, v: bool) {
        self.put(key, ElValue::Int(i32::from(v)));
    }

    pub fn get(&self, key: &str) -> Option<&ElValue> {
        self.values.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(ElValue::as_str)
    }

    /// Every string value stored under `key`, in record order.
    ///
    /// The record is an ordered pair list, so a repeated key carries a list —
    /// a component's pin names rely on this. [`Self::get_str`] returns only the
    /// first.
    pub fn get_all_str(&self, key: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(k, _)| k == key)
            .filter_map(|(_, v)| v.as_str().map(str::to_string))
            .collect()
    }

    pub fn get_real(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(ElValue::as_real)
    }

    pub fn get_int(&self, key: &str) -> Option<i32> {
        self.get(key).and_then(ElValue::as_int)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(ElValue::as_bool)
    }

    pub fn kind(&self) -> Option<&str> {
        self.get_str("KIND")
    }

    pub fn el_id(&self) -> Option<&str> {
        self.get_str("ELID")
    }

    /// Record schema version, or 0 when absent (a malformed record — the schema
    /// check rejects it).
    pub fn schema(&self) -> i32 {
        self.get_int("SCHEMA").unwrap_or(0)
    }

    /// Records newer than this build must not be silently mangled — an older
    /// plugin reading a newer drawing is a loud failure by design.
    fn check_schema(&self) -> Result<(), DecodeError> {
        match self.get_int("SCHEMA") {
            Some(v) if (SCHEMA_MIN_SUPPORTED..=SCHEMA_VERSION).contains(&v) => Ok(()),
            Some(v) if v > SCHEMA_VERSION => Err(DecodeError(format!(
                "record schema {v} is newer than supported {SCHEMA_VERSION}; update the plugin"
            ))),
            Some(v) => Err(DecodeError(format!(
                "record schema {v} predates supported schema {SCHEMA_MIN_SUPPORTED}"
            ))),
            None => Err(DecodeError("missing SCHEMA key".to_string())),
        }
    }

    /// Structural check used before decoding any kind: schema in range, and a
    /// usable identity.
    pub fn validate(&self) -> Result<(), DecodeError> {
        self.check_schema()?;
        if self.el_id().is_none() {
            return Err(DecodeError("missing ELID key".to_string()));
        }
        if self.kind().is_none() {
            return Err(DecodeError("missing KIND key".to_string()));
        }
        Ok(())
    }

    // ── settings ─────────────────────────────────────────────────────────

    pub fn encode_settings(rec: &ElSettingsRec) -> ElRecord {
        let mut r = ElRecord::new(KIND_SETTINGS, "settings");
        r.put_str("SHEET", &rec.sheet);
        r.put_str("DRAWING", &rec.drawing);
        r.put_str("INST", &rec.installation);
        r.put_str("LOC", &rec.location);
        r.put_real("LADW", rec.ladder_width_mm);
        r.put_real("RUNGDIST", rec.rung_spacing_mm);
        r.put_int("RUNGINC", rec.rung_increment);
        r.put_str("RUNG1", &rec.first_rung_ref);
        r.put_int("RUNGCNT", rec.rung_count);
        r.put_str("TAGMODE", rec.tag_mode.as_str());
        r.put_str("TAGFMT", &rec.tag_format);
        r.put_int("TAGSTART", rec.tag_start);
        r.put_str("TAGSUF", &rec.tag_suffixes);
        r.put_str("WIREMODE", rec.wireno_mode.as_str());
        r.put_str("WIREFMT", &rec.wireno_format);
        r.put_int("WIRESTART", rec.wireno_start);
        r.put_int("WINC", rec.wireno_increment);
        r.put_real("WIREGAP", rec.wire_gap_mm);
        r.put_int("REV", rec.revision);
        r
    }

    pub fn decode_settings(&self) -> Result<ElSettingsRec, DecodeError> {
        self.check_schema()?;
        let d = ElSettingsRec::default();
        Ok(ElSettingsRec {
            sheet: self.get_str("SHEET").unwrap_or("").to_string(),
            drawing: self.get_str("DRAWING").unwrap_or("").to_string(),
            installation: self.get_str("INST").unwrap_or("").to_string(),
            location: self.get_str("LOC").unwrap_or("").to_string(),
            ladder_width_mm: self.get_real("LADW").unwrap_or(d.ladder_width_mm),
            rung_spacing_mm: self.get_real("RUNGDIST").unwrap_or(d.rung_spacing_mm),
            rung_increment: self.get_int("RUNGINC").unwrap_or(d.rung_increment),
            first_rung_ref: self.get_str("RUNG1").unwrap_or(&d.first_rung_ref).to_string(),
            rung_count: self.get_int("RUNGCNT").unwrap_or(d.rung_count),
            tag_mode: self
                .get_str("TAGMODE")
                .and_then(TagMode::parse)
                .unwrap_or(d.tag_mode),
            tag_format: self.get_str("TAGFMT").unwrap_or(&d.tag_format).to_string(),
            tag_start: self.get_int("TAGSTART").unwrap_or(d.tag_start),
            tag_suffixes: self.get_str("TAGSUF").unwrap_or(&d.tag_suffixes).to_string(),
            wireno_mode: self
                .get_str("WIREMODE")
                .and_then(WireNoMode::parse)
                .unwrap_or(d.wireno_mode),
            wireno_format: self.get_str("WIREFMT").unwrap_or(&d.wireno_format).to_string(),
            wireno_start: self.get_int("WIRESTART").unwrap_or(d.wireno_start),
            wireno_increment: self.get_int("WINC").unwrap_or(d.wireno_increment),
            wire_gap_mm: self.get_real("WIREGAP").unwrap_or(d.wire_gap_mm),
            revision: self.get_int("REV").unwrap_or(d.revision),
        })
    }

    // ── wire ─────────────────────────────────────────────────────────────

    pub fn encode_wire(rec: &ElWireRec) -> ElRecord {
        let mut r = ElRecord::new(KIND_WIRE, &rec.el_id);
        r.put_str("WREF", &rec.wire_ref);
        r.put_str("FROM", &rec.from_id);
        r.put_str("FROMPIN", &rec.from_pin);
        r.put_str("TO", &rec.to_id);
        r.put_str("TOPIN", &rec.to_pin);
        r.put_str("WTYPE", &rec.wire_type);
        r.put_str("GAUGE", &rec.gauge);
        r.put_str("COLOR", &rec.color);
        r.put_str("NTYPE", rec.number_type.as_str());
        r.put_str("NOTE", &rec.note);
        r.put_bool("GAP", rec.gap);
        r
    }

    pub fn decode_wire(&self) -> Result<ElWireRec, DecodeError> {
        self.check_schema()?;
        let el_id = self
            .el_id()
            .ok_or_else(|| DecodeError("missing ELID key".to_string()))?
            .to_string();
        Ok(ElWireRec {
            el_id,
            wire_ref: self.get_str("WREF").unwrap_or("").to_string(),
            from_id: self.get_str("FROM").unwrap_or("").to_string(),
            from_pin: self.get_str("FROMPIN").unwrap_or("").to_string(),
            to_id: self.get_str("TO").unwrap_or("").to_string(),
            to_pin: self.get_str("TOPIN").unwrap_or("").to_string(),
            wire_type: self.get_str("WTYPE").unwrap_or("").to_string(),
            gauge: self.get_str("GAUGE").unwrap_or("").to_string(),
            color: self.get_str("COLOR").unwrap_or("").to_string(),
            number_type: self
                .get_str("NTYPE")
                .and_then(NumberType::parse)
                .unwrap_or(NumberType::Normal),
            note: self.get_str("NOTE").unwrap_or("").to_string(),
            gap: self.get_bool("GAP").unwrap_or(false),
        })
    }

    // ── signal ───────────────────────────────────────────────────────────

    pub fn encode_signal(rec: &SignalRec) -> ElRecord {
        let mut r = ElRecord::new(KIND_SIGNAL, &rec.el_id);
        r.put_str("NAME", &rec.name);
        r.put_str("DIR", rec.direction.as_str());
        r.put_real("OX", rec.origin_x);
        r.put_real("OY", rec.origin_y);
        r.put_str("WIRE", &rec.wire_el_id);
        r.put_bool("TOEND", rec.at_to_end);
        r
    }

    pub fn decode_signal(&self) -> Result<SignalRec, DecodeError> {
        self.check_schema()?;
        let el_id = self
            .el_id()
            .ok_or_else(|| DecodeError("missing ELID key".to_string()))?
            .to_string();
        Ok(SignalRec {
            el_id,
            name: self.get_str("NAME").unwrap_or("").to_string(),
            direction: self
                .get_str("DIR")
                .and_then(SignalDirection::parse)
                .unwrap_or_default(),
            origin_x: self.get_real("OX").unwrap_or(0.0),
            origin_y: self.get_real("OY").unwrap_or(0.0),
            wire_el_id: self.get_str("WIRE").unwrap_or("").to_string(),
            at_to_end: self.get_bool("TOEND").unwrap_or(false),
        })
    }

    // ── jumper ───────────────────────────────────────────────────────────

    pub fn encode_jumper(rec: &JumperRec) -> ElRecord {
        let mut r = ElRecord::new(KIND_JUMPER, &rec.el_id);
        r.put_str("STRIP", &rec.strip);
        r.put_str("ANO", &rec.a_number);
        r.put_str("BNO", &rec.b_number);
        r
    }

    pub fn decode_jumper(&self) -> Result<JumperRec, DecodeError> {
        self.check_schema()?;
        let el_id = self
            .el_id()
            .ok_or_else(|| DecodeError("missing ELID key".to_string()))?
            .to_string();
        Ok(JumperRec {
            el_id,
            strip: self.get_str("STRIP").unwrap_or("").to_string(),
            a_number: self.get_str("ANO").unwrap_or("").to_string(),
            b_number: self.get_str("BNO").unwrap_or("").to_string(),
        })
    }

    // ── component ────────────────────────────────────────────────────────

    pub fn encode_component(rec: &ElComponentRec) -> ElRecord {
        let mut r = ElRecord::new(KIND_COMPONENT, &rec.el_id);
        r.put_str("TAG", &rec.tag);
        r.put_real("OX", rec.origin_x);
        r.put_real("OY", rec.origin_y);
        r.put_real("ROT", rec.rotation_deg);
        r.put_bool("TAGFIXED", rec.tag_fixed);
        r.put_str("FAMILY", &rec.family);
        r.put_str("SYMBOL", &rec.symbol);
        r.put_bool("CHILD", rec.is_child);
        r.put_str("PTAG", &rec.parent_tag);
        r.put_str("DESC", &rec.description);
        r.put_str("MFG", &rec.mfg);
        r.put_str("CAT", &rec.cat);
        r.put_str("ASSY", &rec.assy);
        // Repeated keys carry the ordered lists.
        for rating in &rec.ratings {
            r.put_str("RATING", rating);
        }
        r.put_str("INST", &rec.installation);
        r.put_str("LOC", &rec.location);
        for pin in &rec.pins {
            r.put_str("PIN", pin);
        }
        r
    }

    pub fn decode_component(&self) -> Result<ElComponentRec, DecodeError> {
        self.check_schema()?;
        let el_id = self
            .el_id()
            .ok_or_else(|| DecodeError("missing ELID key".to_string()))?
            .to_string();
        Ok(ElComponentRec {
            el_id,
            tag: self.get_str("TAG").unwrap_or("").to_string(),
            origin_x: self.get_real("OX").unwrap_or(0.0),
            origin_y: self.get_real("OY").unwrap_or(0.0),
            rotation_deg: self.get_real("ROT").unwrap_or(0.0),
            tag_fixed: self.get_bool("TAGFIXED").unwrap_or(false),
            family: self.get_str("FAMILY").unwrap_or("").to_string(),
            symbol: self.get_str("SYMBOL").unwrap_or("").to_string(),
            is_child: self.get_bool("CHILD").unwrap_or(false),
            parent_tag: self.get_str("PTAG").unwrap_or("").to_string(),
            description: self.get_str("DESC").unwrap_or("").to_string(),
            mfg: self.get_str("MFG").unwrap_or("").to_string(),
            cat: self.get_str("CAT").unwrap_or("").to_string(),
            assy: self.get_str("ASSY").unwrap_or("").to_string(),
            ratings: self.get_all_str("RATING"),
            installation: self.get_str("INST").unwrap_or("").to_string(),
            location: self.get_str("LOC").unwrap_or("").to_string(),
            pins: self.get_all_str("PIN"),
        })
    }

    // ── ladder ───────────────────────────────────────────────────────────

    pub fn encode_ladder(rec: &LadderRec) -> ElRecord {
        let mut r = ElRecord::new(KIND_LADDER, &rec.el_id);
        r.put_real("X", rec.origin_x);
        r.put_real("Y", rec.origin_y);
        r.put_real("WIDTH", rec.width_mm);
        r.put_real("SPACING", rec.rung_spacing_mm);
        r.put_int("RUNGS", rec.rung_count);
        r.put_str("REF1", &rec.first_ref);
        r.put_int("RINC", rec.rung_increment);
        r.put_int("PHASE", i32::from(rec.phase));
        r.put_bool("DRAWRUNGS", rec.draw_rungs);
        r.put_bool("DRAWBUS", rec.draw_bus);
        r.put_bool("SHOWREF", rec.show_references);
        r
    }

    pub fn decode_ladder(&self) -> Result<LadderRec, DecodeError> {
        self.check_schema()?;
        let el_id = self
            .el_id()
            .ok_or_else(|| DecodeError("missing ELID key".to_string()))?
            .to_string();
        Ok(LadderRec {
            el_id,
            origin_x: self.get_real("X").unwrap_or(0.0),
            origin_y: self.get_real("Y").unwrap_or(0.0),
            width_mm: self.get_real("WIDTH").unwrap_or(170.0),
            rung_spacing_mm: self.get_real("SPACING").unwrap_or(10.0),
            rung_count: self.get_int("RUNGS").unwrap_or(1).max(1),
            first_ref: self.get_str("REF1").unwrap_or("100").to_string(),
            rung_increment: self.get_int("RINC").unwrap_or(1),
            phase: self.get_int("PHASE").unwrap_or(1).clamp(1, 3) as u8,
            draw_rungs: self.get_bool("DRAWRUNGS").unwrap_or(true),
            draw_bus: self.get_bool("DRAWBUS").unwrap_or(true),
            show_references: self.get_bool("SHOWREF").unwrap_or(true),
        })
    }

    // ── terminal ─────────────────────────────────────────────────────────

    pub fn encode_terminal(rec: &TerminalRec) -> ElRecord {
        let mut r = ElRecord::new(KIND_TERMINAL, &rec.el_id);
        r.put_real("OX", rec.origin_x);
        r.put_real("OY", rec.origin_y);
        r.put_real("ROT", rec.rotation_deg);
        r.put_str("STRIP", &rec.tag_strip);
        r.put_str("TERM", &rec.number);
        r.put_int("SEQ", rec.strip_seq);
        r.put_str("TTERM", rec.kind.as_str());
        r.put_str("PARTNO", &rec.part_number);
        r.put_str("DESC", &rec.description);
        r.put_str("LINK", &rec.linked);
        for pin in &rec.pins {
            r.put_str("PIN", pin);
        }
        r
    }

    pub fn decode_terminal(&self) -> Result<TerminalRec, DecodeError> {
        self.check_schema()?;
        let el_id = self
            .el_id()
            .ok_or_else(|| DecodeError("missing ELID key".to_string()))?
            .to_string();
        let mut pins = self.get_all_str("PIN");
        if pins.is_empty() {
            // A terminal with no pins recorded is a two-terminal block by
            // convention, which is what a schematic terminal is.
            pins = vec!["1".to_string(), "2".to_string()];
        }
        Ok(TerminalRec {
            el_id,
            origin_x: self.get_real("OX").unwrap_or(0.0),
            origin_y: self.get_real("OY").unwrap_or(0.0),
            rotation_deg: self.get_real("ROT").unwrap_or(0.0),
            tag_strip: self.get_str("STRIP").unwrap_or("").to_string(),
            number: self.get_str("TERM").unwrap_or("").to_string(),
            strip_seq: self.get_int("SEQ").unwrap_or(0),
            kind: self
                .get_str("TTERM")
                .and_then(TerminalKind::parse)
                .unwrap_or_default(),
            part_number: self.get_str("PARTNO").unwrap_or("").to_string(),
            description: self.get_str("DESC").unwrap_or("").to_string(),
            linked: self.get_str("LINK").unwrap_or("").to_string(),
            pins,
        })
    }

    // ── cable ────────────────────────────────────────────────────────────

    pub fn encode_cable(rec: &CableRec) -> ElRecord {
        let mut r = ElRecord::new(KIND_CABLE, &rec.el_id);
        r.put_str("TAG", &rec.tag);
        r.put_str("PTAG", &rec.parent_tag);
        r.put_int("COND", rec.conductor_index);
        r.put_str("COLOR", &rec.color);
        r.put_str("GAUGE", &rec.gauge);
        r.put_str("MFG", &rec.mfg);
        r.put_str("CAT", &rec.cat);
        r.put_str("PARTNO", &rec.part_number);
        r.put_str("DESC", &rec.description);
        r.put_str("FROM", &rec.from_id);
        r.put_str("TO", &rec.to_id);
        r
    }

    pub fn decode_cable(&self) -> Result<CableRec, DecodeError> {
        self.check_schema()?;
        let el_id = self
            .el_id()
            .ok_or_else(|| DecodeError("missing ELID key".to_string()))?
            .to_string();
        Ok(CableRec {
            el_id,
            tag: self.get_str("TAG").unwrap_or("").to_string(),
            parent_tag: self.get_str("PTAG").unwrap_or("").to_string(),
            conductor_index: self.get_int("COND").unwrap_or(1).max(1),
            color: self.get_str("COLOR").unwrap_or("").to_string(),
            gauge: self.get_str("GAUGE").unwrap_or("").to_string(),
            mfg: self.get_str("MFG").unwrap_or("").to_string(),
            cat: self.get_str("CAT").unwrap_or("").to_string(),
            part_number: self.get_str("PARTNO").unwrap_or("").to_string(),
            description: self.get_str("DESC").unwrap_or("").to_string(),
            from_id: self.get_str("FROM").unwrap_or("").to_string(),
            to_id: self.get_str("TO").unwrap_or("").to_string(),
        })
    }

    // ── connector ────────────────────────────────────────────────────────

    pub fn encode_connector(rec: &ConnectorRec) -> ElRecord {
        let mut r = ElRecord::new(KIND_CONNECTOR, &rec.el_id);
        r.put_real("OX", rec.origin_x);
        r.put_real("OY", rec.origin_y);
        r.put_real("ROT", rec.rotation_deg);
        r.put_str("REFDES", &rec.ref_des);
        r.put_real("PINSPACE", rec.pin_spacing_mm);
        r.put_str("ORIENT", rec.orientation.as_str());
        r.put_int("PERROW", rec.pins_per_row);
        r.put_str("MFG", &rec.mfg);
        r.put_str("CAT", &rec.cat);
        r.put_str("PARTNO", &rec.part_number);
        r.put_str("DESC", &rec.description);
        r.put_str("LOC", &rec.location);
        r.put_bool("WNC", rec.wire_number_change);
        // One `PINLIST` entry per pin, in drawing order — the order *is* the
        // geometry, so a set could not carry it.
        for pin in &rec.pins {
            r.put_str("PINLIST", pin);
        }
        r
    }

    pub fn decode_connector(&self) -> Result<ConnectorRec, DecodeError> {
        self.check_schema()?;
        let el_id = self
            .el_id()
            .ok_or_else(|| DecodeError("missing ELID key".to_string()))?
            .to_string();
        Ok(ConnectorRec {
            el_id,
            origin_x: self.get_real("OX").unwrap_or(0.0),
            origin_y: self.get_real("OY").unwrap_or(0.0),
            rotation_deg: self.get_real("ROT").unwrap_or(0.0),
            ref_des: self.get_str("REFDES").unwrap_or("").to_string(),
            pins: self.get_all_str("PINLIST"),
            pin_spacing_mm: self.get_real("PINSPACE").unwrap_or(5.0),
            orientation: self
                .get_str("ORIENT")
                .and_then(ConnectorOrientation::parse)
                .unwrap_or_default(),
            pins_per_row: self.get_int("PERROW").unwrap_or(0),
            mfg: self.get_str("MFG").unwrap_or("").to_string(),
            cat: self.get_str("CAT").unwrap_or("").to_string(),
            part_number: self.get_str("PARTNO").unwrap_or("").to_string(),
            description: self.get_str("DESC").unwrap_or("").to_string(),
            location: self.get_str("LOC").unwrap_or("").to_string(),
            wire_number_change: self.get_bool("WNC").unwrap_or(false),
        })
    }

    // ── wire types ───────────────────────────────────────────────────────

    /// Wire types are stored as a repeated group: `NAME`/`COLOR`/`SIZE`/
    /// `PATTERN`/`NUMBERED` per type, in declaration order. A record is an
    /// ordered pair list, so the group boundaries are positional — each `NAME`
    /// starts a new type.
    pub fn encode_wiretypes(table: &WireTypeTable) -> ElRecord {
        let mut r = ElRecord::new(KIND_WIRETYPES, "wiretypes");
        for t in &table.types {
            r.put_str("NAME", &t.name);
            r.put_str("COLOR", &t.color);
            r.put_str("SIZE", &t.size);
            r.put_str("PATTERN", &t.layer_pattern);
            r.put_bool("NUMBERED", t.numbered);
        }
        r
    }

    pub fn decode_wiretypes(&self) -> Result<WireTypeTable, DecodeError> {
        self.check_schema()?;
        let mut types = Vec::new();
        let mut cur: Option<WireType> = None;
        for (key, value) in &self.values {
            match key.as_str() {
                "NAME" => {
                    if let Some(t) = cur.take() {
                        types.push(t);
                    }
                    let name = value.as_str().unwrap_or_default().to_string();
                    cur = Some(WireType {
                        name,
                        color: String::new(),
                        size: String::new(),
                        layer_pattern: WIRE_LAYER_PATTERN.to_string(),
                        numbered: true,
                    });
                }
                "COLOR" => {
                    if let Some(t) = cur.as_mut() {
                        t.color = value.as_str().unwrap_or_default().to_string();
                    }
                }
                "SIZE" => {
                    if let Some(t) = cur.as_mut() {
                        t.size = value.as_str().unwrap_or_default().to_string();
                    }
                }
                "PATTERN" => {
                    if let Some(t) = cur.as_mut() {
                        t.layer_pattern = value.as_str().unwrap_or_default().to_string();
                    }
                }
                "NUMBERED" => {
                    if let Some(t) = cur.as_mut() {
                        t.numbered = value.as_bool().unwrap_or(true);
                    }
                }
                _ => {}
            }
        }
        if let Some(t) = cur.take() {
            types.push(t);
        }
        // A type with no name is unusable; drop rather than surface a phantom.
        types.retain(|t| !t.name.trim().is_empty());
        Ok(WireTypeTable { types })
    }

    /// Require a specific kind, so a wire record can never be decoded as a
    /// component by accident.
    pub fn require_kind(&self, expected: &str) -> Result<(), DecodeError> {
        match self.kind() {
            Some(k) if k == expected => Ok(()),
            Some(k) => Err(DecodeError(format!("expected kind {expected}, found {k}"))),
            None => Err(DecodeError("missing KIND key".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_keys_are_first_and_ordered() {
        let r = ElRecord::new(KIND_WIRE, "abc");
        assert_eq!(r.values[0].0, "SCHEMA");
        assert_eq!(r.values[1].0, "ELID");
        assert_eq!(r.values[2].0, "KIND");
        assert_eq!(r.get_int("SCHEMA"), Some(SCHEMA_VERSION));
        assert_eq!(r.el_id(), Some("abc"));
    }

    #[test]
    fn settings_round_trip() {
        let mut s = ElSettingsRec::default();
        s.sheet = "03".to_string();
        s.tag_format = "%S-%F%N".to_string();
        s.tag_mode = TagMode::Reference;
        s.wireno_start = 500;
        s.ladder_width_mm = 210.5;
        let back = ElRecord::encode_settings(&s).decode_settings().unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn wire_round_trip() {
        let mut w = ElWireRec::new("id1", "K1", "K2");
        w.wire_ref = "W7".to_string();
        w.from_pin = "A1".to_string();
        w.number_type = NumberType::Fixed;
        let back = ElRecord::encode_wire(&w).decode_wire().unwrap();
        assert_eq!(back, w);
    }

    #[test]
    fn component_round_trip_preserves_ordered_lists() {
        let mut c = ElComponentRec::new("id2", "relay-coil", "K");
        c.tag = "K1".to_string();
        c.ratings = vec!["24VDC".to_string(), "10A".to_string()];
        c.pins = vec!["A1".to_string(), "A2".to_string()];
        c.is_child = true;
        c.parent_tag = "K1".to_string();
        let back = ElRecord::encode_component(&c).decode_component().unwrap();
        assert_eq!(back, c);
        // order is the drawing order, so it must survive exactly
        assert_eq!(back.pins, vec!["A1", "A2"]);
    }

    #[test]
    fn empty_lists_decode_as_empty_not_missing() {
        let c = ElComponentRec::new("id3", "s", "K");
        let back = ElRecord::encode_component(&c).decode_component().unwrap();
        assert!(back.pins.is_empty());
        assert!(back.ratings.is_empty());
    }

    #[test]
    fn unknown_keys_are_preserved_on_decode() {
        // Forward compatibility: a newer writer's extra key must not be lost
        // when an older reader re-encodes, and must not break decoding.
        let mut r = ElRecord::encode_settings(&ElSettingsRec::default());
        r.put_str("FUTURE_THING", "keep me");
        let decoded = r.decode_settings().unwrap();
        assert_eq!(decoded.revision, 1);
        assert!(r.values.iter().any(|(k, _)| k == "FUTURE_THING"));
    }

    #[test]
    fn newer_schema_is_rejected_loudly() {
        let mut r = ElRecord::encode_wire(&ElWireRec::new("id", "a", "b"));
        r.values[0].1 = ElValue::Int(SCHEMA_VERSION + 1);
        let err = r.decode_wire().unwrap_err();
        assert!(err.0.contains("newer than supported"), "got {err}");
    }

    #[test]
    fn missing_schema_is_rejected() {
        let r = ElRecord::default();
        assert!(r.decode_wire().is_err());
        assert!(r.validate().is_err());
    }

    #[test]
    fn validate_requires_identity() {
        let mut r = ElRecord::new(KIND_WIRE, "x");
        r.values.retain(|(k, _)| k != "ELID");
        assert!(r.validate().unwrap_err().0.contains("ELID"));
    }

    #[test]
    fn require_kind_guards_cross_decoding() {
        let r = ElRecord::encode_wire(&ElWireRec::new("id", "a", "b"));
        assert!(r.require_kind(KIND_WIRE).is_ok());
        let err = r.require_kind(KIND_COMPONENT).unwrap_err();
        assert!(err.0.contains("expected kind component"), "got {err}");
    }

    #[test]
    fn bool_maps_to_int() {
        let mut c = ElComponentRec::new("id", "s", "K");
        c.is_child = true;
        let r = ElRecord::encode_component(&c);
        assert_eq!(r.get("CHILD").unwrap().as_int(), Some(1));
        assert_eq!(r.get("CHILD").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn settings_defaults_backfill_for_a_sparse_record() {
        // A record carrying only identity keys must decode to defaults, not
        // fail: that is how a drawing written by an earlier build migrates.
        let mut r = ElRecord::new(KIND_SETTINGS, "settings");
        r.put_str("SHEET", "01");
        let s = r.decode_settings().unwrap();
        assert_eq!(s.sheet, "01");
        assert_eq!(s.tag_format, ElSettingsRec::default().tag_format);
    }

    #[test]
    fn terminal_round_trip() {
        let mut t = TerminalRec::new("id", "X1", TerminalKind::Fused);
        t.number = "12".to_string();
        t.strip_seq = 3;
        t.part_number = "PT-2.5".to_string();
        t.linked = "X1:11".to_string();
        t.pins = vec!["1".to_string(), "2".to_string(), "PE".to_string()];
        let back = ElRecord::encode_terminal(&t).decode_terminal().unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn a_terminal_with_no_pins_recorded_defaults_to_two() {
        // A schematic terminal block is two connections; a record that lost its
        // pin list should not read as a block with no connections.
        let mut r = ElRecord::new(KIND_TERMINAL, "id");
        r.put_str("STRIP", "X1");
        let t = r.decode_terminal().unwrap();
        assert_eq!(t.pins, ["1", "2"]);
    }

    #[test]
    fn cable_round_trip() {
        let mut c = CableRec::new("id", "W1");
        c.parent_tag = "W1".to_string();
        c.conductor_index = 2;
        c.color = "BL".to_string();
        c.gauge = "1.5".to_string();
        c.from_id = "X1".to_string();
        c.to_id = "M1".to_string();
        let back = ElRecord::encode_cable(&c).decode_cable().unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn a_cable_conductor_index_is_at_least_one() {
        let mut r = ElRecord::encode_cable(&CableRec::new("id", "W1"));
        r.values.retain(|(k, _)| k != "COND");
        r.put_int("COND", 0);
        assert_eq!(r.decode_cable().unwrap().conductor_index, 1);
    }

    #[test]
    fn connector_round_trip_preserves_pin_order() {
        let mut c = ConnectorRec::new("id", "X1", 3);
        c.pins = vec!["GND".to_string(), "A".to_string(), "B".to_string()];
        c.pins_per_row = 2;
        c.orientation = ConnectorOrientation::Plug;
        c.wire_number_change = true;
        let back = ElRecord::encode_connector(&c).decode_connector().unwrap();
        assert_eq!(back, c);
        // Order is the drawn order, so it must survive exactly.
        assert_eq!(back.pins, ["GND", "A", "B"]);
    }

    #[test]
    fn a_connector_with_no_pin_list_decodes_as_a_bare_connector() {
        let mut r = ElRecord::new(KIND_CONNECTOR, "id");
        r.put_str("REFDES", "X1");
        let c = r.decode_connector().unwrap();
        assert!(c.pins.is_empty());
        assert_eq!(c.pin_spacing_mm, 5.0, "the default spacing fills in");
        assert_eq!(c.orientation, ConnectorOrientation::Combo);
    }

    #[test]
    fn ladder_round_trip() {
        let mut l = LadderRec::new("id", [10.0, 200.0], 170.0, 10.0, 12);
        l.first_ref = "200".to_string();
        l.phase = 3;
        l.draw_rungs = false;
        l.show_references = false;
        let back = ElRecord::encode_ladder(&l).decode_ladder().unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn ladder_decode_clamps_a_nonsense_phase() {
        let mut r = ElRecord::encode_ladder(&LadderRec::new("id", [0.0, 0.0], 10.0, 10.0, 1));
        r.values.retain(|(k, _)| k != "PHASE");
        r.put_int("PHASE", 9);
        assert_eq!(r.decode_ladder().unwrap().phase, 3, "phase clamps to 1..=3");
        r.values.retain(|(k, _)| k != "PHASE");
        r.put_int("PHASE", 0);
        assert_eq!(r.decode_ladder().unwrap().phase, 1);
    }

    #[test]
    fn wire_types_round_trip_in_order() {
        let table = WireTypeTable::default();
        let back = ElRecord::encode_wiretypes(&table).decode_wiretypes().unwrap();
        assert_eq!(back, table);
        // Order is the declaration order and must survive exactly.
        assert_eq!(back.types[0].name, "BLACK");
        assert!(!back.types[4].numbered, "the ground type opts out of numbering");
    }

    #[test]
    fn wire_types_tolerate_a_missing_pattern() {
        let mut r = ElRecord::new(KIND_WIRETYPES, "wiretypes");
        r.put_str("NAME", "RED");
        r.put_str("COLOR", "RD");
        r.put_str("SIZE", "16");
        let back = r.decode_wiretypes().unwrap();
        assert_eq!(back.types.len(), 1);
        assert_eq!(back.types[0].layer_pattern, WIRE_LAYER_PATTERN);
    }

    #[test]
    fn wire_types_drop_a_nameless_entry() {
        let mut r = ElRecord::new(KIND_WIRETYPES, "wiretypes");
        r.put_str("COLOR", "RD"); // no NAME ever follows
        assert!(r.decode_wiretypes().unwrap().types.is_empty());
    }

    #[test]
    fn empty_wire_type_table_round_trips() {
        let table = WireTypeTable { types: vec![] };
        let back = ElRecord::encode_wiretypes(&table).decode_wiretypes().unwrap();
        assert!(back.types.is_empty());
    }
}
