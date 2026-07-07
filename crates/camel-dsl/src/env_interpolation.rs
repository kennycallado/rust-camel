use regex::Regex;
use std::env;
use std::sync::OnceLock;

static ENV_RE: OnceLock<Regex> = OnceLock::new();

fn env_regex() -> &'static Regex {
    ENV_RE.get_or_init(|| Regex::new(r"\$\{env:([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}").unwrap()) // allow-unwrap
}

/// Replace line/paragraph-break chars with SPACE to prevent YAML
/// structural injection when values are spliced into raw YAML before parse.
///
/// Covers: `\n`, `\r`, `\0`, `\u{2028}`, `\u{2029}`.
///
/// Does NOT strip flow indicators (`[ ] { } ,`), plain-scalar colon
/// (`: `), comment truncation (` #`), or tag/anchor chars (`! & * %`).
/// These can alter YAML within a single line but cannot inject new
/// keys/structure (that requires newlines). Values are inserted into
/// already-trusted template positions, not arbitrary escaping.
fn sanitize_env_value(val: &str) -> String {
    val.chars()
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\0' | '\u{2028}' | '\u{2029}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Interpolates `${env:VAR_NAME}` placeholders in the source string.
///
/// Returns `Err(var_name)` if any referenced variable is not set.
pub fn interpolate_env(src: &str) -> Result<String, String> {
    let re = env_regex();
    let mut error: Option<String> = None;

    let result = re.replace_all(src, |caps: &regex::Captures| {
        if error.is_some() {
            return String::new();
        }
        let var_name = &caps[1];
        let default_value = caps.get(2).map(|m| m.as_str());
        match env::var(var_name) {
            Ok(val) => sanitize_env_value(&val),
            Err(_) => {
                if let Some(default) = default_value {
                    sanitize_env_value(default)
                } else {
                    error = Some(var_name.to_string());
                    String::new()
                }
            }
        }
    });

    if let Some(missing) = error {
        return Err(missing);
    }

    Ok(result.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_no_placeholders() {
        let result = interpolate_env("hello: world").unwrap();
        assert_eq!(result, "hello: world");
    }

    #[test]
    fn single_var_substitution() {
        unsafe { env::set_var("TEST_DSL_HOST", "localhost") };
        let result = interpolate_env("uri: ${env:TEST_DSL_HOST}/path").unwrap();
        assert_eq!(result, "uri: localhost/path");
        unsafe { env::remove_var("TEST_DSL_HOST") };
    }

    #[test]
    fn multiple_vars_all_set() {
        unsafe { env::set_var("TEST_DSL_USER", "admin") };
        unsafe { env::set_var("TEST_DSL_PASS", "secret") };
        let result = interpolate_env("${env:TEST_DSL_USER}:${env:TEST_DSL_PASS}").unwrap();
        assert_eq!(result, "admin:secret");
        unsafe { env::remove_var("TEST_DSL_USER") };
        unsafe { env::remove_var("TEST_DSL_PASS") };
    }

    #[test]
    fn missing_var_returns_err_with_name() {
        unsafe { env::remove_var("TEST_DSL_MISSING") };
        let err = interpolate_env("uri: ${env:TEST_DSL_MISSING}").unwrap_err();
        assert_eq!(err, "TEST_DSL_MISSING");
    }

    #[test]
    fn multiple_vars_first_missing_fails_fast() {
        unsafe { env::remove_var("TEST_DSL_FIRST") };
        unsafe { env::set_var("TEST_DSL_SECOND", "ok") };
        let err = interpolate_env("${env:TEST_DSL_FIRST} ${env:TEST_DSL_SECOND}").unwrap_err();
        assert_eq!(err, "TEST_DSL_FIRST");
        unsafe { env::remove_var("TEST_DSL_SECOND") };
    }

    #[test]
    fn default_value_used_when_var_missing() {
        unsafe { env::remove_var("TEST_DSL_DEFAULT_VAR") };
        let result = interpolate_env("uri: ${env:TEST_DSL_DEFAULT_VAR:-localhost}/path").unwrap();
        assert_eq!(result, "uri: localhost/path");
    }

    #[test]
    fn default_value_not_used_when_var_set() {
        unsafe { env::set_var("TEST_DSL_OVERRIDE", "production") };
        let result = interpolate_env("uri: ${env:TEST_DSL_OVERRIDE:-localhost}/path").unwrap();
        assert_eq!(result, "uri: production/path");
        unsafe { env::remove_var("TEST_DSL_OVERRIDE") };
    }

    #[test]
    fn empty_default_is_valid() {
        unsafe { env::remove_var("TEST_DSL_EMPTY_DEFAULT") };
        let result = interpolate_env("uri: ${env:TEST_DSL_EMPTY_DEFAULT:-}/path").unwrap();
        assert_eq!(result, "uri: /path");
    }

    #[test]
    fn no_default_still_errors() {
        unsafe { env::remove_var("TEST_DSL_NO_DEFAULT") };
        let err = interpolate_env("uri: ${env:TEST_DSL_NO_DEFAULT}/path").unwrap_err();
        assert_eq!(err, "TEST_DSL_NO_DEFAULT");
    }

    #[test]
    fn default_with_special_chars() {
        unsafe { env::remove_var("TEST_DSL_SPECIAL") };
        let result =
            interpolate_env("uri: ${env:TEST_DSL_SPECIAL:-host.example.com:9092}/path").unwrap();
        assert_eq!(result, "uri: host.example.com:9092/path");
    }

    #[test]
    fn interpolates_value_with_newline_replaced() {
        unsafe { env::set_var("TEST_DSL_INJECTION", "safe_value\ninjected_key: malicious") };
        let result = interpolate_env("uri: ${env:TEST_DSL_INJECTION}/path").unwrap();
        unsafe { env::remove_var("TEST_DSL_INJECTION") };
        assert!(
            !result.contains('\n'),
            "interpolated value must not contain newlines"
        );
        // Newline replaced with SPACE (not deleted)
        assert!(
            result.contains("safe_value injected_key"),
            "newline must be replaced with space, not removed"
        );
    }

    #[test]
    fn interpolates_value_replaces_all_control_chars() {
        // \0 cannot be set via env::set_var (C string terminator).
        // Test sanitize_env_value directly for all 5 chars including \0.
        let input = "a\rb\nb\0c\u{2028}d\u{2029}e";
        let sanitized = sanitize_env_value(input);
        assert!(!sanitized.contains('\r'), "CR must be replaced");
        assert!(!sanitized.contains('\n'), "LF must be replaced");
        assert!(!sanitized.contains('\0'), "NUL must be replaced");
        assert!(!sanitized.contains('\u{2028}'), "LS must be replaced");
        assert!(!sanitized.contains('\u{2029}'), "PS must be replaced");
        assert!(
            sanitized.contains("a b b c d e"),
            "all control chars must be replaced with space"
        );
    }

    #[test]
    fn mixed_defaults_and_non_defaults() {
        unsafe { env::remove_var("TEST_DSL_MIX_A") };
        unsafe { env::set_var("TEST_DSL_MIX_B", "actual") };
        let result =
            interpolate_env("${env:TEST_DSL_MIX_A:-fallback}:${env:TEST_DSL_MIX_B}").unwrap();
        assert_eq!(result, "fallback:actual");
        unsafe { env::remove_var("TEST_DSL_MIX_B") };
    }
}
