//! Helpers for loading language-expression sources from script files.

use std::path::Path;
use std::path::PathBuf;

use crate::error::CamelError;

/// Resolve `path` to an absolute path for diagnostics without touching the
/// filesystem: absolute paths pass through, relative paths join the process
/// current working directory.
fn resolve_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// Infer the language registry key for a script file from its extension.
///
/// The extension match is case-sensitive. Relative paths are resolved
/// against the process current working directory for diagnostics.
pub fn infer_script_language(path: &Path) -> Result<String, CamelError> {
    let resolved = resolve_path(path);
    let Some(extension) = path.extension().and_then(std::ffi::OsStr::to_str) else {
        return Err(CamelError::RouteError(format!(
            "cannot infer script language for `{}`: missing file extension; set `language:` explicitly",
            resolved.display()
        )));
    };
    let language = match extension {
        "rhai" => "rhai",
        "js" => "js",
        "jsonpath" => "jsonpath",
        "xpath" => "xpath",
        "jinja" => "minijinja",
        "jinja2" => "minijinja",
        _ => {
            return Err(CamelError::RouteError(format!(
                "cannot infer script language for `{}`: unknown extension `.{extension}`; set `language:` explicitly",
                resolved.display()
            )));
        }
    };
    Ok(language.to_string())
}

/// Read the contents of a script file verbatim.
///
/// Relative paths are resolved against the process current working
/// directory for diagnostics. Empty (or whitespace-only) files are
/// rejected.
pub fn read_script_source(path: &Path) -> Result<String, CamelError> {
    let resolved = resolve_path(path);
    let contents = std::fs::read_to_string(&resolved).map_err(|error| {
        CamelError::RouteError(format!(
            "cannot read script file `{}`: {error}",
            resolved.display()
        ))
    })?;
    if contents.trim().is_empty() {
        return Err(CamelError::RouteError(format!(
            "script file `{}` is empty",
            resolved.display()
        )));
    }
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::infer_script_language;
    use super::read_script_source;

    #[test]
    fn infer_script_language_maps_known_extensions() {
        let dir: TempDir = tempfile::tempdir().expect("create tempdir");
        let cases = [
            ("a.rhai", "rhai"),
            ("b.js", "js"),
            ("c.jsonpath", "jsonpath"),
            ("d.xpath", "xpath"),
            ("e.jinja", "minijinja"),
            ("f.jinja2", "minijinja"),
        ];

        for (name, expected) in cases {
            let path = dir.path().join(name);
            fs::write(&path, "x").expect("write fixture");

            let language =
                infer_script_language(&path).expect("infer language from known extension");

            assert_eq!(language, expected, "language for {name}");
        }
    }

    #[test]
    fn infer_script_language_rejects_unknown_extension() {
        let dir: TempDir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("x.xyz");
        fs::write(&path, "x").expect("write fixture");

        let error = infer_script_language(&path).expect_err("unknown extension must fail");
        let message = error.to_string();

        assert!(
            message.contains(&path.display().to_string()),
            "message: {message}"
        );
        assert!(message.contains("xyz"), "message: {message}");
    }

    #[test]
    fn read_script_source_rejects_missing_file() {
        let dir: TempDir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("absent.rhai");

        let error = read_script_source(&path).expect_err("missing file must fail");
        let message = error.to_string();

        assert!(
            message.contains(&path.display().to_string()),
            "message: {message}"
        );
    }

    #[test]
    fn read_script_source_rejects_empty_file() {
        let dir: TempDir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("blank.rhai");
        fs::write(&path, " \n\t\n").expect("write fixture");

        let error = read_script_source(&path).expect_err("blank file must fail");
        let message = error.to_string();

        assert!(message.contains("empty"), "message: {message}");
        assert!(
            message.contains(&path.display().to_string()),
            "message: {message}"
        );
    }

    #[test]
    fn read_script_source_returns_verbatim_contents() {
        let dir: TempDir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("verbatim.rhai");
        let contents = "${env:SECRET}\n";
        fs::write(&path, contents).expect("write fixture");

        let read = read_script_source(&path).expect("read script source");

        assert_eq!(read, contents);
    }
}
