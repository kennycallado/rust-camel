use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use camel_api::template::TemplateError;
use regex::Regex;

fn placeholder_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_.\-]*)\s*\}\}").unwrap()) // allow-unwrap
}

/// Replace `{{name}}` placeholders in `input` with values from `values`.
///
/// `declared_params` is the set of parameter names declared by the template.
/// Unknown parameters in `values` produce [`TemplateError::UnknownParameter`].
/// Missing required parameters (no default, no value) produce [`TemplateError::MissingParameter`].
pub fn substitute_placeholders(
    input: &str,
    values: &BTreeMap<String, String>,
    declared_params: &[String],
) -> Result<String, TemplateError> {
    let declared: HashSet<&str> = declared_params.iter().map(|s| s.as_str()).collect();

    // Validate that all supplied values correspond to declared parameters.
    for key in values.keys() {
        if !declared.contains(key.as_str()) {
            return Err(TemplateError::UnknownParameter(key.clone()));
        }
    }

    for param in declared_params {
        if !values.contains_key(param) {
            return Err(TemplateError::MissingParameter(param.clone()));
        }
    }

    for cap in placeholder_regex().captures_iter(input) {
        let name = &cap[1];
        if !declared.contains(name) {
            return Err(TemplateError::UnknownParameter(name.to_string()));
        }
    }

    let result = placeholder_regex()
        .replace_all(input, |caps: &regex::Captures| {
            let name = &caps[1];
            values.get(name).cloned().expect("validated above") // allow-unwrap
        })
        .into_owned();

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_params() -> Vec<String> {
        vec![]
    }

    fn params(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn vals(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // --- Basic substitution (5 tests) ---

    #[test]
    fn substitutes_single_placeholder() {
        let result = substitute_placeholders(
            "hello {{name}}",
            &vals(&[("name", "world")]),
            &params(&["name"]),
        )
        .unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn substitutes_multiple_placeholders() {
        let result = substitute_placeholders(
            "{{host}}:{{port}}/api",
            &vals(&[("host", "localhost"), ("port", "8080")]),
            &params(&["host", "port"]),
        )
        .unwrap();
        assert_eq!(result, "localhost:8080/api");
    }

    #[test]
    fn substitutes_same_placeholder_twice() {
        let result =
            substitute_placeholders("{{x}} and {{x}}", &vals(&[("x", "same")]), &params(&["x"]))
                .unwrap();
        assert_eq!(result, "same and same");
    }

    #[test]
    fn no_placeholders_returns_unchanged() {
        let result =
            substitute_placeholders("plain text", &BTreeMap::new(), &empty_params()).unwrap();
        assert_eq!(result, "plain text");
    }

    #[test]
    fn empty_input_returns_empty() {
        let result = substitute_placeholders("", &BTreeMap::new(), &empty_params()).unwrap();
        assert_eq!(result, "");
    }

    // --- Whitespace handling (3 tests) ---

    #[test]
    fn handles_whitespace_inside_braces() {
        let result = substitute_placeholders(
            "{{  name  }}",
            &vals(&[("name", "trimmed")]),
            &params(&["name"]),
        )
        .unwrap();
        assert_eq!(result, "trimmed");
    }

    #[test]
    fn handles_tab_inside_braces() {
        let result = substitute_placeholders(
            "{{\tname\t}}",
            &vals(&[("name", "tab")]),
            &params(&["name"]),
        )
        .unwrap();
        assert_eq!(result, "tab");
    }

    #[test]
    fn handles_newline_inside_braces() {
        let result =
            substitute_placeholders("{{\nname\n}}", &vals(&[("name", "nl")]), &params(&["name"]))
                .unwrap();
        assert_eq!(result, "nl");
    }

    // --- Parameter names (3 tests) ---

    #[test]
    fn accepts_underscore_in_name() {
        let result = substitute_placeholders(
            "{{my_param}}",
            &vals(&[("my_param", "ok")]),
            &params(&["my_param"]),
        )
        .unwrap();
        assert_eq!(result, "ok");
    }

    #[test]
    fn accepts_dot_in_name() {
        let result = substitute_placeholders(
            "{{config.host}}",
            &vals(&[("config.host", "127.0.0.1")]),
            &params(&["config.host"]),
        )
        .unwrap();
        assert_eq!(result, "127.0.0.1");
    }

    #[test]
    fn accepts_dash_in_name() {
        let result = substitute_placeholders(
            "{{my-param}}",
            &vals(&[("my-param", "dashed")]),
            &params(&["my-param"]),
        )
        .unwrap();
        assert_eq!(result, "dashed");
    }

    // --- Error cases (4 tests) ---

    #[test]
    fn errors_on_unknown_parameter() {
        let mut values = vals(&[("known", "val"), ("unknown", "val")]);
        values.insert("known".into(), "val".into());
        let err = substitute_placeholders("{{known}}", &values, &params(&["known"])).unwrap_err();
        assert!(matches!(err, TemplateError::UnknownParameter(ref n) if n == "unknown"));
    }

    #[test]
    fn errors_on_missing_parameter() {
        let err =
            substitute_placeholders("{{host}}", &BTreeMap::new(), &params(&["host"])).unwrap_err();
        assert!(matches!(err, TemplateError::MissingParameter(ref n) if n == "host"));
    }

    #[test]
    fn rejects_name_starting_with_digit() {
        // The regex requires [A-Za-z_] as first char, so {{1bad}} should NOT match.
        let result =
            substitute_placeholders("{{1bad}} stays", &BTreeMap::new(), &empty_params()).unwrap();
        assert_eq!(result, "{{1bad}} stays");
    }

    #[test]
    fn ignores_malformed_braces() {
        // Single brace or missing close-brace should not match.
        let result = substitute_placeholders(
            "{{broken and {not_a_placeholder}",
            &BTreeMap::new(),
            &empty_params(),
        )
        .unwrap();
        assert_eq!(result, "{{broken and {not_a_placeholder}");
    }

    // --- Edge cases (2 tests) ---

    #[test]
    fn substitutes_in_multiline_string() {
        let input = "line1: {{a}}\nline2: {{b}}\nline3: {{a}}";
        let result = substitute_placeholders(
            input,
            &vals(&[("a", "alpha"), ("b", "beta")]),
            &params(&["a", "b"]),
        )
        .unwrap();
        assert_eq!(result, "line1: alpha\nline2: beta\nline3: alpha");
    }

    #[test]
    fn substitution_with_empty_value() {
        let result =
            substitute_placeholders("prefix{{x}}suffix", &vals(&[("x", "")]), &params(&["x"]))
                .unwrap();
        assert_eq!(result, "prefixsuffix");
    }

    #[test]
    fn errors_on_unknown_placeholder_in_text() {
        let err = substitute_placeholders(
            "hello {{typo}}",
            &vals(&[("name", "world")]),
            &params(&["name"]),
        )
        .unwrap_err();
        assert!(matches!(err, TemplateError::UnknownParameter(ref n) if n == "typo"));
    }
}
