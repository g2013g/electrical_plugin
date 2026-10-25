//! `EL_LADDER` and `EL_RUNG` — ladders and their line references.
//!
//! A ladder is the substrate reference-based numbering needs: a wire's number
//! can only be "the line reference of the rung it sits on" if rungs have
//! references. It is also the first object here that is drawn as many entities,
//! which is why it exercises the deferred-write path.

use ocs_plugin_api::host::{CommandStep, HostApi, InteractiveCommand};

use el_core::codec::ElRecord;
use el_core::model::LadderRec;
use el_core::ElIndex;

use crate::cmds::common::{remove_handles, Args};
use crate::geometry::ladder_entities;
use crate::state::{self, PendingLine};

/// Run `EL_LADDER [x,y] [WIDTH=] [SPACING=] [RUNGS=] [REF=] [INC=] [PHASE=]
/// [NORUNGS] [NOBUS] [NOREF] | CLEAR`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);

    if a.free.iter().any(|f| f.eq_ignore_ascii_case("CLEAR")) {
        clear(host);
        return;
    }

    // A bare `EL_LADDER` places interactively; everything else is synchronous,
    // which keeps a placement to one undo step.
    if a.free.is_empty() && a.kv.is_empty() {
        start_interactive(host);
        return;
    }

    // With options but no coordinate, the first rung anchors at the origin —
    // the caller is configuring, not placing, and the geometry is still theirs
    // to move.
    let at = a.first_coord().unwrap_or([0.0, 0.0, 0.0]);
    place(host, at, &mut a);
}

fn clear(host: &mut dyn HostApi) {
    let idx = crate::codec::build_index(host.document());
    if idx.ladders.is_empty() {
        host.push_output("EL_LADDER CLEAR: this drawing has no ladders");
        return;
    }
    let ids: Vec<String> = idx.ladders.keys().cloned().collect();
    let all_parts: Vec<u64> = ids
        .iter()
        .flat_map(|id| idx.handles_for_el_id(id).iter().copied())
        .collect();
    let part_count = all_parts.len();
    host.push_undo("EL_LADDER CLEAR");
    let removed = remove_handles(host, &all_parts, "EL_LADDER CLEAR");
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_LADDER CLEAR: removed {} ladder(s), {removed} of {part_count} entit(ies)",
        ids.len()
    ));
}

/// Build a ladder record from the drawing's defaults plus any overrides.
fn build_record(idx: &ElIndex, at: [f64; 3], a: &mut Args) -> Result<LadderRec, Vec<String>> {
    let s = idx.settings_or_default();
    // A rung count of zero must be refused rather than defaulted: the record
    // clamps to one, so accepting it would silently draw a rung the caller
    // never asked for.
    let rungs = match a.integer("RUNGS") {
        Some(n) if n <= 0 => {
            a.errors.push(format!("RUNGS expects a positive count, got {n}"));
            s.rung_count
        }
        Some(n) => n,
        None => s.rung_count,
    };
    let mut rec = LadderRec::new(
        &el_core::new_el_id(),
        [at[0], at[1]],
        a.number("WIDTH").unwrap_or(s.ladder_width_mm),
        a.number("SPACING").unwrap_or(s.rung_spacing_mm),
        rungs,
    );
    if let Some(r) = a.get("REF").or_else(|| a.get("FIRST_REF")) {
        rec.first_ref = r.to_string();
    } else {
        rec.first_ref = s.first_rung_ref.clone();
    }
    rec.rung_increment = a.integer("INC").unwrap_or(s.rung_increment);
    if let Some(p) = a.integer("PHASE") {
        if !(1..=3).contains(&p) {
            a.errors.push(format!("PHASE expects 1 or 3, got {p}"));
        }
        rec.phase = p.clamp(1, 3) as u8;
    }

    // Flags: a ladder with no rungs still carries references, so at least one
    // of the three must be on or the command would draw nothing.
    if let Some(v) = a.flag("NORUNGS") {
        rec.draw_rungs = !v;
    }
    if let Some(v) = a.flag("NOBUS") {
        rec.draw_bus = !v;
    }
    if let Some(v) = a.flag("NOREF") {
        rec.show_references = !v;
    }
    if !rec.draw_rungs && !rec.draw_bus && !rec.show_references {
        a.errors
            .push("NORUNGS + NOBUS + NOREF leaves nothing to draw".to_string());
    }
    if rec.width_mm <= 0.0 {
        a.errors.push("WIDTH must be positive".to_string());
    }
    if rec.rung_spacing_mm <= 0.0 {
        a.errors.push("SPACING must be positive".to_string());
    }

    if a.errors.is_empty() {
        Ok(rec)
    } else {
        Err(a.errors.clone())
    }
}

/// Place a ladder synchronously: every part, one undo window.
fn place(host: &mut dyn HostApi, at: [f64; 3], a: &mut Args) {
    let idx = crate::codec::build_index(host.document());
    let rec = match build_record(&idx, at, a) {
        Ok(r) => r,
        Err(errors) => {
            for e in errors {
                host.push_error(&format!("EL_LADDER: {e}"));
            }
            return;
        }
    };

    let record = ElRecord::encode_ladder(&rec);
    let parts = ladder_entities(&record, &rec);
    let count = parts.len();

    host.push_undo("EL_LADDER");
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();

    let refs = rec.references();
    host.push_output(&format!(
        "EL_LADDER: {} rung(s) at ({}, {}), width {}mm spacing {}mm, {count} entit(ies) on {}",
        rec.rung_count,
        el_core::fmt_mm(rec.origin_x),
        el_core::fmt_mm(rec.origin_y),
        el_core::fmt_mm(rec.width_mm),
        el_core::fmt_mm(rec.rung_spacing_mm),
        el_core::model::LAYER_LADDER
    ));
    host.push_output(&format!(
        "  references: {} … {}",
        refs.first().cloned().unwrap_or_default(),
        refs.last().cloned().unwrap_or_default()
    ));
}

/// Interactive placement: pick the top-left corner of the ladder.
///
/// The pick produces many entities, and `CommandStep` carries one — so the
/// whole frame is queued and published from a background thread once the host
/// is back in its own event loop. Returning `Done` rather than a commit is what
/// keeps the placement a single undo window (see `state::spawn_flush`).
fn start_interactive(host: &mut dyn HostApi) {
    let idx = crate::codec::build_index(host.document());
    let s = idx.settings_or_default();
    host.push_info(&format!(
        "EL_LADDER: pick the ladder's top-left corner ({} rungs, {}mm spacing, width {}mm — \
         override with RUNGS=, SPACING=, WIDTH=)",
        s.rung_count,
        el_core::fmt_mm(s.rung_spacing_mm),
        el_core::fmt_mm(s.ladder_width_mm)
    ));
    host.start_interactive(Box::new(LadderCmd {
        defaults: s,
        placed: false,
    }));
}

struct LadderCmd {
    defaults: el_core::ElSettingsRec,
    placed: bool,
}

impl InteractiveCommand for LadderCmd {
    fn prompt(&self) -> String {
        "EL_LADDER: pick the top-left corner".to_string()
    }

    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep {
        if self.placed {
            return CommandStep::Done;
        }
        self.placed = true;

        let mut rec = LadderRec::new(
            &el_core::new_el_id(),
            [pt[0], pt[1]],
            self.defaults.ladder_width_mm,
            self.defaults.rung_spacing_mm,
            self.defaults.rung_count,
        );
        rec.first_ref = self.defaults.first_rung_ref.clone();
        rec.rung_increment = self.defaults.rung_increment;

        let record = ElRecord::encode_ladder(&rec);
        let parts = ladder_entities(&record, &rec);
        let count = parts.len();
        let refs = rec.references();

        state::push_entities(parts);
        state::spawn_flush(Some("EL_LADDER".to_string()));
        state::emit(PendingLine::Output(format!(
            "EL_LADDER: {} rung(s) at ({}, {}), {count} entit(ies); references {} … {}",
            rec.rung_count,
            el_core::fmt_mm(rec.origin_x),
            el_core::fmt_mm(rec.origin_y),
            refs.first().cloned().unwrap_or_default(),
            refs.last().cloned().unwrap_or_default()
        )));
        CommandStep::Done
    }

    fn on_enter(&mut self) -> CommandStep {
        CommandStep::Cancel
    }
}

// ── EL_RUNG ──────────────────────────────────────────────────────────────

/// Run `EL_RUNG [ladder|@SEL] [ADD=<n>] [SPACING=] [REF=] [INC=] [RUNGS=]`.
///
/// Revising a ladder rewrites the whole frame: the drawn parts are removed and
/// rebuilt from the record, because the frame's geometry is a pure function of
/// the record and a partial edit would leave the two disagreeing.
pub fn run_rung(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let idx = crate::codec::build_index(host.document());

    if idx.ladders.is_empty() {
        host.push_error("EL_RUNG: this drawing has no ladder — run EL_LADDER first");
        return;
    }

    let target = match resolve_ladder(&idx, &mut a) {
        Ok(l) => l.clone(),
        Err(e) => {
            host.push_error(&format!("EL_RUNG: {e}"));
            return;
        }
    };
    let mut rec = target.rec;

    let mut changed: Vec<String> = Vec::new();
    if let Some(n) = a.integer("ADD") {
        if n <= 0 {
            a.errors.push("ADD expects a positive count".to_string());
        } else {
            rec.rung_count += n;
            changed.push(format!("added {n} rung(s)"));
        }
    }
    if let Some(n) = a.integer("RUNGS") {
        if n <= 0 {
            a.errors.push("RUNGS expects a positive count".to_string());
        } else {
            rec.rung_count = n;
            changed.push(format!("RUNGS={n}"));
        }
    }
    if let Some(v) = a.number("SPACING") {
        if v <= 0.0 {
            a.errors.push("SPACING must be positive".to_string());
        } else {
            rec.rung_spacing_mm = v;
            changed.push(format!("SPACING={}", el_core::fmt_mm(v)));
        }
    }
    if let Some(v) = a.number("WIDTH") {
        if v <= 0.0 {
            a.errors.push("WIDTH must be positive".to_string());
        } else {
            rec.width_mm = v;
            changed.push(format!("WIDTH={}", el_core::fmt_mm(v)));
        }
    }
    if let Some(r) = a.get("REF") {
        rec.first_ref = r.to_string();
        changed.push(format!("REF={r}"));
    }
    if let Some(i) = a.integer("INC") {
        if i == 0 {
            a.errors.push("INC must be non-zero, or every rung shares a reference".to_string());
        } else {
            rec.rung_increment = i;
            changed.push(format!("INC={i}"));
        }
    }

    for e in &a.errors {
        host.push_error(&format!("EL_RUNG: {e}"));
    }
    if !a.errors.is_empty() || changed.is_empty() {
        if changed.is_empty() && a.errors.is_empty() {
            host.push_error("EL_RUNG: nothing to change — pass ADD=, RUNGS=, SPACING=, REF= or INC=");
        }
        return;
    }

    let record = ElRecord::encode_ladder(&rec);
    let parts = ladder_entities(&record, &rec);

    host.push_undo("EL_RUNG");
    // Remove the old frame, then write the record onto the new one. A partial
    // removal would leave two overlapping frames, so it is reported.
    let old_parts: Vec<u64> = idx.handles_for_el_id(&rec.el_id).to_vec();
    remove_handles(host, &old_parts, "EL_RUNG (replacing the old frame)");
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();

    let refs = rec.references();
    host.push_output(&format!(
        "EL_RUNG: {} ({}); now {} rung(s), references {} … {}",
        changed.join(", "),
        rec.el_id,
        rec.rung_count,
        refs.first().cloned().unwrap_or_default(),
        refs.last().cloned().unwrap_or_default()
    ));
}

/// Resolve which ladder a command acts on: an explicit `el_id`, a handle, a
/// picked part, `@SEL`, or — when there is exactly one — that one.
fn resolve_ladder(idx: &ElIndex, a: &mut Args) -> Result<el_core::index::IndexedLadder, String> {
    if let Some(word) = a.first_word() {
        if let Some(pinned) = state::resolve_sel_token(word) {
            let Some(h) = pinned else {
                return Err("nothing inspected yet — pick a ladder or pass its id".to_string());
            };
            return idx
                .ladder_of_handle(h)
                .cloned()
                .ok_or_else(|| format!("handle {h:#x} is not part of a ladder"));
        }
        if let Some(l) = idx.ladder_by_el_id(word) {
            return Ok(l.clone());
        }
        if let Some(h) = crate::hostops::parse_handle(word) {
            return idx
                .ladder_of_handle(h.value())
                .cloned()
                .ok_or_else(|| format!("{word} is not part of a ladder"));
        }
        // A line reference names a rung, so accept one as a locator too.
        let by_ref = idx
            .ladders_ordered()
            .into_iter()
            .find(|l| l.rec.references().iter().any(|r| r == word))
            .cloned();
        if let Some(l) = by_ref {
            return Ok(l);
        }
        return Err(format!("no ladder matches {word:?} — expected an id, a handle, or a line reference"));
    }

    let ladders = idx.ladders_ordered();
    if ladders.len() == 1 {
        return Ok(ladders[0].clone());
    }
    Err(format!(
        "this drawing has {} ladders — name one by id or line reference",
        ladders.len()
    ))
}
