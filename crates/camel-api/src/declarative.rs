use crate::Value;
use crate::error::CamelError;
use crate::script_source;

/// A language expression/predicate reference resolved by the runtime.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub struct LanguageExpressionDef {
    pub language: String,
    pub source: String,
}

impl LanguageExpressionDef {
    /// Build a language expression from a script file, inferring the
    /// language from the file extension.
    ///
    /// Relative paths follow standard library semantics and resolve against
    /// the process current working directory. The inferred language must map
    /// to a known script extension; use [`Self::file_with_language`] when the
    /// extension is not enough.
    pub fn file(path: impl AsRef<std::path::Path>) -> Result<Self, CamelError> {
        let path = path.as_ref();
        let language = script_source::infer_script_language(path)?;
        let source = script_source::read_script_source(path)?;
        Ok(Self { language, source })
    }

    /// Build a language expression from a script file, using the given
    /// language key instead of inferring it from the extension.
    ///
    /// Relative paths follow standard library semantics and resolve against
    /// the process current working directory.
    pub fn file_with_language(
        path: impl AsRef<std::path::Path>,
        language: impl Into<String>,
    ) -> Result<Self, CamelError> {
        let source = script_source::read_script_source(path.as_ref())?;
        Ok(Self {
            language: language.into(),
            source,
        })
    }
}

/// A declarative value source for set_header / set_body.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ValueSourceDef {
    Literal(Value),
    Expression(LanguageExpressionDef),
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::LanguageExpressionDef;

    #[test]
    fn file_constructor_infers_language() {
        let dir: TempDir = tempfile::tempdir().expect("create tempdir");
        let scripts = dir.path().join("scripts");
        fs::create_dir(&scripts).expect("create scripts dir");
        let path = scripts.join("x.rhai");
        let contents = "let value = 1;\n";
        fs::write(&path, contents).expect("write fixture");

        let def = LanguageExpressionDef::file(&path).expect("build from script file");

        assert_eq!(def.language, "rhai");
        assert_eq!(def.source, contents);
    }

    #[test]
    fn file_constructor_explicit_language_wins() {
        let dir: TempDir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("notes.txt");
        let contents = "hello\n";
        fs::write(&path, contents).expect("write fixture");

        let def = LanguageExpressionDef::file_with_language(&path, "simple")
            .expect("build with explicit language");

        assert_eq!(def.language, "simple");
        assert_eq!(def.source, contents);
    }

    #[test]
    fn file_constructor_missing_file_errors() {
        let dir: TempDir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("absent.rhai");

        let error = LanguageExpressionDef::file(&path).expect_err("missing file must fail");
        let message = error.to_string();

        assert!(
            message.contains(&path.display().to_string()),
            "message: {message}"
        );
    }

    #[test]
    fn file_constructor_relative_missing_file_names_absolute_path() {
        let cwd = std::env::current_dir().expect("read current dir");
        let relative = Path::new("nonexistent_dir_xyz/absent.rhai");

        let error = LanguageExpressionDef::file(relative).expect_err("missing file must fail");
        let message = error.to_string();

        let expected = cwd.join(relative);
        assert!(expected.is_absolute());
        assert!(
            message.contains(&expected.display().to_string()),
            "message: {message}"
        );
    }
}
