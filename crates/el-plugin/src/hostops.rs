//! Token classification, picking, and dispatch-time capture.
//!
//! # Why capture exists
//!
//! During an interactive command the host services nested plugin requests with
//! a `NullHost`: `document()`/`document_mut()` **panic**, `add_entity` is a
//! silent no-op returning a null handle, `write_record` fails silently, and
//! `read_record` is always `None`. A single document call from inside
//! `on_point` therefore takes down the GUI thread.
//!
//! So the pattern every interactive command here follows is: **capture
//! everything at dispatch** (a real host context), keep the command's own
//! placements in local state, commit entities through `CommandStep::Commit`,
//! and defer anything cosmetic to the next dispatch-context command.

use std::collections::HashMap;

/// Index-tolerant entity lookup for the **v2026.41 host** (codec 0.6.0):
/// `CadDocument::get_entity` misses entities that arrived through the
/// plugin-request path in an earlier dispatch (its new handle->slot index is
/// not maintained across them), while `entities()` iteration sees
/// everything. Same host-generation trap the wiring-harness plugin hit
/// first; identical mitigation.
pub trait DocEntityExt {
    /// Owned entity lookup - the drop-in replacement for
    /// `CadDocument::get_entity` inside the plugin.
    fn get_entity_owned(&self, handle: Handle) -> Option<EntityType>;
}

impl DocEntityExt for CadDocument {
    fn get_entity_owned(&self, handle: Handle) -> Option<EntityType> {
        // Deliberately NO fast path through `CadDocument::get_entity`: on
        // host v2026.41 (codec 0.6.0) its handle→slot index goes stale across
        // dispatches and the lookup panics on an out-of-range slot — and the
        // plugin process runs with panic=abort, so the panic silently kills
        // the process (observed live in EL_WIRENO). The linear walk over
        // `entities()` is the only safe read; harness-scale documents make
        // the O(n) cost a non-issue.
        self.entities()
            .find(|e| e.common().handle == handle)
            .cloned()
    }
}


use ocs_plugin_api::host::codec::{CadDocument, EntityType, Handle};
use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::{KIND_COMPONENT, KIND_WIRE};
use el_core::ElIndex;

/// A command-line token, classified the way a CAD command line must.
#[allow(dead_code)] // consumed by the Phase 1 placement commands
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Coord([f64; 3]),
    Handle(Handle),
    Word(String),
}

impl Tok {
    /// Classify one token against the current document.
    ///
    /// A handle **must** carry an explicit `0x`/`0X` prefix *and* resolve in the
    /// document. That rule is deliberate: standard electrical reference
    /// designators are valid hexadecimal strings — `C1`, `D1`, `B1`, `F1`, and
    /// every `E`-prefixed tag — so without it a bare `C1` would be read as
    /// handle 0xC1 and silently address the wrong entity.
    #[allow(dead_code)] // consumed by the Phase 1 placement commands
    pub fn classify(tok: &str, doc: &CadDocument) -> Tok {
        if let Some(c) = parse_coord(tok) {
            return Tok::Coord(c);
        }
        if let Some(h) = parse_handle(tok) {
            if doc.get_entity_owned(h).is_some() {
                return Tok::Handle(h);
            }
        }
        Tok::Word(tok.to_string())
    }
}

/// Parse `x,y[,z]` as an absolute coordinate (z defaults to 0). A fourth field
/// is rejected rather than ignored, so a typo is visible.
#[allow(dead_code)] // consumed by the Phase 1 placement commands
pub fn parse_coord(tok: &str) -> Option<[f64; 3]> {
    let parts: Vec<&str> = tok.split(',').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let x = parts[0].trim().parse::<f64>().ok()?;
    let y = parts[1].trim().parse::<f64>().ok()?;
    let z = match parts.get(2) {
        Some(s) => s.trim().parse::<f64>().ok()?,
        None => 0.0,
    };
    Some([x, y, z])
}

/// Parse a `0x`-prefixed entity handle. See [`Tok::classify`] for why the
/// prefix is mandatory.
pub fn parse_handle(tok: &str) -> Option<Handle> {
    if !(tok.starts_with("0x") || tok.starts_with("0X")) {
        return None;
    }
    let v = u64::from_str_radix(&tok[2..], 16).ok()?;
    if v == 0 {
        return None;
    }
    Some(Handle::new(v))
}

/// One entity's record plus its drawn anchor, captured at dispatch.
#[derive(Debug, Clone)]
pub struct CapturedEntity {
    pub handle: u64,
    pub record: ElRecord,
    pub anchor: [f64; 3],
}

/// A resolved electrical target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Component(u64),
    Wire(u64),
}

impl Target {
    pub fn handle(self) -> u64 {
        match self {
            Target::Component(h) | Target::Wire(h) => h,
        }
    }
}

/// Everything an interactive command needs, taken while the host is real.
#[derive(Debug, Clone, Default)]
pub struct Capture {
    /// Carriers by entity handle. Labels carry no record, so they are absent —
    /// which is exactly what makes `resolve_near` pick carriers and not their
    /// companion text.
    pub entities: HashMap<u64, CapturedEntity>,
    pub index: ElIndex,
    /// Ref-des label handle → the carrier it annotates.
    pub label_to_carrier: HashMap<u64, u64>,
}

impl Capture {
    pub fn take(host: &dyn HostApi) -> Capture {
        let doc = host.document();
        let mut entities = HashMap::new();

        for e in doc.entities() {
            let handle = e.common().handle.value();
            let Some(record) = crate::codec::record_of(e) else {
                continue;
            };
            let anchor = crate::geometry::carrier_anchor(e).unwrap_or([0.0, 0.0, 0.0]);
            entities.insert(
                handle,
                CapturedEntity {
                    handle,
                    record,
                    anchor,
                },
            );
        }

        // Second pass: a label whose text matches a component's tag annotates
        // that component. Labels are plain MText with no record of their own,
        // so proximity alone would be ambiguous in a dense drawing.
        let mut tag_to_carrier: HashMap<String, u64> = HashMap::new();
        for c in entities.values() {
            if let Ok(rec) = c.record.decode_component() {
                if !rec.tag.is_empty() {
                    tag_to_carrier.insert(rec.tag.clone(), c.handle);
                }
            }
        }
        let mut label_to_carrier = HashMap::new();
        for e in doc.entities() {
            let EntityType::MText(m) = e else { continue };
            let key = m.value.trim().to_ascii_uppercase();
            if key.is_empty() {
                continue;
            }
            if let Some(&carrier) = tag_to_carrier.get(&key) {
                label_to_carrier.insert(e.common().handle.value(), carrier);
            }
        }

        Capture {
            index: crate::codec::build_index(doc),
            entities,
            label_to_carrier,
        }
    }

    /// The carrier handle a picked entity belongs to: itself, or its carrier
    /// when a ref-des label was clicked.
    pub fn carrier_of(&self, handle: Handle) -> Option<u64> {
        let h = handle.value();
        if self.entities.contains_key(&h) {
            return Some(h);
        }
        self.label_to_carrier.get(&h).copied()
    }

    pub fn get(&self, handle: u64) -> Option<&CapturedEntity> {
        self.entities.get(&handle)
    }

    /// The nearest carrier within `max_dist` — the fallback for a click that
    /// landed on the symbol's outline rather than its anchor.
    pub fn resolve_near(&self, pt: [f64; 3], max_dist: f64) -> Option<&CapturedEntity> {
        self.entities
            .values()
            .map(|c| {
                let dx = c.anchor[0] - pt[0];
                let dy = c.anchor[1] - pt[1];
                (c, (dx * dx + dy * dy).sqrt())
            })
            .filter(|(_, d)| *d <= max_dist)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(c, _)| c)
    }

    /// Resolve a pick: the entity itself, its carrier, or the nearest carrier
    /// to the clicked point.
    pub fn resolve_pick(&self, handle: Handle, pt: [f64; 3], max_dist: f64) -> Option<&CapturedEntity> {
        self.carrier_of(handle)
            .and_then(|h| self.get(h))
            .or_else(|| self.resolve_near(pt, max_dist))
    }

    /// Resolve one reference: a tag, a wire number, or an explicit handle.
    ///
    /// A tag wins over a handle-shaped reading (see [`Tok::classify`]), so
    /// `EL_INFO C1` inspects the component tagged `C1`, never handle 0xC1.
    pub fn resolve_token(&self, token: &str) -> Result<Target, String> {
        if let Some(c) = self.index.component_by_tag(token) {
            return Ok(Target::Component(c.primary()));
        }
        if let Some(w) = self.index.wire_by_ref(token) {
            return Ok(Target::Wire(w.handle));
        }
        if let Some(h) = parse_handle(token) {
            return match self.get(h.value()) {
                Some(c) => match c.record.kind() {
                    Some(KIND_COMPONENT) => Ok(Target::Component(h.value())),
                    Some(KIND_WIRE) => Ok(Target::Wire(h.value())),
                    Some(other) => Err(format!("{token} is a {other} record, not a component or wire")),
                    None => Err(format!("{token} has no record kind")),
                },
                None => Err(format!("no electrical entity at handle {token}")),
            };
        }
        Err(format!(
            "no component or wire matches {token:?} — expected a tag (K1), a wire number (W1), or a 0x-prefixed handle"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_forms_parse() {
        assert_eq!(parse_coord("1,2"), Some([1.0, 2.0, 0.0]));
        assert_eq!(parse_coord("1.5,-2.5,3"), Some([1.5, -2.5, 3.0]));
        assert_eq!(parse_coord("1 2"), None);
        assert_eq!(parse_coord("1,2,3,4"), None, "a fourth field is a typo, not a z");
        assert_eq!(parse_coord("a,b"), None);
    }

    #[test]
    fn handles_require_the_0x_prefix() {
        // The whole point: `C1` is a reference designator, not hex 0xC1.
        assert_eq!(parse_handle("C1"), None);
        assert_eq!(parse_handle("D1"), None);
        assert_eq!(parse_handle("0x2E"), Some(Handle::new(0x2E)));
        assert_eq!(parse_handle("0X2e"), Some(Handle::new(0x2E)));
        assert_eq!(parse_handle("0x0"), None);
        assert_eq!(parse_handle("0xzz"), None);
    }

    #[test]
    fn classify_prefers_a_coordinate_over_everything() {
        let doc = CadDocument::default();
        assert_eq!(Tok::classify("5,5", &doc), Tok::Coord([5.0, 5.0, 0.0]));
    }

    #[test]
    fn classify_treats_a_bare_tag_as_a_word() {
        let doc = CadDocument::default();
        assert_eq!(Tok::classify("C1", &doc), Tok::Word("C1".to_string()));
    }

    #[test]
    fn resolve_token_reports_an_actionable_error() {
        let cap = Capture::default();
        let err = cap.resolve_token("K1").unwrap_err();
        assert!(err.contains("K1"), "error must name what was typed: {err}");
        assert!(err.contains("tag"), "error must say what was expected: {err}");
    }

    #[test]
    fn resolve_near_picks_the_closest_carrier_within_range() {
        let mut cap = Capture::default();
        for (h, x) in [(1u64, 0.0f64), (2, 100.0)] {
            cap.entities.insert(
                h,
                CapturedEntity {
                    handle: h,
                    record: ElRecord::default(),
                    anchor: [x, 0.0, 0.0],
                },
            );
        }
        assert_eq!(cap.resolve_near([1.0, 0.0, 0.0], 25.0).unwrap().handle, 1);
        assert!(cap.resolve_near([50.0, 0.0, 0.0], 25.0).is_none());
        assert_eq!(cap.resolve_near([99.0, 0.0, 0.0], 25.0).unwrap().handle, 2);
    }

    #[test]
    fn resolve_near_breaks_ties_deterministically() {
        // Two carriers equidistant from the click must not make the pick depend
        // on hash iteration order.
        let mut cap = Capture::default();
        for h in [1u64, 2] {
            cap.entities.insert(
                h,
                CapturedEntity {
                    handle: h,
                    record: ElRecord::default(),
                    anchor: [0.0, 0.0, 0.0],
                },
            );
        }
        let first = cap.resolve_near([0.0, 0.0, 0.0], 1.0).unwrap().handle;
        for _ in 0..20 {
            assert_eq!(cap.resolve_near([0.0, 0.0, 0.0], 1.0).unwrap().handle, first);
        }
    }

    #[test]
    fn carrier_of_follows_a_label_to_its_carrier() {
        let mut cap = Capture::default();
        cap.entities.insert(
            7,
            CapturedEntity {
                handle: 7,
                record: ElRecord::default(),
                anchor: [0.0, 0.0, 0.0],
            },
        );
        cap.label_to_carrier.insert(9, 7);
        assert_eq!(cap.carrier_of(Handle::new(7)), Some(7));
        assert_eq!(cap.carrier_of(Handle::new(9)), Some(7));
        assert_eq!(cap.carrier_of(Handle::new(11)), None);
    }
}
