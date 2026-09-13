//! Path validation for security.
//!
//! Helpers extracted from `lib.rs` (modconfinement Task 1.1): lexical
//! pre-validation of user-influenced path segments, canonicalize-based base
//! containment, symlink-leaf refusal on open, and temp-prefix validation.

use camel_component_api::CamelError;
use tokio::fs::OpenOptions;

/// Returns true if the path string contains a `..` component (path traversal).
pub(crate) fn path_contains_traversal(path: &str) -> bool {
    std::path::Path::new(path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Lexical pre-validation for user-influenced path segments (e.g. the resolved
/// `file_name` or a substituted `doneFileName` pattern). Runs BEFORE joining so
/// that an absolute value cannot silently discard the base via `Path::join`,
/// and traversal components are rejected even when the target does not exist
/// yet (where canonicalize-based checks have nothing to resolve).
///
/// `label` identifies the source in the error message (e.g. "fileName",
/// "doneFileName").
pub(crate) fn validate_relative_filename(raw: &str, label: &str) -> Result<(), CamelError> {
    let p = std::path::Path::new(raw);
    if raw.is_empty() {
        return Err(CamelError::ProcessorError(format!(
            "{label} resolved to an empty path"
        )));
    }
    if raw.contains('\0') {
        return Err(CamelError::ProcessorError(format!(
            "{label} contains a NUL byte"
        )));
    }
    if p.is_absolute() {
        return Err(CamelError::ProcessorError(format!(
            "{label} must be relative to the endpoint directory, got absolute path: '{raw}'"
        )));
    }
    if path_contains_traversal(raw) {
        return Err(CamelError::ProcessorError(format!(
            "{label} contains directory traversal: '{raw}'"
        )));
    }
    Ok(())
}

/// Open a file for writing/append, refusing to follow a symlink in the final
/// path component. Closes the TOCTOU window between
/// `validate_path_is_within_base` (which canonicalizes the path by name) and
/// the open itself: even if an attacker swaps the leaf for a symlink after
/// validation, the open fails instead of escaping the base directory.
///
/// On non-Unix platforms there is no `O_NOFOLLOW` equivalent in std; fall back
/// to the plain open (the canonicalize check in `validate_path_is_within_base`
/// still applies).
pub(crate) fn open_options_no_follow() -> OpenOptions {
    let mut opts = OpenOptions::new();
    // Inherent tokio method (cfg-gated on unix inside tokio); no trait import needed.
    #[cfg(unix)]
    opts.custom_flags(libc::O_NOFOLLOW);
    opts
}

pub(crate) fn is_valid_temp_prefix(prefix: &str) -> bool {
    !prefix.contains('\0')
        && !std::path::Path::new(prefix).is_absolute()
        && !prefix.contains(std::path::MAIN_SEPARATOR)
        && !prefix.contains('/')
        && !prefix.contains('\\')
}

pub(crate) fn validate_path_is_within_base(
    base_dir: &std::path::Path,
    target_path: &std::path::Path,
) -> Result<(), CamelError> {
    // If the base exists, enforce strict containment: canonicalize the base,
    // reject any symlinked component below the ORIGINAL (non-canonicalized)
    // base, and require the nearest existing ancestor of the target to stay
    // within the canonicalized base.
    if base_dir.exists() {
        let canonical_base = base_dir.canonicalize().map_err(|e| {
            CamelError::ProcessorError(format!("Cannot canonicalize base directory: {}", e))
        })?;

        // Relative components against the original (non-canonicalized) base.
        let rel = target_path.strip_prefix(base_dir).map_err(|_| {
            CamelError::ProcessorError(format!(
                "Path '{}' is not under base '{}'",
                target_path.display(),
                base_dir.display()
            ))
        })?;
        if path_contains_traversal(&rel.to_string_lossy()) {
            return Err(CamelError::ProcessorError(format!(
                "Path '{}' contains directory traversal",
                target_path.display()
            )));
        }

        // Symlink-chain rejection: every cumulative component below the base
        // must be symlink-free at validation time, including symlinks that
        // resolve inside the canonicalized base (they are mutable
        // retargeting points). The base itself may be a symlink
        // (canonicalized above, per the operator contract).
        let mut cumulative = base_dir.to_path_buf();
        for component in rel.components() {
            cumulative.push(component);
            if std::fs::symlink_metadata(&cumulative).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(CamelError::ProcessorError(format!(
                    "Path '{}' traverses symlinked component '{}' below base '{}'",
                    target_path.display(),
                    cumulative.display(),
                    base_dir.display()
                )));
            }
        }

        // Nearest-existing-ancestor containment: walk up to the closest
        // existing ancestor, canonicalize it, and require it to stay within
        // the canonicalized base. Since the base exists and lexically
        // prefixes the target, the walk terminates at the base at the
        // latest; the parent-less fallback degenerates to base containment.
        let mut ancestor = target_path;
        while std::fs::symlink_metadata(ancestor).is_err() {
            match ancestor.parent() {
                Some(parent) => ancestor = parent,
                None => {
                    ancestor = base_dir;
                    break;
                }
            }
        }
        let canonical_ancestor = ancestor.canonicalize().map_err(|e| {
            CamelError::ProcessorError(format!(
                "Cannot canonicalize existing ancestor '{}': {}",
                ancestor.display(),
                e
            ))
        })?;
        if !canonical_ancestor.starts_with(&canonical_base) {
            return Err(CamelError::ProcessorError(format!(
                "Path '{}' is outside base directory '{}'",
                canonical_ancestor.display(),
                canonical_base.display()
            )));
        }
    } else {
        // Base dir doesn't exist yet (auto_create case): nothing below it
        // can be a pre-existing symlink, so a lexical traversal check
        // suffices.
        let rel = target_path.strip_prefix(base_dir).map_err(|_| {
            CamelError::ProcessorError(format!(
                "Path '{}' is not under base '{}'",
                target_path.display(),
                base_dir.display()
            ))
        })?;
        let rel_str = rel.to_string_lossy();
        if path_contains_traversal(&rel_str) {
            return Err(CamelError::ProcessorError(format!(
                "Path '{}' contains directory traversal",
                target_path.display()
            )));
        }
    }

    Ok(())
}
