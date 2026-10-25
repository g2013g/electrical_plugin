//! Host-aware library access: where the symbol library is, and loading it.
//!
//! Path *rules* live in `el_core::paths`; this module supplies the one thing
//! they cannot — the drawing's folder, from API 5's `HostApi::document_path`.

use std::path::PathBuf;
use std::sync::Mutex;

use ocs_plugin_api::host::HostApi;

use el_core::paths::{
    is_anchored, local_library_path, pick_library_path, resolve_input_in, resolve_output,
    LibraryScope, LIBRARY_FILE_NAME,
};
use el_core::symbol::SymbolLibrary;

/// A session-only library override, set by `EL_LIB PATH <file>`.
///
/// Deliberately not persisted: which vendor library a drawing should use is a
/// per-session decision, and a remembered override would silently change what a
/// later, unrelated drawing resolves.
static LIBRARY_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The shared per-user library file.
pub fn default_library_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("OpenCADStudio").join(LIBRARY_FILE_NAME)
}

/// The drawing's own file path, if it has been saved.
pub fn document_file(host: &dyn HostApi) -> Option<PathBuf> {
    host.document_path(host.tab_id())
}

/// The folder the drawing lives in — the base for relative paths.
pub fn drawing_dir(host: &dyn HostApi) -> Option<PathBuf> {
    document_file(host).and_then(|p| p.parent().map(PathBuf::from))
}

/// Resolve a relative input against the drawing folder, then the library folder.
///
/// Two bases because a vendor library ships its own `drawings/` directory: a
/// path inside the library is relative to the library, not to whichever drawing
/// happens to reference it.
pub fn input_with_library(host: &dyn HostApi, raw: &str) -> PathBuf {
    let dir = drawing_dir(host);
    let lib_dir = library_target(host).0.parent().map(PathBuf::from);
    resolve_input_in(&[dir.as_deref(), lib_dir.as_deref()], raw)
}


/// Resolve an output beside the drawing.
pub fn output(host: &dyn HostApi, raw: &str) -> PathBuf {
    resolve_output(drawing_dir(host).as_deref(), raw)
}

/// Which library file is in effect, and why.
pub fn library_target(host: &dyn HostApi) -> (PathBuf, LibraryScope) {
    let dir = drawing_dir(host);
    let override_path = LIBRARY_OVERRIDE.lock().ok().and_then(|g| g.clone());
    library_target_in(dir.as_deref(), override_path.as_deref())
}

fn library_target_in(
    drawing_dir: Option<&std::path::Path>,
    override_path: Option<&std::path::Path>,
) -> (PathBuf, LibraryScope) {
    pick_library_path(drawing_dir, override_path, &default_library_path())
}

/// Set or clear the session's library override.
pub fn set_library_override(path: Option<PathBuf>) {
    if let Ok(mut g) = LIBRARY_OVERRIDE.lock() {
        *g = path;
    }
}

/// Load the symbol library in effect.
///
/// Falls back to the built-in IEC set when no file exists — a drawing with no
/// library must still be able to place a symbol. A file that exists but does not
/// parse returns `Err`, because silently substituting the built-in set would
/// hide a broken vendor library behind drawings that look plausible.
pub fn load(host: &dyn HostApi) -> Result<SymbolLibrary, String> {
    let (path, scope) = library_target(host);
    if !path.exists() {
        return Ok(SymbolLibrary::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let lib = SymbolLibrary::from_json(&text)
        .map_err(|e| format!("{} is not a valid library: {e}", path.display()))?;
    let _ = scope;
    Ok(lib)
}

/// Write the library back to the file in effect.
pub fn save(host: &dyn HostApi, lib: &SymbolLibrary) -> Result<PathBuf, String> {
    let (path, _) = library_target(host);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(&path, lib.to_json())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// A one-line description of the library in effect, for a report.
pub fn describe(host: &dyn HostApi) -> String {
    let (path, scope) = library_target(host);
    let exists = if path.exists() { "" } else { " (not created yet)" };
    format!("{} [{}{}]", path.display(), scope.as_str(), exists)
}

/// Switch which library file is in effect, or report the current one.
pub fn switch(host: &dyn HostApi, raw: Option<&str>) -> Result<(PathBuf, LibraryScope, bool), String> {
    match raw {
        None => {
            let (path, scope) = library_target(host);
            Ok((path, scope, true))
        }
        Some("GLOBAL" | "global") => {
            set_library_override(None);
            let (path, scope) = library_target(host);
            Ok((path, scope, false))
        }
        Some("LOCAL" | "local") => {
            let Some(dir) = drawing_dir(host) else {
                return Err(
                    "LOCAL needs a saved drawing, because the library sits beside it".to_string(),
                );
            };
            let path = local_library_path(&dir);
            set_library_override(Some(path.clone()));
            Ok((path, LibraryScope::Project, false))
        }
        Some(other) => {
            let raw = other.trim();
            if is_anchored(raw) {
                set_library_override(Some(PathBuf::from(raw)));
            } else {
                let p = input_with_library(host, raw);
                set_library_override(Some(p));
            }
            let (path, scope) = library_target(host);
            Ok((path, scope, false))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn scratch() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("el-lib-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn default_path_sits_under_the_app_data_root() {
        let p = default_library_path();
        assert!(p.to_string_lossy().contains("OpenCADStudio"));
        assert!(p.to_string_lossy().ends_with(LIBRARY_FILE_NAME));
    }

    #[test]
    fn picking_never_creates_the_sidecar() {
        let dir = scratch();
        let global = dir.join("shared").join(LIBRARY_FILE_NAME);
        let (path, scope) = library_target_in(Some(&dir), None);
        // With no APPDATA the default path differs, so only the scope is
        // meaningful here unless the shared file happens to exist.
        if path != global {
            assert_eq!(scope, LibraryScope::Global);
        }
        assert!(!local_library_path(&dir).exists());
    }

    #[test]
    fn an_explicit_override_is_honoured_and_reported() {
        let dir = scratch();
        let explicit = dir.join("vendor.json");
        let (path, scope) = library_target_in(Some(&dir), Some(&explicit));
        assert_eq!(path, explicit);
        assert_eq!(scope, LibraryScope::Override);
    }
}
