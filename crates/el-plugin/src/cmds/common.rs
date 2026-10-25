//! Helpers shared by the command modules.

use ocs_plugin_api::host::codec::Handle;
use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;

use crate::codec::{read_el, scan, write_el};
use crate::hostops::DocEntityExt;
use crate::geometry::meta_carrier;

/// A parsed command line: recognised `KEY=VALUE` pairs, bare tokens, and the
/// arguments that could not be read.
#[derive(Debug, Default, Clone)]
pub struct Args {
    pub kv: Vec<(String, String)>,
    pub free: Vec<String>,
    pub errors: Vec<String>,
}
impl Args {
    /// Parse `KEY=VALUE` and bare tokens. Keys are upper-cased for matching.
    ///
    /// A `+` in a value stands in for a space: the host's command tokenizer
    /// splits on whitespace, so a value needing one has no other way to carry
    /// it.
    pub fn parse(args: &[&str]) -> Args {
        let mut out = Args::default();
        for raw in args {
            match raw.split_once('=') {
                Some((k, v)) => {
                    let key = k.trim().to_ascii_uppercase();
                    if key.is_empty() {
                        out.errors.push(format!("{raw:?} has an empty key"));
                    } else {
                        out.kv.push((key, v.trim().replace('+', " ")));
                    }
                }
                None => out.free.push(raw.to_string()),
            }
        }
        out
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.kv
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    /// A numeric value, recording an error when it is unreadable.
    pub fn number(&mut self, key: &str) -> Option<f64> {
        let raw = self.get(key)?.to_string();
        match raw.parse::<f64>() {
            Ok(v) => Some(v),
            Err(_) => {
                self.errors.push(format!("{key} expects a number, got {raw:?}"));
                None
            }
        }
    }
    pub fn integer(&mut self, key: &str) -> Option<i32> {
        let raw = self.get(key)?.to_string();
        match raw.parse::<i32>() {
            Ok(v) => Some(v),
            Err(_) => {
                self.errors.push(format!("{key} expects an integer, got {raw:?}"));
                None
            }
        }
    }
    /// A flag may be given as a bare token (`NORUNGS`) or as `KEY=0|1|YES|NO`.
    pub fn flag(&self, token: &str) -> Option<bool> {
        if self.free.iter().any(|f| f.eq_ignore_ascii_case(token)) {
            return Some(true);
        }
        // A `NO`-prefixed token given as a value flips it off.
        if let Some(v) = self.get(token) {
            return Some(parse_bool(v));
        }
        let negated = token.strip_prefix("NO")?;
        self.get(negated).map(|v| !parse_bool(v))
    }
    /// The first bare token that is not a coordinate.
    pub fn first_word(&self) -> Option<&str> {
        self.free
            .iter()
            .find(|t| !t.contains(','))
            .map(|s| s.as_str())
    }
    /// The first bare token that reads as a coordinate.
    pub fn first_coord(&self) -> Option<[f64; 3]> {
        self.free.iter().find_map(|t| parse_coord_token(t))
    }
}
/// Read a value as a flag.
///
/// Only an explicit negative spelling is false; *any* other non-empty value is
/// true — including a number. That matters because flags are also queried in
/// their negative form, so `RUNGS=5` has to read as "there are rungs" and
/// therefore make `NORUNGS` false. Treating "5" as unparseable-and-therefore-
/// false would invert the flag and silently suppress the rungs.
pub fn parse_bool(v: &str) -> bool {
    !matches!(
        v.trim().to_ascii_uppercase().as_str(),
        "" | "0" | "NO" | "FALSE" | "OFF" | "N"
    )
}
/// Parse `x,y[,z]`.
pub fn parse_coord_token(tok: &str) -> Option<[f64; 3]> {
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
/// Find the single metadata carrier holding a record of `kind`.
pub fn meta_handle_of_kind(host: &dyn HostApi, kind: &str) -> Option<u64> {
    scan(host.document())
        .into_iter()
        .find(|(_, rec)| rec.kind() == Some(kind))
        .map(|(h, _)| h)
}
/// Read the metadata record of `kind`, if the drawing has one.
pub fn read_meta(host: &dyn HostApi, kind: &str) -> Option<(u64, ElRecord)> {
    let handle = meta_handle_of_kind(host, kind)?;
    let record = read_el(host, Handle::new(handle))?;
    Some((handle, record))
}
/// Write the metadata record of `kind`, creating its carrier when absent.
///
/// The undo window is opened *before* the write: `push_undo` begins a change
/// recorder rather than taking a snapshot, so a push made afterwards would
/// attribute the change to whatever step came next.
pub fn write_meta(host: &mut dyn HostApi, kind: &str, record: &ElRecord, label: &str) -> bool {
    let existing = meta_handle_of_kind(host, kind);
    host.push_undo(label);
    let ok = match existing {
        Some(h) => write_el(host, Handle::new(h), record),
        None => {
            host.add_entity(meta_carrier(record));
            true
        }
    };
    if ok {
        host.bump_geometry();
        host.set_dirty();
    }
    ok
}
/// Remove a set of entities and report anything the host refused.
///
/// A refusal is normally a locked layer, and a caller that silently counts
/// removals would leave the user staring at entities that will not go away with
/// no explanation. The layer names are read back so the message can say which
/// one to unlock.
pub fn remove_handles(host: &mut dyn HostApi, handles: &[u64], what: &str) -> usize {
    let mut removed = 0usize;
    let mut blocked: Vec<(String, String)> = Vec::new();
    for h in handles {
        let handle = Handle::new(*h);
        if host.remove_entity(handle) {
            removed += 1;
            continue;
        }
        // Still present after a refusal: find out which layer stopped it.
        let layer = host
            .document()
            .get_entity_owned(handle)
            .map(|e| e.common().layer.clone())
            .unwrap_or_else(|| "<already gone>".to_string());
        if !blocked.iter().any(|(l, _)| *l == layer) {
            blocked.push((layer.clone(), layer));
        }
    }
    if !blocked.is_empty() {
        let layers: Vec<String> = blocked.into_iter().map(|(l, _)| format!("{l:?}")).collect();
        host.push_error(&format!(
            "{what}: {} of {} entit(ies) could not be removed — is the layer locked? ({})",
            handles.len() - removed,
            handles.len(),
            layers.join(", ")
        ));
    }
    removed
}
/// Remove a whole layer's worth of entities drawn by this plugin, used when a
/// derived layer is rebuilt from records (wire numbers, tables).
pub fn remove_entities_on_layer(host: &mut dyn HostApi, layer: &str) -> usize {
    let doomed: Vec<Handle> = host
        .document()
        .entities()
        .filter(|e| e.common().layer == layer)
        .map(|e| e.common().handle)
        .collect();
    let mut n = 0;
    for h in doomed {
        if host.remove_entity(h) {
            n += 1;
        }
    }
    n
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn args_split_key_values_from_bare_tokens() {
        let a = Args::parse(&["K1", "COLOR=RD", "NOREF", "WIDTH=170"]);
        assert_eq!(a.get("COLOR"), Some("RD"));
        assert_eq!(a.get("WIDTH"), Some("170"));
        assert_eq!(a.free, vec!["K1", "NOREF"]);
        assert!(a.errors.is_empty());
    }
    #[test]
    fn keys_are_case_insensitive_but_values_are_not() {
        let a = Args::parse(&["color=rd"]);
        assert_eq!(a.get("COLOR"), Some("rd"), "a value like 'rd' is the user's");
    }
    #[test]
    fn a_plus_stands_in_for_a_space() {
        let a = Args::parse(&["DESC=Motor+starter"]);
        assert_eq!(a.get("DESC"), Some("Motor starter"));
    }
    #[test]
    fn an_empty_key_is_reported() {
        let a = Args::parse(&["=oops"]);
        assert_eq!(a.errors.len(), 1);
    }
    #[test]
    fn numbers_report_a_bad_value_rather_than_defaulting() {
        let mut a = Args::parse(&["WIDTH=abc"]);
        assert_eq!(a.number("WIDTH"), None);
        assert_eq!(a.errors.len(), 1);
        assert!(a.errors[0].contains("WIDTH"));
    }
    #[test]
    fn integers_parse() {
        let mut a = Args::parse(&["RUNGS=12"]);
        assert_eq!(a.integer("RUNGS"), Some(12));
        assert!(a.errors.is_empty());
        assert_eq!(a.integer("MISSING"), None);
        assert!(a.errors.is_empty(), "an absent key is not an error");
    }
    #[test]
    fn flags_accept_bare_tokens_and_values() {
        let a = Args::parse(&["NORUNGS"]);
        assert_eq!(a.flag("NORUNGS"), Some(true));
        let a = Args::parse(&["RUNGS=0"]);
        assert_eq!(a.flag("RUNGS"), Some(false));
        let a = Args::parse(&["RUNGS=yes"]);
        assert_eq!(a.flag("RUNGS"), Some(true));
        let a = Args::parse(&[]);
        assert_eq!(a.flag("NORUNGS"), None);
    }
    #[test]
    fn flag_negation_lets_a_positive_name_be_turned_off() {
        // Every flag is queryable as its negative (`NOBUS`), so a caller never
        // needs two spellings. `BUS=0` therefore *is* `NOBUS`: there is no bus.
        let a = Args::parse(&["BUS=0"]);
        assert_eq!(a.flag("NOBUS"), Some(true), "BUS=0 means no bus");
        let a = Args::parse(&["BUS=1"]);
        assert_eq!(a.flag("NOBUS"), Some(false), "BUS=1 means the bus is on");
        let a = Args::parse(&["NORUNGS"]);
        assert_eq!(a.flag("NORUNGS"), Some(true), "the bare negative token still reads");
    }
    #[test]
    fn coord_and_word_extraction() {
        let a = Args::parse(&["10,20", "EXTRA"]);
        assert_eq!(a.first_coord(), Some([10.0, 20.0, 0.0]));
        assert_eq!(a.first_word(), Some("EXTRA"), "a coordinate is not a word");
        let a = Args::parse(&["EXTRA", "10,20"]);
        assert_eq!(a.first_word(), Some("EXTRA"));
    }
    #[test]
    fn coordinate_forms() {
        assert_eq!(parse_coord_token("1,2"), Some([1.0, 2.0, 0.0]));
        assert_eq!(parse_coord_token("1,2,3"), Some([1.0, 2.0, 3.0]));
        assert_eq!(parse_coord_token("1,2,3,4"), None);
        assert_eq!(parse_coord_token("1"), None);
        assert_eq!(parse_coord_token("a,b"), None);
    }
    #[test]
    fn parse_bool_only_an_explicit_negative_is_false() {
        for yes in ["1", "yes", "TRUE", "on", "Y", "5", "14"] {
            assert!(parse_bool(yes), "{yes:?} should read as on");
        }
        for no in ["0", "no", "false", "off", "", "N"] {
            assert!(!parse_bool(no), "{no:?} should read as off");
        }
    }
}
