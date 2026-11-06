//! Pure grammar and scan-reply classifier tests for the
//! `redis:` `unlinkPrefix` teardown action (bd rc-ql8eq, task 4.1).
//!
//! Everything here is ungated: the glob escaping, the scan-pattern
//! construction, and the whole-reply alignment check are pure
//! functions, so a featureless build proves the namespace-scoping law
//! without a Redis server.

use crate::redis_action::{build_scan_pattern, classify_scan_reply, escape_redis_glob};

/// The scan pattern carries exactly one unescaped `*`, and every
/// metacharacter in the prefix is backslash-escaped.
#[test]
fn build_scan_pattern_is_single_star() {
    let pattern = build_scan_pattern("112~it~3f9a1c0e~0007~:state:");
    assert_eq!(pattern, "112~it~3f9a1c0e~0007~:state:*");

    let unescaped_stars = pattern
        .chars()
        .enumerate()
        .filter(|(index, c)| *c == '*' && (*index == 0 || pattern.as_bytes()[index - 1] != b'\\'))
        .count();
    assert_eq!(unescaped_stars, 1, "exactly one unescaped star: {pattern}");

    assert_eq!(
        escape_redis_glob("a*b[c]^d\\e"),
        "a\\*b\\[c\\]\\^d\\\\e",
        "every Redis glob metacharacter must be backslash-escaped"
    );
}

/// The whole reply is rejected when any key lies outside the prefix:
/// the FIRST violator aborts with its position, and no partial
/// acceptance happens (the caller unlinks nothing from that reply).
#[test]
fn unlink_prefix_skips_unaligned_scan_results() {
    let prefix = "112~it~3f9a1c0e~0001~state:";

    let (position, key) =
        classify_scan_reply(&["112~it~3f9a1c0e~0001~state:ok", "IMPOSTOR"], prefix)
            .expect_err("an unaligned key must reject the whole reply");
    assert_eq!(position, 1, "the violator position must be reported");
    assert_eq!(key, "IMPOSTOR");

    assert!(
        classify_scan_reply(&["112~it~3f9a1c0e~0001~state:ok"], prefix).is_ok(),
        "an aligned-only reply is accepted"
    );
}

/// `COUNT` is a hint, not a bound: a reply may carry more than 500
/// keys, and a violator beyond the first 500 still rejects the whole
/// reply at its true position.
#[test]
fn classify_scan_reply_refuses_violator_beyond_500() {
    let prefix = "112~it~3f9a1c0e~0001~state:";
    let mut keys: Vec<String> = (0..500)
        .map(|index| format!("{prefix}{index:04}"))
        .collect();
    keys.push("IMPOSTOR".to_string());

    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    let (position, key) = classify_scan_reply(&refs, prefix)
        .expect_err("a violator beyond 500 keys must still reject the reply");
    assert_eq!(position, 500);
    assert_eq!(key, "IMPOSTOR");
}
