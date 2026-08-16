//! Endpoint URI value type: typed fail-closed merge of a base URI with a `parameters:` map
//! and deterministic canonical rendering.

use crate::component_metadata::{ComponentMetadata, ComponentMetadataCatalog, UriOption};
use crate::error::EndpointUriError;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

/// A parsed route endpoint URI: a scheme, a path, and a parameter map, plus the original
/// query bytes for byte-preserving rendering.
///
/// The private `raw_query` field stores the base URI's query exactly as it appeared (without
/// the leading `?`), or `None` when the base URI had no query. `to_canonical_string` replays
/// those bytes verbatim and then appends the `parameters:` entries in sorted order, so
/// rendering is deterministic and never rewrites the caller's original query.
///
/// ADR-0051 credential boundary: redacting-wrapper
#[non_exhaustive]
#[derive(Clone)]
pub struct EndpointUri {
    /// URI scheme: the non-empty substring before the first `:`.
    pub scheme: String,
    /// URI path: everything after the first `:` up to the first `?`.
    pub path: String,
    /// DSL `parameters:` map, keyed by parameter name, rendered in sorted order.
    pub params: BTreeMap<String, String>,
    /// Original query bytes (without the leading `?`), `None` when the base URI had no query.
    raw_query: Option<String>,
}

impl EndpointUri {
    /// Merge a base URI with a `parameters:` map, failing closed on malformed input.
    ///
    /// The base URI must have a non-empty scheme. Query pairs in the base URI are parsed only
    /// to detect empty keys and to reject `parameters:` keys that collide with a raw query key;
    /// the query text itself is preserved byte-for-byte for rendering.
    pub fn try_from_uri_and_params(
        base: &str,
        params: BTreeMap<String, String>,
    ) -> Result<Self, EndpointUriError> {
        let colon = base.find(':').ok_or(EndpointUriError::MissingScheme)?;
        let scheme = &base[..colon];
        if scheme.is_empty() {
            return Err(EndpointUriError::MissingScheme);
        }

        let rest = &base[colon + 1..];
        let (path, raw_query) = match rest.find('?') {
            Some(q) => (&rest[..q], Some(rest[q + 1..].to_string())),
            None => (rest, None),
        };

        // Collect the raw query keys (repeated keys are legal and preserved in order).
        let mut query_keys: Vec<String> = Vec::new();
        if let Some(query) = raw_query.as_deref() {
            for pair in query.split('&') {
                let key = pair.split_once('=').map_or(pair, |(key, _)| key);
                if key.is_empty() {
                    return Err(EndpointUriError::EmptyQueryKey);
                }
                query_keys.push(key.to_string());
            }
        }

        // Validate parameter keys before merging.
        for key in params.keys() {
            if key.is_empty()
                || key
                    .bytes()
                    .any(|b| matches!(b, b'&' | b'=' | b'%' | b'#' | b'?' | b'+' | b' '))
            {
                return Err(EndpointUriError::InvalidParamKey { key: key.clone() });
            }
        }

        // Fail closed on any parameter key colliding with a raw query key.
        for key in params.keys() {
            if query_keys.iter().any(|query_key| query_key == key) {
                return Err(EndpointUriError::DuplicateKey { key: key.clone() });
            }
        }

        Ok(EndpointUri {
            scheme: scheme.to_string(),
            path: path.to_string(),
            params,
            raw_query,
        })
    }

    /// Render the endpoint URI deterministically: `scheme:path`, then the raw query
    /// byte-for-byte (if any), then the parameter entries in `BTreeMap` sorted order.
    pub fn to_canonical_string(&self) -> String {
        let mut out =
            String::with_capacity(self.scheme.len() + self.path.len() + self.params.len() * 16);
        out.push_str(&self.scheme);
        out.push(':');
        out.push_str(&self.path);

        for (i, pair) in self.pairs().enumerate() {
            out.push(if i == 0 { '?' } else { '&' });
            push_pair(&mut out, &pair, |value| value);
        }

        out
    }

    /// Render the endpoint URI with every option value resolved against `catalog`
    /// and masked as `***` unless the catalog affirmatively resolves the key to a
    /// non-secret [`UriOption`].
    ///
    /// The option set is built from BOTH the raw query pairs and the `params` map,
    /// in the same rendering order and encoding as [`Self::to_canonical_string`]:
    /// raw query values are replayed verbatim (after redaction) while `params`
    /// values are percent-encoded. Fails safe: an unregistered scheme, an
    /// unresolved key, and a secret option all render as `***`.
    pub fn to_redacted_string(&self, catalog: &dyn ComponentMetadataCatalog) -> String {
        let metadata = catalog.get_metadata(&self.scheme);
        let mut out =
            String::with_capacity(self.scheme.len() + self.path.len() + self.params.len() * 16);
        out.push_str(&self.scheme);
        out.push(':');
        out.push_str(&mask_userinfo(&self.path));

        for (i, pair) in self.pairs().enumerate() {
            out.push(if i == 0 { '?' } else { '&' });
            push_pair(&mut out, &pair, |value| {
                redact_value(pair.key, value, metadata.as_ref())
            });
        }

        out
    }

    /// Iterate every renderable (key, value) pair in rendering order: raw-query
    /// pairs first (in their original order), then `params` entries in
    /// `BTreeMap` sorted order.
    fn pairs(&self) -> impl Iterator<Item = RenderPair<'_>> {
        let raw = self
            .raw_query
            .as_deref()
            .into_iter()
            .flat_map(|query| query.split('&'))
            .map(|pair| match pair.split_once('=') {
                Some((key, value)) => RenderPair {
                    key,
                    value: Some(value),
                    from_raw_query: true,
                },
                None => RenderPair {
                    key: pair,
                    value: None,
                    from_raw_query: true,
                },
            });
        let params = self.params.iter().map(|(key, value)| {
            // Construction validates param keys; guard against post-construction
            // mutation corrupting the canonical rendering (debug builds only).
            debug_assert!(
                !key.is_empty()
                    && !key
                        .bytes()
                        .any(|b| matches!(b, b'&' | b'=' | b'%' | b'#' | b'?' | b'+' | b' ')),
                "EndpointUri.params key violates the key policy: {key:?}"
            );
            RenderPair {
                key: key.as_str(),
                value: Some(value.as_str()),
                from_raw_query: false,
            }
        });
        raw.chain(params)
    }
}

impl fmt::Debug for EndpointUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EndpointUri")
            .field("scheme", &self.scheme)
            .field("path", &mask_userinfo(&self.path))
            .field("params", &RedactedParams(&self.params))
            .finish()
    }
}

/// Debug helper that renders an [`EndpointUri`]'s parameter map with every value
/// masked as `***` (keys stay visible). The private `raw_query` field is never
/// rendered, so its unmasked bytes cannot leak through `Debug`.
struct RedactedParams<'a>(&'a BTreeMap<String, String>);

impl fmt::Debug for RedactedParams<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for key in self.0.keys() {
            map.entry(key, &"***");
        }
        map.finish()
    }
}

/// Append `value` percent-encoding exactly the reserved characters `& = % # ? +` (uppercase
/// hex over their UTF-8 byte) and space (as `%20`); every other byte — including `:` and
/// multi-byte UTF-8 — passes through verbatim.
fn push_percent_encoded(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("%26"),
            '=' => out.push_str("%3D"),
            '%' => out.push_str("%25"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            '+' => out.push_str("%2B"),
            ' ' => out.push_str("%20"),
            _ => out.push(ch),
        }
    }
}

/// Resolve a URI option key to its catalog [`UriOption`] by exact name, then by
/// alias — the two-phase resolution camel-lint performs (implemented locally
/// because camel-api must not depend on camel-lint).
fn resolve_uri_option<'a>(key: &str, uri_options: &'a [UriOption]) -> Option<&'a UriOption> {
    uri_options
        .iter()
        .find(|uo| uo.pattern.is_none() && uo.name == key)
        .or_else(|| {
            uri_options
                .iter()
                .find(|uo| uo.pattern.is_none() && uo.aliases.iter().any(|alias| alias == key))
        })
}

/// Decide how to render a single option value: the original value when the
/// catalog resolves `key` to a non-secret [`UriOption`], otherwise `***`
/// (fail-safe: unknown scheme, unknown key, and secret options all mask).
fn redact_value<'a>(key: &str, value: &'a str, metadata: Option<&ComponentMetadata>) -> &'a str {
    let non_secret = metadata
        .and_then(|meta| resolve_uri_option(key, &meta.uri_options))
        .is_some_and(|opt| !opt.secret);
    if non_secret { value } else { "***" }
}

/// Mask any RFC 3986 userinfo in `path`: when the path begins with `//`, the
/// authority runs to the first subsequent `/`; if it contains `@`, the userinfo
/// (everything before that `@`) is replaced with `***`, keeping the `@` and the
/// host. Every other path is returned unchanged. Applied in `Debug` and the
/// redacted rendering so credentials cannot leak; the canonical rendering stays
/// byte-faithful and does not call this.
fn mask_userinfo(path: &str) -> Cow<'_, str> {
    let rest = match path.strip_prefix("//") {
        Some(rest) => rest,
        None => return Cow::Borrowed(path),
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let Some(at) = authority.find('@') else {
        return Cow::Borrowed(path);
    };
    let mut masked = String::with_capacity(path.len() + 3);
    masked.push_str("//***");
    masked.push_str(&authority[at..]);
    masked.push_str(&rest[authority_end..]);
    Cow::Owned(masked)
}

/// One renderable key/value pair in rendering order.
struct RenderPair<'a> {
    key: &'a str,
    /// `None` for a raw-query flag (a pair with no `=`).
    value: Option<&'a str>,
    /// True when the pair came from the raw query (value replayed verbatim)
    /// rather than the `params` map (value percent-encoded).
    from_raw_query: bool,
}

/// Render one pair to `out` as `key` or `key=value`. Raw-query values are
/// replayed verbatim; `params` values are percent-encoded. `transform` maps the
/// value first — identity for the canonical rendering, redaction for the
/// redacted rendering.
fn push_pair<'a, F>(out: &mut String, pair: &RenderPair<'a>, transform: F)
where
    F: Fn(&'a str) -> &'a str,
{
    out.push_str(pair.key);
    if let Some(value) = pair.value {
        out.push('=');
        if pair.from_raw_query {
            out.push_str(transform(value));
        } else {
            push_percent_encoded(out, transform(value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_metadata::{
        ComponentMetadata, ComponentMetadataCatalog, OptionKind, UriOption,
    };
    use std::collections::BTreeMap;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn merge_uri_and_params_canonical() {
        let uri = EndpointUri::try_from_uri_and_params(
            "kafka:orders",
            params(&[("brokers", "my-host:9092"), ("acks", "all")]),
        )
        .unwrap();
        assert_eq!(
            uri.to_canonical_string(),
            "kafka:orders?acks=all&brokers=my-host:9092"
        );
    }

    #[test]
    fn duplicate_key_fails_closed() {
        let err = EndpointUri::try_from_uri_and_params(
            "kafka:orders?brokers=a",
            params(&[("brokers", "b")]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            EndpointUriError::DuplicateKey {
                key: "brokers".to_string()
            }
        );
        assert!(err.to_string().contains("brokers"));
    }

    #[test]
    fn repeated_query_keys_preserved() {
        let uri =
            EndpointUri::try_from_uri_and_params("list:demo?item=a&item=b", params(&[])).unwrap();
        assert_eq!(uri.to_canonical_string(), "list:demo?item=a&item=b");
    }

    #[test]
    fn malformed_bases_rejected() {
        // No `:` anywhere — the scheme is absent.
        let err = EndpointUri::try_from_uri_and_params("noscheme", params(&[])).unwrap_err();
        assert_eq!(err, EndpointUriError::MissingScheme);
        assert!(err.to_string().contains("scheme"));

        // `:` first — the scheme is empty.
        let err = EndpointUri::try_from_uri_and_params(":pathonly", params(&[])).unwrap_err();
        assert_eq!(err, EndpointUriError::MissingScheme);
        assert!(err.to_string().contains("scheme"));

        // Query pair `=1` has an empty key.
        let err = EndpointUri::try_from_uri_and_params("timer:tick?=1", params(&[])).unwrap_err();
        assert_eq!(err, EndpointUriError::EmptyQueryKey);
        assert!(err.to_string().contains("empty key"));

        // Parameter key containing a reserved character.
        let err = EndpointUri::try_from_uri_and_params("kafka:orders", params(&[("a&b", "1")]))
            .unwrap_err();
        assert_eq!(
            err,
            EndpointUriError::InvalidParamKey {
                key: "a&b".to_string()
            }
        );
        assert!(err.to_string().contains("a&b"));
    }

    #[test]
    fn deterministic_across_insert_orders() {
        let a =
            EndpointUri::try_from_uri_and_params("x:y", params(&[("b", "2"), ("a", "1")])).unwrap();
        let b =
            EndpointUri::try_from_uri_and_params("x:y", params(&[("a", "1"), ("b", "2")])).unwrap();
        assert_eq!(a.to_canonical_string(), b.to_canonical_string());
    }

    #[test]
    fn existing_query_preserved_byte_identical() {
        let input = "timer:tick?period=1000&repeatCount=6";
        let uri = EndpointUri::try_from_uri_and_params(input, params(&[])).unwrap();
        assert_eq!(uri.to_canonical_string(), input);
    }

    #[test]
    fn golden_reserved_characters() {
        let uri = EndpointUri::try_from_uri_and_params(
            "http:srv?a=1&flag",
            params(&[("z", "100%"), ("q", "a b+c")]),
        )
        .unwrap();
        assert_eq!(
            uri.to_canonical_string(),
            "http:srv?a=1&flag&q=a%20b%2Bc&z=100%25"
        );
    }

    #[test]
    fn pair_without_equals_has_empty_value() {
        let uri = EndpointUri::try_from_uri_and_params("t:x?flag", params(&[("a", "1")])).unwrap();
        assert_eq!(uri.to_canonical_string(), "t:x?flag&a=1");
    }

    // -----------------------------------------------------------------------
    // Redaction tests — TDD: written before implementation
    // -----------------------------------------------------------------------

    struct StubCatalog {
        entries: BTreeMap<String, ComponentMetadata>,
    }

    impl ComponentMetadataCatalog for StubCatalog {
        fn get_metadata(&self, scheme: &str) -> Option<ComponentMetadata> {
            self.entries.get(scheme).cloned()
        }

        fn schemes(&self) -> Vec<String> {
            self.entries.keys().cloned().collect()
        }

        fn all_metadata(&self) -> Vec<ComponentMetadata> {
            self.entries.values().cloned().collect()
        }
    }

    /// `http`: `password` is secret, `timeout` is not; `token` is non-secret
    /// with alias `apikey`; `cfg` is a non-secret prefix-pattern anchor. Every
    /// other scheme is unregistered (so resolution fails safe and masks).
    fn stub_catalog() -> StubCatalog {
        let password = UriOption::new("password", "password", OptionKind::String).secret();
        let timeout = UriOption::new("timeout", "timeout", OptionKind::String);
        let token = UriOption::new("token", "token", OptionKind::String).with_alias("apikey");
        let cfg = UriOption::new("cfg", "cfg", OptionKind::String).pattern_prefix("cfg.");
        let meta = ComponentMetadata::minimal("http")
            .with_uri_options(vec![password, timeout, token, cfg]);
        let mut entries = BTreeMap::new();
        entries.insert("http".to_string(), meta);
        StubCatalog { entries }
    }

    #[test]
    fn debug_masks_all_param_values() {
        let uri = EndpointUri::try_from_uri_and_params(
            "http:srv?password=clear",
            params(&[("delay", "1000")]),
        )
        .unwrap();
        let debug = format!("{uri:?}");
        assert!(debug.contains("***"));
        assert!(!debug.contains("1000"));
        assert!(!debug.contains("clear"));
    }

    #[test]
    fn debug_and_redacted_mask_userinfo() {
        let catalog = stub_catalog();
        let uri =
            EndpointUri::try_from_uri_and_params("http://admin:hunter2@srv/path", params(&[]))
                .unwrap();
        let debug = format!("{uri:?}");
        let redacted = uri.to_redacted_string(&catalog);
        for out in [&debug, &redacted] {
            assert!(!out.contains("hunter2"), "userinfo leak in {out}");
            assert!(!out.contains("admin:"), "userinfo leak in {out}");
        }
        assert_eq!(uri.to_canonical_string(), "http://admin:hunter2@srv/path");
    }

    #[test]
    fn redacted_string_masks_secret_passes_non_secret() {
        let catalog = stub_catalog();
        let uri = EndpointUri::try_from_uri_and_params(
            "http:srv",
            params(&[("password", "hunter2"), ("timeout", "5000")]),
        )
        .unwrap();
        let out = uri.to_redacted_string(&catalog);
        assert!(out.contains("password=***"));
        assert!(out.contains("timeout=5000"));
    }

    #[test]
    fn redacted_string_unknown_scheme_masks() {
        let catalog = stub_catalog();
        let uri =
            EndpointUri::try_from_uri_and_params("not-a-scheme:dest", params(&[("token", "abc")]))
                .unwrap();
        let out = uri.to_redacted_string(&catalog);
        assert!(out.contains("token=***"));
        assert!(!out.contains("abc"));
    }

    #[test]
    fn redacted_string_masks_query_string_secrets() {
        let catalog = stub_catalog();
        let uri =
            EndpointUri::try_from_uri_and_params("http:srv?password=clear", params(&[])).unwrap();
        let out = uri.to_redacted_string(&catalog);
        assert!(out.contains("password=***"));
        assert!(!out.contains("clear"));
    }

    #[test]
    fn redacted_string_alias_resolves_pattern_anchor_does_not() {
        let catalog = stub_catalog();
        let uri = EndpointUri::try_from_uri_and_params(
            "http:srv",
            params(&[("apikey", "abc"), ("cfg.foo", "bar")]),
        )
        .unwrap();
        let out = uri.to_redacted_string(&catalog);
        assert!(out.contains("apikey=abc"));
        assert!(out.contains("cfg.foo=***"));
    }
}
