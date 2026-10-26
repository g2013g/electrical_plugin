//! `EL_CIRCUIT` — the Circuit Builder (textbook CH7): pick a circuit, get it
//! placed and prewired on the ladder.
//!
//! ACADE drives its builder from a spreadsheet; this increment is the same
//! idea with the templates in a **table**: every circuit is a list of symbol
//! placements (symbol, x offset along the rung, rung offset, slot key) and a
//! list of wires between pin names. Parents come first in the table so their
//! children can bind to the allocated tag — that is what makes a contact
//! placed by a template belong to its coil exactly as a hand-placed one
//! does. Wire endpoints resolve through the **slot's own symbol definition**
//! (never a global name lookup), so two contacts sharing one tag — a main
//! and a seal — are never confused. Wires are left unnumbered on purpose:
//! numbering stays the explicit act the rest of the plugin made it.

use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::{ElComponentRec, ElWireRec, LadderRec, LAYER_WIRES};
use el_core::symbol::SymbolDef;
use el_core::{tagging, ElIndex};

use crate::cmds::common::Args;
use crate::cmds::component;
use crate::geometry::{component_entities, wire_polyline};

/// One symbol placement in a template.
struct Slot {
    /// Symbol library name (`pushbutton-no`, `relay-coil`, …).
    symbol: &'static str,
    /// X offset from the ladder's left bus, mm.
    dx: f64,
    /// Rung offset from the anchor rung (0 = the anchor rung itself).
    rung: i32,
    /// The link key wires address this slot by (`{STOP}.2`).
    key: &'static str,
    /// For a child: the key of its parent slot. Children carry the parent's
    /// allocated tag, which is the whole parent/child model.
    parent: Option<&'static str>,
}

/// One wire in a template, between `{KEY}.pin` names.
struct Link {
    from: &'static str,
    to: &'static str,
}

struct Template {
    name: &'static str,
    description: &'static str,
    slots: &'static [Slot],
    links: &'static [Link],
}

const fn slot(symbol: &'static str, dx: f64, rung: i32, key: &'static str) -> Slot {
    Slot { symbol, dx, rung, key, parent: None }
}

const fn child(
    symbol: &'static str,
    dx: f64,
    rung: i32,
    key: &'static str,
    parent: &'static str,
) -> Slot {
    Slot { symbol, dx, rung, key, parent: Some(parent) }
}

const fn link(from: &'static str, to: &'static str) -> Link {
    Link { from, to }
}

/// Start/stop control station: stop, start, seal-in contact, coil.
const START_STOP: Template = Template {
    name: "start-stop",
    description: "start/stop station with seal-in contact",
    slots: &[
        slot("relay-coil", 88.0, 0, "K"),
        slot("pushbutton-nc", 25.0, 0, "STOP"),
        slot("pushbutton-no", 45.0, 0, "START"),
        child("contact-no", 62.0, 0, "SEAL", "K"),
    ],
    links: &[
        link("{STOP}.2", "{START}.1"),
        link("{START}.2", "{K}.A1"),
        link("{SEAL}.1", "{START}.1"),
        link("{SEAL}.2", "{START}.2"),
    ],
};

/// Direct-on-line motor starter: a power rung (breaker → main contact →
/// overload → motor) and the control rung below it.
const DOL: Template = Template {
    name: "dol",
    description: "DOL motor starter: power rung + start/stop control",
    slots: &[
        // The coil first: everything K-related binds to it.
        slot("relay-coil", 88.0, 1, "K"),
        slot("breaker-1p", 18.0, 0, "Q"),
        child("contact-no", 42.0, 0, "MAIN", "K"),
        slot("overload", 64.0, 0, "F"),
        slot("motor", 92.0, 0, "M"),
        slot("pushbutton-nc", 25.0, 1, "STOP"),
        slot("pushbutton-no", 45.0, 1, "START"),
        child("contact-no", 62.0, 1, "SEAL", "K"),
    ],
    links: &[
        // Power: breaker → main contact → overload → motor.
        link("{Q}.2", "{MAIN}.1"),
        link("{MAIN}.2", "{F}.1"),
        link("{F}.2", "{M}.A1"),
        // Control: stop → start → coil, sealed.
        link("{STOP}.2", "{START}.1"),
        link("{START}.2", "{K}.A1"),
        link("{SEAL}.1", "{START}.1"),
        link("{SEAL}.2", "{START}.2"),
    ],
};

/// Forward/reverse control: two contactors, each sealed by its own contact
/// and cross-interlocked by an NC contact of the other. Power rungs are
/// deliberately not part of the template — reversing swaps phases, which
/// deserves a hand-drawn power diagram rather than a canned one.
const FWD_REV: Template = Template {
    name: "fwd-rev",
    description: "forward/reverse control, cross-interlocked (no power rung)",
    slots: &[
        slot("relay-coil", 92.0, 0, "K1"),
        slot("relay-coil", 92.0, 1, "K2"),
        slot("pushbutton-nc", 20.0, 0, "STOP"),
        slot("pushbutton-no", 38.0, 0, "FWD"),
        child("contact-no", 54.0, 0, "ILK2", "K2"),
        child("contact-no", 70.0, 0, "SEAL1", "K1"),
        slot("pushbutton-no", 38.0, 1, "REV"),
        child("contact-no", 54.0, 1, "ILK1", "K1"),
        child("contact-no", 70.0, 1, "SEAL2", "K2"),
    ],
    links: &[
        link("{STOP}.2", "{FWD}.1"),
        link("{FWD}.2", "{ILK2}.1"),
        link("{ILK2}.2", "{K1}.A1"),
        link("{SEAL1}.1", "{FWD}.1"),
        link("{SEAL1}.2", "{FWD}.2"),
        link("{STOP}.2", "{REV}.1"),
        link("{REV}.2", "{ILK1}.1"),
        link("{ILK1}.2", "{K2}.A1"),
        link("{SEAL2}.1", "{REV}.1"),
        link("{SEAL2}.2", "{REV}.2"),
    ],
};


/// Two-wire control: a maintained selector runs the coil through an
/// overload's auxiliary contact — the "hand" circuit against the start/stop
/// station's "momentary" one.
const TWO_WIRE: Template = Template {
    name: "two-wire",
    description: "two-wire control: maintained selector + overload aux + coil",
    slots: &[
        slot("relay-coil", 88.0, 0, "K"),
        slot("overload", 64.0, 0, "F"),
        slot("selector-2pos", 25.0, 0, "SW"),
        child("contact-nc", 44.0, 0, "OL", "F"),
    ],
    links: &[
        link("{SW}.2", "{OL}.1"),
        link("{OL}.2", "{K}.A1"),
    ],
};

/// Star/delta starter control (textbook form): the main contactor seals the
/// start, the timer runs with it, the star contactor drops out on the timer's
/// delayed-NC and the delta contactor picks up on its delayed-NO. Power rungs
/// are deliberately not templated — star/delta reconfigures the motor's
/// windings, which deserves a hand-drawn power diagram.
const STAR_DELTA: Template = Template {
    name: "star-delta",
    description: "star/delta starter control (timer-switched; no power rung)",
    slots: &[
        // Parents first: the coils and the timer.
        slot("relay-coil", 92.0, 0, "KM1"),
        slot("relay-coil", 92.0, 1, "KM2"),
        slot("relay-coil", 110.0, 2, "KM3"),
        slot("timer-coil-on", 92.0, 2, "KT"),
        slot("pushbutton-nc", 20.0, 0, "STOP"),
        slot("pushbutton-no", 38.0, 0, "START"),
        child("contact-no", 54.0, 0, "SEAL1", "KM1"),
        child("contact-no", 38.0, 1, "KM1AUX", "KM1"),
        child("contact-nc-delayed", 54.0, 1, "KTD_NC", "KT"),
        child("contact-no-delayed", 54.0, 2, "KTD_NO", "KT"),
    ],
    links: &[
        // Main: stop → start → KM1, sealed.
        link("{STOP}.2", "{START}.1"),
        link("{START}.2", "{KM1}.A1"),
        link("{SEAL1}.1", "{START}.1"),
        link("{SEAL1}.2", "{START}.2"),
        // Star: while the main is on and the timer has not fired.
        link("{STOP}.2", "{KM1AUX}.1"),
        link("{KM1AUX}.2", "{KTD_NC}.1"),
        link("{KTD_NC}.2", "{KM2}.A1"),
        // Delta: after the timer fires.
        link("{KM1AUX}.2", "{KTD_NO}.1"),
        link("{KTD_NO}.2", "{KM3}.A1"),
        // The timer runs with the main contactor.
        link("{KM1AUX}.2", "{KT}.A1"),
    ],
};

const TEMPLATES: [&Template; 5] = [
    &START_STOP,
    &DOL,
    &FWD_REV,
    &TWO_WIRE,
    &STAR_DELTA,
];

/// An owned template: what the placement loop actually runs. Both the
/// built-in table and an authored `el-circuits.csv` produce one.
struct Spec {
    name: String,
    description: String,
    slots: Vec<SpecSlot>,
    links: Vec<(String, String)>,
    /// Authored templates report themselves in LIST.
    authored: bool,
}

struct SpecSlot {
    symbol: String,
    dx: f64,
    rung: i32,
    key: String,
    parent: Option<String>,
}

impl From<&&Template> for Spec {
    fn from(t: &&Template) -> Self {
        Spec {
            name: t.name.to_string(),
            description: t.description.to_string(),
            slots: t
                .slots
                .iter()
                .map(|s| SpecSlot {
                    symbol: s.symbol.to_string(),
                    dx: s.dx,
                    rung: s.rung,
                    key: s.key.to_string(),
                    parent: s.parent.map(|p| p.to_string()),
                })
                .collect(),
            links: t.links.iter().map(|l| (l.from.to_string(), l.to.to_string())).collect(),
            authored: false,
        }
    }
}

impl From<&el_core::circuit_template::CircuitTemplate> for Spec {
    fn from(t: &el_core::circuit_template::CircuitTemplate) -> Self {
        Spec {
            name: t.name.clone(),
            description: t.description.clone(),
            slots: t
                .slots
                .iter()
                .map(|s| SpecSlot {
                    symbol: s.symbol.clone(),
                    dx: s.dx,
                    rung: s.rung,
                    key: s.key.clone(),
                    parent: s.parent.clone(),
                })
                .collect(),
            links: t.links.clone(),
            authored: true,
        }
    }
}

/// Authored templates: `el-circuits.csv`, drawing folder first, then beside
/// the symbol library, then the shared config folder — the same resolution
/// ladder the symbol library walks. A malformed file is an error, not a
/// silent skip: an authoring typo must be visible.
/// Authored templates plus, when the file exists but is broken, the error.
///
/// A broken authoring file must never take the built-ins down with it: the
/// error is returned alongside whatever parsed, and callers decide — a
/// built-in still runs (with the error surfaced), an authored miss reports
/// the error instead of a bare "no circuit".
fn authored_templates(host: &dyn HostApi) -> (Vec<Spec>, Option<String>) {
    use el_core::circuit_template;
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = crate::library::drawing_dir(host) {
        candidates.push(dir.join("el-circuits.csv"));
    }
    let (lib_path, _) = crate::library::library_target(host);
    if let Some(dir) = lib_path.parent() {
        candidates.push(dir.join("el-circuits.csv"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        candidates.push(
            std::path::PathBuf::from(appdata)
                .join("OpenCADStudio")
                .join("el-circuits.csv"),
        );
    }
    let mut out = Vec::new();
    for c in candidates {
        if !c.exists() {
            continue;
        }
        let parsed = std::fs::read_to_string(&c)
            .map_err(|e| format!("{}: {e}", c.display()))
            .and_then(|text| {
                circuit_template::parse_templates(&text)
                    .map_err(|e| format!("{}: {e}", c.display()))
            });
        return match parsed {
            Ok(templates) => {
                out.extend(templates.into_iter().map(|t| Spec::from(&t)));
                (out, None)
            }
            Err(e) => (out, Some(e)),
        };
    }
    (out, None)
}

/// Every template a drawing can build: built-ins first, then authored, plus
/// the authoring error if the file exists but is broken.
fn all_templates(host: &dyn HostApi) -> (Vec<Spec>, Option<String>) {
    let mut out: Vec<Spec> = TEMPLATES.iter().map(|t| Spec::from(t)).collect();
    let (authored, error) = authored_templates(host);
    out.extend(authored);
    (out, error)
}


/// One placed slot, kept for link resolution: the record and its definition.
struct Placed {
    rec: ElComponentRec,
    def: SymbolDef,
}

/// Run `EL_CIRCUIT <name> [REF=<rung>] | LIST`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let a = Args::parse(args);
    if a.first_word().map(|w| w.eq_ignore_ascii_case("LIST")).unwrap_or(false)
        || a.free.is_empty()
    {
        list(host);
        return;
    }
    let name = a.first_word().unwrap().to_ascii_lowercase();
    let (all, authoring_error) = all_templates(host);
    let Some(spec) = all.iter().find(|t| t.name == name) else {
        // A broken authored file is the more useful answer when it exists.
        if let Some(e) = authoring_error {
            host.push_error(&format!("EL_CIRCUIT: the authoring file is broken: {e}"));
        } else {
            host.push_error(&format!(
                "EL_CIRCUIT: no circuit named {name:?} — try {}",
                all.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        return;
    };
    let t = spec;

    let lib = match crate::library::load(host) {
        Ok(l) => l,
        Err(e) => {
            host.push_error(&format!("EL_CIRCUIT: {e}"));
            return;
        }
    };
    let idx = crate::codec::build_index(host.document());
    let Some((ladder, rung_index)) = anchor(&idx, &a) else {
        host.push_error(
            "EL_CIRCUIT: the circuit needs a ladder — run EL_LADDER first (REF= picks the rung)",
        );
        return;
    };

    host.push_undo(&format!("EL_CIRCUIT {}", t.name));

    // key → placed slot. Parents precede their children in every template,
    // so a child's parent key is always bound when it is reached.
    let mut placed_by_key: Vec<(String, Placed)> = Vec::new();
    let mut summary: Vec<String> = Vec::new();

    for s in &t.slots {
        // A fresh index per placement: tag allocation must see what landed.
        let idx = crate::codec::build_index(host.document());
        let settings = idx.settings_or_default();
        let Some(def) = lib.get(s.symbol.as_str()) else {
            host.push_error(&format!(
                "EL_CIRCUIT: symbol {:?} is not in the library — EL_LIB LIST",
                s.symbol
            ));
            return;
        };
        let ri = (rung_index as i32 + s.rung).max(0) as usize;
        let y = ladder.rung_y(ri);
        let at = [ladder.origin_x + s.dx, y, 0.0];

        let parent_tag = match s.parent.as_deref() {
            Some(key) => match placed_by_key.iter().find(|(k, _)| *k == key) {
                Some((_, p)) => p.rec.tag.clone(),
                None => {
                    host.push_error(&format!(
                        "EL_CIRCUIT: template bug — {key} is not placed before its child"
                    ));
                    return;
                }
            },
            None => String::new(),
        };

        let mut rec = component::build_record(
            def,
            if def.is_child { None } else { None },
            def.is_child,
            &parent_tag,
            &mut Args::default(),
        );
        rec.origin_x = at[0];
        rec.origin_y = at[1];
        if def.is_child {
            rec.tag = parent_tag;
        } else if rec.tag.is_empty() {
            let rung_ref = idx.rung_reference_at([at[0], at[1]], 20.0);
            match tagging::next_tag(&idx, &rec.family, &settings, rung_ref.as_deref()) {
                Some(tag) => rec.tag = tag,
                None => {
                    host.push_error(&format!(
                        "EL_CIRCUIT: cannot allocate a {} tag — check TAG_FORMAT",
                        rec.family
                    ));
                    return;
                }
            }
        }

        let record = ElRecord::encode_component(&rec);
        let parts = component_entities(&record, def, &rec.tag, at, 0.0);
        host.add_entities(parts);
        let tag = rec.tag.clone();
        placed_by_key.push((s.key.clone(), Placed { rec, def: def.clone() }));
        summary.push(format!("{tag} ({})", s.symbol));
    }

    // Wires: `{KEY}.pin` → that slot's own definition's pin. Two contacts
    // sharing a tag are distinct slots, so there is never an ambiguity to
    // resolve — the registry, not a lookup, answers.
    let mut wired = 0usize;
    for l in &t.links {
        let Some(from) = endpoint(&placed_by_key, &l.0) else {
            host.push_error(&format!(
                "EL_CIRCUIT: template bug — {} does not resolve",
                l.0
            ));
            continue;
        };
        let Some(to) = endpoint(&placed_by_key, &l.1) else {
            host.push_error(&format!("EL_CIRCUIT: template bug — {} does not resolve", l.1));
            continue;
        };
        let (fid, fpin) = split_link(&l.0);
        let (tid, tpin) = split_link(&l.1);
        // The record names the slots' *tags*, not the keys: the drawing's
        // connectivity is real the moment the circuit lands.
        let from_tag = tag_of(&placed_by_key, &fid);
        let to_tag = tag_of(&placed_by_key, &tid);
        // No wire type stamped: a typed wire lands on that type's layer
        // (EL_WIRE_BK_14 …), and a built circuit should read — and count —
        // exactly like hand-drawn wires on EL_WIRES until its user types it.
        let mut rec = ElWireRec::new(&el_core::new_el_id(), &from_tag, &to_tag);
        rec.from_pin = fpin;
        rec.to_pin = tpin;
        let record = ElRecord::encode_wire(&rec);
        host.add_entity(wire_polyline(&record, &[from, to], LAYER_WIRES));
        wired += 1;
    }

    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_CIRCUIT {}: {} device(s), {wired} wire(s) — run EL_WIRENO to number them",
        t.name,
        summary.len()
    ));
    for line in &summary {
        host.push_output(&format!("  {line}"));
    }
}

fn list(host: &mut dyn HostApi) {
    host.push_output("EL_CIRCUIT: pick a circuit (REF=<rung> anchors it)");
    let (all, authoring_error) = all_templates(host);
    if let Some(e) = authoring_error {
        host.push_error(&format!("EL_CIRCUIT: the authoring file is broken: {e}"));
    }
    for t in &all {
        host.push_output(&format!(
            "  {:<14} {}{}",
            t.name,
            t.description,
            if t.authored { "  [authored]" } else { "" }
        ));
    }
}

/// The anchor: the rung named REF= (default the first rung) on the
/// only/first ladder.
fn anchor(idx: &ElIndex, a: &Args) -> Option<(LadderRec, usize)> {
    let ladder = idx
        .ladders_ordered()
        .into_iter()
        .next()
        .map(|l| l.rec.clone())?;
    if let Some(r) = a.get("REF") {
        let refs = ladder.references();
        if let Some(i) = refs.iter().position(|x| x.eq_ignore_ascii_case(r)) {
            return Some((ladder, i));
        }
    }
    Some((ladder, 0))
}

/// `{KEY}.pin` → the slot's pin position in the drawing.
fn endpoint(placed: &[(String, Placed)], name: &str) -> Option<[f64; 3]> {
    let (key, pin) = split_link(name);
    let (_, p) = placed.iter().find(|(k, _)| *k == key)?;
    let pt = p.def.pin_at(&pin, p.rec.origin(), p.rec.rotation_rad())?;
    Some([pt[0], pt[1], 0.0])
}

/// The tag a link's key refers to (for the wire's record).
fn tag_of(placed: &[(String, Placed)], key: &str) -> String {
    placed
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, p)| p.rec.tag.clone())
        .unwrap_or_default()
}

/// `{KEY}.pin` → `(KEY, pin)`; a bare `{KEY}` keeps the symbol's first pin.
fn split_link(name: &str) -> (String, String) {
    let inner = name.trim_start_matches('{');
    if let Some((k, p)) = inner.split_once('}') {
        (k.to_string(), p.trim_start_matches('.').to_string())
    } else {
        (name.to_string(), String::new())
    }
}
