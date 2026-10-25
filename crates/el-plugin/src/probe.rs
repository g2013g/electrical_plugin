//! `EL_PROBE` — live capability probe against the running host.
//!
//! Two jobs:
//!
//! 1. **Self-test** the mechanisms every other command depends on: entity
//!    acceptance, XDATA round-trip and payload limits, layer auto-creation,
//!    embedded-XDATA survival through `add_entities`, and the settings record.
//! 2. **The block-definition spike.** Schematic symbols naturally want to be
//!    block references; this plugin draws them as primitives instead because
//!    the contract offers no `AddBlockRecord`. Rather than assume that forever,
//!    the probe tests it: `EL_PROBE BLOCK` attempts the injection and
//!    `EL_PROBE BLOCKCHECK` (a *separate* dispatch — the only way a persistence
//!    question can be answered, since `document_mut()` hands an out-of-process
//!    plugin a local cache) reports whether it survived.
//!
//! Probe entities are removed afterwards, so the drawing is left clean.

use ocs_plugin_api::host::codec::entities::Insert;
use ocs_plugin_api::host::codec::tables::BlockRecord;
use ocs_plugin_api::host::codec::xdata::{ExtendedDataRecord, XDataValue};
use ocs_plugin_api::host::codec::types::Vector3;
use ocs_plugin_api::host::codec::{EntityType, Handle, Point};
use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::{ElSettingsRec, KIND_SETTINGS};

use crate::codec::read_el;
use crate::hostops::DocEntityExt;

/// The block name the spike tries to define.
const SPIKE_BLOCK: &str = "EL_PROBE_BLOCK";

/// Collect report lines; when a path was given they are also written there, so
/// the probe produces durable evidence in headless mode (plugin command output
/// only reaches the GUI command line, and is not forwarded in `--serve`).
fn report(
    host: &mut dyn HostApi,
    lines: &mut Vec<String>,
    name: &str,
    ok: bool,
    detail: String,
) {
    let verdict = if ok { "PASS" } else { "FAIL" };
    let line = format!("[{verdict}] {name}: {detail}");
    host.push_output(&line);
    lines.push(line);
}

fn probe_point(x: f64, y: f64, layer: &str) -> Point {
    let mut p = Point::at(Vector3::new(x, y, 0.0));
    p.common.layer = layer.to_string();
    p
}

/// Run `EL_PROBE [report_path] | BLOCK | BLOCKCHECK [report_path]`.
pub fn run(host: &mut dyn HostApi, arg: Option<&str>, second: Option<&str>) {
    eprintln!("[el] probe::run arg={arg:?} second={second:?}");
    // Re-enabled 2026-10-24 on host v2026.42 (codec 0.6.4): upstream fixed
    // the 0.6.0 entity-index crash this probe used to hit — the Insert-add
    // path and the post-add lookups both survive now. Verified live before
    // re-enabling: capabilities report 14 lines, 0 FAIL, and the
    // two-dispatch block spike wrote its verdict. History: the probe was
    // disabled across the whole v2026.41 generation (codec 0.6.0), where an
    // early return guarded the rest of the plugin from the crash cascade.

    #[allow(unreachable_code)]
    match arg.map(str::to_ascii_uppercase).as_deref() {
        Some("BLOCK") => block_spike(host),
        Some("BLOCKCHECK") => block_check(host, second),
        other => capabilities(host, other),
    }
}

// ── the block-definition spike ───────────────────────────────────────────

/// Phase 1: try to make a block definition exist, and say exactly what was
/// attempted. The insert is deliberately left in the drawing for phase 2.
fn block_spike(host: &mut dyn HostApi) {
    let before = host.document().block_records.len();
    host.push_output(&format!(
        "EL_PROBE BLOCK: {before} block record(s) already present"
    ));

    // The honest question is not "does the block name resolve" — it is whether
    // the host will *accept* a definition from us. A plugin cannot allocate
    // handles, and a BlockRecord is nothing but handles: the BLOCK and ENDBLK
    // entities plus every member entity. `BlockRecord::new` compiles, but its
    // handles are NULL and its member list is empty, so it defines a block with
    // no geometry.
    let mut rec = BlockRecord::new(SPIKE_BLOCK);
    rec.description = "electrical plugin block-definition spike".to_string();

    // Injection goes through `document_mut()`. For an out-of-process plugin the
    // host hands back a local cache, so this cannot reach the real document —
    // which is precisely what phase 2 measures.
    let accepted = {
        let doc = host.document_mut();
        doc.block_records.add(rec).is_ok()
    };
    host.push_output(&format!(
        "EL_PROBE BLOCK: table accepted the record = {accepted} (handle-free, empty member list)"
    ));

    let insert_handle = host.add_entity(EntityType::Insert(Insert::new(
        SPIKE_BLOCK,
        Vector3::new(0.0, 0.0, 0.0),
    )));
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_PROBE BLOCK: insert added at handle {insert_handle:#x}; now run EL_PROBE BLOCKCHECK \
         in a separate command to learn whether either survived the dispatch"
    ));
}

/// Phase 2: did anything from phase 1 reach the real document?
///
/// The verdict is written to a file when a path is given, because this is the
/// answer the whole symbol strategy rests on and plugin output never reaches
/// `--serve`: "it seemed to work" is not evidence.
fn block_check(host: &mut dyn HostApi, out_path: Option<&str>) {
    let mut lines: Vec<String> = Vec::new();
    let doc = host.document();
    let record_survived = doc.block_records.contains(SPIKE_BLOCK);
    let insert_survivors: Vec<Handle> = doc
        .entities()
        .filter(|e| matches!(e, EntityType::Insert(i) if i.block_name == SPIKE_BLOCK))
        .map(|e| e.common().handle)
        .collect();
    let block_count = doc.block_records.len();

    let say = |host: &mut dyn HostApi, lines: &mut Vec<String>, s: String| {
        host.push_output(&s);
        lines.push(s);
    };

    say(
        host,
        &mut lines,
        format!(
            "[{}] block definition survived the dispatch: {record_survived}",
            if record_survived { "PASS" } else { "FAIL" }
        ),
    );
    say(
        host,
        &mut lines,
        format!(
            "[{}] insert survived the dispatch: {} entit(ies)",
            if insert_survivors.is_empty() { "FAIL" } else { "PASS" },
            insert_survivors.len()
        ),
    );
    say(
        host,
        &mut lines,
        format!("block records in the document: {block_count}"),
    );

    if !record_survived {
        say(
            host,
            &mut lines,
            "VERDICT: no block definitions, for two independent reasons, both confirmed — \
             (1) `document_mut()` hands an out-of-process plugin a local cache, so a table insert \
             never reaches the host; and (2) a BlockRecord is a set of handles (BLOCK, ENDBLK, and \
             each member entity) that only the host can allocate, so even a persisted record would \
             define an empty block. Symbols stay primitives — see geometry.rs."
                .to_string(),
        );
    } else if insert_survivors.is_empty() {
        say(
            host,
            &mut lines,
            "VERDICT: the definition persisted but the insert did not — investigate before \
             switching symbols to block references."
                .to_string(),
        );
    } else {
        say(
            host,
            &mut lines,
            "VERDICT: BOTH survived. Block definitions may be viable after all — re-plan the \
             symbol layer in geometry.rs around real blocks with attributes."
                .to_string(),
        );
    }

    // Leave nothing behind either way.
    let mut removed = 0;
    for h in insert_survivors {
        if host.remove_entity(h) {
            removed += 1;
        }
    }
    if removed > 0 {
        host.set_dirty();
        say(host, &mut lines, format!("removed {removed} leftover insert(s)"));
    }

    if let Some(path) = out_path {
        match std::fs::write(path, lines.join("\n") + "\n") {
            Ok(()) => host.push_info(&format!("EL_PROBE: verdict written to {path}")),
            Err(e) => host.push_error(&format!("EL_PROBE: cannot write {path}: {e}")),
        }
    }
}

// ── the capability self-test ─────────────────────────────────────────────

fn capabilities(host: &mut dyn HostApi, out_path: Option<&str>) {
    let cp = |n: &str| out_path.map(|p| format!("{p}.{n}"));
    if let Some(f) = cp("enter") { let _ = std::fs::write(f, "enter"); }
    let mut lines = Vec::new();
    let header = "=== EL_PROBE host capability check (Open CAD Studio v2026.36, plugin API 5) ===";
    host.push_output(header);
    lines.push(header.to_string());
    let mut probe_handles: Vec<Handle> = Vec::new();

    // ── 1. add_entity accepts an Insert (but does it render?) ──
    eprintln!("[el-probe] section 1");
    eprintln!("[el-probe] section 1 start");
    let h = host.add_entity(EntityType::Insert(Insert::new(
        SPIKE_BLOCK,
        Vector3::new(0.0, 0.0, 0.0),
    )));
    probe_handles.push(h);
    let insert_ok = host
        .document()
        .get_entity_owned(h)
        .map(|e| matches!(e, EntityType::Insert(_)))
        .unwrap_or(false);
    let resolves = host.document().block_records.contains(SPIKE_BLOCK);
    report(
        host,
        &mut lines,
        "add_entity(Insert)",
        insert_ok,
        format!("accepted at {h:#x}; block name resolves in the block table = {resolves}"),
    );

    // ── 2. XDATA payload growth: numeric values ──
    eprintln!("[el-probe] section 2");
    eprintln!("[el-probe] section 2 start");
    let carrier = host.add_entity(EntityType::Point(probe_point(5.0, 5.0, "0")));
    probe_handles.push(carrier);
    let mut max_values_ok = 0usize;
    for n in [10usize, 100, 1_000, 5_000, 20_000] {
        let mut rec = ExtendedDataRecord::new(crate::APPID);
        for i in 0..n {
            rec.add_value(XDataValue::Real(i as f64));
        }
        let written = host.write_record(carrier, rec);
        let read_back = host
            .read_record(carrier, crate::APPID)
            .map(|r| r.values.len())
            .unwrap_or(0);
        if written && read_back == n {
            max_values_ok = n;
        } else {
            report(
                host,
                &mut lines,
                "xdata values round-trip",
                false,
                format!("write={written}, read_back={read_back}/{n} values"),
            );
            break;
        }
    }
    report(
        host,
        &mut lines,
        "xdata values round-trip",
        max_values_ok > 0,
        format!("max tested {max_values_ok} Real values (≈{} bytes)", max_values_ok * 8),
    );

    // ── 3. XDATA long string ──
    eprintln!("[el-probe] section 3");
    eprintln!("[el-probe] section 3 start");
    for len in [255usize, 4_096, 65_536, 262_144] {
        let payload = "x".repeat(len);
        let mut rec = ExtendedDataRecord::new(crate::APPID);
        rec.add_value(XDataValue::String(payload));
        let written = host.write_record(carrier, rec);
        let read_len = host
            .read_record(carrier, crate::APPID)
            .and_then(|r| r.values.first())
            .and_then(|v| match v {
                XDataValue::String(s) => Some(s.len()),
                _ => None,
            })
            .unwrap_or(0);
        let ok = written && read_len == len;
        report(
            host,
            &mut lines,
            "xdata long string",
            ok,
            format!("{len} chars → read back {read_len}"),
        );
        if !ok {
            break;
        }
    }

    // ── 4. Layer auto-creation ──
    eprintln!("[el-probe] section 4");
    eprintln!("[el-probe] section 4 start");
    let probe_layer = "EL_PROBE_LAYER";
    let lh = host.add_entity(EntityType::Point(probe_point(10.0, 10.0, probe_layer)));
    probe_handles.push(lh);
    let layer_ok = host.document().layers.iter().any(|l| l.name == probe_layer);
    report(
        host,
        &mut lines,
        "layer auto-create on add_entity",
        layer_ok,
        format!("layer {probe_layer:?} {}", if layer_ok { "exists" } else { "missing" }),
    );

    // ── 5. Embedded XDATA survives add_entity (no separate write_record) ──
    eprintln!("[el-probe] section 5");
    eprintln!("[el-probe] section 5 start");
    //
    // This is the mechanism every symbol here relies on: the record is attached
    // before the entity reaches the host, so symbol and data commit together
    // and there is no window in which a symbol exists without its record.
    let srec = ElRecord::encode_settings(&ElSettingsRec::default());
    let sh = host.add_entity(crate::geometry::meta_carrier(&srec));
    probe_handles.push(sh);
    let embedded = host
        .document()
        .get_entity_owned(sh)
        .and_then(|e| crate::codec::record_of(&e))
        .map(|r| r.kind() == Some(KIND_SETTINGS))
        .unwrap_or(false);
    report(
        host,
        &mut lines,
        "embedded xdata through add_entity",
        embedded,
        format!("record visible on the committed entity = {embedded}"),
    );

    // And that the scan path (index build) sees it.
    let idx = el_core::ElIndex::build(crate::codec::scan(host.document()));
    report(
        host,
        &mut lines,
        "scan→index sees embedded record",
        idx.settings.is_some(),
        format!("settings recovered = {}", idx.settings.is_some()),
    );

    // ── 6. Settings record round-trip through the host ──
    eprintln!("[el-probe] section 6");
    eprintln!("[el-probe] section 6 start");
    let wrote = crate::codec::write_el(host, sh, &srec);
    let read_back = read_el(host, sh).and_then(|r| r.decode_settings().ok());
    report(
        host,
        &mut lines,
        "settings record round-trip",
        wrote && read_back.is_some(),
        format!("revision {:?}", read_back.map(|s| s.revision)),
    );

    // ── 7. App-id isolation from the sibling plugin ──
    eprintln!("[el-probe] section 7");
    eprintln!("[el-probe] section 7 start");
    //
    // Both plugins can be installed at once, and both scan every entity on
    // every command. If our scan could see `OCSWH` records it would try to
    // decode a wiring-harness connector as an electrical component.
    let iso = host.add_entity(EntityType::Point(probe_point(20.0, 5.0, "0")));
    probe_handles.push(iso);
    let mut foreign = ExtendedDataRecord::new("OCSWH");
    foreign.add_value(XDataValue::String("KIND".to_string()));
    foreign.add_value(XDataValue::String("connector".to_string()));
    let foreign_written = host.write_record(iso, foreign);
    let iso_value = iso.value();
    let foreign_ignored = crate::codec::scan(host.document())
        .iter()
        .all(|(h, _)| *h != iso_value);
    report(
        host,
        &mut lines,
        "sibling app-id records are invisible",
        foreign_written && foreign_ignored,
        format!(
            "an OCSWH record written on {iso:#x} is not returned by the OCSEL scan = {foreign_ignored}"
        ),
    );

    // ── cleanup ──
    let mut removed = 0usize;
    for h in probe_handles {
        if host.remove_entity(h) {
            removed += 1;
        }
    }
    host.set_dirty();
    // The probe's contract is to leave no trace, and that includes session
    // state: a stale pinned target or a queued line from the probe would
    // surface during the user's next command and look like a bug in it.
    let queued = crate::state::pending_len();
    crate::state::clear_pending();
    crate::state::clear_last_target();
    let done = format!(
        "=== probe done, {removed} probe entity/entities removed, {queued} queued line(s) discarded ==="
    );
    host.push_output(&done);
    lines.push(done);
    let note = "EL_PROBE: handle stability across save/reopen is tested headless by the serve driver";
    host.push_info(note);
    lines.push(note.to_string());

    if let Some(path) = out_path {
        match std::fs::write(path, lines.join("\n") + "\n") {
            Ok(()) => host.push_info(&format!("EL_PROBE: report written to {path}")),
            Err(e) => host.push_error(&format!("EL_PROBE: cannot write {path}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spike_block_name_is_stable() {
        // The two phases communicate through this name across dispatches, so a
        // rename would silently break the check.
        assert_eq!(SPIKE_BLOCK, "EL_PROBE_BLOCK");
    }

    #[test]
    fn a_fresh_block_record_has_no_members() {
        // The evidence behind "no block definitions": a record we can construct
        // carries no handles, so it would define an empty block even if it
        // reached the document.
        let rec = BlockRecord::new(SPIKE_BLOCK);
        assert!(rec.entity_handles.is_empty());
        assert_eq!(rec.handle, Handle::NULL);
        assert_eq!(rec.block_entity_handle, Handle::NULL);
        assert_eq!(rec.block_end_handle, Handle::NULL);
    }
}
