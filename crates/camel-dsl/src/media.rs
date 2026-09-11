//! RFC 7231/9110 media-type subset parsing and matching for REST content
//! negotiation (default-strict L2): concrete media declarations, Accept
//! media ranges with quality values, and the 415/406 gate decision.
//!
//! Quoted parameter values (RFC 9110 quoted-string) are not part of this
//! subset; entries containing them are treated as malformed and degrade per
//! the malformed-header rule.

// Staged landing (task 3.1): the gate's consumers arrive with the
// ContentNegotiationProcessor wiring in task 3.2 — drop this allow then.
#![allow(dead_code)]

use camel_api::CamelError;

use crate::yaml::valid_header_token;

// ---------------------------------------------------------------------------
// Moved verbatim from rest.rs (task 3.1) — behavior unchanged
// ---------------------------------------------------------------------------

/// The media type base: trimmed, parameters after the first `;` dropped.
pub(crate) fn split_media_base(media: &str) -> &str {
    media.trim().split(';').next().unwrap_or_default()
}

/// True iff `media` is a valid `type/subtype` declaration; parameters are
/// ignored (opaque).
pub(crate) fn is_valid_media_declaration(media: &str) -> bool {
    match split_media_base(media).split_once('/') {
        Some((ty, sub)) => valid_header_token(ty) && valid_header_token(sub),
        None => false,
    }
}

/// True iff `media` is a JSON media type: a valid `type/subtype` whose
/// subtype is `json` or ends with `+json` (case-insensitive, ASCII).
pub(crate) fn is_json_media_type(media: &str) -> bool {
    let Some((ty, sub)) = split_media_base(media).split_once('/') else {
        return false;
    };
    if !valid_header_token(ty) || !valid_header_token(sub) {
        return false;
    }
    let sub = sub.to_ascii_lowercase();
    sub == "json" || sub.ends_with("+json")
}

// ---------------------------------------------------------------------------
// Media ranges
// ---------------------------------------------------------------------------

/// A parsed `type/subtype(+suffix)?` media range, stored lowercase. A
/// wildcard range has `type_ == "*"` and/or `subtype == "*"`; `suffix` is
/// the structured syntax suffix (`+json` style), lowercase.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MediaRange {
    type_: String,
    subtype: String,
    suffix: Option<String>,
}

/// Parse a concrete `type/subtype("+" suffix)?` with optional `;`-parameters
/// (skipped). Rejects ANY wildcard. Case-insensitive, normalized to
/// lowercase. `None` on any failure.
pub(crate) fn parse_concrete_media_type(value: &str) -> Option<MediaRange> {
    parse_media_range(split_media_base(value), false)
}

/// Parse one Accept-list entry: a media range (wildcards `*/*` and `type/*`
/// allowed) plus a quality value. `;q=` is matched case-insensitively and
/// defaults to 1.0; q outside `0.0..=1.0` (including NaN) is rejected.
pub(crate) fn parse_accept_entry(value: &str) -> Option<(MediaRange, f32)> {
    let trimmed = value.trim();
    let (base, params) = match trimmed.split_once(';') {
        Some((base, params)) => (base, params),
        None => (trimmed, ""),
    };
    let range = parse_media_range(base, true)?;
    let mut quality = 1.0_f32;
    for param in params.split(';') {
        if param.trim().is_empty() {
            continue;
        }
        // Quoted parameter values are outside this subset: any segment
        // containing a `"` makes the whole entry malformed.
        if param.contains('"') {
            return None;
        }
        let (name, raw) = param.split_once('=')?;
        if name.trim().eq_ignore_ascii_case("q") {
            quality = raw.trim().parse::<f32>().ok()?;
            if !(0.0..=1.0).contains(&quality) {
                return None;
            }
        }
    }
    Some((range, quality))
}

/// The negotiated contract of a lowered REST operation: the parsed concrete
/// `consumes`/`produces` ranges (`None` = permissive side, e.g. a wildcard or
/// garbage declaration admitted by the raw-mode tchar check) plus the
/// TRIMMED original declarations, echoed verbatim in error payloads.
#[derive(Debug, Clone)]
pub(crate) struct MediaContract {
    consumes: Option<MediaRange>,
    produces: Option<MediaRange>,
    consumes_declared: String,
    produces_declared: String,
}

/// Parse the declared `consumes`/`produces` pair. Infallible: a side that
/// fails `parse_concrete_media_type` becomes permissive (`None`).
pub(crate) fn parse_contract(consumes: &str, produces: &str) -> MediaContract {
    MediaContract {
        consumes: parse_concrete_media_type(consumes.trim()),
        produces: parse_concrete_media_type(produces.trim()),
        consumes_declared: consumes.trim().to_string(),
        produces_declared: produces.trim().to_string(),
    }
}

/// Parse the `type/subtype(+suffix)?` base of a media range. Both halves are
/// validated with the RFC 9110 `tchar` token rule `is_valid_media_declaration`
/// uses. When `allow_wildcards` is false, any `*` half is rejected; when
/// true, `*/*` and `type/*` are legal but `*/subtype` is not.
fn parse_media_range(base: &str, allow_wildcards: bool) -> Option<MediaRange> {
    let (ty, sub_full) = base.split_once('/')?;
    if !valid_header_token(ty) {
        return None;
    }
    let (subtype, suffix) = match sub_full.rsplit_once('+') {
        Some((sub, suffix)) => (sub, Some(suffix)),
        None => (sub_full, None),
    };
    if !valid_header_token(subtype) || suffix.is_some_and(|s| !valid_header_token(s)) {
        return None;
    }
    let type_ = ty.to_ascii_lowercase();
    let subtype = subtype.to_ascii_lowercase();
    let wildcard_type = type_ == "*";
    let wildcard_subtype = subtype == "*";
    // RFC 9110: structured syntax suffixes are not allowed on wildcards.
    if suffix.is_some() && (wildcard_type || wildcard_subtype) {
        return None;
    }
    if wildcard_type && (!allow_wildcards || !wildcard_subtype) {
        return None;
    }
    if !allow_wildcards && wildcard_subtype {
        return None;
    }
    Some(MediaRange {
        type_,
        subtype,
        suffix: suffix.map(|s| s.to_ascii_lowercase()),
    })
}

/// JSON-family media per the L1 essence rule: `application/json` itself or
/// any `+json` structured syntax suffix.
pub(crate) fn is_json_family(range: &MediaRange) -> bool {
    (range.type_ == "application" && range.subtype == "json")
        || range.suffix.as_deref() == Some("json")
}

/// Whether an Accept entry satisfies a declared representation: exact
/// `type/subtype` equality, the JSON-essence rule, `*/*`, or `type/*` with
/// the declared type.
pub(crate) fn entry_satisfies(entry: &MediaRange, declared: &MediaRange) -> bool {
    (entry.type_ == declared.type_ && entry.subtype == declared.subtype)
        || (is_json_family(entry) && is_json_family(declared))
        || (entry.type_ == "*" && entry.subtype == "*")
        || (entry.subtype == "*" && entry.type_ == declared.type_)
}

/// Specificity tier of a satisfying entry: 1 exact `type/subtype`, 2
/// JSON-essence, 3 `type/*`, 4 `*/*`. `None` when the entry does not
/// satisfy the declaration.
pub(crate) fn specificity(entry: &MediaRange, declared: &MediaRange) -> Option<u8> {
    if !entry_satisfies(entry, declared) {
        return None;
    }
    if entry.type_ == declared.type_ && entry.subtype == declared.subtype {
        Some(1)
    } else if is_json_family(entry) && is_json_family(declared) {
        Some(2)
    } else if entry.type_ == "*" {
        Some(4)
    } else {
        // entry_satisfies + not exact/essence/`*/*` ⇒ matching `type/*`.
        Some(3)
    }
}

/// The media negotiation gate: 415 on `Content-Type` mismatch against the
/// declared `consumes`, 406 on an `Accept` header that admits no acceptable
/// entry for the declared `produces`. Absent headers and permissive
/// contract sides are skipped.
pub(crate) fn check_request(
    content_type: Option<&str>,
    accept: Option<&str>,
    contract: &MediaContract,
) -> Result<(), CamelError> {
    // Content-Type side: skip when absent or the consumes side is permissive.
    if let (Some(raw), Some(consumes)) = (content_type, contract.consumes.as_ref()) {
        let trimmed = raw.trim();
        let accepted = match parse_concrete_media_type(trimmed) {
            Some(candidate) => entry_satisfies(&candidate, consumes),
            None => false,
        };
        if !accepted {
            return Err(CamelError::UnsupportedMediaType {
                consumed: trimmed.to_string(),
                declared: contract.consumes_declared.clone(),
            });
        }
    }

    // Accept side: skip when absent or the produces side is permissive.
    if let (Some(raw), Some(produces)) = (accept, contract.produces.as_ref()) {
        let trimmed = raw.trim();
        let entries = match parse_accept_entries(trimmed) {
            Some(entries) => entries,
            // A malformed header is treated as a permissive `*/*` (q=1.0).
            None => vec![wildcard_range()],
        };
        // Governing entry: lowest specificity tier, then lowest quality
        // among ties (fail closed).
        let mut governing: Option<(u8, f32)> = None;
        for (entry, q) in &entries {
            if let Some(tier) = specificity(entry, produces) {
                let tighter = match governing {
                    None => true,
                    Some((governing_tier, _)) => tier < governing_tier,
                };
                let lower_q = matches!(governing, Some((governing_tier, governing_q)) if tier == governing_tier && *q < governing_q);
                if tighter || lower_q {
                    governing = Some((tier, *q));
                }
            }
        }
        let acceptable = matches!(governing, Some((_, q)) if q > 0.0);
        if !acceptable {
            return Err(CamelError::NotAcceptable {
                accept: trimmed.to_string(),
                produced: contract.produces_declared.clone(),
            });
        }
    }
    Ok(())
}

/// Parse a comma-separated Accept header into entries; `None` when ANY
/// entry fails to parse.
fn parse_accept_entries(header: &str) -> Option<Vec<(MediaRange, f32)>> {
    let mut entries = Vec::new();
    for segment in header.split(',') {
        entries.push(parse_accept_entry(segment)?);
    }
    Some(entries)
}

/// The permissive `*/*` range with default quality.
fn wildcard_range() -> (MediaRange, f32) {
    (
        MediaRange {
            type_: "*".to_string(),
            subtype: "*".to_string(),
            suffix: None,
        },
        1.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_concrete_accepts_params_suffix_and_case() {
        let plain = parse_concrete_media_type("application/json")
            .expect("plain application/json must parse");
        assert_eq!(plain.type_, "application");
        assert_eq!(plain.subtype, "json");
        assert_eq!(plain.suffix, None);

        let upper = parse_concrete_media_type("APPLICATION/JSON; charset=utf-8")
            .expect("upper-case with parameters must parse");
        assert_eq!(upper.type_, "application");
        assert_eq!(upper.subtype, "json");
        assert_eq!(upper.suffix, None);

        let suffixed = parse_concrete_media_type("application/vnd.api+json")
            .expect("structured syntax suffix must parse");
        assert_eq!(suffixed.type_, "application");
        assert_eq!(suffixed.subtype, "vnd.api");
        assert_eq!(suffixed.suffix.as_deref(), Some("json"));
    }

    #[test]
    fn parse_concrete_rejects_wildcards_and_garbage() {
        for bad in [
            "*/*",
            "application/*",
            "application",
            "application/",
            "application/ json",
        ] {
            assert!(
                parse_concrete_media_type(bad).is_none(),
                "expected rejection: {bad:?}"
            );
        }
    }

    #[test]
    fn parse_accept_entry_q_handling() {
        let (range, q) = parse_accept_entry("application/json").expect("default quality");
        assert_eq!(range.type_, "application");
        assert_eq!(range.subtype, "json");
        assert_eq!(q, 1.0);

        let (_, q) = parse_accept_entry("application/json;q=0").expect("explicit q=0");
        assert_eq!(q, 0.0);

        let (_, q) =
            parse_accept_entry("application/json;Q=0.5").expect("case-insensitive q param");
        assert_eq!(q, 0.5);

        let (range, q) = parse_accept_entry("application/*;q=0.9").expect("type/* wildcard");
        assert_eq!(range.type_, "application");
        assert_eq!(range.subtype, "*");
        assert_eq!(q, 0.9);

        let (range, q) = parse_accept_entry("*/*").expect("full wildcard");
        assert_eq!(range.type_, "*");
        assert_eq!(range.subtype, "*");
        assert_eq!(q, 1.0);

        assert!(
            parse_accept_entry("application/json;q=1.5").is_none(),
            "q=1.5 must be rejected"
        );
    }

    #[test]
    fn parse_contract_wildcard_declaration_is_permissive() {
        let wildcard = parse_contract("*/*", "application/json");
        assert!(
            wildcard.consumes.is_none(),
            "wildcard declaration is permissive"
        );
        assert!(wildcard.produces.is_some());
        assert!(
            check_request(Some("text/plain"), None, &wildcard).is_ok(),
            "permissive consumes side must skip the CT check"
        );

        let garbage = parse_contract("garbage", "application/json");
        assert!(
            garbage.consumes.is_none(),
            "garbage declaration is permissive"
        );
        assert!(garbage.produces.is_some());
    }

    #[test]
    fn check_request_415_paths() {
        let contract = parse_contract("application/json", "application/json");
        for ct in ["text/plain", "not a type", "*/*"] {
            match check_request(Some(ct), None, &contract) {
                Err(CamelError::UnsupportedMediaType { consumed, declared }) => {
                    assert_eq!(consumed, ct.trim(), "consumed must echo trimmed input");
                    assert_eq!(declared, "application/json");
                }
                other => panic!("expected UnsupportedMediaType for {ct:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn check_request_415_passes() {
        let contract = parse_contract("application/json", "application/json");
        for ct in [
            Some("application/json"),
            Some("application/json; charset=utf-8"),
            Some("application/vnd.api+json"),
            Some("APPLICATION/JSON"),
            None,
        ] {
            assert!(
                check_request(ct, None, &contract).is_ok(),
                "expected CT pass for {ct:?}"
            );
        }
    }

    #[test]
    fn check_request_406_reject_paths() {
        let contract = parse_contract("application/json", "application/json");
        for accept in [
            "application/xml",
            "application/json;q=0",
            "application/json;q=0, */*;q=1",
            "application/json;q=0, application/json;q=1",
        ] {
            match check_request(None, Some(accept), &contract) {
                Err(CamelError::NotAcceptable {
                    accept: got,
                    produced,
                }) => {
                    assert_eq!(got, accept.trim(), "accept must echo trimmed header");
                    assert_eq!(produced, "application/json");
                }
                other => panic!("expected NotAcceptable for {accept:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn check_request_406_pass_paths() {
        let contract = parse_contract("application/json", "application/json");
        for accept in [
            "application/json",
            "APPLICATION/JSON; charset=utf-8",
            "*/*",
            "application/*",
            "text/html, application/xhtml+xml, application/json;q=0.9",
        ] {
            assert!(
                check_request(None, Some(accept), &contract).is_ok(),
                "expected Accept pass for {accept:?}"
            );
        }
        assert!(
            check_request(None, None, &contract).is_ok(),
            "absent Accept is permissive"
        );
    }

    #[test]
    fn parse_rejects_suffix_on_wildcard() {
        assert!(
            parse_accept_entry("*/*+json").is_none(),
            "suffix on */* must be rejected"
        );
        assert!(
            parse_accept_entry("application/*+json").is_none(),
            "suffix on type/* must be rejected"
        );
        let contract = parse_contract("application/json", "application/json");
        assert!(
            check_request(None, Some("application/*+json"), &contract).is_ok(),
            "suffix-on-wildcard Accept entry is malformed and degrades to */*"
        );
    }

    #[test]
    fn parse_quoted_params_degrade_to_malformed() {
        let raw = "application/json;foo=\"a;b\";q=0";
        assert!(
            parse_accept_entry(raw).is_none(),
            "quoted param value must make the entry malformed"
        );
        let contract = parse_contract("application/json", "application/json");
        assert!(
            check_request(None, Some(raw), &contract).is_ok(),
            "quoted-param Accept entry degrades to */*"
        );
    }

    #[test]
    fn check_request_malformed_accept_is_permissive() {
        let contract = parse_contract("application/json", "application/json");
        for accept in ["garbage header!!", "application/json, garbage entry!!"] {
            assert!(
                check_request(None, Some(accept), &contract).is_ok(),
                "malformed Accept must be treated as */*: {accept:?}"
            );
        }
    }

    #[test]
    fn moved_helpers_behavior_unchanged() {
        // (input, is_valid_media_declaration, is_json_media_type) — the
        // booleans the pre-move rest.rs tests pin for these inputs.
        let matrix: &[(&str, bool, bool)] = &[
            ("application/json", true, true),
            ("image/png", true, false),
            ("png", false, false),
            ("image / png", false, false),
            ("/png", false, false),
            ("text/plain; charset=utf-8", true, false),
            ("application/vnd.api+json", true, true),
        ];
        for (media, valid, json) in matrix {
            assert_eq!(
                is_valid_media_declaration(media),
                *valid,
                "declaration validity for {media:?}"
            );
            assert_eq!(
                is_json_media_type(media),
                *json,
                "JSON family for {media:?}"
            );
        }
    }
}
