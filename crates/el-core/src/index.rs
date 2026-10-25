//! Resolved connectivity index over a document's electrical records.
//!
//! The index is rebuilt from scratch whenever a command needs it (the host
//! hands a plugin a document snapshot, not a live database), so it is a pure
//! function of the `(handle, ElRecord)` pairs it is built from. Geometry is
//! deliberately absent: connectivity here is *stated* — records name each
//! other — while the geometric rules (does a drawn wire actually touch the pin
//! it claims) belong to the plugin crate, which can see entity positions.

use std::collections::{BTreeMap, BTreeSet};

use crate::codec::{DecodeError, ElRecord};
use crate::model::*;

/// One indexed wire: its record plus the handle carrying it.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedWire {
    pub handle: u64,
    pub rec: ElWireRec,
}

/// One indexed device.
///
/// Keyed by `el_id`, not by handle, because a device is drawn as many entities —
/// a coil is a rectangle, two leads and a designator — and every part carries the
/// same record. Keying by handle would make a four-part coil look like four
/// devices, which is not a cosmetic problem: a retag would allocate it four tags
/// and the last write would win, and a delete would remove one part and leave
/// three orphans.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedComponent {
    pub el_id: String,
    pub rec: ElComponentRec,
    /// Every drawn part, in scan order.
    pub handles: Vec<u64>,
}

impl IndexedComponent {
    /// The part that stands for the device in a report or a message.
    pub fn primary(&self) -> u64 {
        self.handles.first().copied().unwrap_or(0)
    }

    pub fn parts(&self) -> &[u64] {
        &self.handles
    }
}

/// One indexed terminal block.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedTerminal {
    pub el_id: String,
    pub rec: TerminalRec,
    pub handles: Vec<u64>,
}

impl IndexedTerminal {
    pub fn primary(&self) -> u64 {
        self.handles.first().copied().unwrap_or(0)
    }
}

/// One indexed cable marker.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedCable {
    pub el_id: String,
    pub rec: CableRec,
    pub handles: Vec<u64>,
}

impl IndexedCable {
    pub fn primary(&self) -> u64 {
        self.handles.first().copied().unwrap_or(0)
    }
}

/// One indexed connector.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedConnector {
    pub el_id: String,
    pub rec: ConnectorRec,
    pub handles: Vec<u64>,
}

impl IndexedConnector {
    pub fn primary(&self) -> u64 {
        self.handles.first().copied().unwrap_or(0)
    }
}

/// One indexed signal arrow.
///
/// Like a component: drawn as several primitives sharing one record, folded
/// by `el_id`, deletable as a unit no matter which part a user picked.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedSignal {
    pub el_id: String,
    pub rec: SignalRec,
    pub handles: Vec<u64>,
}

impl IndexedSignal {
    pub fn primary(&self) -> u64 {
        self.handles.first().copied().unwrap_or(0)
    }
}

/// One indexed jumper — a single polyline carrying its record, folded like
/// every other object for the day it draws as more.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedJumper {
    pub el_id: String,
    pub rec: JumperRec,
    pub handles: Vec<u64>,
}

impl IndexedJumper {
    pub fn primary(&self) -> u64 {
        self.handles.first().copied().unwrap_or(0)
    }
}

/// The drawing's title block — one per drawing, folded like everything else.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedTitleBlock {
    pub el_id: String,
    pub rec: TitleBlockRec,
    pub handles: Vec<u64>,
}

/// One PLC module, folded like everything else.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedPlc {
    pub el_id: String,
    pub rec: PlcRec,
    pub handles: Vec<u64>,
}

impl IndexedPlc {
    pub fn primary(&self) -> u64 {
        self.handles.first().copied().unwrap_or(0)
    }
}

/// One panel footprint, folded like everything else.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedFootprint {
    pub el_id: String,
    pub rec: FootprintRec,
    pub handles: Vec<u64>,
}

impl IndexedFootprint {
    pub fn primary(&self) -> u64 {
        self.handles.first().copied().unwrap_or(0)
    }
}

/// One drawn strip diagram — folded like everything else, one plot per strip.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedStripPlot {
    pub el_id: String,
    pub rec: StripPlotRec,
    pub handles: Vec<u64>,
}

impl IndexedStripPlot {
    pub fn primary(&self) -> u64 {
        self.handles.first().copied().unwrap_or(0)
    }
}

/// One indexed ladder.
///
/// Keyed by `el_id`, not by handle, because a ladder is drawn as many entities
/// — bus verticals, rungs, a reference per rung — and all of them carry the
/// same record. `handles` is every drawn part, so the ladder can be redrawn or
/// deleted as a unit regardless of which part a user picked.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedLadder {
    pub el_id: String,
    pub rec: LadderRec,
    pub handles: Vec<u64>,
}

impl IndexedLadder {
    /// The part that should be reported as the ladder's own entity.
    pub fn primary(&self) -> Option<u64> {
        self.handles.first().copied()
    }
}

/// A resolved view of every electrical record in one drawing.
#[derive(Debug, Clone, Default)]
pub struct ElIndex {
    pub settings: Option<ElSettingsRec>,
    /// The drawing's wire types; `None` means the drawing has none and the
    /// built-in defaults apply.
    pub wire_types: Option<WireTypeTable>,
    /// Ladders by `el_id`.
    pub ladders: BTreeMap<String, IndexedLadder>,
    /// Wires by their **primary run's handle**. A gapped wire is drawn as
    /// several runs sharing one record; only the first run's row appears here,
    /// the rest are geometry under `handles_for_el_id(rec.el_id)`.
    pub wires: BTreeMap<u64, IndexedWire>,
    /// Devices by `el_id`.
    pub components: BTreeMap<String, IndexedComponent>,
    /// Terminal blocks by `el_id`.
    pub terminals: BTreeMap<String, IndexedTerminal>,
    /// Cable markers by `el_id`.
    pub cables: BTreeMap<String, IndexedCable>,
    /// Connectors by `el_id`.
    pub connectors: BTreeMap<String, IndexedConnector>,
    /// Signal arrows by `el_id`.
    pub signals: BTreeMap<String, IndexedSignal>,
    /// Terminal jumpers by `el_id`.
    pub jumpers: BTreeMap<String, IndexedJumper>,
    /// Drawn strip diagrams by `el_id`.
    pub strip_plots: BTreeMap<String, IndexedStripPlot>,
    /// Panel footprints by `el_id`.
    pub footprints: BTreeMap<String, IndexedFootprint>,
    /// PLC modules by `el_id`.
    pub plcs: BTreeMap<String, IndexedPlc>,
    /// The title block, if the drawing has one.
    pub title_block: Option<IndexedTitleBlock>,

    /// Handles whose record failed to decode, with the reason. Surfaced as an
    /// audit finding rather than swallowed: a corrupt record is a real defect.
    pub decode_errors: Vec<(u64, DecodeError)>,

    /// Kinds present in the drawing that this build does not know about. Not an
    /// error (forward compatibility), but worth reporting.
    pub unknown_kinds: BTreeMap<String, usize>,

    // derived lookups
    component_by_tag: BTreeMap<String, String>,
    /// Which device each drawn part belongs to.
    handle_to_component: BTreeMap<u64, String>,
    handle_to_terminal: BTreeMap<u64, String>,
    handle_to_cable: BTreeMap<u64, String>,
    handle_to_connector: BTreeMap<u64, String>,
    handle_to_signal: BTreeMap<u64, String>,
    handle_to_jumper: BTreeMap<u64, String>,
    handle_to_strip_plot: BTreeMap<u64, String>,
    handle_to_footprint: BTreeMap<u64, String>,
    handle_to_plc: BTreeMap<u64, String>,
    handle_to_titleblock: BTreeMap<u64, String>,
    cable_parent_by_tag: BTreeMap<String, String>,
    wire_by_ref: BTreeMap<String, u64>,
    /// Which wire each drawn run belongs to. A gapped wire is several polyline
    /// runs sharing one `el_id`, so *any* of them must resolve to the whole
    /// wire — the same treatment components and ladders get.
    handle_to_wire: BTreeMap<u64, String>,
    /// The primary run of each wire, by `el_id`.
    wire_by_el_id: BTreeMap<String, u64>,
    /// Every drawn part of each object, by `el_id`. An object may be several
    /// entities — a ladder is a frame, not a line — so identity is the `el_id`
    /// and geometry is however many entities carry it.
    el_id_to_handles: BTreeMap<String, Vec<u64>>,
    duplicate_tags: Vec<String>,
    duplicate_wire_refs: Vec<String>,
}

impl ElIndex {
    /// Build from owned `(handle, record)` pairs.
    pub fn build<I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (u64, ElRecord)>,
    {
        let mut idx = ElIndex::default();
        for (handle, rec) in pairs {
            let Some(kind) = rec.kind().map(str::to_string) else {
                // A record without a KIND cannot be routed; treat it as corrupt
                // rather than silently dropping it.
                idx.decode_errors
                    .push((handle, DecodeError("missing KIND key".to_string())));
                continue;
            };
            match kind.as_str() {
                KIND_SETTINGS => match rec.decode_settings() {
                    Ok(s) => idx.settings = Some(s),
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_WIRETYPES => match rec.decode_wiretypes() {
                    Ok(t) => idx.wire_types = Some(t),
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_LADDER => match rec.decode_ladder() {
                    Ok(l) => {
                        idx.el_id_to_handles
                            .entry(l.el_id.clone())
                            .or_default()
                            .push(handle);
                        // The first part's record defines the ladder; every part
                        // carries an identical copy, so the rest are only
                        // geometry.
                        idx.ladders
                            .entry(l.el_id.clone())
                            .or_insert_with(|| IndexedLadder {
                                el_id: l.el_id.clone(),
                                rec: l,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_WIRE => match rec.decode_wire() {
                    Ok(w) => {
                        // A gapped wire is drawn as several runs carrying the
                        // same record. The first run's row is the wire; the
                        // rest are geometry, reachable through
                        // `handles_for_el_id` / `wire_of_handle` exactly like
                        // a ladder's or a component's parts.
                        let first = !idx.el_id_to_handles.contains_key(&w.el_id);
                        idx.el_id_to_handles
                            .entry(w.el_id.clone())
                            .or_default()
                            .push(handle);
                        idx.handle_to_wire.insert(handle, w.el_id.clone());
                        if first {
                            idx.wires.insert(handle, IndexedWire { handle, rec: w });
                        }
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_COMPONENT => match rec.decode_component() {
                    Ok(c) => {
                        idx.el_id_to_handles
                            .entry(c.el_id.clone())
                            .or_default()
                            .push(handle);
                        idx.handle_to_component.insert(handle, c.el_id.clone());
                        // The first part's record defines the device; every part
                        // carries an identical copy, so the rest are geometry.
                        idx.components
                            .entry(c.el_id.clone())
                            .or_insert_with(|| IndexedComponent {
                                el_id: c.el_id.clone(),
                                rec: c,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_TERMINAL => match rec.decode_terminal() {
                    Ok(t) => {
                        idx.el_id_to_handles.entry(t.el_id.clone()).or_default().push(handle);
                        idx.handle_to_terminal.insert(handle, t.el_id.clone());
                        idx.terminals
                            .entry(t.el_id.clone())
                            .or_insert_with(|| IndexedTerminal {
                                el_id: t.el_id.clone(),
                                rec: t,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_CABLE => match rec.decode_cable() {
                    Ok(c) => {
                        idx.el_id_to_handles.entry(c.el_id.clone()).or_default().push(handle);
                        idx.handle_to_cable.insert(handle, c.el_id.clone());
                        idx.cables
                            .entry(c.el_id.clone())
                            .or_insert_with(|| IndexedCable {
                                el_id: c.el_id.clone(),
                                rec: c,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_CONNECTOR => match rec.decode_connector() {
                    Ok(x) => {
                        idx.el_id_to_handles.entry(x.el_id.clone()).or_default().push(handle);
                        idx.handle_to_connector.insert(handle, x.el_id.clone());
                        idx.connectors
                            .entry(x.el_id.clone())
                            .or_insert_with(|| IndexedConnector {
                                el_id: x.el_id.clone(),
                                rec: x,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_SIGNAL => match rec.decode_signal() {
                    Ok(s) => {
                        idx.el_id_to_handles.entry(s.el_id.clone()).or_default().push(handle);
                        idx.handle_to_signal.insert(handle, s.el_id.clone());
                        idx.signals
                            .entry(s.el_id.clone())
                            .or_insert_with(|| IndexedSignal {
                                el_id: s.el_id.clone(),
                                rec: s,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_JUMPER => match rec.decode_jumper() {
                    Ok(j) => {
                        idx.el_id_to_handles.entry(j.el_id.clone()).or_default().push(handle);
                        idx.handle_to_jumper.insert(handle, j.el_id.clone());
                        idx.jumpers
                            .entry(j.el_id.clone())
                            .or_insert_with(|| IndexedJumper {
                                el_id: j.el_id.clone(),
                                rec: j,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_STRIPPLOT => match rec.decode_stripplot() {
                    Ok(sp) => {
                        idx.el_id_to_handles.entry(sp.el_id.clone()).or_default().push(handle);
                        idx.handle_to_strip_plot.insert(handle, sp.el_id.clone());
                        idx.strip_plots
                            .entry(sp.el_id.clone())
                            .or_insert_with(|| IndexedStripPlot {
                                el_id: sp.el_id.clone(),
                                rec: sp,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_FOOTPRINT => match rec.decode_footprint() {
                    Ok(f) => {
                        idx.el_id_to_handles.entry(f.el_id.clone()).or_default().push(handle);
                        idx.handle_to_footprint.insert(handle, f.el_id.clone());
                        idx.footprints
                            .entry(f.el_id.clone())
                            .or_insert_with(|| IndexedFootprint {
                                el_id: f.el_id.clone(),
                                rec: f,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_TITLEBLOCK => match rec.decode_titleblock() {
                    Ok(t) => {
                        idx.el_id_to_handles.entry(t.el_id.clone()).or_default().push(handle);
                        idx.handle_to_titleblock.insert(handle, t.el_id.clone());
                        let entry = idx.title_block.get_or_insert(IndexedTitleBlock {
                            el_id: t.el_id.clone(),
                            rec: t,
                            handles: Vec::new(),
                        });
                        entry.handles.push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                KIND_PLC => match rec.decode_plc() {
                    Ok(m) => {
                        idx.el_id_to_handles.entry(m.el_id.clone()).or_default().push(handle);
                        idx.handle_to_plc.insert(handle, m.el_id.clone());
                        idx.plcs
                            .entry(m.el_id.clone())
                            .or_insert_with(|| IndexedPlc {
                                el_id: m.el_id.clone(),
                                rec: m,
                                handles: Vec::new(),
                            })
                            .handles
                            .push(handle);
                    }
                    Err(e) => idx.decode_errors.push((handle, e)),
                },
                // Kinds this build does not implement yet are counted, not
                // rejected — a newer build's drawing must remain openable.
                other => *idx.unknown_kinds.entry(other.to_string()).or_insert(0) += 1,
            }
        }
        idx.rebuild_derived();
        idx
    }

    fn rebuild_derived(&mut self) {
        self.component_by_tag.clear();
        self.wire_by_ref.clear();
        self.wire_by_el_id.clear();
        self.duplicate_tags.clear();
        self.duplicate_wire_refs.clear();

        // The primary run of each wire, by identity.
        for w in self.wires.values() {
            self.wire_by_el_id
                .insert(w.rec.el_id.clone(), w.handle);
        }

        // Only *parents* can collide. A child is tagged with its parent's tag by
        // design, so counting it would report every correctly-linked contact as a
        // duplicate — the exact opposite of what this rule is for.
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for c in self.components.values() {
            if c.rec.tag.is_empty() || c.rec.is_child {
                continue;
            }
            if seen.insert(c.rec.tag.as_str(), c.el_id.as_str()).is_some() {
                self.duplicate_tags.push(c.rec.tag.clone());
            }
        }
        // The tag map answers "what is tagged K1?" for both roles, but a parent
        // wins: it is the device a lookup almost always means.
        for c in self.components.values() {
            if c.rec.tag.is_empty() || c.rec.is_child {
                continue;
            }
            self.component_by_tag
                .insert(c.rec.tag.clone(), c.el_id.clone());
        }
        for c in self.components.values() {
            if c.rec.tag.is_empty() || !c.rec.is_child {
                continue;
            }
            self.component_by_tag
                .entry(c.rec.tag.clone())
                .or_insert_with(|| c.el_id.clone());
        }

        let mut seen_w: BTreeMap<&str, u64> = BTreeMap::new();
        for (handle, w) in &self.wires {
            if w.rec.wire_ref.is_empty() {
                continue;
            }
            if seen_w.insert(w.rec.wire_ref.as_str(), *handle).is_some() {
                self.duplicate_wire_refs.push(w.rec.wire_ref.clone());
            }
            self.wire_by_ref.insert(w.rec.wire_ref.clone(), *handle);
        }

        self.duplicate_tags.sort();
        self.duplicate_tags.dedup();
        self.duplicate_wire_refs.sort();
        self.duplicate_wire_refs.dedup();

        // The parent marker of each cable owns its identity, so it is what the
        // catalogue, the BOM and every conductor lookup resolve through.
        self.cable_parent_by_tag.clear();
        for c in self.cables.values() {
            if c.rec.is_parent() && !c.rec.tag.is_empty() {
                self.cable_parent_by_tag
                    .insert(c.rec.tag.clone(), c.rec.el_id.clone());
            }
        }
    }

    // ── lookups ──────────────────────────────────────────────────────────

    pub fn duplicate_tags(&self) -> &[String] {
        &self.duplicate_tags
    }

    pub fn duplicate_wire_refs(&self) -> &[String] {
        &self.duplicate_wire_refs
    }

    pub fn component_by_tag(&self, tag: &str) -> Option<&IndexedComponent> {
        self.component_by_tag
            .get(tag)
            .and_then(|id| self.components.get(id))
    }

    /// The device a drawn part belongs to.
    ///
    /// A symbol is several entities, so a picked handle is a *part*, not a
    /// device — resolving it here is what lets a click on a coil's outline act on
    /// the coil.
    pub fn component_of_handle(&self, handle: u64) -> Option<&IndexedComponent> {
        self.handle_to_component
            .get(&handle)
            .and_then(|id| self.components.get(id))
    }

    /// The *parent* device carrying this tag.
    ///
    /// A child shares its parent's tag, so a bare tag lookup can land on the
    /// child itself — which would make a child look like its own parent and hide
    /// exactly the defect cross-referencing exists to find. Anything asking
    /// "where is the parent of this?" must come through here.
    pub fn parent_by_tag(&self, tag: &str) -> Option<&IndexedComponent> {
        self.components
            .values()
            .find(|c| !c.rec.is_child && c.rec.tag == tag)
    }

    pub fn wire_by_ref(&self, wire_ref: &str) -> Option<&IndexedWire> {
        self.wire_by_ref.get(wire_ref).and_then(|h| self.wires.get(h))
    }

    /// The wire a drawn run belongs to.
    ///
    /// A gapped wire is several polyline runs sharing one `el_id`; picking any
    /// of them resolves to the whole wire, the same way picking any part of a
    /// ladder resolves to the ladder.
    pub fn wire_of_handle(&self, handle: u64) -> Option<&IndexedWire> {
        let el = self.handle_to_wire.get(&handle)?;
        self.wire_by_el_id.get(el).and_then(|h| self.wires.get(h))
    }

    pub fn handle_for_el_id(&self, el_id: &str) -> Option<u64> {
        self.el_id_to_handles
            .get(el_id)
            .and_then(|v| v.first().copied())
    }

    /// Every drawn part of one object, in the order the scan found them.
    ///
    /// This is how a multi-entity object (a ladder) is deleted or redrawn as a
    /// unit: the record's `el_id` names the parts, and no handle is ever stored
    /// in the record — handles are renumbered by DWG round-trips, WBLOCK and
    /// copy/paste, so a stored handle is a dangling reference waiting to happen.
    pub fn handles_for_el_id(&self, el_id: &str) -> &[u64] {
        self.el_id_to_handles
            .get(el_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Every `el_id` in the drawing.
    pub fn el_ids(&self) -> Vec<&str> {
        self.el_id_to_handles.keys().map(|s| s.as_str()).collect()
    }

    /// Does any record claim this node name — a component tag or a wire ref?
    pub fn node_exists(&self, name: &str) -> bool {
        self.component_by_tag.contains_key(name) || self.wire_by_ref.contains_key(name)
    }

    /// Every wire with one end on `node`, by tag or `el_id`.
    pub fn wires_touching(&self, node: &str) -> Vec<&IndexedWire> {
        self.wires
            .values()
            .filter(|w| w.rec.from_id == node || w.rec.to_id == node)
            .collect()
    }

    // ── terminals, cables, connectors ────────────────────────────────────

    pub fn terminal_of_handle(&self, handle: u64) -> Option<&IndexedTerminal> {
        self.handle_to_terminal
            .get(&handle)
            .and_then(|id| self.terminals.get(id))
    }

    pub fn cable_of_handle(&self, handle: u64) -> Option<&IndexedCable> {
        self.handle_to_cable
            .get(&handle)
            .and_then(|id| self.cables.get(id))
    }

    pub fn connector_of_handle(&self, handle: u64) -> Option<&IndexedConnector> {
        self.handle_to_connector
            .get(&handle)
            .and_then(|id| self.connectors.get(id))
    }

    /// The signal arrow a drawn part belongs to.
    pub fn signal_of_handle(&self, handle: u64) -> Option<&IndexedSignal> {
        self.handle_to_signal
            .get(&handle)
            .and_then(|id| self.signals.get(id))
    }

    /// The jumper a drawn part belongs to.
    pub fn jumper_of_handle(&self, handle: u64) -> Option<&IndexedJumper> {
        self.handle_to_jumper
            .get(&handle)
            .and_then(|id| self.jumpers.get(id))
    }

    /// The title block a drawn part belongs to.
    pub fn titleblock_of_handle(&self, handle: u64) -> Option<&IndexedTitleBlock> {
        self.handle_to_titleblock
            .get(&handle)
            .and_then(|id| {
                self.title_block
                    .as_ref()
                    .filter(|t| &t.el_id == id)
            })
    }

    /// The PLC module a drawn part belongs to.
    pub fn plc_of_handle(&self, handle: u64) -> Option<&IndexedPlc> {
        self.handle_to_plc.get(&handle).and_then(|id| self.plcs.get(id))
    }

    /// The module carrying `tag`.
    pub fn plc_by_tag(&self, tag: &str) -> Option<&IndexedPlc> {
        self.plcs
            .values()
            .find(|m| m.rec.tag.eq_ignore_ascii_case(tag))
    }

    /// The footprint a drawn part belongs to.
    pub fn footprint_of_handle(&self, handle: u64) -> Option<&IndexedFootprint> {
        self.handle_to_footprint
            .get(&handle)
            .and_then(|id| self.footprints.get(id))
    }

    /// The footprint carrying `tag`, if the panel holds one.
    pub fn footprint_by_tag(&self, tag: &str) -> Option<&IndexedFootprint> {
        self.footprints
            .values()
            .find(|f| f.rec.tag.eq_ignore_ascii_case(tag))
    }

    /// The strip diagram a drawn part belongs to.
    pub fn strip_plot_of_handle(&self, handle: u64) -> Option<&IndexedStripPlot> {
        self.handle_to_strip_plot
            .get(&handle)
            .and_then(|id| self.strip_plots.get(id))
    }

    /// The marker that owns a cable's identity.
    pub fn cable_parent(&self, tag: &str) -> Option<&IndexedCable> {
        self.cable_parent_by_tag
            .get(tag)
            .and_then(|id| self.cables.get(id))
    }

    /// Every marker of one cable, parent first then by conductor.
    pub fn cable_markers(&self, tag: &str) -> Vec<&IndexedCable> {
        let mut v: Vec<&IndexedCable> = self
            .cables
            .values()
            .filter(|c| c.rec.tag == tag)
            .collect();
        v.sort_by_key(|c| (!c.rec.is_parent(), c.rec.conductor_index));
        v
    }

    /// The conductor indices a cable already uses — what a new marker must not
    /// take.
    pub fn conductors_used(&self, tag: &str) -> BTreeSet<i32> {
        self.cables
            .values()
            .filter(|c| c.rec.tag == tag)
            .map(|c| c.rec.conductor_index)
            .collect()
    }

    /// Terminals grouped by strip, each strip sorted by its own sequence then by
    /// designator — the order a terminal plan is read in.
    pub fn strips(&self) -> BTreeMap<String, Vec<&IndexedTerminal>> {
        let mut out: BTreeMap<String, Vec<&IndexedTerminal>> = BTreeMap::new();
        for t in self.terminals.values() {
            if t.rec.tag_strip.is_empty() {
                continue;
            }
            out.entry(t.rec.tag_strip.clone()).or_default().push(t);
        }
        for list in out.values_mut() {
            list.sort_by(|a, b| {
                a.rec
                    .strip_seq
                    .cmp(&b.rec.strip_seq)
                    .then_with(|| crate::refdes_cmp(&a.rec.designator(), &b.rec.designator()))
            });
        }
        out
    }

    /// The next free terminal number on a strip.
    ///
    /// Counts what is already there rather than taking the highest: a strip with
    /// terminals 1 and 3 wants 2, which is what keeps a terminal plan dense.
    pub fn next_terminal_number(&self, strip: &str) -> String {
        let used: BTreeSet<i32> = self
            .terminals
            .values()
            .filter(|t| t.rec.tag_strip == strip)
            .filter_map(|t| t.rec.number.trim().parse::<i32>().ok())
            .collect();
        let mut n = 1i32;
        while used.contains(&n) {
            n += 1;
        }
        n.to_string()
    }

    /// The next free conductor index on a cable.
    pub fn next_conductor(&self, tag: &str) -> i32 {
        let used = self.conductors_used(tag);
        let mut n = 1i32;
        while used.contains(&n) {
            n += 1;
        }
        n
    }

    /// The next free cable tag, by the drawing's own naming.
    pub fn next_cable_tag(&self) -> String {
        let mut max = 0i32;
        for c in self.cables.values() {
            if let Some(n) = c
                .rec
                .tag
                .trim_start_matches(|ch: char| !ch.is_ascii_digit())
                .parse::<i32>()
                .ok()
            {
                max = max.max(n);
            }
        }
        format!("W{}", max + 1)
    }

    /// Connectors carrying a reference designator, in tag order.
    pub fn connectors_ordered(&self) -> Vec<&IndexedConnector> {
        let mut v: Vec<&IndexedConnector> = self.connectors.values().collect();
        v.sort_by(|a, b| crate::refdes_cmp(&a.rec.ref_des, &b.rec.ref_des));
        v
    }

    /// The settings in effect, or the built-in defaults when the drawing has
    /// none yet. Never fails: a drawing without settings is a normal starting
    /// state, not an error.
    pub fn settings_or_default(&self) -> ElSettingsRec {
        self.settings.clone().unwrap_or_default()
    }

    /// The wire types in effect, falling back to the built-in set.
    ///
    /// The fallback matters: a drawing made before `EL_WIRETYPE` ever ran still
    /// has to resolve a wire to a type, or nothing could be numbered.
    pub fn wire_types_or_default(&self) -> WireTypeTable {
        self.wire_types.clone().unwrap_or_default()
    }

    // ── ladders and rungs ────────────────────────────────────────────────

    /// Ladders top-first, then left-first — the order a drafter reads them and
    /// the order a project-wide renumber should walk.
    pub fn ladders_ordered(&self) -> Vec<&IndexedLadder> {
        let mut v: Vec<&IndexedLadder> = self.ladders.values().collect();
        v.sort_by(|a, b| {
            b.rec
                .origin_y
                .partial_cmp(&a.rec.origin_y)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.rec
                        .origin_x
                        .partial_cmp(&b.rec.origin_x)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });
        v
    }

    pub fn ladder_by_el_id(&self, el_id: &str) -> Option<&IndexedLadder> {
        self.ladders.get(el_id)
    }

    /// The ladder a handle belongs to, whether the handle is the carrier or one
    /// of the frame's other parts.
    pub fn ladder_of_handle(&self, handle: u64) -> Option<&IndexedLadder> {
        self.ladders
            .values()
            .find(|l| l.handles.contains(&handle))
    }

    /// The line reference covering a point, if it is on a ladder.
    pub fn rung_reference_at(&self, pt: [f64; 2], tol: f64) -> Option<String> {
        let l = self.ladder_at(pt, tol)?;
        let index = l.rec.nearest_rung(pt[1], tol)?;
        Some(l.rec.rung_reference(index))
    }

    /// Every rung reference in the drawing, top to bottom.
    pub fn all_references(&self) -> Vec<String> {
        self.ladders_ordered()
            .iter()
            .flat_map(|l| l.rec.references())
            .collect()
    }

    /// The ladder a point belongs to, if any.
    ///
    /// Two ladders can share rung heights, so being inside a ladder's
    /// horizontal span beats merely having the right Y — otherwise a wire on
    /// the right-hand ladder would be numbered from the left-hand one.
    pub fn ladder_at(&self, pt: [f64; 2], tol: f64) -> Option<&IndexedLadder> {
        let mut best: Option<(f64, &IndexedLadder)> = None;
        for l in self.ladders_ordered() {
            let Some(index) = l.rec.nearest_rung(pt[1], tol) else {
                continue;
            };
            let inside_x = pt[0] >= l.rec.origin_x - tol && pt[0] <= l.rec.right_x() + tol;
            let dy = (l.rec.rung_y(index) - pt[1]).abs();
            let score = if inside_x { dy } else { dy + tol * 10.0 };
            match best {
                Some((s, _)) if s <= score => {}
                _ => best = Some((score, l)),
            }
        }
        best.map(|(_, l)| l)
    }

    /// Is this wire type name (or layer) numbered?
    ///
    /// An unknown type is treated as numbered: refusing to number a wire
    /// because its type is unrecognised would hide the real problem (a missing
    /// type) behind a silent skip.
    pub fn type_is_numbered(&self, name_or_layer: &str) -> bool {
        if name_or_layer.trim().is_empty() {
            return self
                .wire_types_or_default()
                .default_type()
                .map(|t| t.numbered)
                .unwrap_or(true);
        }
        let table = self.wire_types_or_default();
        table
            .get(name_or_layer)
            .or_else(|| table.by_layer(name_or_layer))
            .map(|t| t.numbered)
            .unwrap_or(true)
    }

    /// The layer a wire of this type should be drawn on, resolving a type name
    /// or passing an already-resolved layer through.
    pub fn wire_layer_for(&self, name_or_layer: &str) -> String {
        let table = self.wire_types_or_default();
        if let Some(t) = table.get(name_or_layer) {
            return t.layer();
        }
        if let Some(t) = table.by_layer(name_or_layer) {
            return t.layer();
        }
        if name_or_layer.trim().is_empty() {
            return LAYER_WIRES.to_string();
        }
        name_or_layer.to_string()
    }

    // ── allocators ───────────────────────────────────────────────────────

    /// The next free number for `family`, honouring the drawing's start value.
    ///
    /// Scans every existing tag with that family for its trailing integer and
    /// returns one past the highest, so a deleted device's number is reused
    /// only when it was the top one — which is what a drafter expects.
    pub fn next_tag_number(&self, family: &str) -> i32 {
        let s = self.settings_or_default();
        let mut max = s.tag_start - 1;
        for c in self.components.values() {
            let (fam, num) = split_family_number(&c.rec.tag);
            if fam.eq_ignore_ascii_case(family) {
                if let Some(n) = num {
                    max = max.max(n);
                }
            }
        }
        max + 1
    }

    /// The next free wire number.
    pub fn next_wire_number(&self) -> i32 {
        let s = self.settings_or_default();
        let mut max = s.wireno_start - 1;
        for w in self.wires.values() {
            if let Some(n) = trailing_number(&w.rec.wire_ref) {
                max = max.max(n);
            }
        }
        max + s.wireno_increment.max(1)
    }

    /// Auto-assign a tag for `family` using the drawing's tag format.
    pub fn suggest_tag(&self, family: &str) -> String {
        let s = self.settings_or_default();
        s.format_tag(family, self.next_tag_number(family))
    }

    /// Auto-assign a wire number using the drawing's wire-number format.
    pub fn suggest_wire_number(&self) -> String {
        let s = self.settings_or_default();
        s.format_wireno(self.next_wire_number())
    }

    /// The distinct family codes in use, for a report or a dropdown.
    pub fn families_in_use(&self) -> BTreeSet<String> {
        self.components
            .values()
            .map(|c| c.rec.family.clone())
            .filter(|f| !f.is_empty())
            .collect()
    }

    /// Components whose parent tag does not resolve to a placed **parent**.
    ///
    /// Resolution goes through [`Self::parent_by_tag`], not a bare tag lookup: a
    /// child is tagged with its parent's tag, so a bare lookup would find the
    /// child and report every orphan as linked.
    pub fn orphan_children(&self) -> Vec<&IndexedComponent> {
        self.components
            .values()
            .filter(|c| c.rec.is_child && !c.rec.parent_tag.is_empty())
            .filter(|c| self.parent_by_tag(&c.rec.parent_tag).is_none())
            .collect()
    }

    /// Children grouped under a parent tag, in ref-des order.
    pub fn children_of(&self, parent_tag: &str) -> Vec<&IndexedComponent> {
        let mut out: Vec<&IndexedComponent> = self
            .components
            .values()
            .filter(|c| c.rec.is_child && c.rec.parent_tag == parent_tag)
            .collect();
        out.sort_by(|a, b| crate::refdes_cmp(&a.rec.tag, &b.rec.tag));
        out
    }
}

/// Split a tag into its alpha family and trailing number, tolerating a
/// separator (`X1A` → `("X", Some(1))`; `K12` → `("K", Some(12))`).
fn split_family_number(tag: &str) -> (String, Option<i32>) {
    let family = family_of(tag);
    (family, trailing_number(tag))
}

/// The first integer embedded in a string, if any.
fn trailing_number(s: &str) -> Option<i32> {
    let digits: String = s.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<i32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ElValue;

    fn component(handle: u64, tag: &str, family: &str) -> (u64, ElRecord) {
        let mut c = ElComponentRec::new(&format!("id{handle}"), "sym", family);
        c.tag = tag.to_string();
        (handle, ElRecord::encode_component(&c))
    }

    fn wire(handle: u64, wref: &str, from: &str, to: &str) -> (u64, ElRecord) {
        let mut w = ElWireRec::new(&format!("wid{handle}"), from, to);
        w.wire_ref = wref.to_string();
        (handle, ElRecord::encode_wire(&w))
    }

    fn fixture() -> ElIndex {
        ElIndex::build(vec![
            (1, ElRecord::encode_settings(&ElSettingsRec::default())),
            component(2, "K1", "K"),
            component(3, "K2", "K"),
            component(4, "Q1", "Q"),
            wire(5, "W1", "K1", "K2"),
            wire(6, "W2", "K2", "Q1"),
        ])
    }

    #[test]
    fn builds_and_resolves_lookups() {
        let idx = fixture();
        assert!(idx.settings.is_some());
        assert_eq!(idx.components.len(), 3);
        assert_eq!(idx.wires.len(), 2);
        assert_eq!(idx.component_by_tag("K2").unwrap().primary(), 3);
        assert_eq!(idx.wire_by_ref("W1").unwrap().handle, 5);
        assert_eq!(idx.handle_for_el_id("id2"), Some(2));
    }

    #[test]
    fn next_tag_number_is_one_past_the_highest_in_family() {
        let idx = fixture();
        assert_eq!(idx.next_tag_number("K"), 3);
        assert_eq!(idx.next_tag_number("Q"), 2);
        // A family with no members starts at the drawing's start value.
        assert_eq!(idx.next_tag_number("F"), 1);
    }

    #[test]
    fn next_wire_number_follows_the_increment() {
        let idx = fixture();
        assert_eq!(idx.next_wire_number(), 3);
    }

    #[test]
    fn suggest_tag_uses_the_drawing_format() {
        let mut s = ElSettingsRec::default();
        s.tag_format = "%F-%N".to_string();
        let idx = ElIndex::build(vec![
            (1, ElRecord::encode_settings(&s)),
            component(2, "K-1", "K"),
        ]);
        assert_eq!(idx.suggest_tag("K"), "K-2");
    }

    #[test]
    fn tag_start_is_respected_for_a_fresh_drawing() {
        let mut s = ElSettingsRec::default();
        s.tag_start = 100;
        let idx = ElIndex::build(vec![(1, ElRecord::encode_settings(&s))]);
        assert_eq!(idx.next_tag_number("K"), 100);
    }

    #[test]
    fn duplicate_tags_are_detected_and_reported_sorted() {
        let idx = ElIndex::build(vec![component(1, "K1", "K"), component(2, "K1", "K")]);
        assert_eq!(idx.duplicate_tags(), ["K1"]);
    }

    #[test]
    fn duplicate_wire_refs_are_detected() {
        let idx = ElIndex::build(vec![
            wire(1, "W1", "a", "b"),
            wire(2, "W1", "c", "d"),
        ]);
        assert_eq!(idx.duplicate_wire_refs(), ["W1"]);
    }

    #[test]
    fn untagged_and_unnumbered_records_do_not_count_as_duplicates() {
        // Many components legitimately have no tag yet while being placed.
        let idx = ElIndex::build(vec![
            (1, ElRecord::encode_component(&ElComponentRec::new("a", "s", "K"))),
            (2, ElRecord::encode_component(&ElComponentRec::new("b", "s", "K"))),
        ]);
        assert!(idx.duplicate_tags().is_empty());
    }

    #[test]
    fn wires_touching_finds_both_directions() {
        let idx = fixture();
        assert_eq!(idx.wires_touching("K1").len(), 1);
        assert_eq!(idx.wires_touching("K2").len(), 2);
        assert_eq!(idx.wires_touching("nope").len(), 0);
    }

    #[test]
    fn corrupt_record_is_reported_not_dropped_silently() {
        let mut bad = ElRecord::new(KIND_WIRE, "x");
        bad.values[0].1 = ElValue::Int(99); // schema from the future
        let idx = ElIndex::build(vec![(7, bad)]);
        assert_eq!(idx.decode_errors.len(), 1);
        assert_eq!(idx.decode_errors[0].0, 7);
        assert!(idx.wires.is_empty());
    }

    #[test]
    fn unknown_kind_is_counted_not_rejected() {
        let mut r = ElRecord::new("future-thing", "x");
        r.put_str("A", "1");
        let idx = ElIndex::build(vec![(1, r)]);
        assert_eq!(idx.unknown_kinds.get("future-thing"), Some(&1));
        assert!(idx.decode_errors.is_empty());
    }

    #[test]
    fn settings_or_default_never_fails() {
        let idx = ElIndex::build(vec![]);
        assert_eq!(idx.settings_or_default().tag_format, "%F%N");
    }

    #[test]
    fn a_multi_part_device_is_one_device() {
        // The defect that folding fixes: a four-part coil keyed by handle looks
        // like four devices, so a retag gives it four tags and a delete leaves
        // three orphaned parts behind.
        let rec = {
            let mut c = ElComponentRec::new("shared-id", "relay-coil", "K");
            c.tag = "K1".to_string();
            ElRecord::encode_component(&c)
        };
        let idx = ElIndex::build(vec![(1, rec.clone()), (2, rec.clone()), (3, rec)]);
        assert_eq!(idx.components.len(), 1, "one device, not three");
        assert_eq!(idx.handles_for_el_id("shared-id"), &[1, 2, 3]);
        // Any drawn part resolves to the device it belongs to.
        assert_eq!(idx.component_of_handle(2).unwrap().rec.tag, "K1");
        assert_eq!(idx.component_of_handle(2).unwrap().primary(), 1);
        assert_eq!(idx.component_by_tag("K1").unwrap().parts().len(), 3);
        // And it does not look like a duplicate tag.
        assert!(idx.duplicate_tags().is_empty());
    }

    #[test]
    fn a_wire_is_one_entity_so_per_handle_keying_is_exact() {
        // Wires are keyed by handle rather than by `el_id`. That is only valid
        // while a wire is a single polyline; this pins the invariant.
        let idx = fixture();
        for (handle, w) in &idx.wires {
            assert_eq!(
                idx.handles_for_el_id(&w.rec.el_id),
                &[*handle],
                "a wire must be drawn as exactly one entity"
            );
        }
    }

    #[test]
    fn orphan_children_and_children_of() {
        let mut child = ElComponentRec::new("idc", "contact-no", "K");
        child.tag = "K1".to_string();
        child.is_child = true;
        child.parent_tag = "K9".to_string(); // no such parent
        let mut child2 = ElComponentRec::new("idc2", "contact-no", "K");
        child2.tag = "K1".to_string();
        child2.is_child = true;
        child2.parent_tag = "K1".to_string();
        let idx = ElIndex::build(vec![
            component(1, "K1", "K"),
            (2, ElRecord::encode_component(&child)),
            (3, ElRecord::encode_component(&child2)),
        ]);
        assert_eq!(idx.orphan_children().len(), 1);
        assert_eq!(idx.children_of("K1").len(), 1);
    }

    #[test]
    fn families_in_use_is_deduped() {
        let idx = fixture();
        let fams = idx.families_in_use();
        assert!(fams.contains("K"));
        assert!(fams.contains("Q"));
        assert_eq!(fams.len(), 2);
    }

    #[test]
    fn trailing_number_reads_through_separators() {
        assert_eq!(trailing_number("K-12"), Some(12));
        assert_eq!(trailing_number("W7A"), Some(7));
        assert_eq!(trailing_number("K"), None);
    }

    // ── terminals, cables, connectors ────────────────────────────────────

    fn terminal(handle: u64, strip: &str, number: &str, kind: TerminalKind) -> (u64, ElRecord) {
        let mut t = TerminalRec::new(&format!("tid{handle}"), strip, kind);
        t.number = number.to_string();
        t.strip_seq = number.parse::<i32>().unwrap_or(0);
        (handle, ElRecord::encode_terminal(&t))
    }

    fn cable(handle: u64, tag: &str, parent: bool, conductor: i32) -> (u64, ElRecord) {
        let mut c = CableRec::new(&format!("cid{handle}"), tag);
        if !parent {
            c.parent_tag = tag.to_string();
        }
        c.conductor_index = conductor;
        (handle, ElRecord::encode_cable(&c))
    }

    #[test]
    fn terminals_group_into_strips_in_sequence() {
        let idx = ElIndex::build(vec![
            terminal(1, "X1", "2", TerminalKind::Screw),
            terminal(2, "X1", "1", TerminalKind::Screw),
            terminal(3, "X2", "1", TerminalKind::Screw),
        ]);
        let strips = idx.strips();
        assert_eq!(strips.len(), 2);
        let x1 = &strips["X1"];
        assert_eq!(x1.len(), 2);
        assert_eq!(x1[0].rec.number, "1", "sequence order, not scan order");
        assert_eq!(x1[1].rec.number, "2");
    }

    #[test]
    fn the_next_terminal_number_fills_the_lowest_gap() {
        let idx = ElIndex::build(vec![
            terminal(1, "X1", "1", TerminalKind::Screw),
            terminal(2, "X1", "3", TerminalKind::Screw),
        ]);
        assert_eq!(idx.next_terminal_number("X1"), "2");
        // Another strip starts from 1.
        assert_eq!(idx.next_terminal_number("X2"), "1");
        // Non-numeric numbers do not break the sequence.
        let idx2 = ElIndex::build(vec![terminal(1, "X1", "PE", TerminalKind::Ground)]);
        assert_eq!(idx2.next_terminal_number("X1"), "1");
    }

    #[test]
    fn a_multi_part_terminal_is_one_terminal() {
        let rec = {
            let mut t = TerminalRec::new("shared", "X1", TerminalKind::Screw);
            t.number = "1".to_string();
            ElRecord::encode_terminal(&t)
        };
        let idx = ElIndex::build(vec![(1, rec.clone()), (2, rec)]);
        assert_eq!(idx.terminals.len(), 1);
        assert_eq!(idx.handles_for_el_id("shared"), &[1, 2]);
        assert!(idx.terminal_of_handle(2).is_some());
    }

    #[test]
    fn cable_markers_resolve_through_their_parent() {
        let idx = ElIndex::build(vec![
            cable(1, "W1", true, 1),
            cable(2, "W1", false, 2),
            cable(3, "W2", true, 1),
        ]);
        assert_eq!(idx.cables.len(), 3);
        let parent = idx.cable_parent("W1").unwrap();
        assert_eq!(parent.primary(), 1);
        assert_eq!(idx.cable_parent("W9"), None);

        let markers = idx.cable_markers("W1");
        assert_eq!(markers.len(), 2);
        assert!(markers[0].rec.is_parent(), "the parent sorts first");
        assert_eq!(markers[1].rec.conductor_index, 2);
    }

    #[test]
    fn used_conductors_and_the_next_free_one() {
        let idx = ElIndex::build(vec![
            cable(1, "W1", true, 1),
            cable(2, "W1", false, 3),
        ]);
        let used = idx.conductors_used("W1");
        assert!(used.contains(&1) && used.contains(&3));
        assert_eq!(idx.next_conductor("W1"), 2, "the gap is filled");
        assert_eq!(idx.next_conductor("W2"), 1);
    }

    #[test]
    fn the_next_cable_tag_follows_the_existing_ones() {
        let idx = ElIndex::build(vec![cable(1, "W1", true, 1), cable(2, "W7", true, 1)]);
        assert_eq!(idx.next_cable_tag(), "W8");
        let empty = ElIndex::build(vec![]);
        assert_eq!(empty.next_cable_tag(), "W1");
    }

    #[test]
    fn connectors_are_ordered_by_designator() {
        let mk = |handle: u64, refdes: &str| {
            (handle, ElRecord::encode_connector(&ConnectorRec::new(&format!("xid{handle}"), refdes, 4)))
        };
        let idx = ElIndex::build(vec![mk(1, "X10"), mk(2, "X2"), mk(3, "X1")]);
        let refs: Vec<&str> = idx.connectors_ordered().iter().map(|c| c.rec.ref_des.as_str()).collect();
        assert_eq!(refs, ["X1", "X2", "X10"], "natural order, so X2 precedes X10");
        assert!(idx.connector_of_handle(2).is_some());
    }

    #[test]
    fn a_multi_part_connector_is_one_connector() {
        let rec = ElRecord::encode_connector(&ConnectorRec::new("shared", "X1", 4));
        let idx = ElIndex::build(vec![(1, rec.clone()), (2, rec)]);
        assert_eq!(idx.connectors.len(), 1);
        assert_eq!(idx.handles_for_el_id("shared"), &[1, 2]);
    }

    #[test]
    fn a_parent_and_its_children_do_not_look_like_duplicate_tags() {
        // The rule that matters: a contact is tagged with its parent's tag by
        // design, so reporting that as a duplicate would flag every correctly
        // linked device and make the check useless.
        let mut parent = ElComponentRec::new("p", "relay-coil", "K");
        parent.tag = "K1".to_string();
        let mut child = ElComponentRec::new("c", "contact-no", "K");
        child.tag = "K1".to_string();
        child.is_child = true;
        child.parent_tag = "K1".to_string();
        let idx = ElIndex::build(vec![
            (1, ElRecord::encode_component(&parent)),
            (2, ElRecord::encode_component(&child)),
        ]);
        assert!(idx.duplicate_tags().is_empty(), "parent + child is not a collision");

        // Two *parents* on one tag still is.
        let mut parent2 = ElComponentRec::new("p2", "relay-coil", "K");
        parent2.tag = "K1".to_string();
        let idx2 = ElIndex::build(vec![
            (1, ElRecord::encode_component(&parent)),
            (2, ElRecord::encode_component(&parent2)),
        ]);
        assert_eq!(idx2.duplicate_tags(), ["K1"]);

        // And a tag lookup finds the parent, not the contact.
        assert_eq!(idx.parent_by_tag("K1").unwrap().primary(), 1);
        assert_eq!(idx.component_by_tag("K1").unwrap().primary(), 1);
    }

    // ── ladders and rungs ────────────────────────────────────────────────

    fn ladder(handle: u64, x: f64, y: f64, count: i32) -> (u64, ElRecord) {
        let mut l = LadderRec::new(&format!("lid{handle}"), [x, y], 100.0, 10.0, count);
        l.first_ref = "100".to_string();
        (handle, ElRecord::encode_ladder(&l))
    }

    #[test]
    fn ladders_index_by_el_id_and_order_top_first() {
        let idx = ElIndex::build(vec![ladder(1, 0.0, 50.0, 3), ladder(2, 0.0, 200.0, 3)]);
        assert_eq!(idx.ladders.len(), 2);
        let order = idx.ladders_ordered();
        assert_eq!(order[0].el_id, "lid2", "the higher ladder reads first");
        assert_eq!(order[1].el_id, "lid1");
        // Each ladder's drawn parts are collectable, which is what lets it be
        // redrawn or deleted as a unit.
        assert_eq!(idx.handles_for_el_id("lid1"), &[1]);
        assert_eq!(idx.ladder_of_handle(1).unwrap().el_id, "lid1");
        assert!(idx.ladder_of_handle(99).is_none());
    }

    #[test]
    fn a_multi_part_ladder_is_one_ladder_with_every_handle() {
        // The frame of a ladder carries the same record on every part, so the
        // index must fold them into one object rather than N duplicates.
        let mut l = LadderRec::new("lidX", [0.0, 100.0], 100.0, 10.0, 2);
        l.first_ref = "100".to_string();
        let rec = ElRecord::encode_ladder(&l);
        let idx = ElIndex::build(vec![(10, rec.clone()), (11, rec.clone()), (12, rec)]);
        assert_eq!(idx.ladders.len(), 1, "one ladder, not three");
        assert_eq!(idx.handles_for_el_id("lidX"), &[10, 11, 12]);
        // Any part resolves to the ladder.
        assert_eq!(idx.ladder_of_handle(12).unwrap().el_id, "lidX");
        assert_eq!(idx.ladders_ordered().len(), 1);
        // And its references are listed once, not three times.
        assert_eq!(idx.all_references(), ["100", "101"]);
    }

    #[test]
    fn rung_reference_at_resolves_a_point_on_a_rung() {
        let idx = ElIndex::build(vec![ladder(1, 10.0, 100.0, 5)]);
        // Rung 0 at y=100, rung 2 at y=80.
        assert_eq!(idx.rung_reference_at([50.0, 100.0], 5.0).as_deref(), Some("100"));
        assert_eq!(idx.rung_reference_at([50.0, 80.4], 5.0).as_deref(), Some("102"));
        // Off the ladder entirely.
        assert_eq!(idx.rung_reference_at([50.0, 130.0], 5.0), None);
    }

    #[test]
    fn rung_lookup_prefers_a_ladder_whose_span_contains_the_point() {
        // Two ladders at the same rung height; the point is inside the second.
        let idx = ElIndex::build(vec![ladder(1, 0.0, 100.0, 3), ladder(2, 500.0, 100.0, 3)]);
        let r = idx.rung_reference_at([550.0, 100.0], 5.0);
        assert_eq!(r.as_deref(), Some("100"));
        // Both would answer "100", so assert the *ladder* chosen instead.
        let l = idx.ladder_at([550.0, 100.0], 5.0).unwrap();
        assert_eq!(l.el_id, "lid2");
    }

    #[test]
    fn all_references_walks_every_ladder() {
        let idx = ElIndex::build(vec![ladder(1, 0.0, 100.0, 2), ladder(2, 0.0, 50.0, 2)]);
        assert_eq!(idx.all_references(), ["100", "101", "100", "101"]);
    }

    #[test]
    fn ladder_participates_in_the_wire_type_and_settings_defaults() {
        let idx = ElIndex::build(vec![ladder(1, 0.0, 0.0, 1)]);
        assert!(idx.settings_or_default().revision >= 1);
        // A drawing with no wire types still resolves a layer.
        assert_eq!(idx.wire_layer_for(""), LAYER_WIRES);
    }

    // ── wire types ───────────────────────────────────────────────────────

    #[test]
    fn wire_types_index_and_resolve_layers() {
        let idx = ElIndex::build(vec![(
            1,
            ElRecord::encode_wiretypes(&WireTypeTable::default()),
        )]);
        assert_eq!(idx.wire_types_or_default().types.len(), 5);
        assert_eq!(idx.wire_layer_for("RED"), "EL_WIRE_RD_14");
        assert_eq!(idx.wire_layer_for("EL_WIRE_RD_14"), "EL_WIRE_RD_14");
        // An unknown name passes through unchanged rather than vanishing.
        assert_eq!(idx.wire_layer_for("EL_CUSTOM"), "EL_CUSTOM");
    }

    #[test]
    fn an_unknown_wire_type_is_treated_as_numbered() {
        // Silently skipping an unrecognised type would hide the real problem.
        let idx = ElIndex::build(vec![]);
        assert!(idx.type_is_numbered("SOMETHING-NEW"));
        assert!(idx.type_is_numbered(""));
    }

    #[test]
    fn a_ground_wire_type_opts_out_of_numbering() {
        let idx = ElIndex::build(vec![(
            1,
            ElRecord::encode_wiretypes(&WireTypeTable::default()),
        )]);
        assert!(!idx.type_is_numbered("GREEN-YELLOW"));
        assert!(idx.type_is_numbered("BLACK"));
    }

    #[test]
    fn a_gapped_wire_is_one_wire_whatever_how_many_runs_it_has() {
        // A gap-flagged wire is drawn as several polylines sharing one record.
        // Keyed by handle, they would read as duplicate wire references and
        // double every count; folded, they are one wire with many parts.
        let mut rec = ElWireRec::new("wid", "K1", "K2");
        rec.wire_ref = "W1".to_string();
        rec.gap = true;
        let idx = ElIndex::build(vec![
            (1, ElRecord::encode_wire(&rec)),
            (2, ElRecord::encode_wire(&rec)),
            (3, ElRecord::encode_wire(&rec)),
        ]);
        assert_eq!(idx.wires.len(), 1);
        assert!(idx.wire_by_ref("W1").is_some());
        assert_eq!(idx.handles_for_el_id("wid"), &[1, 2, 3]);
        // Any run resolves to the whole wire.
        assert_eq!(idx.wire_of_handle(2).unwrap().rec.el_id, "wid");
        assert_eq!(idx.wire_of_handle(3).unwrap().rec.wire_ref, "W1");
        assert!(idx.wire_of_handle(9).is_none());
        // A single-run wire behaves as before.
        let other = ElWireRec::new("wid2", "K2", "K3");
        let idx2 = ElIndex::build(vec![(5, ElRecord::encode_wire(&other))]);
        assert!(idx2.wire_of_handle(5).is_some());
    }
}
