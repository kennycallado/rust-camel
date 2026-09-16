//! Canonical string-based URL redaction for diagnostic surfaces (ADR-0051).
//!
//! String surgery over raw URL bytes: userinfo in every authority window
//! is masked as `***`, query/fragment content is replaced with
//! `?[redacted]` / `#[redacted]` sentinels, and the result is capped at
//! 256 bytes on a UTF-8 char boundary. Authority windows are enumerated
//! over maximal runs of `/` and `\` characters: pure-slash runs (the
//! landed `//` rule) open a window at two or more characters,
//! backslash-bearing runs only behind an RFC 3986 scheme prefix. These
//! helpers never parse — they are the strictest feasible handling for
//! strings that may be malformed or hostile.
//!
//! Distinct from [`crate::endpoint_uri::EndpointUri::to_redacted_string`],
//! which redacts the catalog-driven authored-URI layer (structured
//! `EndpointUri` values) and is out of scope here.

/// Return `input` with userinfo masked in every authority window. Windows
/// are enumerated over maximal runs of `/` and `\` characters (see
/// [`authority_windows`]); a window containing `@` carries userinfo, and
/// the bytes from window start through the LAST `@` are replaced with
/// `***` (over-masking is safe, under-masking is not). Windows are
/// collected on the input and masked in reverse offset order (with
/// duplicates deduped) so an edit never shifts a yet-to-be-processed
/// window. Idempotent: an already-masked `***@host` window rewrites to
/// itself.
fn mask_authority_windows(input: &str) -> String {
    let mut out = input.to_string();
    let mut windows = authority_windows(input);
    // The run scanner emits one window per maximal run in ascending order;
    // sort+dedup are belt-and-braces.
    windows.sort_unstable();
    windows.dedup();
    for (start, end) in windows.into_iter().rev() {
        if let Some(at) = out[start..end].rfind('@') {
            out.replace_range(start..start + at, "***");
        }
    }
    out
}

/// Authority-window enumeration (bd rc-f05q8). Windows are derived from
/// maximal runs of `/` and `\` characters, and a qualifying run opens
/// exactly one window that ends at the next `/`, `?`, or `#` (or end of
/// input). A pure-slash run qualifies at length >= 2 (the landed `//`
/// rule) and its window starts at the run's end, keeping the slashes
/// visible. A backslash-bearing run qualifies only behind an RFC 3986
/// scheme prefix ending immediately before the run (see
/// [`scheme_prefix_len`]): length >= 2 needs any scheme; a single `\`
/// needs a scheme of two or more characters, or a one-character scheme
/// whose candidate window content is credential-shaped (see
/// [`single_backslash_credential_gate`]). Every qualifying window starts
/// at the run's end — the spec's uniform rule: a window begins
/// immediately after a maximal run of `/` and `\` — so the introducer
/// slashes and backslashes stay visible. Scheme-less backslash runs —
/// Windows drive paths and UNC paths — never open a window.
fn authority_windows(input: &str) -> Vec<(usize, usize)> {
    let bytes = input.as_bytes();
    let mut windows = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !matches!(bytes[i], b'/' | b'\\') {
            i += 1;
            continue;
        }
        let run_start = i;
        let mut has_backslash = false;
        while i < bytes.len() && matches!(bytes[i], b'/' | b'\\') {
            has_backslash |= bytes[i] == b'\\';
            i += 1;
        }
        let run_len = i - run_start;
        let opens = if has_backslash {
            match scheme_prefix_len(input, run_start) {
                None => false,
                Some(scheme_len) => {
                    run_len >= 2
                        || scheme_len >= 2
                        || (scheme_len == 1 && single_backslash_credential_gate(input, i))
                }
            }
        } else {
            run_len >= 2
        };
        if opens {
            let end = input[i..]
                .find(['/', '?', '#'])
                .map_or(input.len(), |offset| i + offset);
            // Every window starts at the run's end (spec: a window begins
            // immediately after a maximal run of `/` and `\`), so the
            // introducer slashes and backslashes stay visible.
            let start = i;
            windows.push((start, end));
        }
    }
    windows
}

/// Length of the RFC 3986 scheme name (`[a-zA-Z][a-zA-Z0-9+.-]*`) whose
/// `:` ends immediately before `run_start` — the LONGEST valid match, so
/// the name is the maximal run of scheme characters directly before the
/// colon and its first character must be alphabetic. `None` when
/// `input[..run_start]` does not end with `scheme:`.
fn scheme_prefix_len(input: &str, run_start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    if run_start < 2 || bytes[run_start - 1] != b':' {
        return None;
    }
    let mut start = run_start - 1;
    while start > 0
        && (bytes[start - 1].is_ascii_alphanumeric()
            || matches!(bytes[start - 1], b'+' | b'.' | b'-'))
    {
        start -= 1;
    }
    let len = run_start - 1 - start;
    if len == 0 || !bytes[start].is_ascii_alphabetic() {
        return None;
    }
    Some(len)
}

/// Credential-shaped content gate for a single `\` behind a
/// one-character scheme (bd rc-f05q8): the candidate window content —
/// from `content_start` (the run's end) to the next `/`, `?`, or `#` —
/// must carry a `:` strictly before its LAST `@`. Without an `@` there is
/// no userinfo to hide, so the gate fails.
fn single_backslash_credential_gate(input: &str, content_start: usize) -> bool {
    let end = input[content_start..]
        .find(['/', '?', '#'])
        .map_or(input.len(), |offset| content_start + offset);
    let window = &input[content_start..end];
    match window.rfind('@') {
        Some(at) => window[..at].contains(':'),
        None => false,
    }
}

/// Truncate `s` to at most `max` bytes, walking the cut down to the nearest
/// UTF-8 char boundary so a multibyte character straddling the cap cannot
/// panic.
fn truncate_utf8_safe(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

/// Single left-to-right pass over `bytes` that decodes `%HH` sequences
/// where `decode(hi, lo)` yields a byte; every other byte is copied
/// through unchanged. `decode` is consulted only when `%` is followed by
/// two bytes — a trailing `%`, a stray `%`, and an undecodable pair all
/// fall through to the literal copy. The caller decides which escapes
/// count (see [`minimal_decode_pair`] and [`decode_match_key`]).
fn percent_scan(bytes: &[u8], decode: impl Fn(u8, u8) -> Option<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = decode(bytes[i + 1], bytes[i + 2])
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Single left-to-right pass over a query pair that decodes ONLY the three
/// sequences a credential URL cannot be recognized without — `%40` → `@`,
/// `%3a`/`%3A` → `:`, `%2f`/`%2F` → `/` — and copies every other byte
/// through (other `%HH` escapes, stray `%`, and incomplete escapes
/// included). Valid UTF-8 is preserved byte-for-byte; invalid bytes become
/// `U+FFFD`, which cannot match an ASCII needle, so the shape checks below
/// are unaffected. This "minimal decode" (bd rc-r7v8s) is deliberately
/// weaker than a full percent-decoder: it exists only so the allowlist
/// redactor can recognize credential-shaped values hidden behind encoding,
/// while never giving a benign pair a reason to change. Double-encoded
/// input (`%2540`) stays encoded — out of scope by design.
fn minimal_decode_pair(pair: &str) -> String {
    let decoded = percent_scan(pair.as_bytes(), |hi, lo| match (hi, lo) {
        (b'4', b'0') => Some(b'@'),
        (b'3', b'a' | b'A') => Some(b':'),
        (b'2', b'f' | b'F') => Some(b'/'),
        _ => None,
    });
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Match key for the sensitive-substring check: a single left-to-right
/// `%HH` decode over the raw key bytes (ANY two hex digits after `%`,
/// case-insensitive; invalid sequences copied verbatim), then lowercased.
/// Unlike [`minimal_decode_pair`] this is a full digit decode because the
/// denylist matches literal text (`pass%77ord` must hit `password`), not
/// credential shape. The result is only used for substring matching and is
/// never rendered; invalid UTF-8 from a decoded byte becomes `U+FFFD`,
/// which cannot match an ASCII needle.
fn decode_match_key(raw_key: &str) -> String {
    let decoded = percent_scan(raw_key.as_bytes(), |hi, lo| {
        let hi = (hi as char).to_digit(16)?;
        let lo = (lo as char).to_digit(16)?;
        Some((hi * 16 + lo) as u8)
    });
    String::from_utf8_lossy(&decoded).to_lowercase()
}

/// Canonical strict redaction of a raw URL string for diagnostic surfaces
/// (ADR-0051, bd rc-eh49): authority windows are enumerated over maximal
/// runs of `/` and `\` characters — a pure-slash run of two or more
/// characters opens a window, a backslash-bearing run opens one only
/// behind an RFC 3986 scheme prefix (`[a-zA-Z][a-zA-Z0-9+.-]*:`) ending
/// immediately before the run (a single `\` additionally needs a scheme of
/// two-plus characters, or a one-character scheme whose candidate window
/// content is credential-shaped) — and each window ends at the next `/`,
/// `?`, or `#`. A window containing `@` carries userinfo,
/// and the bytes from window start through the LAST `@` are masked in
/// place as `***@` (over-masking is safe, under-masking is not). Every
/// window is scanned, so credentials cannot hide in a later window behind
/// a benign first one. Everything from the earliest `?` or `#` is dropped;
/// the sentinels
/// compose: each distinct introducer character (`?` and/or `#`) that occurs
/// anywhere in the URL appends its matching `?[redacted]` / `#[redacted]`
/// sentinel in first-occurrence order — queries and fragments routinely
/// carry tokens. The result is capped at 256 bytes on a UTF-8 char
/// boundary. There is no URL parser here, so in-place windowed masking is
/// the strictest feasible handling. This is the string layer;
/// [`crate::endpoint_uri::EndpointUri::to_redacted_string`] redacts the
/// catalog-driven authored-URI surface instead.
pub fn redact_url(raw: &str) -> String {
    let mut out = mask_authority_windows(raw);
    if let Some(i) = out.find(['?', '#']) {
        // Compose-both: one sentinel per distinct introducer found in the
        // raw URL, in first-occurrence order.
        let query_pos = out.find('?');
        let fragment_pos = out.find('#');
        out.truncate(i);
        // Reserve the sentinel bytes before truncating so the cap never
        // splits an appended sentinel (e_gpt stage-4).
        let sentinel_total = match (query_pos, fragment_pos) {
            (Some(_), Some(_)) => 22,
            (Some(_), None) | (None, Some(_)) => 11,
            (None, None) => 0,
        };
        if sentinel_total > 0 {
            truncate_utf8_safe(&mut out, 256 - sentinel_total);
        }
        match (query_pos, fragment_pos) {
            (Some(q), Some(f)) if f < q => out.push_str("#[redacted]?[redacted]"),
            (Some(_), Some(_)) => out.push_str("?[redacted]#[redacted]"),
            (Some(_), None) => out.push_str("?[redacted]"),
            (None, Some(_)) => out.push_str("#[redacted]"),
            (None, None) => {}
        }
    }
    truncate_utf8_safe(&mut out, 256);
    out
}

/// Fail-closed variant of [`redact_url`]: when any authority window of
/// `raw` carries an `@`, the whole string is replaced with `[redacted]` —
/// a string with an unvalidated authority marker may carry credentials
/// nothing validated, so nothing of it is rendered (deliberate fail-closed
/// over-redaction per ADR-0051). Otherwise identical to [`redact_url`].
pub fn redact_url_fail_closed(raw: &str) -> String {
    if window_has_at_sign(raw) {
        "[redacted]".to_string()
    } else {
        redact_url(raw)
    }
}

/// Broker-style redaction: window-mask userinfo like [`redact_url`], then
/// redact sensitive query params per key while keeping benign ones. A pair
/// whose match key — the raw key single-pass `%HH`-decoded then lowercased
/// (bd rc-r7v8s) — contains any of `sensitive_key_substrings` renders as
/// `{raw_key}=<redacted>` (the key keeps its original encoded bytes).
/// Otherwise, if the pair's `minimal_decode_pair` output is
/// credential-shaped (contains `@` and also `:` or `//`), the whole pair is
/// replaced with a bare `<redacted>`: encoded or literal
/// `user:secret@host` values must not survive under a benign key, while a
/// lone `@` (an email address) keeps the pair visible. Every other pair
/// survives byte-for-byte, because
/// non-secret transport policy in query params (ActiveMQ failover URIs) is
/// the sole diagnostic value of logging the broker URL. Fragments are never
/// echoed: everything from the first `#` is dropped and replaced with the
/// `#[redacted]` sentinel. The result is capped at 256 bytes on a UTF-8
/// char boundary. String-based; no URL parser.
pub fn redact_url_with_query_allowlist(raw: &str, sensitive_key_substrings: &[&str]) -> String {
    // 1) Mask userinfo in every authority window (see [`mask_authority_windows`]).
    let after_userinfo = mask_authority_windows(raw);
    // 2) Redact sensitive query params, keep the rest for diagnosability.
    let mut out = match after_userinfo.split_once('?') {
        Some((base, query)) => {
            let redacted: Vec<String> = query
                .split('&')
                .map(|pair| {
                    let raw_key = pair.split('=').next().unwrap_or(pair);
                    let match_key = decode_match_key(raw_key);
                    if sensitive_key_substrings
                        .iter()
                        .any(|s| match_key.contains(s))
                    {
                        format!("{raw_key}=<redacted>")
                    } else {
                        let decoded = minimal_decode_pair(pair);
                        if decoded.contains('@')
                            && (decoded.contains(':') || decoded.contains("//"))
                        {
                            "<redacted>".to_string()
                        } else {
                            pair.to_string()
                        }
                    }
                })
                .collect();
            format!("{base}?{}", redacted.join("&"))
        }
        None => after_userinfo,
    };
    // 3) Fragments are never echoed. Reserve the sentinel bytes before
    // truncating so the cap never splits the appended `#[redacted]`
    // (e_gpt stage-4); the kept query content is part of the base.
    if let Some(i) = out.find('#') {
        out.truncate(i);
        truncate_utf8_safe(&mut out, 256 - 11);
        out.push_str("#[redacted]");
    }
    truncate_utf8_safe(&mut out, 256);
    out
}

/// Whether a `@` appears in any authority window of `raw`. Windows come
/// from maximal runs of `/` and `\` characters — the same enumeration the
/// mask uses: pure-slash runs of two or more characters always open a
/// window; backslash-bearing runs open one only behind an RFC 3986 scheme
/// prefix ending immediately before the run. Every run is scanned, so
/// credentials cannot hide in a later window behind a benign first one
/// (`http://h/a//user:pass@e/`) or behind a scheme-prefixed backslash
/// authority (`foo:\u:p@e/`), while scheme-less drive and UNC paths stay
/// window-free.
pub fn window_has_at_sign(raw: &str) -> bool {
    authority_windows(raw)
        .into_iter()
        .any(|(start, end)| raw[start..end].contains('@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Key denylist used by the camel-jms broker redactor (7 entries, same
    /// order as the landed `redact_broker_url` list).
    const JMS_KEYS: &[&str] = &[
        "password",
        "passwd",
        "secret",
        "credential",
        "token",
        "username",
        "user",
    ];

    // ── Spec-pinned scenarios (redact2 security requirement) ─────────────

    #[test]
    fn strict_masks_windows_and_composes_sentinels() {
        assert_eq!(
            redact_url("https://user:pass@h/p?a=1#t=x"),
            "https://***@h/p?[redacted]#[redacted]"
        );
        // A later `//` window is masked too: credentials cannot hide behind
        // a benign first window.
        assert_eq!(redact_url("https://h//u2:p2@evil/"), "https://h//***@evil/");
    }

    #[test]
    fn fail_closed_suppresses_window_with_at() {
        assert_eq!(
            redact_url_fail_closed("http://u:secretpw@host:99999/x"),
            "[redacted]"
        );
    }

    #[test]
    fn sentinel_never_splits_at_cap() {
        // Pre-sentinel base (up to the earliest `?`) is 316 bytes; the
        // 3-byte `日` straddles the reserved cut at byte 234, forcing the
        // boundary walk. Both sentinels must render intact.
        let mut url = format!("https://host/{}日{}", "x".repeat(220), "x".repeat(80));
        url.push_str("?a=1#f");
        assert!(url.len() > 300);
        let redacted = redact_url(&url);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(
            redacted.ends_with("?[redacted]#[redacted]"),
            "sentinels must render intact: {redacted}"
        );
        assert!(std::str::from_utf8(redacted.as_bytes()).is_ok());
        assert!(
            !redacted.contains('日'),
            "straddling char dropped whole: {redacted}"
        );
        assert!(
            redacted.starts_with("https://host/"),
            "host kept: {redacted}"
        );
    }

    #[test]
    fn idempotent_mask_composition_pin() {
        // An already-masked window rewrites to itself and the sentinels
        // still compose.
        assert_eq!(
            redact_url("https://***@host/p?a=1#f"),
            "https://***@host/p?[redacted]#[redacted]"
        );
    }

    #[test]
    fn allowlist_keeps_benign_and_redacts_sensitive() {
        assert_eq!(
            redact_url_with_query_allowlist(
                "tcp://host:61616?password=p&user=u&keepAlive=true",
                JMS_KEYS
            ),
            "tcp://host:61616?password=<redacted>&user=<redacted>&keepAlive=true"
        );
    }

    // ── Phase 2: minimal-decode rule (bd rc-r7v8s) ──────────────────────

    /// Percent-encoded credentials under a benign key must not smuggle
    /// through: the single-pass minimal decode of the pair carries `@` and
    /// `//`, so the whole pair redacts. Uppercase hex form.
    #[test]
    fn allowlist_masks_percent_encoded_credentials_uppercase() {
        let redacted = redact_url_with_query_allowlist(
            "tcp://h:61616?redirect=http%3A%2F%2Fuser%3Asecret%40host",
            JMS_KEYS,
        );
        assert!(
            !redacted.contains("secret"),
            "percent-encoded credential leaked: {redacted}"
        );
        assert!(
            !redacted.contains("user%3Asecret%40"),
            "encoded userinfo leaked: {redacted}"
        );
        assert_eq!(redacted, "tcp://h:61616?<redacted>");
    }

    /// Fully-lowercase hex variant of the same smuggling shape.
    #[test]
    fn allowlist_masks_percent_encoded_credentials_lowercase() {
        let redacted = redact_url_with_query_allowlist(
            "tcp://h:61616?redirect=http%3a%2f%2fuser%3asecret%40host",
            JMS_KEYS,
        );
        assert_eq!(redacted, "tcp://h:61616?<redacted>");
        assert!(
            !redacted.contains("secret"),
            "percent-encoded credential leaked: {redacted}"
        );
    }

    /// Encoded slashes plus a literal `@` decode to a `//`-bearing
    /// credential shape under a benign key.
    #[test]
    fn allowlist_masks_literal_at_bypass() {
        let redacted =
            redact_url_with_query_allowlist("tcp://h:61616?next=%2F%2Fuser:pass@host", JMS_KEYS);
        assert_eq!(redacted, "tcp://h:61616?<redacted>");
        assert!(
            !redacted.contains("pass"),
            "literal credential leaked: {redacted}"
        );
    }

    /// Fully literal `user:pass@host` under a benign key.
    #[test]
    fn allowlist_masks_fully_literal_credential_pair() {
        let redacted =
            redact_url_with_query_allowlist("tcp://h:61616?next=user:pass@host", JMS_KEYS);
        assert_eq!(redacted, "tcp://h:61616?<redacted>");
        assert!(
            !redacted.contains("pass"),
            "literal credential leaked: {redacted}"
        );
    }

    /// Regression pin at the under-redaction boundary: `@` without `:` or
    /// `//` is an email address, not a credential — the pair stays visible.
    #[test]
    fn allowlist_keeps_lone_email_value() {
        assert_eq!(
            redact_url_with_query_allowlist("tcp://h:61616?contact=admin%40corp.example", JMS_KEYS),
            "tcp://h:61616?contact=admin%40corp.example"
        );
    }

    /// Over-mask stance (ADR-0051): a value decoding to `user@host:port`
    /// is credential-shaped even though no literal `//` or `:` is present.
    #[test]
    fn allowlist_masks_credential_shaped_userhostport() {
        let redacted =
            redact_url_with_query_allowlist("tcp://h:61616?next=user%40host%3Aport", JMS_KEYS);
        assert_eq!(redacted, "tcp://h:61616?<redacted>");
    }

    /// A percent-encoded sensitive key decodes to `password` and must
    /// redact; the rendered key keeps its original encoded bytes.
    #[test]
    fn allowlist_decodes_percent_encoded_sensitive_key() {
        let redacted =
            redact_url_with_query_allowlist("tcp://host:61616?pass%77ord=shortsecret", JMS_KEYS);
        assert!(
            redacted.contains("pass%77ord=<redacted>"),
            "encoded key must redact keeping original bytes: {redacted}"
        );
        assert!(
            !redacted.contains("shortsecret"),
            "secret value leaked: {redacted}"
        );
    }

    /// Regression pin (redact2): non-ASCII bytes in a pair are preserved
    /// byte-for-byte (valid UTF-8), never Latin-1-transcoded, and a decoded
    /// invalid byte becomes `U+FFFD` — the decoded output feeds only ASCII
    /// shape checks, so neither case can flip a redaction decision.
    #[test]
    fn minimal_decode_preserves_non_ascii_bytes() {
        // `é` (0xC3 0xA9) must survive as itself, not as `Ã©`.
        assert_eq!(minimal_decode_pair("café"), "café");
        // Escapes still decode around non-ASCII bytes.
        assert_eq!(minimal_decode_pair("café%40x"), "café@x");
        // A decoded invalid byte (0xFF from `%FF`) becomes U+FFFD, which
        // cannot match an ASCII needle (exercised via the full-hex decoder).
        assert_eq!(decode_match_key("a%FFb"), "a\u{FFFD}b");
    }

    // ── Migrated: camel-config config_tests/url_redaction_tests.rs ──────

    #[test]
    fn redact_url_keeps_userinfo_mask_shape() {
        assert_eq!(
            redact_url("redis://user:secret@h:6379"),
            "redis://***@h:6379"
        );
    }

    /// Intentional semantic change: the query was previously echoed verbatim,
    /// leaking `?password=...` cache credentials into Debug output.
    #[test]
    fn redact_url_drops_query_secrets() {
        assert_eq!(
            redact_url("redis://h:6379/0?password=hunter2"),
            "redis://h:6379/0?[redacted]"
        );
    }

    #[test]
    fn redact_url_drops_fragment() {
        assert_eq!(
            redact_url("redis://h:6379/0#tok=x"),
            "redis://h:6379/0#[redacted]"
        );
    }

    /// Multiple `@` in the window: mask through the LAST one — over-masking is
    /// safe, under-masking is not.
    #[test]
    fn redact_url_masks_through_last_at() {
        assert_eq!(redact_url("redis://user:p@ss@h:6379"), "redis://***@h:6379");
    }

    /// A slash run after `//` must not hide userinfo behind it; extra leading
    /// slashes are kept byte-for-byte.
    #[test]
    fn redact_url_slash_run_evader_masked() {
        assert_eq!(
            redact_url("redis:////user:pass@h:6379/0"),
            "redis:////***@h:6379/0"
        );
    }

    /// An `@` outside any `//` window (path data here) is not userinfo.
    #[test]
    fn redact_url_at_outside_window_visible() {
        assert_eq!(
            redact_url("redis://h:6379/0/user@x"),
            "redis://h:6379/0/user@x"
        );
    }

    /// A `//` window after the first is scanned too: credentials cannot hide
    /// behind a benign first window.
    #[test]
    fn redact_url_later_window_masked() {
        assert_eq!(redact_url("redis://h//user:pass@x/"), "redis://h//***@x/");
    }

    /// Compose-both rule: one sentinel per distinct introducer found in the
    /// raw URL, in first-occurrence order.
    #[test]
    fn redact_url_sentinels_compose_both() {
        assert_eq!(
            redact_url("redis://h:6379/0?password=x#tok=y"),
            "redis://h:6379/0?[redacted]#[redacted]"
        );
    }

    #[test]
    fn redact_url_sentinels_compose_fragment_first() {
        assert_eq!(
            redact_url("redis://h:6379/0#tok=y?password=x"),
            "redis://h:6379/0#[redacted]?[redacted]"
        );
    }

    /// The 256-byte cap must land on a UTF-8 char boundary: a 3-byte char
    /// straddling byte 256 forces the cut to walk back instead of panicking.
    #[test]
    fn redact_url_truncates_256_utf8_safe() {
        let mut url = format!("redis://{}{}", "x".repeat(246), '日');
        url.push_str(&"tail".repeat(20));
        assert!(url.len() > 300);
        let redacted = redact_url(&url);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(redacted.starts_with("redis://"));
    }

    /// e_gpt stage-4: the 256-byte cap must not split an appended sentinel.
    /// The base is truncated at `256 - sentinel_len` BEFORE the sentinel is
    /// appended, so the sentinel always renders intact and the total stays
    /// ≤ 256.
    #[test]
    fn redact_url_keeps_sentinel_intact_under_256_cap() {
        // Base (masked, cut at the `?`) is 248 bytes, so byte 256 lands inside
        // the appended `?[redacted]` (starts at 248) pre-fix.
        let url = format!("http://{}?x=1", "a".repeat(240));
        let redacted = redact_url(&url);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(
            redacted.ends_with("?[redacted]"),
            "sentinel must render intact: {redacted}"
        );
    }

    // ── Migrated: camel-jms config.rs broker allowlist tests ────────────

    /// Audit 2026-08-31, F5-3: JMS broker URLs must not leak credentials
    /// through Debug output.
    #[test]
    fn redact_broker_url_masks_userinfo_and_sensitive_query() {
        // userinfo form
        let redacted = redact_url_with_query_allowlist(
            "tcp://admin:secretpass@broker.example.com:61616",
            JMS_KEYS,
        );
        assert!(
            !redacted.contains("secretpass"),
            "password masked: {redacted}"
        );
        assert!(
            redacted.contains("broker.example.com"),
            "host visible: {redacted}"
        );

        // failover + query-param form (ActiveMQ style)
        let redacted = redact_url_with_query_allowlist(
            "failover:(tcp://host:61616)?jms.userName=admin&jms.password=secret&keepAlive=true",
            JMS_KEYS,
        );
        assert!(
            !redacted.contains("secret"),
            "password param masked: {redacted}"
        );
        assert!(
            !redacted.contains("=admin"),
            "username param masked: {redacted}"
        );
        assert!(
            redacted.contains("keepAlive=true"),
            "benign param kept: {redacted}"
        );

        // clean URL untouched
        assert_eq!(
            redact_url_with_query_allowlist("tcp://host:61616", JMS_KEYS),
            "tcp://host:61616"
        );
    }

    /// Exact-output pin: userinfo is fully masked by `***`, never partially
    /// truncated, in scheme://...@ authority position.
    #[test]
    fn redact_exact_userinfo_mask() {
        assert_eq!(
            redact_url_with_query_allowlist(
                "tcp://admin:secretpass@broker.example.com:61616",
                JMS_KEYS
            ),
            "tcp://***@broker.example.com:61616"
        );
    }

    /// Exact-output pin: sensitive query params redact to `<redacted>`,
    /// benign params survive byte-for-byte, `&` separators preserved.
    #[test]
    fn redact_exact_query_join() {
        assert_eq!(
            redact_url_with_query_allowlist(
                "tcp://host:61616?password=p&user=u&keepAlive=true",
                JMS_KEYS
            ),
            "tcp://host:61616?password=<redacted>&user=<redacted>&keepAlive=true"
        );
    }

    /// Exact-output pin: a bare `user@host` (no scheme) is NOT an authority
    /// position — passthrough untouched.
    #[test]
    fn redact_exact_bare_at_passthrough() {
        assert_eq!(
            redact_url_with_query_allowlist("admin@host", JMS_KEYS),
            "admin@host"
        );
    }

    /// Exact-output pin: delimiter-exact query redaction on the ActiveMQ
    /// failover form — the whole query part after '?' must match verbatim,
    /// no dropped params, no mangled separators.
    #[test]
    fn redact_exact_failover_param_boundaries() {
        let redacted = redact_url_with_query_allowlist(
            "failover:(tcp://host:61616)?jms.userName=admin&jms.password=secret&keepAlive=true",
            JMS_KEYS,
        );
        let (_, query) = redacted.split_once('?').expect("query segment after '?'");
        assert_eq!(
            query,
            "jms.userName=<redacted>&jms.password=<redacted>&keepAlive=true"
        );
    }

    /// bd rc-eh49 exact pin: an `@` riding the query is NOT userinfo — the
    /// URL must pass through byte-for-byte. The old whole-string
    /// `split_once('@')` mangled this to `failover:(tcp://***@b`.
    #[test]
    fn redact_broker_url_query_at_no_misfire() {
        assert_eq!(
            redact_url_with_query_allowlist("failover:(tcp://h:61616)?x=a@b", JMS_KEYS),
            "failover:(tcp://h:61616)?x=a@b"
        );
    }

    /// bd rc-eh49 exact pin: a slash run after `//` is preserved
    /// byte-for-byte and the windowed mask composes with the per-key query
    /// allowlist. The old whole-string scan rewrote from the scheme's own
    /// `://` and swallowed the extra slashes.
    #[test]
    fn redact_exact_slash_run_window_composition() {
        assert_eq!(
            redact_url_with_query_allowlist("tcp:////user:pass@h:61616?keepAlive=true", JMS_KEYS),
            "tcp:////***@h:61616?keepAlive=true"
        );
    }

    /// bd rc-eh49 exact pin: the window mask consumes through the LAST `@`
    /// of the window, so an `@` embedded in the userinfo cannot keep a
    /// spoofable prefix alive (the old first-`@` scan left `a@` visible).
    #[test]
    fn redact_exact_last_at_in_window() {
        assert_eq!(
            redact_url_with_query_allowlist("tcp://u:p@a@h:61616", JMS_KEYS),
            "tcp://***@h:61616"
        );
    }

    /// bd rc-eh49: the 256-byte cap cuts on a char boundary — a multibyte
    /// character straddling byte 256 is dropped whole, never split
    /// mid-encode.
    #[test]
    fn redact_broker_url_truncate_multibyte_boundary() {
        let mut url = String::from("tcp://broker:61616/");
        url.push_str(&"x".repeat(236)); // 255 ASCII bytes before the multibyte char
        url.push('日'); // 3 bytes straddling the 256-byte cap
        url.push_str(&"y".repeat(50)); // push the total past 300 bytes
        assert!(url.len() > 300);
        let redacted = redact_url_with_query_allowlist(&url, JMS_KEYS);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(std::str::from_utf8(redacted.as_bytes()).is_ok());
        assert!(
            !redacted.contains('日'),
            "straddling char dropped whole: {redacted}"
        );
    }

    /// bd rc-eh49: a `//` window after the first is scanned too; the
    /// benign query stays visible per the broker allowlist exception.
    #[test]
    fn redact_broker_url_later_window_masked() {
        assert_eq!(
            redact_url_with_query_allowlist("tcp://h//user:pass@x/?keepAlive=true", JMS_KEYS),
            "tcp://h//***@x/?keepAlive=true"
        );
    }

    /// bd rc-eh49: fragments are never echoed — everything from the first
    /// `#` is dropped and the `#[redacted]` sentinel appended; processed
    /// query params stay visible.
    #[test]
    fn redact_broker_url_drops_fragment() {
        assert_eq!(
            redact_url_with_query_allowlist("tcp://h:61616?keepAlive=true#tok=x", JMS_KEYS),
            "tcp://h:61616?keepAlive=true#[redacted]"
        );
    }

    /// bd rc-eh49: broker URLs are capped at 256 bytes on a UTF-8 char
    /// boundary.
    #[test]
    fn redact_broker_url_truncates() {
        let mut url = format!("tcp://broker:61616/{}", "x".repeat(300));
        url.push('日');
        url.push_str(&"tail".repeat(20));
        assert!(url.len() > 300);
        let redacted = redact_url_with_query_allowlist(&url, JMS_KEYS);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(redacted.starts_with("tcp://broker:61616/"));
    }

    // ── Migrated: camel-jms component.rs redact_url_* block ─────────────

    #[test]
    fn redact_url_strips_userinfo_with_password() {
        assert_eq!(
            redact_url("tcp://admin:s3cret@broker:61616"),
            "tcp://***@broker:61616"
        );
    }

    #[test]
    fn redact_url_strips_userinfo_without_password() {
        assert_eq!(
            redact_url("tcp://admin@broker:61616"),
            "tcp://***@broker:61616"
        );
    }

    #[test]
    fn redact_url_passes_clean_url_unchanged() {
        assert_eq!(redact_url("tcp://localhost:61616"), "tcp://localhost:61616");
    }

    #[test]
    fn redact_url_handles_ssl_scheme() {
        assert_eq!(
            redact_url("ssl://user:pass@secure-broker:61617"),
            "ssl://***@secure-broker:61617"
        );
    }

    /// bd rc-eh49: the earliest `?`/`#` introducer wins; both sentinels
    /// compose in first-occurrence order when the raw URL carries both
    /// introducers, and fragment bytes after the cut are dropped with it.
    #[test]
    fn redact_url_drops_query_and_fragment() {
        assert_eq!(
            redact_url("tcp://broker:61616?user=a#tok=x"),
            "tcp://broker:61616?[redacted]#[redacted]"
        );
    }

    /// bd rc-eh49 compose-both rule: a `#` before `?` flips the sentinel
    /// order accordingly. (Name prefixed `jms_`: the redis fixture from
    /// camel-config already owns the unprefixed name above.)
    #[test]
    fn jms_redact_url_sentinels_compose_fragment_first() {
        assert_eq!(
            redact_url("tcp://broker:61616#tok=x?user=a"),
            "tcp://broker:61616#[redacted]?[redacted]"
        );
    }

    /// bd rc-eh49: a `//` window after the first is scanned too —
    /// credentials cannot hide behind a benign first window. (Name prefixed
    /// `jms_`: the redis fixture from camel-config already owns the
    /// unprefixed name above.)
    #[test]
    fn jms_redact_url_later_window_masked() {
        assert_eq!(redact_url("tcp://h//user:pass@x/"), "tcp://h//***@x/");
    }

    /// bd rc-eh49: a slash run after `//` cannot hide userinfo from the
    /// window scan.
    #[test]
    fn redact_url_slash_run_masked() {
        let redacted = redact_url("tcp:////user:pass@broker:61616");
        assert!(!redacted.contains("user:pass"), "leaked: {redacted}");
        assert!(
            redacted.contains("***@broker:61616"),
            "masked in place: {redacted}"
        );
    }

    /// bd rc-eh49: the old first-`@`-anywhere scan masked through an `@`
    /// riding the query (`tcp://***@b`); only a window `@` is userinfo.
    #[test]
    fn redact_url_at_in_query_not_userinfo_mask() {
        assert_eq!(
            redact_url("tcp://broker:61616?q=a@b"),
            "tcp://broker:61616?[redacted]"
        );
    }

    /// bd rc-eh49: the 256-byte cap cuts on a char boundary — a multibyte
    /// character straddling byte 256 is dropped whole, never split
    /// mid-encode.
    #[test]
    fn redact_url_truncate_multibyte_boundary() {
        let mut url = String::from("tcp://broker:61616/");
        url.push_str(&"x".repeat(236)); // 255 ASCII bytes before the multibyte char
        url.push('日'); // 3 bytes straddling the 256-byte cap
        url.push_str(&"y".repeat(50)); // push the total past 300 bytes
        assert!(url.len() > 300);
        let redacted = redact_url(&url);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(std::str::from_utf8(redacted.as_bytes()).is_ok());
        assert!(
            !redacted.contains('日'),
            "straddling char dropped whole: {redacted}"
        );
    }

    /// e_gpt stage-4: the 256-byte cap must not split an appended sentinel.
    /// The base is truncated at `256 - sentinel_len` BEFORE the sentinel is
    /// appended, so the sentinel always renders intact and the total stays
    /// ≤ 256. Covers both redactors: `redact_url` (compose sentinels) and
    /// `redact_url_with_query_allowlist` (fragment sentinel over kept query
    /// content).
    #[test]
    fn redact_url_keeps_sentinels_intact_under_256_cap() {
        // `redact_url`: base (masked, cut at the `?`) is 255 bytes, so byte
        // 256 lands inside the appended `?[redacted]` (starts at 255) pre-fix.
        let url = format!("tcp://{}?x=1", "a".repeat(250));
        let redacted = redact_url(&url);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(
            redacted.ends_with("?[redacted]"),
            "redact_url sentinel must render intact: {redacted}"
        );

        // `redact_url_with_query_allowlist`: base + kept query is 252 bytes,
        // so byte 256 lands inside the appended `#[redacted]` (starts at
        // 252) pre-fix.
        let broker = format!("tcp://{}?keep=1#frag", "a".repeat(240));
        let redacted = redact_url_with_query_allowlist(&broker, JMS_KEYS);
        assert!(redacted.len() <= 256, "len={}", redacted.len());
        assert!(
            redacted.ends_with("#[redacted]"),
            "allowlist sentinel must render intact: {redacted}"
        );
    }

    // ── Migrated: camel-http lib.rs pure-string redact tests ────────────
    // These landed against the Err arm of `redact_url_for_diagnostics`
    // (unparseable inputs); that arm delegates to `redact_url` /
    // `redact_url_fail_closed`, so the fixtures pin the canonical helpers
    // byte-identically. The `url::Url::parse` precondition asserts of the
    // landed tests do not apply here: this module never parses.

    #[test]
    fn redact_url_unparseable_fragment_credentials_dropped() {
        let raw = "ht tps://app.example/cb#access_token=SECRET";
        let redacted = redact_url(raw);
        assert!(
            !redacted.contains("SECRET"),
            "unparseable fragment token leaked: {redacted}"
        );
        assert!(
            !redacted.contains("access_token"),
            "unparseable fragment bytes leaked: {redacted}"
        );
        assert!(
            redacted.contains("#[redacted]"),
            "unparseable fragment must end in the sentinel: {redacted}"
        );
    }

    #[test]
    fn redact_url_empty_host_userinfo_sentinel() {
        // The landed http test drives this through the parse-failure arm;
        // the canonical helper fails closed on the window `@` directly.
        let redacted = redact_url_fail_closed("scheme://user@");
        assert_eq!(
            redacted, "[redacted]",
            "empty-host userinfo must fail closed: {redacted}"
        );
    }

    #[test]
    fn redact_url_unparseable_slash_run_evader_sentinel() {
        let raw = "schem e:////user:pass@evil/";
        let redacted = redact_url_fail_closed(raw);
        assert_eq!(
            redacted, "[redacted]",
            "unparseable slash-run evader must fail closed: {redacted}"
        );
    }

    #[test]
    fn redact_url_unparseable_later_window_userinfo_sentinel() {
        // The first `//` window ("ho st") carries no `@`, but a later
        // `//user:pass@evil/` window does. The scan must consider every
        // `//` window, not just the first, or the credentials echo.
        let raw = "http://ho st/a//user:pass@evil/";
        let redacted = redact_url_fail_closed(raw);
        assert_eq!(
            redacted, "[redacted]",
            "userinfo in a later // window must fail closed: {redacted}"
        );
    }

    #[test]
    fn redact_url_truncates_unparseable() {
        let long = "x".repeat(1000);
        let redacted = redact_url(&long);
        assert_eq!(redacted.len(), 256, "unparseable URL must be truncated");
    }

    #[test]
    fn redact_url_suppresses_unparseable_authority_credentials() {
        let fixtures = [
            "http://u:secretpw@/x",
            "http://u:secretpw@host:99999/x",
            "http://u:secretpw@host:99999",
            "//u:secretpw@h/x",
        ];
        for fixture in fixtures {
            assert_eq!(
                redact_url_fail_closed(fixture),
                "[redacted]",
                "credential-bearing authority must be suppressed: {fixture}"
            );
        }
    }

    #[test]
    fn redact_url_unparseable_query_redacted_short_and_long() {
        let short = "http://host:99999/path?token=shortsecret";
        let redacted = redact_url(short);
        assert_eq!(
            redacted, "http://host:99999/path?[redacted]",
            "short unparseable query must end with the suffix: {redacted}"
        );

        let mut long = String::from("http://host:99999/");
        long.push_str(&"a".repeat(300));
        long.push_str("?token=longsecret");
        let redacted = redact_url(&long);
        assert!(
            !redacted.contains("longsecret"),
            "long unparseable query leaked a query byte: {redacted}"
        );
        assert!(
            redacted.len() <= 256,
            "long unparseable query must be capped: {} bytes",
            redacted.len()
        );
    }

    #[test]
    fn redact_url_unparseable_sentinels_compose_both() {
        // Compose-both rule: one sentinel per distinct introducer found in
        // the raw string, in first-occurrence order.
        let raw = "ht tp://h.example/p?a=1#tok=x";
        assert_eq!(
            redact_url(raw),
            "ht tp://h.example/p?[redacted]#[redacted]",
            "query and fragment sentinels must compose: {raw}"
        );
    }

    #[test]
    fn redact_url_unparseable_sentinels_compose_fragment_first() {
        let raw = "ht tp://h.example/p#tok=x?a=1";
        assert_eq!(
            redact_url(raw),
            "ht tp://h.example/p#[redacted]?[redacted]",
            "sentinels must follow the introducers' first-occurrence order: {raw}"
        );
    }

    #[test]
    fn redact_url_unparseable_utf8_straddle_no_panic() {
        let fixture = format!("a{}", "é".repeat(200));
        let redacted = redact_url(&fixture);
        assert!(
            redacted.len() <= 256,
            "straddle fixture must be capped: {} bytes",
            redacted.len()
        );
        assert!(
            redacted.len() >= 253,
            "straddle fixture must not over-truncate: {} bytes",
            redacted.len()
        );
        assert!(
            fixture.is_char_boundary(redacted.len()),
            "cut must land on a UTF-8 char boundary: {} bytes",
            redacted.len()
        );
    }

    #[test]
    fn redact_url_at_sign_outside_authority_window_visible() {
        let at_sign_in_path = "http://host:99999/x@y";
        assert_eq!(
            redact_url_fail_closed(at_sign_in_path),
            at_sign_in_path,
            "at-sign in path must not be suppressed"
        );
        // Opaque non-hierarchical strings pass through byte-identically.
        assert_eq!(
            redact_url("mailto:user@example.com"),
            "mailto:user@example.com",
            "at-sign in mailto must round-trip byte-identically"
        );
    }

    // ── Phase 3: scheme-gated backslash windows (rc-f05q8) ──────────────

    /// A backslash authority after a non-special scheme opens a window:
    /// `foo:\user:pass@evil/` carries no `//` run, yet the scheme-prefixed
    /// backslash run must mask the userinfo. The fail-closed variant
    /// suppresses the whole string; the clean sibling stays visible.
    #[test]
    fn backslash_run_non_special_scheme_masked() {
        assert_eq!(
            redact_url("foo:\\user:pass@evil/"),
            "foo:\\***@evil/",
            "non-special-scheme backslash authority must mask userinfo"
        );
        assert_eq!(
            redact_url_fail_closed("foo:\\user:pass@evil/"),
            "[redacted]",
            "non-special-scheme backslash authority must fail closed"
        );
        assert_eq!(
            redact_url("foo:\\clean/path"),
            "foo:\\clean/path",
            "clean backslash sibling stays visible"
        );
    }

    /// A single backslash after a multi-character scheme opens a window
    /// even though the run is one character long.
    #[test]
    fn backslash_single_after_multi_char_scheme_masked() {
        let redacted = redact_url("http:\\user:pass@evil\\path");
        assert!(
            !redacted.contains("user:pass"),
            "single-backslash authority leaked: {redacted}"
        );
        assert!(
            redacted.contains("***@"),
            "single-backslash authority must mask userinfo: {redacted}"
        );
    }

    /// A single backslash after a one-character scheme opens a window when
    /// the candidate window content is credential-shaped (`:` before the
    /// last `@`).
    #[test]
    fn backslash_single_after_one_char_scheme_credential_shaped_masked() {
        let redacted = redact_url("x:\\user:pass@evil");
        assert!(
            !redacted.contains("user:pass"),
            "one-char-scheme backslash authority leaked: {redacted}"
        );
        assert!(
            redacted.contains("***@"),
            "one-char-scheme backslash authority must mask userinfo: {redacted}"
        );
    }

    /// Windows drive path: single backslash after a one-character scheme
    /// with no `:` in the candidate window — no qualifying window, the
    /// string renders unchanged.
    #[test]
    fn drive_path_stays_visible() {
        assert_eq!(
            redact_url("C:\\Users\\x@corp\\file"),
            "C:\\Users\\x@corp\\file"
        );
    }

    /// UNC path: no scheme prefix before the backslash run — no qualifying
    /// window, the string renders unchanged.
    #[test]
    fn unc_path_stays_visible() {
        assert_eq!(redact_url("\\\\server\\x@y"), "\\\\server\\x@y");
    }

    /// The fail-closed window scan must see scheme-prefixed backslash
    /// windows, while drive paths stay window-free.
    #[test]
    fn window_has_at_sign_sees_backslash_windows() {
        assert!(
            window_has_at_sign("foo:\\u:p@e/"),
            "scheme-prefixed backslash window must carry the at-sign"
        );
        assert!(
            !window_has_at_sign("C:\\Users\\x@corp\\file"),
            "drive path must not open a backslash window"
        );
    }

    /// Branch pins for the scheme-gated backslash window rule: a len-2
    /// backslash run behind a scheme prefix opens (introducer stays
    /// visible), a mixed `/`+`\` run of length 2 behind a 1-char scheme
    /// opens, an invalid scheme char before the run gates it off, a
    /// scheme-less mixed run stays closed, and a single `\` behind a
    /// 1-char scheme with no `:` in the candidate content stays closed.
    #[test]
    fn backslash_gate_branches_pinned() {
        assert_eq!(redact_url("foo:\\\\user:pass@evil/"), "foo:\\\\***@evil/");
        assert_eq!(redact_url("a:/\\user:pass@evil/"), "a:/\\***@evil/");
        assert_eq!(
            redact_url("notscheme%\\user:pass@evil/"),
            "notscheme%\\user:pass@evil/"
        );
        assert_eq!(redact_url("/\\user:pass@evil/"), "/\\user:pass@evil/");
        assert_eq!(redact_url("a:\\user@evil"), "a:\\user@evil");
        assert!(window_has_at_sign("foo:\\\\u:p@e/"));
        assert!(!window_has_at_sign("a:\\user@evil"));
    }
}
