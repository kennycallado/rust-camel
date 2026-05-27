use crate::config::apply_profile_lenient;
use config::ConfigError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Validates and resolves an include path relative to `base_dir`.
///
/// Rules:
/// - Absolute paths → rejected.
/// - URL-like schemes (`://`) → rejected.
/// - Post-canonicalization traversal outside `base_dir` → rejected (catches `..` and symlinks).
pub fn resolve_include_path(base_dir: &Path, include: &str) -> Result<PathBuf, ConfigError> {
    if Path::new(include).is_absolute() || include.contains("://") {
        return Err(ConfigError::Message(format!(
            "include path must be relative (no absolute paths or URL schemes): \"{}\"",
            include
        )));
    }

    let candidate = base_dir.join(include);
    let canonical_candidate = candidate.canonicalize().map_err(|e| {
        ConfigError::Message(format!(
            "included file not found: \"{}\" (base: {}): {}",
            include,
            base_dir.display(),
            e
        ))
    })?;
    let canonical_base = base_dir.canonicalize().map_err(|e| {
        ConfigError::Message(format!(
            "failed to canonicalize base dir {}: {}",
            base_dir.display(),
            e
        ))
    })?;

    // Path::starts_with is component-based (not string prefix), safe on all platforms.
    // Both sides are canonicalized, so casing is consistent from the OS.
    if !canonical_candidate.starts_with(&canonical_base) {
        return Err(ConfigError::Message(format!(
            "include path escapes base directory (path traversal rejected): \"{}\" (base: {})",
            include,
            canonical_base.display()
        )));
    }

    Ok(canonical_candidate)
}

/// Loads and pre-processes each included file in order.
///
/// Returns a `Vec<String>` of flat TOML strings ready to be added as `config::Config` sources.
/// Ordered lowest-priority first. Caller adds root file last so root wins.
pub fn load_includes(
    base_dir: &Path,
    includes: &[String],
    profile: Option<&str>,
) -> Result<Vec<String>, ConfigError> {
    let mut seen: HashMap<PathBuf, usize> = HashMap::new();
    let mut sources = Vec::with_capacity(includes.len());

    for (idx, raw_path) in includes.iter().enumerate() {
        let canonical = resolve_include_path(base_dir, raw_path)?;

        if let Some(first_idx) = seen.get(&canonical) {
            return Err(ConfigError::Message(format!(
                "duplicate include path at include[{}]: \"{}\" (first seen at include[{}])",
                idx, raw_path, first_idx
            )));
        }
        seen.insert(canonical.clone(), idx);

        let content = std::fs::read_to_string(&canonical).map_err(|e| {
            ConfigError::Message(format!(
                "failed to read included file \"{}\": {}",
                canonical.display(),
                e
            ))
        })?;

        let mut value: toml::Value = toml::from_str(&content).map_err(|e| {
            ConfigError::Message(format!(
                "failed to parse included TOML \"{}\": {}",
                canonical.display(),
                e
            ))
        })?;

        if let toml::Value::Table(ref mut table) = value
            && table.remove("include").is_some()
        {
            tracing::warn!(
                path = %canonical.display(),
                "included file contains 'include' field; recursive includes are not supported in V1 — ignoring"
            );
        }

        apply_profile_lenient(&mut value, profile);

        let flat = toml::to_string(&value).map_err(|e| {
            ConfigError::Message(format!(
                "failed to re-serialize included config \"{}\": {}",
                canonical.display(),
                e
            ))
        })?;

        sources.push(flat);
    }

    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_file(dir: &std::path::Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, "").unwrap();
        p
    }

    fn write_toml(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, content).unwrap();
        p
    }

    fn get_str(toml_str: &str, key: &str) -> Option<String> {
        let val: toml::Value = toml::from_str(toml_str).unwrap();
        val.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
    }

    // --- resolve_include_path tests ---

    #[test]
    fn rejects_absolute_path() {
        let dir = TempDir::new().unwrap();
        let err = resolve_include_path(dir.path(), "/etc/passwd").unwrap_err();
        assert!(
            err.to_string().contains("absolute") || err.to_string().contains("relative"),
            "expected absolute/relative error, got: {err}"
        );
    }

    #[test]
    fn rejects_url_scheme() {
        let dir = TempDir::new().unwrap();
        let err = resolve_include_path(dir.path(), "file://config.toml").unwrap_err();
        assert!(
            err.to_string().contains("absolute")
                || err.to_string().contains("scheme")
                || err.to_string().contains("relative"),
            "expected scheme/absolute error, got: {err}"
        );
    }

    #[test]
    fn rejects_path_traversal() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().parent().unwrap();
        let outside = parent.join("secret.toml");
        fs::write(&outside, "").unwrap();

        let err = resolve_include_path(dir.path(), "../secret.toml").unwrap_err();
        assert!(
            err.to_string().contains("traversal") || err.to_string().contains("escapes"),
            "expected traversal error, got: {err}"
        );

        let _ = fs::remove_file(outside);
    }

    #[test]
    fn accepts_valid_relative_path() {
        let dir = TempDir::new().unwrap();
        make_file(dir.path(), "config/auth.toml");

        let result = resolve_include_path(dir.path(), "config/auth.toml");
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        assert!(result.unwrap().exists());
    }

    #[test]
    fn rejects_missing_file() {
        let dir = TempDir::new().unwrap();
        let err = resolve_include_path(dir.path(), "config/missing.toml").unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("No such"),
            "expected not-found error, got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let outside = dir.path().parent().unwrap().join("outside.toml");
        fs::write(&outside, "").unwrap();
        symlink(&outside, dir.path().join("escape.toml")).unwrap();

        let err = resolve_include_path(dir.path(), "escape.toml").unwrap_err();
        assert!(
            err.to_string().contains("traversal") || err.to_string().contains("escapes"),
            "expected traversal error for symlink escape, got: {err}"
        );

        let _ = fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[test]
    fn accepts_symlink_inside() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let real_file = dir.path().join("real.toml");
        fs::write(&real_file, "").unwrap();
        symlink(&real_file, dir.path().join("link.toml")).unwrap();

        let result = resolve_include_path(dir.path(), "link.toml");
        assert!(
            result.is_ok(),
            "symlink inside base dir should be allowed: {:?}",
            result
        );
    }

    // --- load_includes tests ---

    #[test]
    fn load_includes_basic() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "base.toml", r#"log_level = "debug""#);

        let sources = load_includes(dir.path(), &["base.toml".to_string()], None).unwrap();

        assert_eq!(sources.len(), 1);
        assert_eq!(get_str(&sources[0], "log_level").as_deref(), Some("debug"));
    }

    #[test]
    fn load_includes_strips_include_field() {
        let dir = TempDir::new().unwrap();
        write_toml(
            dir.path(),
            "nested.toml",
            r#"log_level = "info"
include = ["should-be-stripped.toml"]
"#,
        );

        let sources = load_includes(dir.path(), &["nested.toml".to_string()], None).unwrap();
        assert_eq!(sources.len(), 1);
        assert!(
            !sources[0].contains("should-be-stripped"),
            "include field was not stripped: {}",
            sources[0]
        );
    }

    #[test]
    fn load_includes_profile_propagates() {
        let dir = TempDir::new().unwrap();
        write_toml(
            dir.path(),
            "auth.toml",
            r#"[default]
log_level = "info"

[production]
log_level = "warn"
"#,
        );

        let sources =
            load_includes(dir.path(), &["auth.toml".to_string()], Some("production")).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(get_str(&sources[0], "log_level").as_deref(), Some("warn"));
    }

    #[test]
    fn load_includes_flat_file_with_active_profile_succeeds() {
        // Flat file (no profile sections) should work when a profile is active
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "flat.toml", r#"log_level = "debug""#);

        // This MUST NOT error — apply_profile_lenient handles this gracefully
        let sources =
            load_includes(dir.path(), &["flat.toml".to_string()], Some("production")).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(get_str(&sources[0], "log_level").as_deref(), Some("debug"));
    }

    #[test]
    fn load_includes_duplicate_entry_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(dir.path(), "base.toml", r#"log_level = "debug""#);

        let err = load_includes(
            dir.path(),
            &["base.toml".to_string(), "base.toml".to_string()],
            None,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("duplicate"),
            "expected duplicate error, got: {err}"
        );
    }

    #[test]
    fn load_includes_invalid_toml_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            dir.path(),
            "broken.toml",
            "this is {{ not valid toml at all",
        );

        let err = load_includes(dir.path(), &["broken.toml".to_string()], None).unwrap_err();

        assert!(
            err.to_string().contains("failed to parse"),
            "expected parse error, got: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_includes_unreadable_file_is_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = write_toml(dir.path(), "secret.toml", r#"key = "value""#);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        // Skip when running as root — root bypasses file-permission checks.
        if std::fs::read_to_string(&path).is_ok() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            return;
        }

        let err = load_includes(dir.path(), &["secret.toml".to_string()], None).unwrap_err();

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(
            err.to_string().contains("failed to read"),
            "expected read error, got: {err}"
        );
    }
}
