//! Alignment of `redact_url` with the camel-http reference semantics
//! (bd rc-eh49, audit `docs/audits/redaction-surface-2026-09.md`): every
//! `//`-window userinfo mask through the last `@`, earliest-`?`/`#`
//! sentinel drop, and a 256-byte UTF-8-safe cap.

use super::redact_url;

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
