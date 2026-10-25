//! `EL_WIRETYPE` — the drawing's wire types.
//!
//! A wire type is a named conductor class that resolves to a layer through a
//! pattern. In ACADE the layer *is* the wire's identity — connectivity and
//! numbering both key off it — so this command is really "define the layers a
//! wire can live on", and it is what a wire is resolved back to its type
//! through.

use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::{WireType, WireTypeTable, WIRE_LAYER_PATTERN};

use crate::cmds::common::{read_meta, write_meta, Args};
use crate::codec::build_index;

const KIND: &str = el_core::model::KIND_WIRETYPES;

/// Run `EL_WIRETYPE [LIST] | ADD|EDIT <NAME> [COLOR=] [SIZE=] [PATTERN=]
/// [NONUMBERED] | DEL <NAME> | CLEAR`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let mut a = Args::parse(args);
    let action = a
        .first_word()
        .map(|w| w.to_ascii_uppercase())
        .unwrap_or_else(|| "LIST".to_string());

    match action.as_str() {
        "LIST" | "SHOW" => list(host),
        "ADD" | "EDIT" | "SET" => add(host, &mut a, action == "ADD"),
        "DEL" | "DELETE" | "REMOVE" => delete(host, &mut a),
        "CLEAR" | "RESET" => clear(host),
        "IMPORT" => {
            host.push_error(
                "EL_WIRETYPE IMPORT: importing types from another drawing needs a file path and is \
                 not implemented yet — add them with EL_WIRETYPE ADD",
            );
        }
        other => {
            host.push_error(&format!(
                "EL_WIRETYPE: unknown action {other:?} — try LIST, ADD, EDIT, DEL or CLEAR"
            ));
        }
    }
}

/// The drawing's table, falling back to the built-in set.
fn table_of(host: &dyn HostApi) -> WireTypeTable {
    build_index(host.document()).wire_types_or_default()
}

fn list(host: &mut dyn HostApi) {
    let idx = build_index(host.document());
    let table = idx.wire_types_or_default();
    let source = if idx.wire_types.is_some() {
        "drawing"
    } else {
        "built-in defaults (no EL_WIRETYPE record in this drawing)"
    };
    host.push_output(&format!("EL_WIRETYPE — {source}, {} type(s)", table.types.len()));
    for t in &table.types {
        host.push_output(&format!(
            "  {:<14} {:<5} {:<4} layer={:<20} numbering={}",
            t.name,
            t.color,
            t.size,
            t.layer(),
            if t.numbered { "yes" } else { "NO" }
        ));
    }
    // The pattern is the mechanism, so show it once rather than per row.
    let pattern = table
        .types
        .first()
        .map(|t| t.layer_pattern.clone())
        .unwrap_or_else(|| WIRE_LAYER_PATTERN.to_string());
    host.push_output(&format!(
        "  pattern {pattern:?} — %C = colour, %S = size"
    ));
}

fn add(host: &mut dyn HostApi, a: &mut Args, is_add: bool) {
    // `ADD <NAME> …` / `EDIT <NAME> …`: the name is the first bare token after
    // the action, so drop the action itself first.
    let name = a
        .free
        .iter()
        .skip(1)
        .find(|t| !t.contains(','))
        .cloned();

    let Some(name) = name else {
        host.push_error(&format!(
            "EL_WIRETYPE {}: name the type, e.g. EL_WIRETYPE {} RED COLOR=RD SIZE=14",
            if is_add { "ADD" } else { "EDIT" },
            if is_add { "ADD" } else { "EDIT" }
        ));
        return;
    };

    let mut table = table_of(host);
    let existing = table.get(&name).cloned();

    if is_add && existing.is_some() {
        host.push_error(&format!(
            "EL_WIRETYPE ADD: {name:?} already exists — use EDIT to change it"
        ));
        return;
    }
    if !is_add && existing.is_none() {
        host.push_error(&format!(
            "EL_WIRETYPE EDIT: no type named {name:?} — use ADD to create it"
        ));
        return;
    }

    // Start from the existing type so an edit only changes what was named.
    let mut wt = existing.unwrap_or_else(|| WireType::new(&name, "BK", "14"));
    let mut changed: Vec<String> = Vec::new();

    if let Some(c) = a.get("COLOR") {
        wt.color = c.to_string();
        changed.push(format!("COLOR={c}"));
    }
    if let Some(s) = a.get("SIZE") {
        wt.size = s.to_string();
        changed.push(format!("SIZE={s}"));
    }
    if let Some(p) = a.get("PATTERN") {
        wt.layer_pattern = p.to_string();
        changed.push(format!("PATTERN={p}"));
    }
    if let Some(v) = a.flag("NONUMBERED") {
        wt.numbered = !v;
        changed.push(format!("NUMBERED={}", wt.numbered));
    }

    if changed.is_empty() {
        host.push_error(&format!(
            "EL_WIRETYPE: nothing to set for {name} — pass COLOR=, SIZE=, PATTERN= or NONUMBERED"
        ));
        return;
    }

    // Two types resolving to the same layer would make a picked wire ambiguous
    // and wire numbering inconsistent, so refuse the collision loudly.
    if let Some(clash) = table
        .types
        .iter()
        .find(|t| !t.name.eq_ignore_ascii_case(&name) && t.layer() == wt.layer())
    {
        host.push_error(&format!(
            "EL_WIRETYPE: {name} would resolve to layer {:?}, which {} already uses — \
             change COLOR=, SIZE= or PATTERN=",
            wt.layer(),
            clash.name
        ));
        return;
    }

    let replaced = table.upsert(wt.clone());
    let record = ElRecord::encode_wiretypes(&table);
    let label = format!("EL_WIRETYPE {} {name}", if is_add { "ADD" } else { "EDIT" });
    if write_meta(host, KIND, &record, &label) {
        host.push_output(&format!(
            "EL_WIRETYPE: {name} {} ({}) → layer {}",
            if replaced { "updated" } else { "added" },
            changed.join(", "),
            wt.layer()
        ));
    } else {
        host.push_error("EL_WIRETYPE: the host refused the write");
    }
}

fn delete(host: &mut dyn HostApi, a: &mut Args) {
    let name = a.free.iter().skip(1).find(|t| !t.contains(',')).cloned();
    let Some(name) = name else {
        host.push_error("EL_WIRETYPE DEL: name the type to remove");
        return;
    };

    let mut table = table_of(host);
    let Some(doomed) = table.get(&name).cloned() else {
        host.push_error(&format!("EL_WIRETYPE DEL: no type named {name:?}"));
        return;
    };

    // Refuse when wires are still drawn on the type's layer: removing it would
    // leave wires whose type no longer exists, and numbering would silently
    // treat them as unknown.
    let in_use = host
        .document()
        .entities()
        .filter(|e| e.common().layer == doomed.layer())
        .count();
    if in_use > 0 {
        host.push_error(&format!(
            "EL_WIRETYPE DEL: {name:?} is still used by {in_use} entit(ies) on layer {} — \
             move or delete them first",
            doomed.layer()
        ));
        return;
    }

    table.remove(&name);
    let record = ElRecord::encode_wiretypes(&table);
    if write_meta(host, KIND, &record, "EL_WIRETYPE DEL") {
        host.push_output(&format!(
            "EL_WIRETYPE DEL: removed {name}; {} type(s) remain",
            table.types.len()
        ));
    } else {
        host.push_error("EL_WIRETYPE DEL: the host refused the write");
    }
}

fn clear(host: &mut dyn HostApi) {
    let Some((handle, _)) = read_meta(host, KIND) else {
        host.push_output("EL_WIRETYPE CLEAR: this drawing has no wire-type record");
        return;
    };
    host.push_undo("EL_WIRETYPE CLEAR");
    if host.remove_entity(ocs_plugin_api::host::codec::Handle::new(handle)) {
        host.bump_geometry();
        host.set_dirty();
        host.push_output(
            "EL_WIRETYPE CLEAR: record removed; the drawing is back on the built-in type set",
        );
    } else {
        host.push_error("EL_WIRETYPE CLEAR: the host refused to remove the record");
    }
}
