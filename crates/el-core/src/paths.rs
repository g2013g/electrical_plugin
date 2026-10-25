//! Host-free path resolution rules.
//!
//! Two bases for inputs, one for outputs — the same split the sibling
//! wiring-harness plugin uses, and for the same reason: a vendor library ships
//! as a library file plus a directory of drawings, so a `DRAWING`-style relative
//! reference is relative to the *library*, not to the drawing that mentions it.
//! Outputs, by contrast, always belong beside the drawing.
//!
//! Everything here is a pure function of paths, so the rules are testable
//! without a host and cannot drift between callers.

use std::path::{Component, Path, PathBuf};

/// The library file's default name, used both as the shared per-user file and
/// as the name a project-local sidecar takes.
pub const LIBRARY_FILE_NAME: &str = "el-library.json";

/// Where a library was found, reported to the user by `EL_LIB PATH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryScope {
    /// Chosen explicitly by the user for this session.
    Override,
    /// A sidecar beside the drawing.
    Project,
    /// The shared per-user file.
    Global,
}

impl LibraryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            LibraryScope::Override => "explicit",
            LibraryScope::Project => "project-local (beside the drawing)",
            LibraryScope::Global => "shared per-user",
        }
    }
}

/// Is this path absolute, drive-prefixed or UNC? An anchored path is never
/// reinterpreted against a base — it means exactly what it says.
pub fn is_anchored(raw: &str) -> bool {
    let p = Path::new(raw);
    p.is_absolute()
        || matches!(
            p.components().next(),
            Some(Component::Prefix(_)) | Some(Component::RootDir)
        )
}

/// Resolve an input against each base in order, returning the first that
/// exists. A relative path that exists nowhere is returned verbatim, so the
/// caller's error message names what the user actually typed.
pub fn resolve_input_in(bases: &[Option<&Path>], raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if is_anchored(trimmed) {
        return PathBuf::from(trimmed);
    }
    for base in bases.iter().flatten() {
        let candidate = base.join(trimmed);
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(trimmed)
}

/// Resolve an input against the drawing's folder.
pub fn resolve_input(drawing_dir: Option<&Path>, raw: &str) -> PathBuf {
    resolve_input_in(&[drawing_dir], raw)
}

/// Resolve an output path: beside the drawing when one is known, otherwise
/// relative to the working directory.
pub fn resolve_output(drawing_dir: Option<&Path>, raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if is_anchored(trimmed) {
        return PathBuf::from(trimmed);
    }
    match drawing_dir {
        Some(dir) => dir.join(trimmed),
        None => PathBuf::from(trimmed),
    }
}

/// The project-local library path for a drawing folder.
pub fn local_library_path(drawing_dir: &Path) -> PathBuf {
    drawing_dir.join(LIBRARY_FILE_NAME)
}

/// Which library file is in effect, and why.
///
/// Order: an explicit override, then a project-local sidecar **that already
/// exists**, then the shared per-user file. A project-local library is never
/// created implicitly: silently switching the write target would strand an
/// existing shared library the user is still editing.
pub fn pick_library_path(
    drawing_dir: Option<&Path>,
    override_path: Option<&Path>,
    global_default: &Path,
) -> (PathBuf, LibraryScope) {
    if let Some(o) = override_path {
        return (o.to_path_buf(), LibraryScope::Override);
    }
    if let Some(dir) = drawing_dir {
        let local = local_library_path(dir);
        if local.exists() {
            return (local, LibraryScope::Project);
        }
    }
    (global_default.to_path_buf(), LibraryScope::Global)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// A throwaway directory, so the existence rules can be tested for real.
    fn scratch(tag: &str) -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("el-paths-{}-{tag}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn anchored_paths_are_returned_verbatim() {
        for raw in [
            r"C:\libs\el.json",
            "/usr/share/el.json",
            r"\\server\share\el.json",
        ] {
            assert!(is_anchored(raw), "{raw} should read as anchored");
            assert_eq!(resolve_input(None, raw), PathBuf::from(raw));
            assert_eq!(resolve_output(None, raw), PathBuf::from(raw));
        }
    }

    #[test]
    fn relative_paths_are_not_anchored() {
        for raw in ["el.json", r"libs\el.json", "./el.json", "../el.json"] {
            assert!(!is_anchored(raw), "{raw} should read as relative");
        }
    }

    #[test]
    fn an_input_resolves_against_the_first_base_that_has_it() {
        let a = scratch("a");
        let b = scratch("b");
        fs::write(b.join("only-b.json"), "{}").unwrap();
        assert_eq!(
            resolve_input_in(&[Some(&a), Some(&b)], "only-b.json"),
            b.join("only-b.json"),
            "the library folder is tried when the drawing folder misses"
        );
    }

    #[test]
    fn a_missing_input_is_returned_verbatim_so_the_error_names_it() {
        let a = scratch("a");
        assert_eq!(
            resolve_input(Some(&a), "nowhere.json"),
            PathBuf::from("nowhere.json"),
            "not a/<name> — the caller's message must quote what was typed"
        );
    }

    #[test]
    fn an_output_lands_beside_the_drawing() {
        let a = scratch("a");
        assert_eq!(resolve_output(Some(&a), "out.json"), a.join("out.json"));
    }

    #[test]
    fn an_output_with_no_drawing_stays_where_it_was_given() {
        assert_eq!(resolve_output(None, "out.json"), PathBuf::from("out.json"));
    }

    #[test]
    fn whitespace_is_trimmed() {
        let a = scratch("a");
        assert_eq!(resolve_output(Some(&a), "  out.json  "), a.join("out.json"));
    }

    #[test]
    fn an_explicit_override_always_wins() {
        let (path, scope) = pick_library_path(
            None,
            Some(Path::new("/explicit/el.json")),
            Path::new("/global/el.json"),
        );
        assert_eq!(path, PathBuf::from("/explicit/el.json"));
        assert_eq!(scope, LibraryScope::Override);
    }

    #[test]
    fn a_project_local_library_is_used_only_when_it_already_exists() {
        let dir = scratch("proj");
        let global = dir.join("global").join(LIBRARY_FILE_NAME);

        // Nothing beside the drawing yet: the shared file is in effect.
        let (path, scope) = pick_library_path(Some(&dir), None, &global);
        assert_eq!(path, global);
        assert_eq!(scope, LibraryScope::Global);
        assert!(
            !local_library_path(&dir).exists(),
            "picking must not create the sidecar"
        );

        // Once one exists, it takes over.
        fs::write(local_library_path(&dir), "{}").unwrap();
        let (path, scope) = pick_library_path(Some(&dir), None, &global);
        assert_eq!(path, local_library_path(&dir));
        assert_eq!(scope, LibraryScope::Project);
    }

    #[test]
    fn with_no_drawing_folder_only_the_shared_file_remains() {
        let global = Path::new("/global/el.json");
        let (path, scope) = pick_library_path(None, None, global);
        assert_eq!(path, global);
        assert_eq!(scope, LibraryScope::Global);
    }

    #[test]
    fn scopes_describe_themselves() {
        assert!(LibraryScope::Project.as_str().contains("drawing"));
        assert!(LibraryScope::Global.as_str().contains("per-user"));
        assert_eq!(LibraryScope::Override.as_str(), "explicit");
    }
}
