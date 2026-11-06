//! Process-global registry of embedded xslt/xsd/sql assets (mission 351).
//!
//! Sealed artifacts carry their xslt/xsd/sql resources as verbatim store
//! assets. At boot, `camel-cli` registers every xslt-, xsd-, and
//! sql-class entry here under its store path (`assets/xslt/t.xslt`)
//! from digest-verified store entries, and rewrites the corresponding
//! endpoint/site references to `camel-embedded:<asset path>`. The
//! in-process resolvers read such references from this registry — no
//! file is written for those classes, so a sealed artifact boots on a
//! host with no writable temp directory. This module mirrors the
//! `camel-proto-compiler::embedded` registry (mission 350) that serves
//! the proto class; the two live in separate leaf crates because the
//! proto compiler and this API crate cannot depend on each other.
//!
//! # Reserved reference prefix
//!
//! [`EMBEDDED_REF_PREFIX`] (`camel-embedded:`) and its percent-encoded
//! spelling (`camel-embedded%3A`) are reserved: a value that starts with
//! either spelling is resolved against this registry and never against
//! the filesystem. A real file whose name starts with either spelling is
//! therefore shadowed — an accepted reservation, documented here and in
//! the consuming components' acceptance rules.
//!
//! Resolution fails closed: a reference whose key was never registered
//! resolves to `None`, and the caller reports a boot error instead of
//! falling back to the host filesystem.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Prefix marking an in-memory asset reference: `camel-embedded:<asset
/// path>`. Reserved across the workspace; see the module documentation.
pub const EMBEDDED_REF_PREFIX: &str = "camel-embedded:";

/// Percent-encoded spelling of [`EMBEDDED_REF_PREFIX`], used when the
/// reference travels inside a URI component.
const ENCODED_REF_PREFIX: &str = "camel-embedded%3A";

type Registry = HashMap<String, Arc<[u8]>>;

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Registers `bytes` under `name` (the store asset path, e.g.
/// `assets/xslt/t.xslt`). A later registration for the same name
/// replaces the earlier bytes. Idempotent and infallible: the registry
/// is populated once per boot from verified store entries.
pub fn register(name: &str, bytes: Vec<u8>) {
    let mut guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    guard.insert(name.to_owned(), Arc::from(bytes.into_boxed_slice()));
}

/// Returns the bytes registered under `name`, or `None` when the name is
/// not registered (the reference then fails closed at resolution).
pub fn get(name: &str) -> Option<Arc<[u8]>> {
    let guard = registry().lock().unwrap_or_else(|p| p.into_inner());
    guard.get(name).cloned()
}

/// Resolves an embedded asset reference to its registered bytes.
///
/// Two postures, distinguished by spelling:
///
/// - a value starting with the canonical [`EMBEDDED_REF_PREFIX`] is a
///   literal registry key after prefix removal; the suffix is NEVER
///   decoded (store paths may contain spaces, Unicode, colons, and
///   literal `%` sequences, and decoding would corrupt the key);
/// - a value starting with the encoded prefix (`camel-embedded%3A`) is
///   percent-decoded in full, exactly once, and the canonical prefix is
///   then stripped from the result. Invalid UTF-8 after decoding yields
///   `None`.
///
/// Returns `None` when the value carries neither spelling or when the
/// key is not registered (fail closed; no filesystem fallback).
pub fn resolve_ref(value: &str) -> Option<Arc<[u8]>> {
    if let Some(name) = value.strip_prefix(EMBEDDED_REF_PREFIX) {
        return get(name);
    }
    if value.starts_with(ENCODED_REF_PREFIX) {
        let decoded = percent_encoding::percent_decode_str(value)
            .decode_utf8()
            .ok()?;
        let name = decoded.strip_prefix(EMBEDDED_REF_PREFIX)?;
        return get(name);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{register, resolve_ref};
    use std::sync::Arc;

    fn bytes(value: Option<Arc<[u8]>>) -> Vec<u8> {
        value.expect("registered reference resolves").to_vec()
    }

    #[test]
    fn encoded_prefix_is_the_canonical_prefix_with_encoded_colon() {
        assert_eq!(
            super::ENCODED_REF_PREFIX,
            super::EMBEDDED_REF_PREFIX.replace(':', "%3A")
        );
    }

    #[test]
    fn resolve_ref_resolves_canonical_and_percent_encoded_spellings() {
        register("assets/xslt/t.xslt", b"stylesheet-bytes".to_vec());
        assert_eq!(
            &bytes(resolve_ref("camel-embedded:assets/xslt/t.xslt"))[..],
            b"stylesheet-bytes"
        );
        assert_eq!(
            &bytes(resolve_ref("camel-embedded%3Aassets%2Fxslt%2Ft.xslt"))[..],
            b"stylesheet-bytes"
        );
    }

    #[test]
    fn resolve_ref_canonical_suffix_is_literal() {
        register("assets/x/a%20b.xslt", b"pct".to_vec());
        register("assets/x/a b.xslt", b"space".to_vec());
        assert_eq!(
            &bytes(resolve_ref("camel-embedded:assets/x/a%20b.xslt"))[..],
            b"pct",
            "the canonical suffix must never be percent-decoded"
        );
        assert_eq!(
            &bytes(resolve_ref("camel-embedded:assets/x/a b.xslt"))[..],
            b"space"
        );
    }

    #[test]
    fn resolve_ref_encoded_handles_spaces_unicode_colon_and_double_encode() {
        register("assets/styles/a b.xslt", b"s".to_vec());
        register("assets/styles/ünïcode.xslt", b"u".to_vec());
        register("assets/styles/a:b.xslt", b"c".to_vec());
        register("assets/x/lit%20.xslt", b"litpct".to_vec());
        register("assets/x/lit.xslt", b"lit".to_vec());
        // The two `lit` fixtures back the literal canonical spellings: the
        // percent sequence in the first key stays undecoded.
        assert_eq!(
            &bytes(resolve_ref("camel-embedded:assets/x/lit%20.xslt"))[..],
            b"litpct"
        );
        assert_eq!(
            &bytes(resolve_ref("camel-embedded:assets/x/lit.xslt"))[..],
            b"lit"
        );
        // Re-registered with the same bytes so this test does not depend on
        // the sibling test's registration order (the registry is global).
        register("assets/x/a%20b.xslt", b"pct".to_vec());
        register("assets/x/a b.xslt", b"space".to_vec());

        assert_eq!(
            &bytes(resolve_ref("camel-embedded%3Aassets%2Fstyles%2Fa%20b.xslt"))[..],
            b"s"
        );
        assert_eq!(
            &bytes(resolve_ref(
                "camel-embedded%3Aassets%2Fstyles%2F%C3%BCn%C3%AFcode.xslt"
            ))[..],
            b"u"
        );
        assert_eq!(
            &bytes(resolve_ref("camel-embedded%3Aassets%2Fstyles%2Fa%3Ab.xslt"))[..],
            b"c"
        );

        // Decode-once: `%2520` → `%20`, so the literal key (with the percent
        // sequence) is looked up and resolves to the `pct` registration.
        assert_eq!(
            &bytes(resolve_ref("camel-embedded%3Aassets%2Fx%2Fa%2520b.xslt"))[..],
            b"pct"
        );
        assert_eq!(
            &bytes(resolve_ref("camel-embedded%3Aassets%2Fx%2Fa%20b.xslt"))[..],
            b"space"
        );

        // `%253A` decodes exactly once to the literal key
        // `assets/x/lit%3A.xslt` (the `lit` fixtures above are unrelated —
        // they only back the canonical assertions). That key is never
        // registered, so resolution fails closed.
        assert!(
            resolve_ref("camel-embedded%3Aassets%2Fx%2Flit%253A.xslt").is_none(),
            "a double-encoded reference must decode exactly once"
        );
    }

    #[test]
    fn resolve_ref_fails_closed_on_unknown_or_plain_values() {
        assert!(resolve_ref("plain/path").is_none());
        assert!(resolve_ref("camel-embedded:assets/never-registered").is_none());
        assert!(
            resolve_ref("camel-embedded%3Aassets%2Fx%2F%FFbad").is_none(),
            "invalid UTF-8 after percent-decoding must fail closed"
        );
    }
}
