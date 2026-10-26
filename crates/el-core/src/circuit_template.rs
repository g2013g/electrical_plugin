//! Circuit-template authoring from a spreadsheet (textbook CH7's
//! `ace_circuit_builder.xls`, as CSV).
//!
//! A template is *data*: symbol slots with rung/x offsets and wires between
//! `{KEY}.pin` names. Users author them in `el-circuits.csv` — drawing
//! folder first, then beside the library — without touching the plugin.
//!
//! CSV grammar (case-insensitive keywords, `#` comments, blank lines skipped):
//!
//! ```text
//! NAME,my-circuit,what it does
//! SLOT,COIL,relay-coil,88,0,
//! SLOT,STOP,pushbutton-nc,25,0,
//! SLOT,SEAL,contact-no,62,0,COIL
//! LINK,{STOP}.2,{START}.1
//! ```
//!
//! `NAME` starts a circuit (a file may hold several). `SLOT` fields are
//! key, symbol, x-offset, rung offset, parent-key (empty for a parent).
//! `LINK` fields are `{FROM-KEY}.pin`, `{TO-KEY}.pin`. Parents must precede
//! children — the same rule the built-in table follows — and every link and
//! parent reference must name a slot of the same circuit.

/// One symbol placement in an authored template.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateSlot {
    /// The link key wires address this slot by (`{COIL}.A1`).
    pub key: String,
    /// Symbol library name (`relay-coil`, …).
    pub symbol: String,
    /// X offset from the ladder's left bus, mm.
    pub dx: f64,
    /// Rung offset from the anchor rung.
    pub rung: i32,
    /// Parent slot's key, for a child (a contact).
    pub parent: Option<String>,
}

/// One authored template.
#[derive(Debug, Clone, PartialEq)]
pub struct CircuitTemplate {
    /// The `EL_CIRCUIT <name>` word.
    pub name: String,
    pub description: String,
    pub slots: Vec<TemplateSlot>,
    /// `(from, to)` in `{KEY}.pin` form, in author order.
    pub links: Vec<(String, String)>,
}

impl CircuitTemplate {
    /// Every slot key, for validation and listing.
    pub fn slot_keys(&self) -> Vec<&str> {
        self.slots.iter().map(|s| s.key.as_str()).collect()
    }
}

/// Parse a spreadsheet into templates. A malformed row is an error naming
/// the line — an authoring typo must be visible, not skipped.
pub fn parse_templates(text: &str) -> Result<Vec<CircuitTemplate>, String> {
    let mut out: Vec<CircuitTemplate> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split(',').map(|c| c.trim()).collect();
        let keyword = cols
            .first()
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or_default();
        let at = |msg: String| format!("el-circuits.csv line {}: {msg}", n + 1);
        match keyword.as_str() {
            "NAME" => {
                if cols.len() < 3 {
                    return Err(at("NAME rows are NAME,<name>,<description>".into()));
                }
                let (name, description) = (cols[1], cols[2]);
                if name.is_empty() {
                    return Err(at("a circuit needs a name".into()));
                }
                out.push(CircuitTemplate {
                    name: name.to_string(),
                    description: description.to_string(),
                    slots: Vec::new(),
                    links: Vec::new(),
                });
            }
            "SLOT" => {
                let Some(t) = out.last_mut() else {
                    return Err(at("a SLOT row needs a NAME row above it".into()));
                };
                if cols.len() < 5 {
                    return Err(at(
                        "SLOT rows are SLOT,<key>,<symbol>,<dx>,<rung>[,<parent>]".into(),
                    ));
                }
                let (key, symbol) = (cols[1], cols[2]);
                if key.is_empty() || symbol.is_empty() {
                    return Err(at("a slot needs a key and a symbol".into()));
                }
                if t.slots.iter().any(|s| s.key.eq_ignore_ascii_case(key)) {
                    return Err(at(format!("duplicate slot key {key:?}")));
                }
                let dx: f64 = cols[3]
                    .parse()
                    .map_err(|_| at(format!("dx {:?} is not a number", cols[3])))?;
                let rung: i32 = cols[4]
                    .parse()
                    .map_err(|_| at(format!("rung {:?} is not an integer", cols[4])))?;
                let parent = cols.get(5).filter(|p| !p.is_empty()).map(|p| p.to_string());
                // A parent key must name a slot already in this circuit —
                // parents precede children, the same rule the built-ins follow.
                if let Some(p) = &parent {
                    if !t.slots.iter().any(|s| s.key.eq_ignore_ascii_case(p)) {
                        return Err(at(format!(
                            "parent {p:?} is not a slot placed earlier in this circuit"
                        )));
                    }
                }
                t.slots.push(TemplateSlot {
                    key: key.to_string(),
                    symbol: symbol.to_string(),
                    dx,
                    rung,
                    parent,
                });
            }
            "LINK" => {
                let Some(t) = out.last_mut() else {
                    return Err(at("a LINK row needs a NAME row above it".into()));
                };
                if cols.len() < 3 {
                    return Err(at("LINK rows are LINK,<from>,<to>".into()));
                }
                for endpoint in [cols[1], cols[2]] {
                    let key = endpoint
                        .trim_start_matches('{')
                        .split('}')
                        .next()
                        .unwrap_or(endpoint);
                    if !t.slots.iter().any(|s| s.key.eq_ignore_ascii_case(key)) {
                        return Err(at(format!(
                            "link endpoint {endpoint:?} names no slot of this circuit"
                        )));
                    }
                }
                t.links.push((cols[1].to_string(), cols[2].to_string()));
            }
            other => {
                return Err(at(format!(
                    "{other:?} is not a row keyword — try NAME, SLOT or LINK"
                )));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# authored circuit
NAME,my-station,the hand-made one
SLOT,K,relay-coil,88,0,
SLOT,STOP,pushbutton-nc,25,0,
SLOT,START,pushbutton-no,45,0,
SLOT,SEAL,contact-no,62,0,K
LINK,{STOP}.2,{START}.1
LINK,{SEAL}.1,{START}.1
"#;

    #[test]
    fn a_file_parses_into_a_template() {
        let t = parse_templates(SAMPLE).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, "my-station");
        assert_eq!(t[0].description, "the hand-made one");
        assert_eq!(t[0].slots.len(), 4);
        assert_eq!(t[0].links.len(), 2);
        // The child's parent resolved to its key.
        assert_eq!(t[0].slots[3].parent.as_deref(), Some("K"));
    }

    #[test]
    fn several_circuits_live_in_one_file() {
        let text = "NAME,a,A\nSLOT,K,relay-coil,10,0,\nNAME,b,B\nSLOT,K,relay-coil,20,0,\n";
        let t = parse_templates(text).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].name.as_str(), t[1].name.as_str()), ("a", "b"));
    }

    #[test]
    fn every_authoring_typo_is_a_named_error() {
        for (bad, why) in [
            ("SLOT,K,relay-coil,10,0,\n", "NAME"),          // slot before name
            ("NAME,x,X\nSLOT,K,relay-coil,10,0,\nSLOT,K,fuse,20,0,\n", "duplicate"),
            ("NAME,x,X\nLINK,{K}.1,{K}.2\n", "no slot"),     // link before slots
            ("NAME,x,X\nSLOT,K,relay-coil,10,0,\nLINK,{Z}.1,{K}.2\n", "names no slot"),
            ("NAME,x,X\nSLOT,SEAL,fuse,10,0,NOPE\n", "not a slot placed"),
            ("NAME,x,X\nSLOT,K,relay-coil,abc,0,\n", "not a number"),
            ("WIDGET,x\n", "not a row keyword"),
        ] {
            let e = parse_templates(bad).unwrap_err();
            assert!(e.contains(why), "{bad:?}: expected {why} in {e}");
        }
    }

    #[test]
    fn comments_and_blank_lines_are_free() {
        let t = parse_templates("# hi\n\nNAME,x,X\n\n").unwrap();
        assert_eq!(t.len(), 1);
    }
}
