// SYNC: `interpolate_env_with` below mirrors
// camel-dsl::env_interpolation::interpolate_env_with (rc-93wct); crate
// purity forbids the dependency. Update both together.
// `interpolated_validation_copy` / `whole_scalar_env_token` are lint-side
// helpers (validation copy + typing mirror) with no camel-dsl counterpart.
// The clean-integer gate for the typing-mirror integer-position carve-out
// lives in rschema.rs (`clean_integer`) — SYNC'd with camel-dsl
// `env_int_probe::clean_integer` and camel-config `clean_i64`; this file
// has no clean-integer logic and needs no behavioral counterpart.

use regex::Regex;
use std::sync::OnceLock;

static ENV_RE: OnceLock<Regex> = OnceLock::new();

pub(crate) fn env_regex() -> &'static Regex {
    // Escape alternatives exist so `$${env:X}` never falls through to plain
    // resolution; the full escape form is listed before bare `$$` so it is
    // consumed atomically.
    ENV_RE.get_or_init(|| {
        Regex::new(r"(\$\$\{env:[^}]*\})|(\$\$)|(\$\{env:([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\})")
            .unwrap() // allow-unwrap
    })
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
pub(crate) fn sanitize_env_value(val: &str) -> String {
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

/// Lookup-injectable interpolation of `${env:VAR_NAME}` placeholders.
///
/// `$${env:VAR_NAME}` yields the literal text `${env:VAR_NAME}` (escape),
/// and a standalone `$$` yields a single `$`.
///
/// Returns `Err(var_name)` if any referenced variable is not resolved.
///
/// SYNC mirror of `camel_dsl::env_interpolation::interpolate_env_with` —
/// keep both arms byte-equivalent. No production consumer in this crate
/// (R-SCHEMA goes through [`interpolated_validation_copy`]); it exists as
/// the parity anchor exercised by `mirror_parity_table`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn interpolate_env_with(
    src: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let re = env_regex();
    let mut error: Option<String> = None;

    let result = re.replace_all(src, |caps: &regex::Captures| {
        if error.is_some() {
            return String::new();
        }
        // `$${env:...}` escape: emit the literal placeholder text (strip one `$`).
        if let Some(escaped) = caps.get(1) {
            return escaped.as_str()[1..].to_string();
        }
        // Standalone `$$` escape: emit a single `$`.
        if caps.get(2).is_some() {
            return "$".to_string();
        }
        let var_name = &caps[4];
        let default_value = caps.get(5).map(|m| m.as_str());
        match lookup(var_name) {
            Some(val) => sanitize_env_value(&val),
            None => {
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

/// One `${env:VAR:-default}` token resolved while building a validation copy.
pub(crate) struct SubstitutedDefault {
    pub var: String,
    pub default: String,
}

/// Build the per-token validation copy R-SCHEMA validates.
///
/// `${env:X:-d}` tokens resolve to `d` (recorded in the returned list);
/// `$${env:...}` and `$$` escapes apply; a no-default token is left
/// literally untouched so schema validation still flags the genuinely
/// undefined variable. Per-token semantics: a whole-document `Err → raw`
/// fallback would re-literal defaulted tokens too, which the route-lint
/// mixed-document scenario forbids.
pub(crate) fn interpolated_validation_copy(raw: &str) -> (String, Vec<SubstitutedDefault>) {
    let re = env_regex();
    let mut substituted = Vec::new();
    let copy = re
        .replace_all(raw, |caps: &regex::Captures| {
            // `$${env:...}` escape: emit the literal placeholder text (strip one `$`).
            if let Some(escaped) = caps.get(1) {
                return escaped.as_str()[1..].to_string();
            }
            // Standalone `$$` escape: emit a single `$`.
            if caps.get(2).is_some() {
                return "$".to_string();
            }
            // No-default token: leave the whole match literally untouched.
            let Some(default) = caps.get(5) else {
                return caps[0].to_string();
            };
            substituted.push(SubstitutedDefault {
                var: caps[4].to_string(),
                default: default.as_str().to_string(),
            });
            sanitize_env_value(default.as_str())
        })
        .into_owned();
    (copy, substituted)
}

/// A whole-scalar `${env:...}` token parsed from an authored value scalar
/// (rc-93wct rev 2 typing mirror).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WholeScalarEnvToken {
    /// `${env:VAR:-default}` — substituted; the typing mirror validates the
    /// leaf as the STRING `default` (numeric/boolean re-inference
    /// suppressed, replicating the boot tree-walk).
    WithDefault { default: String },
    /// `${env:VAR}` — no default: unresolved at every value position (boot
    /// hard-fails on it).
    NoDefault { var: String },
}

static WHOLE_SCALAR_RE: OnceLock<Regex> = OnceLock::new();

fn whole_scalar_regex() -> &'static Regex {
    // Anchored both ends: the ENTIRE scalar must be one token. A leading
    // `$$` (escape) breaks the exact match, so `$${env:X}` never parses as
    // a plain token.
    WHOLE_SCALAR_RE.get_or_init(|| {
        Regex::new(r"^\$\{env:([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}$").unwrap() // allow-unwrap
    })
}

/// Detect an authored scalar that is EXACTLY one `${env:...}` token.
///
/// `authored` is the raw CST slice of a value leaf; surrounding whitespace
/// and ONE pair of matching quotes (`'` / `"`) are trimmed first, so
/// quoted scalars (`"${env:X:-d}"`) and flow-style scalars count as
/// whole-scalar too. A token embedded in a larger string
/// (`${env:X:-a}/b`), two concatenated tokens, and `$${env:...}` escapes
/// are NOT whole-scalar.
pub(crate) fn whole_scalar_env_token(authored: &str) -> Option<WholeScalarEnvToken> {
    let trimmed = authored.trim();
    let unquoted = strip_one_quote_pair(trimmed);
    let caps = whole_scalar_regex().captures(unquoted)?;
    Some(match caps.get(2) {
        Some(default) => WholeScalarEnvToken::WithDefault {
            default: default.as_str().to_string(),
        },
        None => WholeScalarEnvToken::NoDefault {
            var: caps[1].to_string(),
        },
    })
}

/// Strip one layer of matching surrounding quotes, if present.
fn strip_one_quote_pair(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_parity_table() {
        // Replicates camel-dsl::env_interpolation vectors with identical
        // inputs/outputs, resolved through the injected lookup (the mirror
        // never reads the process environment — determinism gate).
        // Passthrough.
        assert_eq!(
            interpolate_env_with("hello: world", &|_| None).unwrap(),
            "hello: world"
        );
        // Single-var substitution.
        assert_eq!(
            interpolate_env_with("uri: ${env:HOST}/path", &|name| (name == "HOST")
                .then(|| "localhost".to_string()))
            .unwrap(),
            "uri: localhost/path"
        );
        // Default fallback.
        assert_eq!(
            interpolate_env_with("uri: ${env:MISSING:-localhost}/path", &|_| None).unwrap(),
            "uri: localhost/path"
        );
        // Empty default.
        assert_eq!(
            interpolate_env_with("uri: ${env:MISSING:-}/path", &|_| None).unwrap(),
            "uri: /path"
        );
        // `$$` escape yields a single `$`.
        assert_eq!(interpolate_env_with("a$$b", &|_| None).unwrap(), "a$b");
        // `$${env:X}` escape yields the literal placeholder.
        assert_eq!(
            interpolate_env_with("$${env:X}", &|name| (name == "X")
                .then(|| "real-val".to_string()))
            .unwrap(),
            "${env:X}"
        );
        // Unset with no default is Err(var).
        assert_eq!(
            interpolate_env_with("uri: ${env:NOPE}", &|_| None).unwrap_err(),
            "NOPE"
        );
    }

    #[test]
    fn whole_scalar_detection_edges() {
        use WholeScalarEnvToken::{NoDefault, WithDefault};
        // Plain whole-scalar with default.
        assert_eq!(
            whole_scalar_env_token("${env:X:-2}"),
            Some(WithDefault {
                default: "2".to_string()
            })
        );
        // Quoted whole-scalar (double and single) with surrounding
        // whitespace — quotes are trimmed before the exact match.
        assert_eq!(
            whole_scalar_env_token("  \"${env:X:-d}\"  "),
            Some(WithDefault {
                default: "d".to_string()
            })
        );
        assert_eq!(
            whole_scalar_env_token("'${env:X:-}'"),
            Some(WithDefault {
                default: String::new()
            })
        );
        // No default.
        assert_eq!(
            whole_scalar_env_token("${env:NOPE}"),
            Some(NoDefault {
                var: "NOPE".to_string()
            })
        );
        // Escape: the leading `$$` breaks the exact match.
        assert_eq!(whole_scalar_env_token("$${env:X:-2}"), None);
        // Tokens embedded in a larger string are NOT whole-scalar.
        assert_eq!(whole_scalar_env_token("${env:X:-a}/b"), None);
        assert_eq!(whole_scalar_env_token("pre ${env:X}"), None);
        // Two concatenated tokens are not one whole-scalar token.
        assert_eq!(whole_scalar_env_token("${env:A:-1}${env:B:-2}"), None);
        // Non-token scalars (including empty miss spans) never match.
        assert_eq!(whole_scalar_env_token(""), None);
        assert_eq!(whole_scalar_env_token("plain"), None);
    }
}
