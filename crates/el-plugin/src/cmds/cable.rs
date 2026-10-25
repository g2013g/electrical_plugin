//! `EL_CABLE` — cable markers, and the conductors they belong to.
//!
//! A cable is modelled the way AutoCAD Electrical models it: as a *marker per
//! conductor*. The first marker placed becomes the parent and carries the cable's
//! tag and part; each later one is a child that pulls the parent's tag and takes
//! the next unused conductor colour. That is what makes a four-conductor cable
//! readable as four markers that obviously belong together.
//!
//! The conductor colour order is a convention rather than a rule — a drafter who
//! knows the cable overrides it with `COLOR=` — but generating the next free one
//! is what stops two markers claiming one conductor, which is the mistake the
//! audit then catches.

use ocs_plugin_api::host::HostApi;
use ocs_plugin_api::host::{CommandStep, InteractiveCommand};

use el_core::codec::ElRecord;
use el_core::model::{CableRec, COLOR_CODES};

use crate::cmds::common::Args;
use crate::geometry::cable_marker_entities;
use crate::state::{self, PendingLine};

/// The default conductor colours, in the order a marker takes them.
///
/// The first is the one a single-conductor cable gets, which is why black leads:
/// it is the ordinary case, and the colours that carry meaning come later.
pub fn default_conductor_colors() -> Vec<String> {
    COLOR_CODES.iter().map(|c| c.to_string()).collect()
}

/// Run `EL_CABLE [tag] [x,y] [COND=] [COLOR=] [GAUGE=] [PART=] [MFG=] [CAT=]
/// [FROM=] [TO=] [DESC=] [ROT=]`
/// or `EL_CABLE NEW [tag] …` / `EL_CABLE LIST` / `EL_CABLE CLEAR`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);

    let word = a.first_word().map(|w| w.to_ascii_uppercase());
    match word.as_deref() {
        Some("LIST") => {
            list(host);
            return;
        }
        Some("CLEAR") => {
            clear(host);
            return;
        }
        _ => {}
    }

    let idx = crate::codec::build_index(host.document());

    // The tag: an explicit one, or the drawing's next free cable.
    let words: Vec<String> = a
        .free
        .iter()
        .filter(|t| !t.contains(',') && !t.eq_ignore_ascii_case("NEW"))
        .cloned()
        .collect();
    let tag = words
        .first()
        .cloned()
        .or_else(|| a.get("TAG").map(str::to_string))
        .unwrap_or_else(|| idx.next_cable_tag());

    let existing = idx.cable_markers(&tag).len();
    let conductor = a
        .integer("COND")
        .unwrap_or_else(|| idx.next_conductor(&tag));
    if conductor <= 0 {
        host.push_error("EL_CABLE: COND must be a positive conductor number");
        return;
    }
    if idx.conductors_used(&tag).contains(&conductor) {
        host.push_error(&format!(
            "EL_CABLE: conductor {conductor} of {tag} already has a marker — \
             run EL_CABLE LIST to see the cable"
        ));
        return;
    }

    // The colour: the caller's, else the one for this conductor's position.
    let defaults = default_conductor_colors();
    let color = a.get("COLOR").map(str::to_string).unwrap_or_else(|| {
        defaults
            .get((conductor as usize).saturating_sub(1))
            .cloned()
            .unwrap_or_default()
    });

    let mut rec = CableRec::new(&el_core::new_el_id(), &tag);
    rec.conductor_index = conductor;
    rec.color = color;
    if existing > 0 {
        // A marker after the first belongs to the marker that came before it.
        rec.parent_tag = tag.clone();
    }
    if let Some(v) = a.get("GAUGE") {
        rec.gauge = v.to_string();
    }
    if let Some(v) = a.get("PART") {
        rec.part_number = v.to_string();
    }
    if let Some(v) = a.get("MFG") {
        rec.mfg = v.to_string();
    }
    if let Some(v) = a.get("CAT") {
        rec.cat = v.to_string();
    }
    if let Some(v) = a.get("FROM") {
        rec.from_id = v.to_string();
    }
    if let Some(v) = a.get("TO") {
        rec.to_id = v.to_string();
    }
    rec.description = a.get("DESC").map(str::to_string).unwrap_or_default();
    let rot = a.number("ROT").unwrap_or(0.0).to_radians();

    for e in &a.errors {
        host.push_error(&format!("EL_CABLE: {e}"));
    }

    let at = a.first_coord().map(|c| [c[0], c[1], 0.0]);
    match at {
        Some(at) => place(host, rec, at, rot),
        None => start_interactive(host, rec),
    }
}

/// The marker text: the cable tag, plus the conductor when it is not the first.
///
/// A single-conductor cable reads as `W1`; a four-conductor one reads `W1.2`,
/// which is how a wireman finds conductor 2 in the loom.
pub fn marker_text(rec: &CableRec) -> String {
    if rec.tag.is_empty() {
        return String::new();
    }
    if rec.conductor_index <= 1 {
        rec.tag.clone()
    } else {
        format!("{}.{}", rec.tag, rec.conductor_index)
    }
}

fn place(host: &mut dyn HostApi, rec: CableRec, at: [f64; 3], rot: f64) {
    let record = ElRecord::encode_cable(&rec);
    let text = marker_text(&rec);
    let parts = cable_marker_entities(&record, &text, at, rot, &rec.color);
    let count = parts.len();

    host.push_undo("EL_CABLE");
    host.add_entities(parts);
    host.bump_geometry();
    host.set_dirty();

    host.push_output(&format!(
        "EL_CABLE: {text} (conductor {}/{}) colour {} at ({}, {}) — {count} entit(ies)",
        rec.conductor_index,
        describe_role(&rec),
        if rec.color.is_empty() {
            "—".to_string()
        } else {
            rec.color.clone()
        },
        el_core::fmt_mm(at[0]),
        el_core::fmt_mm(at[1])
    ));
}

fn describe_role(rec: &CableRec) -> String {
    if rec.is_parent() {
        "parent marker".to_string()
    } else {
        format!("marker of {}", rec.parent_tag)
    }
}

fn start_interactive(host: &mut dyn HostApi, rec: CableRec) {
    host.push_info(&format!(
        "EL_CABLE {}: pick where the marker goes (conductor {}, colour {})",
        marker_text(&rec),
        rec.conductor_index,
        if rec.color.is_empty() { "—" } else { &rec.color }
    ));
    host.start_interactive(Box::new(CableCmd { rec, placed: false }));
}

struct CableCmd {
    rec: CableRec,
    placed: bool,
}

impl InteractiveCommand for CableCmd {
    fn prompt(&self) -> String {
        "EL_CABLE: pick the marker position".to_string()
    }

    fn on_point(&mut self, pt: [f64; 3]) -> CommandStep {
        if self.placed {
            return CommandStep::Done;
        }
        self.placed = true;

        let record = ElRecord::encode_cable(&self.rec);
        let text = marker_text(&self.rec);
        let parts = cable_marker_entities(&record, &text, pt, 0.0, &self.rec.color);
        let count = parts.len();

        state::push_entities(parts);
        state::spawn_flush(Some("EL_CABLE".to_string()));
        state::emit(PendingLine::Output(format!(
            "EL_CABLE: {text} at ({}, {}) — {count} entit(ies)",
            el_core::fmt_mm(pt[0]),
            el_core::fmt_mm(pt[1])
        )));
        CommandStep::Done
    }

    fn on_enter(&mut self) -> CommandStep {
        CommandStep::Cancel
    }
}

fn list(host: &mut dyn HostApi) {
    let idx = crate::codec::build_index(host.document());
    if idx.cables.is_empty() {
        host.push_output("EL_CABLE: this drawing has no cables");
        return;
    }
    let mut tags: Vec<String> = idx
        .cables
        .values()
        .map(|c| c.rec.tag.clone())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort_by(|a, b| el_core::refdes_cmp(a, b));
    tags.dedup();

    host.push_output(&format!("EL_CABLE — {} cable(s)", tags.len()));
    for tag in tags {
        let markers = idx.cable_markers(&tag);
        let part = idx
            .cable_parent(&tag)
            .map(|p| {
                if p.rec.cat.is_empty() {
                    p.rec.part_number.clone()
                } else {
                    p.rec.cat.clone()
                }
            })
            .unwrap_or_default();
        host.push_output(&format!(
            "  {tag}: {} conductor(s) [{}]{}",
            markers.len(),
            markers
                .iter()
                .map(|m| if m.rec.color.is_empty() {
                    "—".to_string()
                } else {
                    m.rec.color.clone()
                })
                .collect::<Vec<_>>()
                .join(","),
            if part.is_empty() {
                String::new()
            } else {
                format!(" part {part}")
            }
        ));
        if idx.cable_parent(&tag).is_none() {
            host.push_error(&format!(
                "  {tag} has no parent marker, so its part cannot be resolved"
            ));
        }
    }
}

fn clear(host: &mut dyn HostApi) {
    let idx = crate::codec::build_index(host.document());
    if idx.cables.is_empty() {
        host.push_output("EL_CABLE CLEAR: this drawing has no cables");
        return;
    }
    host.push_undo("EL_CABLE CLEAR");
    let n = crate::cmds::common::remove_entities_on_layer(host, el_core::model::LAYER_CABLES);
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!("EL_CABLE CLEAR: removed {n} entit(ies)"));
}
