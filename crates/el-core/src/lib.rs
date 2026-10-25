//! Host-free domain logic for the electrical plugin.
//!
//! This crate deliberately imports neither `iced`, `acadrust`, nor
//! `OpenCADStudio` (the Layer-C rule from the host's plugin architecture doc),
//! so everything in it is testable with plain `cargo test` and is insulated
//! from host ABI churn. The `el-plugin` crate owns the thin conversion shim
//! between [`codec::ElRecord`] and the host's `ExtendedDataRecord`.

pub mod catalog;
pub mod codec;
pub mod csv;
pub mod drc;
pub mod geom;
pub mod index;
pub mod model;
pub mod net;
pub mod numbering;
pub mod paths;
pub mod project;
pub mod report;
pub mod symbol;
pub mod tables;
pub mod wiregap;
pub mod tagging;

pub use codec::ElRecord;
pub use index::ElIndex;
pub use net::{Net, NetIndex};
pub use model::{
    ElComponentRec, ElSettingsRec, ElWireRec, LadderRec, WireType, WireTypeTable, APPID,
    SCHEMA_VERSION,
};
pub use numbering::{plan as plan_wire_numbers, WireNumberInput, WireNumberPlan};
pub use catalog::{Assignment, Catalog, PartEntry};
pub use drc::{check as run_drc, Issue, Severity};
pub use symbol::{SymbolDef, SymbolLibrary};
pub use report::{build as build_report, ReportKind};
pub use tables::TableLayout;
pub use tagging::{build_xrefs, plan_tags, TagInput, TagPlan, XrefGroup};

/// Generate a new `el_id` (UUID v4, hyphenated lowercase).
///
/// Records reference each other by this id and never by entity handle: handles
/// are renumbered by DWG/DXF round-trips, WBLOCK and copy/paste, so a handle
/// stored in a record is a dangling reference waiting to happen.
pub fn new_el_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Format a millimetre length without trailing noise: whole numbers print as
/// integers, otherwise two decimals.
pub fn fmt_mm(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e12 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}

/// Compare two reference designators the way a drafter reads them: the shared
/// alpha prefix compares lexically, the trailing number numerically, so `W2`
/// sorts before `W10` (a plain string compare gets this backwards, which is
/// visible in every report).
pub fn refdes_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| -> (String, i64, String) {
        let prefix: String = s.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let rest: String = s.chars().skip(prefix.len()).collect();
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let tail: String = rest.chars().skip(digits.len()).collect();
        (prefix, digits.parse::<i64>().unwrap_or(-1), tail)
    };
    let (ap, an, at) = split(a);
    let (bp, bn, bt) = split(b);
    ap.cmp(&bp)
        .then_with(|| an.cmp(&bn))
        .then_with(|| at.cmp(&bt))
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_el_id_is_unique() {
        assert_ne!(new_el_id(), new_el_id());
    }

    #[test]
    fn fmt_mm_drops_trailing_noise() {
        assert_eq!(fmt_mm(12.0), "12");
        assert_eq!(fmt_mm(12.5), "12.50");
    }

    #[test]
    fn refdes_sorts_numerically_within_a_prefix() {
        let mut v = vec!["W10", "W2", "W1", "K3", "K1"];
        v.sort_by(|a, b| refdes_cmp(a, b));
        assert_eq!(v, vec!["K1", "K3", "W1", "W2", "W10"]);
    }
}
