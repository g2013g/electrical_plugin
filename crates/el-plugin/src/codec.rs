//! Bridge between the host-free `ElRecord` and the host's `ExtendedDataRecord`,
//! plus document scanning helpers shared by all commands.

use ocs_plugin_api::host::codec::xdata::{ExtendedDataRecord, XDataValue};
use ocs_plugin_api::host::codec::{CadDocument, EntityType, Handle};
use ocs_plugin_api::host::HostApi;

use el_core::codec::{ElRecord, ElValue};
use el_core::ElIndex;

use crate::APPID;

/// Convert an electrical record to host XDATA.
pub fn to_xdata(rec: &ElRecord) -> ExtendedDataRecord {
    let mut out = ExtendedDataRecord::new(APPID);
    for (key, value) in &rec.values {
        out.add_value(XDataValue::String(key.clone()));
        out.add_value(match value {
            ElValue::Str(s) => XDataValue::String(s.clone()),
            ElValue::Real(r) => XDataValue::Real(*r),
            ElValue::Int(i) => XDataValue::Integer32(*i),
        });
    }
    out
}

/// Convert host XDATA to an electrical record; `None` when the record belongs
/// to another application or has unbalanced key/value pairs.
///
/// The application check is what keeps the two installed plugins from reading
/// each other's data: the harness plugin's `OCSWH` records are simply not ours.
pub fn from_xdata(rec: &ExtendedDataRecord) -> Option<ElRecord> {
    if rec.application_name != APPID {
        return None;
    }
    let mut out = ElRecord::default();
    let mut iter = rec.values.iter();
    while let Some(key) = iter.next() {
        let XDataValue::String(key) = key else {
            return None; // keys must be strings
        };
        let Some(value) = iter.next() else {
            return None; // dangling key
        };
        let v = match value {
            XDataValue::String(s) => ElValue::Str(s.clone()),
            XDataValue::Real(r) => ElValue::Real(*r),
            XDataValue::Integer16(i) => ElValue::Int(i32::from(*i)),
            XDataValue::Integer32(i) => ElValue::Int(*i),
            _ => return None, // unexpected payload type
        };
        out.values.push((key.clone(), v));
    }
    Some(out)
}

/// Read this plugin's record from one entity, if present.
pub fn read_el(host: &dyn HostApi, handle: Handle) -> Option<ElRecord> {
    host.read_record(handle, APPID).and_then(from_xdata)
}

/// Write an electrical record onto an entity (registers the `OCSEL` APPID).
pub fn write_el(host: &mut dyn HostApi, handle: Handle, rec: &ElRecord) -> bool {
    host.write_record(handle, to_xdata(rec))
}

/// Scan the whole document for electrical records: `(handle, ElRecord)` pairs.
///
/// Only `OCSEL` is ever read, so the wiring-harness plugin's records are
/// invisible here even when both plugins are loaded.
pub fn scan(doc: &CadDocument) -> Vec<(u64, ElRecord)> {
    doc.entities()
        .filter_map(|e| {
            let handle = e.common().handle;
            e.common()
                .extended_data
                .get_record(APPID)
                .and_then(from_xdata)
                .map(|rec| (handle.value(), rec))
        })
        .collect()
}

/// Build the resolved electrical index from a document.
pub fn build_index(doc: &CadDocument) -> ElIndex {
    ElIndex::build(scan(doc))
}

/// Decode the electrical record directly off an entity reference.
pub fn record_of(entity: &EntityType) -> Option<ElRecord> {
    entity
        .common()
        .extended_data
        .get_record(APPID)
        .and_then(from_xdata)
}
