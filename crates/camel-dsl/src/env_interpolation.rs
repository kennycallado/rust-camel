use regex::Regex;
use std::env;
use std::sync::OnceLock;

static ENV_RE: OnceLock<Regex> = OnceLock::new();

fn env_regex() -> &'static Regex {
    ENV_RE.get_or_init(|| Regex::new(r"\$\{env:([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}").unwrap())
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
            Ok(val) => val,
            Err(_) => {
                if let Some(default) = default_value {
                    default.to_string()
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
    fn mixed_defaults_and_non_defaults() {
        unsafe { env::remove_var("TEST_DSL_MIX_A") };
        unsafe { env::set_var("TEST_DSL_MIX_B", "actual") };
        let result =
            interpolate_env("${env:TEST_DSL_MIX_A:-fallback}:${env:TEST_DSL_MIX_B}").unwrap();
        assert_eq!(result, "fallback:actual");
        unsafe { env::remove_var("TEST_DSL_MIX_B") };
    }
}
