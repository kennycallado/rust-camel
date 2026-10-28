//! Shared fail-closed helpers for directory walks.

use std::path::Path;
use walkdir::DirEntry;

/// Fail-closed directory walk: every `walkdir` error is surfaced as `Err`.
///
/// The message shape matches `lint_no_deprecated`: `cannot walk <path>: <error>`.
pub fn propagate_walk_errors<I>(
    root: &Path,
    it: I,
) -> impl Iterator<Item = Result<DirEntry, String>>
where
    I: Iterator<Item = walkdir::Result<DirEntry>>,
{
    it.map(move |e| {
        e.map_err(|err| {
            let affected = err
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| root.display().to_string());
            format!("cannot walk {affected}: {err}")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::propagate_walk_errors;
    use std::path::Path;
    use walkdir::WalkDir;

    #[test]
    fn injected_walk_error_surfaces_as_err() {
        let missing = Path::new("/nonexistent-437-xtask-walk-errors");
        let item = propagate_walk_errors(missing, WalkDir::new(missing).into_iter()).next();

        let err = item
            .expect("walk must yield one item")
            .expect_err("missing root must be an error");
        assert!(
            err.contains("cannot walk"),
            "error must name the walk failure: {err}"
        );
    }

    #[test]
    fn happy_path_preserves_entries_and_order() {
        let dir = tempfile::tempdir().expect("temporary dir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b")).expect("create nested dirs");
        std::fs::write(root.join("a/1.rs"), "").expect("write a/1.rs");
        std::fs::write(root.join("a/b/2.rs"), "").expect("write a/b/2.rs");
        std::fs::write(root.join("c.txt"), "").expect("write c.txt");

        let raw: Vec<_> = WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .map(|e| e.path().to_path_buf())
            .collect();
        let propagated: Vec<_> = propagate_walk_errors(root, WalkDir::new(root).into_iter())
            .collect::<Result<Vec<_>, _>>()
            .expect("clean tree");
        let propagated: Vec<_> = propagated
            .into_iter()
            .map(|e| e.path().to_path_buf())
            .collect();

        assert_eq!(raw.len(), propagated.len(), "same entry count");
        assert_eq!(raw, propagated, "same paths in same order");
    }

    /// Readability is probed directly instead of trusting a `USER` identity
    /// check: if the mode change does not make the directory unreadable (root,
    /// or a mode-insensitive filesystem), the test skips.
    #[cfg(unix)]
    #[test]
    fn unix_unreadable_dir_is_err() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temporary dir");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).expect("create sub dir");
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o000))
            .expect("make sub unreadable");

        if std::fs::read_dir(&sub).is_ok() {
            std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755))
                .expect("restore sub permissions");
            eprintln!("skipped: directory still readable (root or mode-insensitive fs)");
            return;
        }

        let results: Vec<_> =
            propagate_walk_errors(dir.path(), WalkDir::new(dir.path()).into_iter()).collect();

        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755))
            .expect("restore sub permissions");

        assert!(
            results
                .iter()
                .any(|r| r.as_ref().err().is_some_and(|e| e.contains("sub"))),
            "unreadable sub dir must surface an error naming it: {results:?}"
        );
    }
}
