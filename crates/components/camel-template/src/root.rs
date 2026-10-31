//! Root anchoring for external template entries (ADR-0047 Stage 2,
//! Task 1.3).
//!
//! Resolves a parsed [`TemplateEntry`] to the root directory and the
//! root-relative entry name that the lifecycle and closure walker consume.
//! Two anchors are supported:
//!
//! - [`RootAnchor::EntryParent`] — the historic behavior: the root is the
//!   entry's parent directory, with no canonicalization and no confinement
//!   beyond what `openat` already enforces.
//! - [`RootAnchor::ConfiguredRoot`] — the operator-configured
//!   `[components.template] root`: relative entries resolve under it, and
//!   absolute entries must be spelled under it.
//!
//! Canonicalization here is **validation only**. Enforcement (opening the
//! original path components with `O_NOFOLLOW`) stays in
//! `crate::lifecycle` via `path_util::open_root` / `open_relative`.

use std::fs;
use std::path::{Path, PathBuf};

use camel_api::CamelError;

use crate::error::TemplateReloadError;
use crate::uri::TemplateEntry;

/// The directory an entry path is resolved against.
#[derive(Debug, Clone)]
pub(crate) enum RootAnchor {
    /// Historic behavior: the entry's parent directory anchors the closure.
    EntryParent,
    /// Operator-configured `[components.template] root`.
    ConfiguredRoot(PathBuf),
}

/// Resolve `entry` against `anchor` into `(root_path, entry_name)`.
///
/// `entry_name` is the root-relative name used both as the MiniJinja
/// environment key and as the closure key.
///
/// ## Errors
///
/// - [`RootAnchor::EntryParent`] preserves the historic error contract: a
///   missing parent or file name maps through
///   [`TemplateReloadError::PathEscape`] to [`CamelError::TemplateReload`],
///   not [`CamelError::Config`].
/// - [`RootAnchor::ConfiguredRoot`] performs validation canonicalization and
///   returns [`CamelError::Config`] for a missing root, a missing entry, or an
///   absolute entry not spelled under the canonical root. A canonicalized
///   entry that escapes the root maps through
///   [`TemplateReloadError::PathEscape`].
pub(crate) fn resolve_entry(
    anchor: &RootAnchor,
    entry: &TemplateEntry,
) -> Result<(PathBuf, String), CamelError> {
    match anchor {
        RootAnchor::EntryParent => resolve_entry_parent(entry),
        RootAnchor::ConfiguredRoot(root_cfg) => resolve_entry_configured(root_cfg, entry),
    }
}

/// Historic entry-parent anchoring: no canonicalization, no extra I/O.
fn resolve_entry_parent(entry: &TemplateEntry) -> Result<(PathBuf, String), CamelError> {
    let path = match entry {
        TemplateEntry::Absolute(path) => path,
        TemplateEntry::Relative(_) => {
            return Err(CamelError::Config(
                "relative template entry requires a configured root".into(),
            ));
        }
    };

    let root = path.parent().ok_or_else(|| {
        CamelError::from(TemplateReloadError::PathEscape(
            "entry has no parent directory".into(),
        ))
    })?;
    let name = path.file_name().ok_or_else(|| {
        CamelError::from(TemplateReloadError::PathEscape(
            "entry has no file name".into(),
        ))
    })?;

    Ok((root.to_path_buf(), name.to_string_lossy().into_owned()))
}

/// Configured-root anchoring with validation-before-confinement.
fn resolve_entry_configured(
    root_cfg: &Path,
    entry: &TemplateEntry,
) -> Result<(PathBuf, String), CamelError> {
    let base = if root_cfg.is_absolute() {
        root_cfg.to_path_buf()
    } else {
        let cwd = std::env::current_dir().map_err(|error| {
            CamelError::Config(format!(
                "configured template root '{}' cannot be resolved: cannot determine current directory: {error}",
                root_cfg.display()
            ))
        })?;
        resolve_root_against(&cwd, root_cfg)
    };

    let canonical_root = fs::canonicalize(&base).map_err(|error| {
        CamelError::Config(format!(
            "configured template root '{}' is not accessible: {error}",
            root_cfg.display()
        ))
    })?;

    let original_rel = match entry {
        TemplateEntry::Relative(rel) => rel.clone(),
        TemplateEntry::Absolute(path) => path
            .strip_prefix(&canonical_root)
            .map_err(|_| {
                CamelError::Config(format!(
                    "absolute entry '{}' is not spelled under the canonical root '{}'; spell the entry relative to the configured root",
                    path.display(),
                    canonical_root.display()
                ))
            })?
            .to_path_buf(),
    };

    if original_rel.as_os_str().is_empty() {
        let entry_display = match entry {
            TemplateEntry::Absolute(path) => path.display().to_string(),
            TemplateEntry::Relative(rel) => rel.display().to_string(),
        };
        return Err(CamelError::Config(format!(
            "template entry '{entry_display}' resolves to the configured root itself; name a file under {}",
            canonical_root.display()
        )));
    }

    let candidate = canonical_root.join(&original_rel);
    let canonical_entry = fs::canonicalize(&candidate).map_err(|error| {
        CamelError::Config(format!(
            "template entry '{}' under configured root '{}' is not accessible: {error}",
            original_rel.display(),
            canonical_root.display()
        ))
    })?;

    if !canonical_entry.starts_with(&canonical_root) {
        return Err(CamelError::from(TemplateReloadError::PathEscape(format!(
            "entry '{}' (resolved to '{}') escapes configured root '{}'",
            original_rel.display(),
            canonical_entry.display(),
            canonical_root.display()
        ))));
    }

    Ok((canonical_root, original_rel.to_string_lossy().into_owned()))
}

/// Join a configured root against a base directory unless it is absolute.
///
/// Extraction keeps tests independent of the process CWD.
fn resolve_root_against(base: &Path, root_cfg: &Path) -> PathBuf {
    if root_cfg.is_absolute() {
        root_cfg.to_path_buf()
    } else {
        base.join(root_cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn resolve_root_against_joins_cwd_base() {
        assert_eq!(
            resolve_root_against(Path::new("/base"), Path::new("sub/t")),
            PathBuf::from("/base/sub/t")
        );
        assert_eq!(
            resolve_root_against(Path::new("/base"), Path::new("/abs")),
            PathBuf::from("/abs")
        );
    }

    #[test]
    fn resolve_entry_parent_anchors_parent_and_basename() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("page.html"), "x").expect("write entry");

        let (resolved_root, name) = resolve_entry(
            &RootAnchor::EntryParent,
            &TemplateEntry::Absolute(root.join("page.html")),
        )
        .expect("entry-parent resolution");

        assert_eq!(resolved_root, root.to_path_buf());
        assert_eq!(name, "page.html");
    }

    #[test]
    fn resolve_entry_parent_rejects_relative() {
        let result = resolve_entry(
            &RootAnchor::EntryParent,
            &TemplateEntry::Relative(PathBuf::from("x.html")),
        );

        assert!(matches!(result, Err(CamelError::Config(_))));
    }

    #[test]
    fn resolve_entry_parent_missing_segments_preserve_path_escape() {
        let result = resolve_entry(
            &RootAnchor::EntryParent,
            &TemplateEntry::Absolute(PathBuf::from("/")),
        );

        match result {
            Err(CamelError::TemplateReload(message)) => assert_eq!(
                message,
                TemplateReloadError::PathEscape("entry has no parent directory".into()).to_string()
            ),
            other => panic!("expected TemplateReload(PathEscape), got {other:?}"),
        }
    }

    #[test]
    fn resolve_configured_relative_entry_under_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonicalize root");
        fs::create_dir_all(root.join("templates")).expect("create templates dir");
        fs::write(root.join("templates/page.html"), "x").expect("write entry");

        let (resolved_root, name) = resolve_entry(
            &RootAnchor::ConfiguredRoot(root.clone()),
            &TemplateEntry::Relative(PathBuf::from("templates/page.html")),
        )
        .expect("configured resolution");

        assert_eq!(resolved_root, root);
        assert_eq!(name, "templates/page.html");
    }

    #[test]
    fn resolve_configured_missing_root_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope");

        let err = resolve_entry(
            &RootAnchor::ConfiguredRoot(missing.clone()),
            &TemplateEntry::Relative(PathBuf::from("page.html")),
        )
        .expect_err("missing root must error");

        match err {
            CamelError::Config(message) => assert!(
                message.contains(&missing.display().to_string()),
                "message {message:?} must name the configured root"
            ),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_configured_missing_entry_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonicalize root");

        let err = resolve_entry(
            &RootAnchor::ConfiguredRoot(root.clone()),
            &TemplateEntry::Relative(PathBuf::from("gone.html")),
        )
        .expect_err("missing entry must error");

        match err {
            CamelError::Config(message) => {
                assert!(
                    message.contains("gone.html"),
                    "message {message:?} must name the entry"
                );
                assert!(
                    message.contains(&root.display().to_string()),
                    "message {message:?} must name the root"
                );
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_configured_entry_equal_to_root_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonicalize root");

        let err = resolve_entry(
            &RootAnchor::ConfiguredRoot(root.clone()),
            &TemplateEntry::Absolute(root.clone()),
        )
        .expect_err("entry equal to the configured root must error");

        match err {
            CamelError::Config(message) => assert!(
                message.contains("root itself"),
                "message {message:?} must explain the entry names the root"
            ),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_configured_absolute_outside_root_errors() {
        let root_dir = tempfile::tempdir().expect("root tempdir");
        let outside_dir = tempfile::tempdir().expect("outside tempdir");
        let root = fs::canonicalize(root_dir.path()).expect("canonicalize root");
        let outside = fs::canonicalize(outside_dir.path()).expect("canonicalize outside");
        let entry = outside.join("page.html");
        fs::write(&entry, "x").expect("write outside entry");

        let err = resolve_entry(
            &RootAnchor::ConfiguredRoot(root.clone()),
            &TemplateEntry::Absolute(entry.clone()),
        )
        .expect_err("absolute entry outside root must error");

        match err {
            CamelError::Config(message) => {
                assert!(
                    message.contains(&entry.display().to_string()),
                    "message {message:?} must name the entry"
                );
                assert!(
                    message.contains(&root.display().to_string()),
                    "message {message:?} must name the canonical root"
                );
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_configured_absolute_inside_root_strips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonicalize root");
        fs::create_dir_all(root.join("templates")).expect("create templates dir");
        fs::write(root.join("templates/page.html"), "x").expect("write entry");

        let (resolved_root, name) = resolve_entry(
            &RootAnchor::ConfiguredRoot(root.clone()),
            &TemplateEntry::Absolute(root.join("templates/page.html")),
        )
        .expect("absolute entry inside root");

        assert_eq!(resolved_root, root);
        assert_eq!(name, "templates/page.html");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_configured_symlink_entry_outside_root_errors() {
        use std::os::unix::fs as unix_fs;

        let root_dir = tempfile::tempdir().expect("root tempdir");
        let outside_dir = tempfile::tempdir().expect("outside tempdir");
        let root = fs::canonicalize(root_dir.path()).expect("canonicalize root");
        let outside = fs::canonicalize(outside_dir.path()).expect("canonicalize outside");
        let real = outside.join("real.html");
        fs::write(&real, "x").expect("write outside file");
        unix_fs::symlink(&real, root.join("link.html")).expect("create symlink");

        let err = resolve_entry(
            &RootAnchor::ConfiguredRoot(root.clone()),
            &TemplateEntry::Relative(PathBuf::from("link.html")),
        )
        .expect_err("symlink escaping root must error");

        let message = err.to_string();
        assert!(
            message.contains("link.html"),
            "message {message:?} must name the entry"
        );
        assert!(
            message.contains(&root.display().to_string()),
            "message {message:?} must name the canonical root"
        );
    }
}
