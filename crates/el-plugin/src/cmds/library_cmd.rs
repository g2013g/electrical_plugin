//! `EL_LIB` — the symbol library: what is in it, and which file is in effect.
//!
//! The library is a JSON file, so a vendor set is something a user can read,
//! diff and edit. `EL_LIB PATH` reporting *which* file is in effect and *why* is
//! not a nicety: with a project-local sidecar, an explicit override and a shared
//! per-user file all candidates, "the symbol is missing" and "you are editing a
//! different library" look identical from the drawing.

use ocs_plugin_api::host::HostApi;

use el_core::symbol::{SymbolLibrary, SymbolDef};

use crate::cmds::common::Args;
use crate::library;

/// Run `EL_LIB [LIST | SHOW <name> | FAMILIES | PATH [LOCAL|GLOBAL|<file>] |
/// EXPORT <file> | RELOAD]`.
pub fn run(host: &mut dyn HostApi, args: &[&str]) {
    let a = Args::parse(args);
    let action = a
        .first_word()
        .map(|w| w.to_ascii_uppercase())
        .unwrap_or_else(|| "LIST".to_string());

    match action.as_str() {
        "LIST" => list(host, &a),
        "SHOW" => show(host, &a),
        "FAMILIES" => families(host),
        "PATH" => path(host, &a),
        "EXPORT" | "SAVE" => export(host, &a),
        "RELOAD" => reload(host),
        other => host.push_error(&format!(
            "EL_LIB: unknown action {other:?} — try LIST, SHOW, FAMILIES, PATH, EXPORT or RELOAD"
        )),
    }
}

fn lib_or_report(host: &mut dyn HostApi) -> Option<SymbolLibrary> {
    match library::load(host) {
        Ok(l) => Some(l),
        Err(e) => {
            host.push_error(&format!("EL_LIB: {e}"));
            None
        }
    }
}

fn list(host: &mut dyn HostApi, a: &Args) {
    let Some(lib) = lib_or_report(host) else {
        return;
    };
    // An optional family filter: `EL_LIB LIST K` is how a user finds the symbol
    // they want without reading the whole JSON.
    let filter = a.free.iter().skip(1).find(|t| !t.contains(',')).cloned();
    let shown: Vec<&SymbolDef> = match &filter {
        Some(f) => lib.by_family(f),
        None => lib.symbols.iter().collect(),
    };

    host.push_output(&format!(
        "EL_LIB — {} (v{}), {} symbol(s){}",
        lib.name,
        lib.version,
        lib.symbols.len(),
        match &filter {
            Some(f) => format!(", showing family {f}"),
            None => String::new(),
        }
    ));
    host.push_output(&format!("  {}", library::describe(host)));

    if shown.is_empty() {
        host.push_error(&format!(
            "EL_LIB: nothing in family {:?}",
            filter.unwrap_or_default()
        ));
        return;
    }

    for s in shown.iter().take(40) {
        host.push_output(&format!(
            "  {:<18} {:<3} {:<9} {:<24} {}{}",
            s.name,
            s.family,
            s.contact.as_str(),
            s.description,
            if s.is_child { "child" } else { "" },
            if s.pins.is_empty() {
                " (no pins!)".to_string()
            } else {
                String::new()
            }
        ));
    }
    if shown.len() > 40 {
        host.push_output(&format!("  … and {} more", shown.len() - 40));
    }

    let invalid = lib.invalid();
    if !invalid.is_empty() {
        host.push_error(&format!(
            "EL_LIB: {} symbol(s) are unusable: {}",
            invalid.len(),
            invalid.join("; ")
        ));
    }
}

fn show(host: &mut dyn HostApi, a: &Args) {
    let Some(lib) = lib_or_report(host) else {
        return;
    };
    let Some(name) = a.free.iter().skip(1).find(|t| !t.contains(',')).cloned() else {
        host.push_error("EL_LIB SHOW: name a symbol, e.g. EL_LIB SHOW contact-no");
        return;
    };
    let Some(s) = lib.get(&name) else {
        let close: Vec<String> = lib
            .symbols
            .iter()
            .filter(|x| x.family.eq_ignore_ascii_case(&name) || x.name.contains(&name))
            .map(|x| x.name.clone())
            .take(8)
            .collect();
        host.push_error(&format!(
            "EL_LIB SHOW: no symbol {name:?}{}",
            if close.is_empty() {
                String::new()
            } else {
                format!(" — did you mean: {}", close.join(", "))
            }
        ));
        return;
    };

    host.push_output(&format!("EL_LIB SHOW {} — {}", s.name, s.description));
    host.push_output(&format!(
        "  family {}  role {}  contact {}",
        s.family,
        if s.is_child { "child" } else { "parent" },
        if s.contact.as_str().is_empty() {
            "—"
        } else {
            s.contact.as_str()
        }
    ));
    host.push_output(&format!(
        "  pins: {}",
        s.pins
            .iter()
            .map(|p| format!("{}@({}, {})", p.name, el_core::fmt_mm(p.at[0]), el_core::fmt_mm(p.at[1])))
            .collect::<Vec<_>>()
            .join("  ")
    ));
    if !s.default_ratings.is_empty() {
        host.push_output(&format!("  ratings: {}", s.default_ratings.join(", ")));
    }
    let e = s.extent();
    host.push_output(&format!(
        "  extent: {}×{} mm",
        el_core::fmt_mm(e[2] - e[0]),
        el_core::fmt_mm(e[3] - e[1])
    ));
    host.push_output(&format!("  glyph: {} primitive(s)", s.glyph.len()));
}

fn families(host: &mut dyn HostApi) {
    let Some(lib) = lib_or_report(host) else {
        return;
    };
    host.push_output(&format!("EL_LIB — families in {}", lib.name));
    for (code, desc) in lib.families() {
        let members = lib.by_family(&code).len();
        host.push_output(&format!("  {code:<4} {desc} ({members} symbol(s))"));
    }
}

fn path(host: &mut dyn HostApi, a: &Args) {
    let arg = a.free.iter().skip(1).find(|t| !t.contains(',')).cloned();
    match library::switch(host, arg.as_deref()) {
        Ok((path, scope, changed)) => {
            let exists = path.exists();
            host.push_output(&format!(
                "EL_LIB PATH: {} [{}]{}",
                path.display(),
                scope.as_str(),
                if exists { "" } else { " — does not exist yet" }
            ));
            if changed {
                host.push_info(
                    "  GLOBAL, LOCAL, or a file path switches the library for this session only",
                );
            } else if !exists {
                host.push_info(
                    "  the first EL_LIB ADD or EL_SYMBOL will create it from the built-in set",
                );
            }
        }
        Err(e) => host.push_error(&format!("EL_LIB PATH: {e}")),
    }
}

fn export(host: &mut dyn HostApi, a: &Args) {
    let Some(lib) = lib_or_report(host) else {
        return;
    };
    let target = a
        .free
        .iter()
        .skip(1)
        .find(|t| !t.contains(','))
        .cloned();
    let path = match target {
        Some(raw) => library::output(host, &raw),
        None => library::library_target(host).0,
    };
    match std::fs::write(&path, lib.to_json()) {
        Ok(()) => host.push_output(&format!(
            "EL_LIB EXPORT: {} symbol(s) written to {}",
            lib.symbols.len(),
            path.display()
        )),
        Err(e) => host.push_error(&format!("EL_LIB EXPORT: cannot write {}: {e}", path.display())),
    }
}

fn reload(host: &mut dyn HostApi) {
    match library::load(host) {
        Ok(lib) => {
            let invalid = lib.invalid();
            host.push_output(&format!(
                "EL_LIB RELOAD: {} — {} symbol(s){}",
                lib.name,
                lib.symbols.len(),
                if invalid.is_empty() {
                    String::new()
                } else {
                    format!(", {} UNUSABLE", invalid.len())
                }
            ));
            for e in invalid {
                host.push_error(&format!("  {e}"));
            }
        }
        Err(e) => host.push_error(&format!("EL_LIB RELOAD: {e}")),
    }
}
