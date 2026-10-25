//! Net tracing: which wires are electrically the same conductor.
//!
//! A net is a set of wires joined through the devices they name. Two wires that
//! both name `K1.A1` are the same net; a chain `K1.A1 → X1:1` and `X1:2 → M1.U`
//! through a terminal strip is two nets joined only if the *strip* joins them,
//! which it does not — a terminal is two separate circuits unless a jumper says
//! otherwise.
//!
//! That distinction is the whole value: tracing connects what is connected and
//! refuses to guess. The union is over **node names**, never over proximity, so a
//! wire that merely crosses another is not swept into its net.
//!
//! Tracing is host-free: it is a pure function of the records, which is what makes
//! it testable and what keeps it identical to what a report would say.

use std::collections::{BTreeMap, BTreeSet};

use crate::index::ElIndex;

/// One electrical net.
#[derive(Debug, Clone, PartialEq)]
pub struct Net {
    /// The net's own name: an explicit `net=` label if any wire carries one,
    /// otherwise derived from its nodes.
    pub label: String,
    /// Every node the net touches, sorted.
    pub nodes: BTreeSet<String>,
    /// The wires in it, by handle. One wire alone is a net of one.
    pub wires: Vec<u64>,
}

impl Net {
    /// A display name for a net with no explicit label.
    ///
    /// Built from its nodes so two runs of a report on the same drawing agree —
    /// an index-based name would change as soon as anything was added.
    pub fn derived_label(nodes: &BTreeSet<String>) -> String {
        match nodes.len() {
            0 => "(unconnected)".to_string(),
            1 => nodes.iter().next().cloned().unwrap_or_default(),
            _ => {
                let first = nodes.iter().next().cloned().unwrap_or_default();
                let last = nodes.iter().last().cloned().unwrap_or_default();
                format!("{first}–{last}")
            }
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// Every net in a drawing, plus the lookups a command needs.
#[derive(Debug, Clone, Default)]
pub struct NetIndex {
    pub nets: Vec<Net>,
    wire_net: BTreeMap<u64, usize>,
    node_net: BTreeMap<String, usize>,
}

impl NetIndex {
    /// Build the nets from a resolved drawing.
    pub fn build(idx: &ElIndex) -> Self {
        // Union-find over node names. A wire unions its two node names, and every
        // wire naming the same node therefore ends up in one net.
        let mut parent: BTreeMap<String, String> = BTreeMap::new();
        fn find(parent: &mut BTreeMap<String, String>, key: &str) -> String {
            let mut root = key.to_string();
            while let Some(p) = parent.get(&root) {
                if p == &root {
                    break;
                }
                root = p.clone();
            }
            // Path compression, so a long chain does not degrade the walk.
            let mut cursor = key.to_string();
            while let Some(p) = parent.get(&cursor).cloned() {
                if p == cursor {
                    break;
                }
                parent.insert(cursor.clone(), root.clone());
                cursor = p;
            }
            root
        }
        fn union(parent: &mut BTreeMap<String, String>, a: &str, b: &str) {
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        }

        // Node names only ever join through a wire, so a name that no wire
        // mentions is not part of any net.
        for w in idx.wires.values() {
            for node in [&w.rec.from_id, &w.rec.to_id] {
                if !node.trim().is_empty() {
                    parent
                        .entry(node.clone())
                        .or_insert_with(|| node.clone());
                }
            }
            if !w.rec.from_id.trim().is_empty() && !w.rec.to_id.trim().is_empty() {
                union(&mut parent, &w.rec.from_id, &w.rec.to_id);
            }
        }

        // A jumper makes two terminals of one strip a single circuit, so nets
        // merge across it exactly as if a wire had been drawn — and two wires
        // landing one per side of the jumper belong to the same net. This must
        // run before the grouping pass below, which reads the union-find.
        for j in idx.jumpers.values() {
            let (a, b) = j.rec.designators();
            if a.trim().is_empty() || b.trim().is_empty() {
                continue;
            }
            parent.entry(a.clone()).or_insert_with(|| a.clone());
            parent.entry(b.clone()).or_insert_with(|| b.clone());
            union(&mut parent, &a, &b);
        }

        // Group wires by the root of their ends.
        let mut by_root: BTreeMap<String, Net> = BTreeMap::new();
        for w in idx.wires.values() {
            let anchor = if !w.rec.from_id.trim().is_empty() {
                w.rec.from_id.clone()
            } else if !w.rec.to_id.trim().is_empty() {
                w.rec.to_id.clone()
            } else {
                // A wire with no named ends cannot be unioned with anything, so it
                // is its own net — keyed by its handle so two such wires stay
                // separate rather than merging by accident.
                format!("\u{0}wire:{}", w.handle)
            };
            let root = find(&mut parent, &anchor);
            let net = by_root.entry(root).or_insert_with(|| Net {
                label: String::new(),
                nodes: BTreeSet::new(),
                wires: Vec::new(),
            });
            for node in [&w.rec.from_id, &w.rec.to_id] {
                if !node.trim().is_empty() {
                    net.nodes.insert(node.clone());
                }
            }
            net.wires.push(w.handle);
        }

        // An explicit `net=` label wins; a generated one is only a fallback.
        for w in idx.wires.values() {
            let Some(handle) = idx
                .wires
                .values()
                .find(|x| x.handle == w.handle)
                .map(|x| x.handle)
            else {
                continue;
            };
            let _ = handle;
            let label = w.rec.note.trim();
            if label.is_empty() {
                continue;
            }
            let anchor = if !w.rec.from_id.trim().is_empty() {
                w.rec.from_id.clone()
            } else {
                format!("\u{0}wire:{}", w.handle)
            };
            let root = find(&mut parent, &anchor);
            if let Some(net) = by_root.get_mut(&root) {
                if net.label.is_empty() {
                    net.label = label.to_string();
                }
            }
        }

        let mut nets: Vec<Net> = by_root.into_values().collect();
        for net in &mut nets {
            net.wires.sort_unstable();
            net.wires.dedup();
            if net.label.is_empty() {
                net.label = Net::derived_label(&net.nodes);
            }
        }
        // Sorted by label so two runs produce the same order — a report a
        // drafter cannot diff between runs is not much use.
        nets.sort_by(|a, b| crate::refdes_cmp(&a.label, &b.label));

        let mut wire_net = BTreeMap::new();
        let mut node_net = BTreeMap::new();
        for (i, net) in nets.iter().enumerate() {
            for w in &net.wires {
                wire_net.insert(*w, i);
            }
            for n in &net.nodes {
                node_net.insert(n.clone(), i);
            }
        }

        NetIndex {
            nets,
            wire_net,
            node_net,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    pub fn len(&self) -> usize {
        self.nets.len()
    }

    pub fn net_of_wire(&self, handle: u64) -> Option<&Net> {
        self.wire_net.get(&handle).and_then(|i| self.nets.get(*i))
    }

    pub fn net_of_node(&self, node: &str) -> Option<&Net> {
        self.node_net.get(node).and_then(|i| self.nets.get(*i))
    }

    /// Every wire in the net a wire belongs to.
    pub fn wires_touching(&self, handle: u64) -> Vec<u64> {
        self.net_of_wire(handle)
            .map(|n| n.wires.clone())
            .unwrap_or_default()
    }

    /// The nets with more than one node — the ones that actually join something.
    pub fn connected_nets(&self) -> Vec<&Net> {
        self.nets.iter().filter(|n| n.node_count() > 1).collect()
    }

    /// Nets of a single wire with a single node: a stub hanging off a device.
    pub fn stubs(&self) -> Vec<&Net> {
        self.nets
            .iter()
            .filter(|n| n.node_count() == 1 && n.wires.len() == 1)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ElRecord;
    use crate::model::{ElSettingsRec, ElWireRec};

    fn wire(handle: u64, wref: &str, from: &str, to: &str) -> (u64, ElRecord) {
        let mut w = ElWireRec::new(&format!("id{handle}"), from, to);
        w.wire_ref = wref.to_string();
        (handle, ElRecord::encode_wire(&w))
    }

    fn with_wires(wires: Vec<(u64, ElRecord)>) -> ElIndex {
        let mut all = vec![(900u64, ElRecord::encode_settings(&ElSettingsRec::default()))];
        all.extend(wires);
        ElIndex::build(all)
    }

    #[test]
    fn one_wire_is_one_net() {
        let idx = with_wires(vec![wire(1, "W1", "K1", "K2")]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.len(), 1);
        let n = nets.net_of_wire(1).unwrap();
        assert_eq!(n.node_count(), 2);
        assert_eq!(n.wires, vec![1]);
    }

    #[test]
    fn a_jumper_merges_the_nets_on_either_side_of_it() {
        use crate::model::{JumperRec, TerminalKind, TerminalRec};
        // Two wires land one per terminal of strip X1; without the jumper they
        // are two nets, with it they are one circuit — the terminal-strip
        // editor's whole point.
        let mut t1 = TerminalRec::new("t1", "X1", TerminalKind::Screw);
        t1.number = "1".to_string();
        let mut t2 = TerminalRec::new("t2", "X1", TerminalKind::Screw);
        t2.number = "2".to_string();
        let jumper = JumperRec::new("j1", "X1", "1", "2");
        let idx = ElIndex::build(vec![
            (900, ElRecord::encode_settings(&ElSettingsRec::default())),
            (1, ElRecord::encode_terminal(&t1)),
            (2, ElRecord::encode_terminal(&t2)),
            (3, ElRecord::encode_jumper(&jumper)),
            wire(10, "W1", "K1", "X1:1"),
            wire(11, "W2", "K2", "X1:2"),
        ]);

        // Sanity without the jumper would be two nets; assert the union.
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.len(), 1, "the jumper merges both wires into one net");
        let n = nets.net_of_wire(10).unwrap();
        assert_eq!(nets.net_of_wire(11).map(|x| x.label.clone()), Some(n.label.clone()));
        assert!(n.nodes.contains("X1:1") && n.nodes.contains("X1:2"));
    }

    #[test]
    fn wires_sharing_a_node_are_one_net() {
        // Two wires both landing on K2 are the same conductor.
        let idx = with_wires(vec![
            wire(1, "W1", "K1", "K2"),
            wire(2, "W2", "K2", "K3"),
        ]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.len(), 1, "a chain is one net: {:?}", nets.nets);
        let n = nets.net_of_node("K1").unwrap();
        assert_eq!(n.wires, vec![1, 2]);
        assert_eq!(n.node_count(), 3);
    }

    #[test]
    fn a_longer_chain_still_joins() {
        // Union-find has to survive more than one hop.
        let idx = with_wires(vec![
            wire(1, "W1", "A", "B"),
            wire(2, "W2", "B", "C"),
            wire(3, "W3", "C", "D"),
            wire(4, "W4", "D", "E"),
        ]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.len(), 1, "got {:?}", nets.nets);
        assert_eq!(nets.net_of_node("A").unwrap().wires.len(), 4);
        assert_eq!(nets.net_of_node("E").unwrap().node_count(), 5);
    }

    #[test]
    fn unrelated_wires_stay_separate() {
        let idx = with_wires(vec![
            wire(1, "W1", "K1", "K2"),
            wire(2, "W2", "Q1", "Q2"),
        ]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.len(), 2);
        assert_ne!(
            nets.net_of_wire(1).unwrap().label,
            nets.net_of_wire(2).unwrap().label
        );
    }

    #[test]
    fn a_terminal_does_not_join_its_two_sides() {
        // The property that makes tracing trustworthy: X1:1 and X1:2 are different
        // circuits unless a jumper says otherwise, so they must NOT be unioned.
        let idx = with_wires(vec![
            wire(1, "W1", "K1", "X1:1"),
            wire(2, "W2", "X1:2", "M1"),
        ]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.len(), 2, "a terminal is two circuits, got {:?}", nets.nets);
        assert_eq!(nets.net_of_node("X1:1").unwrap().wires, vec![1]);
        assert_eq!(nets.net_of_node("X1:2").unwrap().wires, vec![2]);
    }

    #[test]
    fn a_wire_with_unnamed_ends_is_its_own_net() {
        // Two such wires must not merge just because neither names anything.
        let idx = with_wires(vec![wire(1, "W1", "", ""), wire(2, "W2", "", "")]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.len(), 2, "got {:?}", nets.nets);
        assert!(nets.net_of_node("K1").is_none());
        assert_eq!(nets.net_of_wire(1).unwrap().wires, vec![1]);
        assert_eq!(nets.net_of_wire(2).unwrap().wires, vec![2]);
    }

    #[test]
    fn a_wire_touching_itself_is_one_net() {
        let idx = with_wires(vec![wire(1, "W1", "K1", "K1")]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets.net_of_wire(1).unwrap().node_count(), 1);
    }

    #[test]
    fn an_explicit_net_label_wins_over_a_derived_one() {
        let mut w = ElWireRec::new("id1", "K1", "K2");
        w.wire_ref = "W1".to_string();
        w.note = "24V-RAIL".to_string();
        let idx = with_wires(vec![(1, ElRecord::encode_wire(&w))]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.nets[0].label, "24V-RAIL");
    }

    #[test]
    fn a_derived_label_is_stable_across_runs() {
        // An index-based name would change as soon as anything was added, and a
        // report a drafter cannot diff between runs is not much use.
        let mk = || {
            with_wires(vec![
                wire(1, "W1", "K1", "K2"),
                wire(2, "W2", "K2", "K3"),
            ])
        };
        let a = NetIndex::build(&mk());
        let b = NetIndex::build(&mk());
        assert_eq!(a.nets[0].label, b.nets[0].label);
        assert_eq!(a.nets[0].label, "K1–K3");
    }

    #[test]
    fn an_empty_drawing_has_no_nets() {
        let idx = ElIndex::build(vec![]);
        let nets = NetIndex::build(&idx);
        assert!(nets.is_empty());
        assert!(nets.connected_nets().is_empty());
    }

    #[test]
    fn connected_nets_and_stubs_are_distinguished() {
        // A net that joins two devices is not the same thing as one wire hanging
        // off one pin, and a dashboard has to say which is which.
        let idx = with_wires(vec![
            wire(1, "W1", "K1", "K2"),
            wire(2, "W2", "Q1", ""),
        ]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.connected_nets().len(), 1);
        assert_eq!(nets.stubs().len(), 1);
        assert_eq!(nets.stubs()[0].nodes.iter().next().unwrap(), "Q1");
    }

    #[test]
    fn nets_are_ordered_by_label() {
        let idx = with_wires(vec![
            wire(1, "W1", "Q1", "Q2"),
            wire(2, "W2", "K1", "K2"),
        ]);
        let nets = NetIndex::build(&idx);
        let labels: Vec<&str> = nets.nets.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, ["K1–K2", "Q1–Q2"]);
    }

    #[test]
    fn wires_touching_finds_the_whole_net() {
        let idx = with_wires(vec![
            wire(1, "W1", "A", "B"),
            wire(2, "W2", "B", "C"),
            wire(3, "W3", "Z", "Y"),
        ]);
        let nets = NetIndex::build(&idx);
        assert_eq!(nets.wires_touching(1), vec![1, 2]);
        assert_eq!(nets.wires_touching(3), vec![3]);
        assert!(nets.wires_touching(99).is_empty());
    }
}
