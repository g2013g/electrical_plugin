//! `EL_TERMINAL` — place a terminal block, and number a strip.
//!
//! A terminal is a device with *strip* semantics: it belongs to a strip
//! (`TAGSTRIP`), occupies a position in that strip's sequence, and its designator
//! is the pair — `X1:12`. That is why it is its own record rather than a
//! component: the terminal plan and the strip audits both key off the strip.
//!
//! Numbering is deliberate rather than automatic. A terminal's number is what a
//! wireman reads on the block, so it is assigned by `EL_TERMINAL NUMBER`, not as a
//! side effect of placement — the same principle as wire numbers.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::{CommandStep, HostApi, InteractiveCommand};

use el_core::codec::ElRecord;
use el_core::model::{TerminalKind, TerminalRec};

use crate::cmds::common::{remove_entities_on_layer, Args};
use crate::hostops::DocEntityExt;
use crate::geometry::{terminal_entities, terminal_pin_point};
use crate::state::{self, PendingLine};

/// Run `EL_TERMINAL [kind] [strip] [x,y] [NO=] [PART=] [DESC=] [LINK=] [ROT=]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);

    // Actions first: they are unambiguous words, and a terminal is never called
    // "NUMBER".
    let word = a.first_word().map(|w| w.to_ascii_uppercase());
    match word.as_deref() {
        Some("NUMBER") | Some("RENUMBER") => {
            renumber(host, &mut a);
            return;
        }
        Some("CLEAR") => {
            clear(host);
            return;
        }
        Some("LIST") => {
            list(host);
            return;
        }
        _ => {}
    }

    let idx = crate::codec::build_index(host.document());

    // `<kind> <strip>`: the kind is a known word, the strip is not. Both optional,
    // both defaulting from the drawing.
    let words: Vec<String> = a
        .free
        .iter()
        .filter(|t| !t.contains(','))
        .cloned()
        .collect();
    let kind = words
        .first()
        .and_then(|w| TerminalKind::parse(w))
        .unwrap_or_default();
    if let Some(w) = words.first() {
        if TerminalKind::parse(w).is_none() && words.len() > 1 {
            host.push_error(&format!(
                "EL_TERMINAL: {w:?} is not a terminal kind — try screw, spring, ground, fused, \
                 disconnect, multi or test"
            ));
            return;
        }
    }
    let strip = words
        .iter()
        .find(|w| TerminalKind::parse(w).is_none())
        .cloned()
        .or_else(|| a.get("STRIP").map(str::to_string))
        .or_else(|| {
            // An existing strip is the likely intent when none is named.
            idx.strips().keys().next().cloned()
        });

    let at = a.first_coord().map(|c| [c[0], c[1], 0.0]);
    let spec = TerminalSpec {
        kind,
        strip,
        at,
        number: a.get("NO").map(str::to_string),
        part: a.get("PART").map(str::to_string),
        description: a.get("DESC").map(str::to_string),
        linked: a.get("LINK").map(str::to_string),
        rot: a.number("ROT").unwrap_or(0.0).to_radians(),
    };

    for e in &a.errors {
        host.push_error(&format!("EL_TERMINAL: {e}"));
    }

    match spec.at {
        Some(at) => place(host, &spec, at),
        None => start_interactive(host, spec),
    }
}

/// Everything a placement needs, so the interactive path and the synchronous one
/// cannot diverge.
#[derive(Debug, Clone)]
struct TerminalSpec {
    kind: TerminalKind,
    strip: Option<String>,
    at: Option<[f64; 3]>,
    number: Option<String>,
    part: Option<String>,
    description: Option<String>,
    linked: Option<String>,
    rot: f64,
}

impl TerminalSpec {
    fn build_record(&self, idx: &el_core::ElIndex, at: [f64; 3]) -> Result<TerminalRec, String> {
        let Some(strip) = self.strip.clone().filter(|s| !s.is_empty()) else {
            return Err(
                "a terminal needs a strip designator — name one, e.g. EL_TERMINAL screw X1"
                    .to_string(),
            );
        };
        let mut rec = TerminalRec::new(&el_core::new_el_id(), &strip, self.kind);
        rec.origin_x = at[0];
        rec.origin_y = at[1];
        rec.rotation_deg = self.rot.to_degrees();
        // A number is derived from the strip unless one was given, so a strip
        // drawn left to right numbers itself.
        rec.number = self
            .number
            .clone()
            .unwrap_or_else(|| idx.next_terminal_number(&strip));
        rec.strip_seq = rec.number.parse::<i32>().unwrap_or(0);
        rec.part_number = self.part.clone().unwrap_or_default();
        rec.description = self.description.clone().unwrap_or_else(|| {
            format!("Terminal block, {}", self.kind.as_str().to_ascii_lowercase())
        });
        rec.linked = self.linked.clone().unwrap_or_default();
        let _ = at;
        Ok(rec)
    }
}

fn place(host: &mut dyn HostApi, spec: &TerminalSpec, at: [f64; 3]) {
    let idx = crate::codec::build_index(host.document());
    let rec = match spec.build_record(&idx, at) {
        Ok(r) => r,
        Err(e) => {
            host.push_error(&format!("EL_TERMINAL: {e}"));
            return;
        }
    };

    let record = ElRecord::encode_terminal(&rec);
    let parts = terminal_entities(&record, &rec.designator(), at, spec.rot, rec.kind);
    let count = parts.len();

    host.push_undo("EL_TERMINAL");
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();

    host.push_output(&format!(
        "EL_TERMINAL: {} ({}) at ({}, {}) — {count} entit(ies) on {}",
        rec.designator(),
        rec.kind.as_str(),
        el_core::fmt_mm(at[0]),
        el_core::fmt_mm(at[1]),
        el_core::model::LAYER_TERMINALS
    ));
    print_pins(host, at, spec.rot);
}

fn print_pins(host: &mut dyn HostApi, at: [f64; 3], rot: f64) {
    let l = terminal_pin_point(at, rot, false);
    let r = terminal_pin_point(at, rot, true);
    host.push_output(&format!(
        "  connections: ({}, {}) and ({}, {})",
        el_core::fmt_mm(l[0]),
        el_core::fmt_mm(l[1]),
        el_core::fmt_mm(r[0]),
        el_core::fmt_mm(r[1])
    ));
}

fn start_interactive(host: &mut dyn HostApi, spec: TerminalSpec) {
    let Some(strip) = spec.strip.clone().filter(|s| !s.is_empty()) else {
        host.push_error(
            "EL_TERMINAL: name the strip before placing, e.g. EL_TERMINAL screw X1",
        );
        return;
    };
    let capture = crate::hostops::Capture::take(host);
    let next = capture.index.next_terminal_number(&strip);
    host.push_info(&format!(
        "EL_TERMINAL {} {}: pick the block position (next free number is {next})",
        spec.kind.as_str().to_ascii_lowercase(),
        strip
    ));
    host.start_interactive(Box::new(TerminalCmd {
        spec,
        idx: capture.index,
        placed: false,
    }));
}

struct TerminalCmd {
    spec: TerminalSpec,
    idx: el_core::ElIndex,
    placed: bool,
}

impl InteractiveCommand for TerminalCmd {
    fn prompt(&self) -> String {
        "EL_TERMINAL: pick the block position".to_string()
    }

    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep {
        if self.placed {
            return CommandStep::Done;
        }
        self.placed = true;

        let rec = match self.spec.build_record(&self.idx, pt) {
            Ok(r) => r,
            Err(e) => {
                state::emit(PendingLine::Error(format!("EL_TERMINAL: {e}")));
                return CommandStep::Done;
            }
        };
        let record = ElRecord::encode_terminal(&rec);
        let parts = terminal_entities(&record, &rec.designator(), pt, self.spec.rot, rec.kind);
        let count = parts.len();

        state::push_entities(parts);
        state::spawn_flush(Some("EL_TERMINAL".to_string()));
        state::emit(PendingLine::Output(format!(
            "EL_TERMINAL: {} at ({}, {}) — {count} entit(ies)",
            rec.designator(),
            el_core::fmt_mm(pt[0]),
            el_core::fmt_mm(pt[1])
        )));
        CommandStep::Done
    }

    fn on_enter(&mut self) -> CommandStep {
        CommandStep::Cancel
    }
}

/// Number every terminal on a strip, in strip order.
///
/// A strip is a physical row of blocks and every block has a position number, so
/// all of them are numbered — including a protective-earth block, whose number is
/// its position. Leaving one out is not just untidy: the skipped terminal keeps
/// whatever number it already had, and the next block numbered into that slot
/// collides with it.
fn renumber(host: &mut dyn HostApi, a: &mut Args) {
    let idx = crate::codec::build_index(host.document());
    if idx.terminals.is_empty() {
        host.push_error("EL_TERMINAL NUMBER: this drawing has no terminals");
        return;
    }

    let only = a
        .free
        .iter()
        .skip(1)
        .find(|t| !t.contains(','))
        .cloned()
        .filter(|t| !t.eq_ignore_ascii_case("ALL"));

    let start = a.integer("START").unwrap_or(1);
    if start <= 0 {
        host.push_error("EL_TERMINAL NUMBER: START must be positive");
        return;
    }

    // Group by strip, ordering by the drawn position so a strip is numbered the
    // way it is read rather than the order the scan found entities.
    let mut work: Vec<(String, Vec<(u64, [f64; 3], TerminalKind)>)> = Vec::new();
    for t in idx.terminals.values() {
        if t.rec.tag_strip.is_empty() {
            continue;
        }
        if let Some(f) = &only {
            if !t.rec.tag_strip.eq_ignore_ascii_case(f) {
                continue;
            }
        }
        let anchor = host
            .document()
            .get_entity_owned(Handle::new(t.primary()))
            .and_then(|e| crate::geometry::carrier_anchor(&e))
            .unwrap_or([0.0, 0.0, 0.0]);
        match work.iter_mut().find(|(s, _)| *s == t.rec.tag_strip) {
            Some((_, list)) => list.push((t.primary(), anchor, t.rec.kind)),
            None => work.push((
                t.rec.tag_strip.clone(),
                vec![(t.primary(), anchor, t.rec.kind)],
            )),
        }
    }

    if work.is_empty() {
        host.push_error(&format!(
            "EL_TERMINAL NUMBER: no terminals on strip {:?}",
            only.unwrap_or_default()
        ));
        return;
    }

    host.push_undo("EL_TERMINAL NUMBER");
    let mut written = 0usize;
    let mut strips = 0usize;
    // The records as they now stand, so the glyphs are rebuilt from the numbers
    // this run assigned rather than from a snapshot that predates the write.
    let mut updated: Vec<TerminalRec> = Vec::new();

    for (strip, mut list) in work {
        strips += 1;
        // Left to right, then top to bottom — the order a strip is read.
        list.sort_by(|a, b| {
            a.1[0]
                .partial_cmp(&b.1[0])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.1[1].partial_cmp(&a.1[1]).unwrap_or(std::cmp::Ordering::Equal))
        });

        let mut n = start;
        for (handle, _, _kind) in list {
            let Some(t) = idx.terminal_of_handle(handle) else {
                continue;
            };
            let mut rec = t.rec.clone();
            rec.number = n.to_string();
            rec.strip_seq = n;
            let record = ElRecord::encode_terminal(&rec);
            // Every drawn part carries the record, so all of them are rewritten.
            let mut part_ok = true;
            for h in idx.handles_for_el_id(&rec.el_id) {
                if !crate::codec::write_el(host, Handle::new(*h), &record) {
                    part_ok = false;
                }
            }
            if part_ok {
                written += 1;
                updated.push(rec);
            }
            n += 1;
        }
        host.push_output(&format!("  {strip}: {} terminal(s) numbered", n - start));
    }

    // The designator is drawn text, so every strip's glyphs are rebuilt.
    rebuild_glyphs(host, &idx, &updated);

    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_TERMINAL NUMBER: {written} terminal(s) numbered across {strips} strip(s)"
    ));
}

/// Rebuild every terminal glyph, because the designator is drawn text.
///
/// `updated` carries the records this run changed; every other terminal is
/// rebuilt from the record the index already holds, so one strip being renumbered
/// does not leave its neighbours showing stale designators.
fn rebuild_glyphs(
    host: &mut dyn HostApi,
    idx: &el_core::ElIndex,
    updated: &[TerminalRec],
) {
    for t in idx.terminals.values() {
        let rec = updated
            .iter()
            .find(|u| u.el_id == t.el_id)
            .cloned()
            .unwrap_or_else(|| t.rec.clone());
        let Some(anchor) = host
            .document()
            .get_entity_owned(Handle::new(t.primary()))
            .and_then(|e| crate::geometry::carrier_anchor(&e))
        else {
            continue;
        };
        let record = ElRecord::encode_terminal(&rec);
        let parts = crate::geometry::terminal_entities(
            &record,
            &rec.designator(),
            anchor,
            0.0,
            rec.kind,
        );
        for h in idx.handles_for_el_id(&t.el_id) {
            host.remove_entity(Handle::new(*h));
        }
        host.add_entities(parts);
    }
}

fn clear(host: &mut dyn HostApi) {
    let idx = crate::codec::build_index(host.document());
    if idx.terminals.is_empty() {
        host.push_output("EL_TERMINAL CLEAR: this drawing has no terminals");
        return;
    }
    host.push_undo("EL_TERMINAL CLEAR");
    let n = remove_entities_on_layer(host, el_core::model::LAYER_TERMINALS);
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_TERMINAL CLEAR: removed {n} entit(ies)"));
}

fn list(host: &mut dyn HostApi) {
    let idx = crate::codec::build_index(host.document());
    let strips = idx.strips();
    if strips.is_empty() {
        host.push_output("EL_TERMINAL: this drawing has no terminals");
        return;
    }
    let total: usize = strips.values().map(|v| v.len()).sum();
    host.push_output(&format!(
        "EL_TERMINAL — {} strip(s), {total} terminal(s)",
        strips.len()
    ));
    for (strip, terminals) in strips {
        host.push_output(&format!(
            "  {strip}: {}",
            terminals
                .iter()
                .map(|t| if t.rec.number.is_empty() {
                    "—".to_string()
                } else {
                    t.rec.number.clone()
                })
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
}
