//! `EL_INFO` — read back one electrical entity's record.
//!
//! Two modes, and the difference matters:
//!
//! * `EL_INFO K1` / `EL_INFO 0x2E` is synchronous — everything happens in
//!   dispatch context, where the host is real.
//! * Bare `EL_INFO` starts an object-pick command. That callback runs against
//!   the `NullHost` stub, so it cannot read the document or print anything;
//!   hence the capture taken *before* the command starts, and the deferred
//!   output line (`state::emit`) instead of a direct `push_output`.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::{CommandStep, HostApi, InteractiveCommand};

use el_core::model::KIND_COMPONENT;

use crate::hostops::Capture;
use crate::state::{self, PendingLine};

/// Run `EL_INFO [tag|wirenr|handle|@SEL]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    if args.is_empty() {
        start_interactive(host);
        return;
    }

    let token = args[0];
    // `@SEL` resolves through the pinned entity rather than by name.
    if let Some(pinned) = state::resolve_sel_token(token) {
        let Some(handle) = pinned else {
            host.push_error(
                "EL_INFO @SEL: nothing inspected yet — run EL_INFO without arguments and pick an entity",
            );
            return;
        };
        let cap = Capture::take(host);
        match cap.get(handle) {
            Some(entity) => {
                state::set_last_target(handle);
                host.push_output(&describe(&cap, entity.handle, &entity.record));
            }
            None => host.push_error(&format!(
                "EL_INFO @SEL: handle {handle:#x} no longer carries an electrical record"
            )),
        }
        return;
    }

    let cap = Capture::take(host);
    match cap.resolve_token(token) {
        Ok(target) => {
            state::set_last_target(target.handle());
            let Some(entity) = cap.get(target.handle()) else {
                host.push_error(&format!("EL_INFO: {token} resolved but its record vanished"));
                return;
            };
            host.push_output(&describe(&cap, entity.handle, &entity.record));
        }
        Err(e) => {
            host.push_error(&format!("EL_INFO: {e}"));
            let summary = drawing_summary(&cap);
            if summary.is_some() {
                host.push_info(&summary.unwrap());
            }
        }
    }
}

/// Start the interactive object-pick flow.
fn start_interactive(host: &mut dyn HostApi) {
    // Capture now: the callback cannot touch the document.
    let capture = Capture::take(host);
    let count = capture.entities.len();
    if count == 0 {
        host.push_error("EL_INFO: this drawing has no electrical entities yet");
        return;
    }
    host.push_info(&format!(
        "EL_INFO: pick an electrical entity ({count} in this drawing)"
    ));
    host.start_interactive(Box::new(InfoCmd {
        capture,
        reported: false,
    }));
}

/// Interactive inspector: click once, print, release the command line.
struct InfoCmd {
    capture: Capture,
    reported: bool,
}

impl InteractiveCommand for InfoCmd {
    fn prompt(&self) -> String {
        "EL_INFO: pick an electrical entity".to_string()
    }

    fn needs_object_pick(&self) -> bool {
        true
    }

    fn on_point(&mut self, _pt: [f64; 3]) -> CommandStep {
        // A point click means the pick missed every entity. Say so and finish,
        // rather than leaving the command line frozen waiting for a pick the
        // user may not realise it wants.
        if self.reported {
            return CommandStep::Done;
        }
        self.reported = true;
        state::emit(PendingLine::Error(
            "EL_INFO: that pick landed on empty space — pick a symbol body or its ref-des label"
                .to_string(),
        ));
        CommandStep::Done
    }

    fn on_object_pick(&mut self, handle: Handle, pt: [f64; 3]) -> CommandStep {
        // The host routes typed command-line input to an active interactive
        // command and tries to parse it as `x,y`. Returning `NeedPoint` here
        // would therefore keep the command line frozen and turn the user's next
        // keystroke into a confusing "not a Cartesian point" error. This is an
        // inspect-and-report tool, so it finishes on the pick.
        if self.reported {
            return CommandStep::Done;
        }
        self.reported = true;

        match self.capture.resolve_pick(handle, pt, 25.0) {
            Some(entity) => {
                state::set_last_target(entity.handle);
                let text = describe(&self.capture, entity.handle, &entity.record);
                for line in text.lines() {
                    state::emit(PendingLine::Output(line.to_string()));
                }
            }
            None => {
                // A miss is worth saying out loud: silently doing nothing reads
                // as a broken command.
                state::emit(PendingLine::Error(
                    "EL_INFO: that entity carries no electrical record — pick a symbol's body or its ref-des label"
                        .to_string(),
                ));
            }
        }
        CommandStep::Done
    }

    fn on_enter(&mut self) -> CommandStep {
        CommandStep::Cancel
    }
}

/// Render one entity's record, and for a component its resolved relationships.
fn describe(cap: &Capture, handle: u64, record: &el_core::codec::ElRecord) -> String {
    let mut out = String::new();
    let kind = record.kind().unwrap_or("?");
    out.push_str(&format!("EL_INFO {kind} @ 0x{handle:X}"));

    match record.clone().decode_signal() {
        Ok(s) if kind == el_core::model::KIND_SIGNAL => {
            out.push_str(&format!("\n  signal   : {}", or_dash(&s.name)));
            out.push_str(&format!("\n  direction: {}", s.direction.as_str()));
            out.push_str(&format!(
                "\n  anchors  : {}",
                match s.at_to_end {
                    false => "from end",
                    true => "to end",
                }
            ));
            // The pairing note lives in the project register, which needs the
            // host to find — EL_SIGNAL LIST shows it.
            out.push_str(&format!("\n  wire id  : {}", or_dash(&s.wire_el_id)));
        }
        _ => {}
    }

    match record.clone().decode_component() {
        Ok(c) if kind == KIND_COMPONENT => {
            out.push_str(&format!("\n  tag      : {}", or_dash(&c.tag)));
            out.push_str(&format!("\n  symbol   : {}", or_dash(&c.symbol)));
            out.push_str(&format!("\n  family   : {}", or_dash(&c.family)));
            let role = if c.is_child { "child" } else { "parent" };
            out.push_str(&format!(
                "\n  role     : {role}{}",
                if c.is_child && !c.parent_tag.is_empty() {
                    format!(" of {}", c.parent_tag)
                } else {
                    String::new()
                }
            ));
            out.push_str(&format!("\n  catalog  : {}/{}", or_dash(&c.mfg), or_dash(&c.cat)));
            out.push_str(&format!(
                "\n  ratings  : {}",
                or_dash(&c.ratings.join(", "))
            ));
            out.push_str(&format!("\n  desc     : {}", or_dash(&c.description)));
            out.push_str(&format!(
                "\n  inst/loc : {}/{}",
                or_dash(&c.installation),
                or_dash(&c.location)
            ));
            out.push_str(&format!(
                "\n  pins     : {}",
                or_dash(&c.pins.join(", "))
            ));

            // Cross-references are the reason a component record exists: a coil
            // without its contacts listed is unusable to a drafter.
            let children = cap.index.children_of(&c.tag);
            if !c.is_child && !children.is_empty() {
                let refs: Vec<String> = children.iter().map(|x| x.rec.tag.clone()).collect();
                out.push_str(&format!("\n  contacts : {}", refs.join(", ")));
            }
            if c.is_child && !c.parent_tag.is_empty() && cap.index.component_by_tag(&c.parent_tag).is_none() {
                out.push_str(&format!(
                    "\n  WARNING  : parent {} is not placed in this drawing",
                    c.parent_tag
                ));
            }
        }
        _ => match record.clone().decode_wire() {
            Ok(w) => {
                out.push_str(&format!("\n  number   : {}", or_dash(&w.wire_ref)));
                out.push_str(&format!(
                    "\n  from     : {}{}",
                    or_dash(&w.from_id),
                    pin_suffix(&w.from_pin)
                ));
                out.push_str(&format!(
                    "\n  to       : {}{}",
                    or_dash(&w.to_id),
                    pin_suffix(&w.to_pin)
                ));
                out.push_str(&format!("\n  type     : {}", or_dash(&w.wire_type)));
                out.push_str(&format!("{}", {
                    let g = format!("{}/{}", w.gauge, w.color);
                    format!("\n  gauge/col: {}", g.trim_matches('/'))
                }));
                out.push_str(&format!("\n  ntype    : {}", w.number_type.as_str()));
                if !w.is_connected() {
                    out.push_str("\n  WARNING  : one end is unconnected");
                }
            }
            Err(_) => out.push_str(&format!("\n  (record of kind {kind} has no reader in this build)")),
        },
    }
    out
}

fn pin_suffix(pin: &str) -> String {
    if pin.is_empty() {
        String::new()
    } else {
        format!(".{pin}")
    }
}

fn or_dash(s: &str) -> String {
    if s.trim().is_empty() {
        "—".to_string()
    } else {
        s.to_string()
    }
}

/// A one-line inventory, offered when a lookup fails so the user learns what
/// *is* in the drawing instead of just that their token was wrong.
fn drawing_summary(cap: &Capture) -> Option<String> {
    // Gate on the records this actually summarizes, so "nothing to say" and
    // "nothing to summarize" are the same condition.
    if cap.index.components.is_empty() && cap.index.wires.is_empty() {
        return None;
    }
    // Read tags from the index, not from the capture's entity map: the index is
    // where every other line of this summary comes from, and a tag is a
    // property of the record, not of the drawn carrier.
    let mut tags: Vec<String> = cap
        .index
        .components
        .values()
        .map(|c| c.rec.tag.clone())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort_by(|a, b| el_core::refdes_cmp(a, b));
    let mut wires: Vec<String> = cap
        .index
        .wires
        .values()
        .map(|w| w.rec.wire_ref.clone())
        .filter(|w| !w.is_empty())
        .collect();
    wires.sort_by(|a, b| el_core::refdes_cmp(a, b));
    let mut s = format!(
        "this drawing: {} component(s), {} wire(s)",
        cap.index.components.len(),
        cap.index.wires.len()
    );
    if !tags.is_empty() {
        s.push_str(&format!("\n  tags : {}", tags.join(", ")));
    }
    if !wires.is_empty() {
        s.push_str(&format!("\n  wires: {}", wires.join(", ")));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use el_core::codec::ElRecord;
    use el_core::model::{ElComponentRec, ElWireRec};

    fn component(handle: u64, tag: &str) -> (u64, ElRecord) {
        let mut c = ElComponentRec::new(&format!("id{handle}"), "relay-coil", "K");
        c.tag = tag.to_string();
        (handle, ElRecord::encode_component(&c))
    }

    fn cap_of(pairs: Vec<(u64, ElRecord)>) -> Capture {
        Capture {
            index: el_core::ElIndex::build(pairs),
            ..Default::default()
        }
    }

    #[test]
    fn describe_component_names_its_contacts() {
        let mut child = ElComponentRec::new("idc", "contact-no", "K");
        child.tag = "K1".to_string();
        child.is_child = true;
        child.parent_tag = "K1".to_string();
        let pairs = vec![component(1, "K1"), (2, ElRecord::encode_component(&child))];
        let cap = cap_of(pairs);
        let rec = cap.index.component_of_handle(1).unwrap().rec.clone();

        let text = describe(&cap, 1, &ElRecord::encode_component(&rec));
        assert!(text.contains("K1"), "{text}");
        assert!(text.contains("contacts : K1"), "{text}");
        assert!(text.contains("parent"), "{text}");
    }

    #[test]
    fn describe_flags_an_unplaced_parent() {
        let mut child = ElComponentRec::new("idc", "contact-no", "K");
        child.tag = "K1".to_string();
        child.is_child = true;
        child.parent_tag = "K9".to_string();
        let cap = cap_of(vec![(2, ElRecord::encode_component(&child))]);
        let text = describe(&cap, 2, &ElRecord::encode_component(&child));
        assert!(text.contains("parent K9 is not placed"), "{text}");
    }

    #[test]
    fn describe_wire_shows_both_ends_and_flags_a_dangling_one() {
        let mut w = ElWireRec::new("idw", "K1", "");
        w.wire_ref = "W1".to_string();
        w.from_pin = "A1".to_string();
        let cap = cap_of(vec![(3, ElRecord::encode_wire(&w))]);
        let text = describe(&cap, 3, &ElRecord::encode_wire(&w));
        assert!(text.contains("W1"), "{text}");
        assert!(text.contains("K1.A1"), "pin must render against its node: {text}");
        assert!(text.contains("unconnected"), "{text}");
    }

    #[test]
    fn describe_uses_a_dash_for_blank_fields() {
        let c = ElComponentRec::new("id", "sym", "K");
        let cap = cap_of(vec![(1, ElRecord::encode_component(&c))]);
        let text = describe(&cap, 1, &ElRecord::encode_component(&c));
        assert!(text.contains("tag      : —"), "{text}");
    }

    #[test]
    fn drawing_summary_is_absent_for_an_empty_drawing() {
        let cap = Capture::default();
        assert!(drawing_summary(&cap).is_none());
    }

    #[test]
    fn drawing_summary_lists_what_exists() {
        let cap = cap_of(vec![component(1, "K1"), component(2, "K2")]);
        let s = drawing_summary(&cap).unwrap();
        assert!(s.contains("2 component(s)"), "{s}");
        assert!(s.contains("K1"), "{s}");
    }
}
