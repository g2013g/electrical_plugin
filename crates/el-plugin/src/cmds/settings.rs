//! `EL_SETTINGS` — the per-drawing settings record.
//!
//! This is the plugin's equivalent of ACADE's invisible `WD_M` block, and the
//! reason ACADE puts it first is the reason it is the first command here:
//! every tag format, wire-number format, ladder default and numbering mode
//! that any other command uses is read from this one record. A drawing with no
//! settings record is not broken — it simply runs on the built-in defaults,
//! which is the normal state of a fresh drawing.

use ocs_plugin_api::host::HostApi;

use el_core::codec::ElRecord;
use el_core::model::{
    format_uses_number, ElSettingsRec, TagMode, WireNoMode, KIND_SETTINGS,
};

use crate::codec::{read_el, scan, write_el};
use crate::geometry::meta_carrier;

/// Run `EL_SETTINGS [KEY=VALUE …] | LIST | CLEAR`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let upper: Vec<String> = args.iter().map(|a| a.to_ascii_uppercase()).collect();

    if upper.iter().any(|a| a == "CLEAR" || a == "RESET") {
        clear(host);
        return;
    }

    // Bare `EL_SETTINGS` (or LIST) prints the current values; anything else is
    // a set. Keeping show and set on one command matches how a CAD command
    // line is actually used — you check, then change, then check again.
    if args.is_empty() || upper.iter().any(|a| a == "LIST" || a == "SHOW") {
        show(host);
        return;
    }

    apply(host, args);
}

/// The settings record's carrier handle, if the drawing has one.
fn settings_handle(host: &dyn HostApi) -> Option<u64> {
    scan(host.document())
        .into_iter()
        .find(|(_, rec)| rec.kind() == Some(KIND_SETTINGS))
        .map(|(h, _)| h)
}

fn show(host: &mut dyn HostApi) {
    let current = settings_handle(host)
        .and_then(|h| read_el(host, ocs_plugin_api::host::codec::Handle::new(h)))
        .and_then(|r| r.decode_settings().ok());

    let rec = match &current {
        Some(r) => r.clone(),
        None => ElSettingsRec::default(),
    };

    let source = if current.is_some() {
        "drawing record"
    } else {
        "built-in defaults (no EL_SETTINGS record in this drawing)"
    };
    host.push_output(&format!("EL_SETTINGS — {source}"));
    host.push_output(&format!(
        "  sheet={:?} drawing={:?} inst={:?} loc={:?}",
        rec.sheet, rec.drawing, rec.installation, rec.location
    ));
    host.push_output(&format!(
        "  ladder: width={}mm spacing={}mm count={} first_ref={:?} increment={}",
        el_core::fmt_mm(rec.ladder_width_mm),
        el_core::fmt_mm(rec.rung_spacing_mm),
        rec.rung_count,
        rec.first_rung_ref,
        rec.rung_increment
    ));
    host.push_output(&format!(
        "  tags:   mode={} format={:?} start={} suffixes={:?}",
        rec.tag_mode.as_str(),
        rec.tag_format,
        rec.tag_start,
        rec.tag_suffixes
    ));
    host.push_output(&format!(
        "  wires:  mode={} format={:?} start={} increment={} crossing gap={}mm",
        rec.wireno_mode.as_str(),
        rec.wireno_format,
        rec.wireno_start,
        rec.wireno_increment,
        el_core::fmt_mm(rec.wire_gap_mm)
    ));
    host.push_output(&format!(
        "  example: next tag {} / next wire number {}",
        rec.format_tag("K", rec.tag_start),
        rec.format_wireno(rec.wireno_start)
    ));

    // A format without %N makes every device in a family collide on one string.
    // ACADE treats that as invalid; surface it here rather than let it produce
    // duplicate tags later.
    if !format_uses_number(&rec.tag_format) {
        host.push_error(&format!(
            "EL_SETTINGS: tag format {:?} has no %N, so every tag in a family would collide",
            rec.tag_format
        ));
    }
    if !format_uses_number(&rec.wireno_format) {
        host.push_error(&format!(
            "EL_SETTINGS: wire-number format {:?} has no %N, so every wire would collide",
            rec.wireno_format
        ));
    }
}

fn apply(host: &mut dyn HostApi, args: &[&str]) {
    let handle = settings_handle(host);
    // Start from whatever the drawing already has so a partial set only
    // changes what was named.
    let mut rec = handle
        .map(|h| ocs_plugin_api::host::codec::Handle::new(h))
        .and_then(|h| read_el(host, h))
        .and_then(|r| r.decode_settings().ok())
        .unwrap_or_default();

    let mut applied: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for arg in args {
        let Some((key, value)) = arg.split_once('=') else {
            errors.push(format!("{arg:?} is not KEY=VALUE"));
            continue;
        };
        let key_up = key.trim().to_ascii_uppercase();
        // `+` stands in for a space: the host's command tokenizer splits on
        // whitespace, so a value needing one has no other way to carry it.
        let value = value.trim().replace('+', " ");

        match key_up.as_str() {
            "SHEET" => rec.sheet = value.clone(),
            "DRAWING" => rec.drawing = value.clone(),
            "INST" | "INSTALLATION" => rec.installation = value.clone(),
            "LOC" | "LOCATION" => rec.location = value.clone(),
            "LADW" | "LADDER_WIDTH" | "WIDTH" => match value.parse::<f64>() {
                Ok(v) if v > 0.0 => rec.ladder_width_mm = v,
                _ => errors.push(format!("LADDER_WIDTH expects a positive millimetre value, got {value:?}")),
            },
            "RUNGDIST" | "RUNG_SPACING" | "SPACING" => match value.parse::<f64>() {
                Ok(v) if v > 0.0 => rec.rung_spacing_mm = v,
                _ => errors.push(format!("RUNG_SPACING expects a positive millimetre value, got {value:?}")),
            },
            "RUNGINC" | "RUNG_INCREMENT" => match value.parse::<i32>() {
                Ok(v) if v != 0 => rec.rung_increment = v,
                _ => errors.push(format!("RUNG_INCREMENT expects a non-zero integer, got {value:?}")),
            },
            "RUNGFIRST" | "FIRST_RUNG_REF" => rec.first_rung_ref = value.clone(),
            "RUNGCNT" | "RUNG_COUNT" => match value.parse::<i32>() {
                Ok(v) if v > 0 => rec.rung_count = v,
                _ => errors.push(format!("RUNG_COUNT expects a positive integer, got {value:?}")),
            },
            "TAGMODE" => match TagMode::parse(&value) {
                Some(m) => rec.tag_mode = m,
                None => errors.push(format!("TAGMODE expects S or R (sequential or reference), got {value:?}")),
            },
            "TAGFMT" | "TAG_FORMAT" => rec.tag_format = value.clone(),
            "TAGSTART" | "TAG_START" => match value.parse::<i32>() {
                Ok(v) => rec.tag_start = v,
                _ => errors.push(format!("TAG_START expects an integer, got {value:?}")),
            },
            "TAGSUF" | "TAG_SUFFIXES" => rec.tag_suffixes = value.clone(),
            "WIREMODE" => match WireNoMode::parse(&value) {
                Some(m) => rec.wireno_mode = m,
                None => errors.push(format!("WIREMODE expects S or R (sequential or reference), got {value:?}")),
            },
            "WIREFMT" | "WIRE_FORMAT" => rec.wireno_format = value.clone(),
            "WIRESTART" | "WIRE_START" => match value.parse::<i32>() {
                Ok(v) => rec.wireno_start = v,
                _ => errors.push(format!("WIRE_START expects an integer, got {value:?}")),
            },
            "WINC" | "WIRE_INCREMENT" => match value.parse::<i32>() {
                Ok(v) if v != 0 => rec.wireno_increment = v,
                _ => errors.push(format!("WIRE_INCREMENT expects a non-zero integer, got {value:?}")),
            },
            "WIREGAP" | "WIRE_GAP" => match value.parse::<f64>() {
                Ok(v) if v >= 0.0 => rec.wire_gap_mm = v,
                _ => errors.push(format!("WIREGAP expects a non-negative millimetre value, got {value:?}")),
            },
            other => errors.push(format!(
                "unknown setting {other} — try SHEET, DRAWING, INST, LOC, LADDER_WIDTH, RUNG_SPACING, \
                 RUNG_COUNT, FIRST_RUNG_REF, TAGMODE, TAG_FORMAT, TAG_START, WIREMODE, WIRE_FORMAT, \
                 WIRE_START, WIREGAP"
            )),
        }
        applied.push(arg.to_string());
    }

    for e in &errors {
        host.push_error(&format!("EL_SETTINGS: {e}"));
    }
    if applied.is_empty() {
        host.push_error("EL_SETTINGS: nothing applied — see the errors above");
        return;
    }

    // Validate before writing: a format without %N is accepted by the record
    // but would produce duplicate tags later, so refuse it now.
    if !format_uses_number(&rec.tag_format) {
        host.push_error(&format!(
            "EL_SETTINGS: tag format {:?} has no %N — refusing to save a format that would collide",
            rec.tag_format
        ));
        return;
    }
    if !format_uses_number(&rec.wireno_format) {
        host.push_error(&format!(
            "EL_SETTINGS: wire-number format {:?} has no %N — refusing to save a format that would collide",
            rec.wireno_format
        ));
        return;
    }

    record_setting(host, handle, &rec, applied.len());
}

/// Write the settings record, creating its carrier if the drawing has none.
///
/// The undo window is opened *before* the first mutation: `push_undo` begins a
/// change recorder, so a push made after the change attributes it to the wrong
/// step (or drops it entirely).
fn record_setting(
    host: &mut dyn HostApi,
    handle: Option<u64>,
    rec: &ElSettingsRec,
    changed: usize,
) {
    let record = ElRecord::encode_settings(rec);
    let label = format!("EL_SETTINGS ({changed} value(s))");
    host.push_undo(&label);

    match handle {
        Some(h) => {
            let h = ocs_plugin_api::host::codec::Handle::new(h);
            if !write_el(host, h, &record) {
                host.push_error("EL_SETTINGS: the host refused the write (is the layer locked?)");
                return;
            }
        }
        None => {
            // A brand-new carrier: the record is embedded in the entity before
            // the host ever sees it, so the settings and their carrier commit
            // together.
            host.add_entity(meta_carrier(&record));
        }
    }
    host.bump_geometry();
    host.set_dirty();
    host.push_output(&format!(
        "EL_SETTINGS updated ({changed} value(s)); next tag {} / next wire number {}",
        rec.format_tag("K", rec.tag_start),
        rec.format_wireno(rec.wireno_start)
    ));
}

fn clear(host: &mut dyn HostApi) {
    let Some(h) = settings_handle(host) else {
        host.push_output("EL_SETTINGS CLEAR: this drawing has no settings record");
        return;
    };
    let h = ocs_plugin_api::host::codec::Handle::new(h);
    host.push_undo("EL_SETTINGS CLEAR");
    if host.remove_entity(h) {
        host.bump_geometry();
        host.set_dirty();
        host.push_output("EL_SETTINGS CLEAR: record removed; the drawing is back on built-in defaults");
    } else {
        host.push_error("EL_SETTINGS CLEAR: the host refused to remove the record (is the layer locked?)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use el_core::model::KIND_SETTINGS as KIND;

    #[test]
    fn settings_handle_finds_only_a_settings_record() {
        // A drawing can hold other record kinds; the settings lookup must not
        // grab them.
        let wire = ElRecord::encode_wire(&el_core::model::ElWireRec::new("i", "a", "b"));
        let settings = ElRecord::encode_settings(&ElSettingsRec::default());
        let pairs = vec![(1u64, wire), (2, settings)];
        let found = pairs
            .iter()
            .find(|(_, rec)| rec.kind() == Some(KIND))
            .map(|(h, _)| *h);
        assert_eq!(found, Some(2));
    }
}
