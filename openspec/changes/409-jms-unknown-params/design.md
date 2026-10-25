# Design: 409-jms-unknown-params

## Approach

All endpoint-parser changes live in `crates/components/camel-jms`; one
additive helper lands in `crates/camel-api`.

**1. One declaration drives recognition AND dispatch (`config.rs`).**

```rust
#[derive(Clone, Copy, PartialEq)]
enum KnownParam { Broker, AckMode, MessageSelector, ConcurrentConsumers,
    TransactionMode, TimeToLive, Priority, PersistentDelivery,
    MapJmsHeaders, ExchangePattern, HeaderAllowlist }

/// Single source of truth: every recognized key, mapped to its group.
/// The first name for a variant is its canonical (camelCase) form.
const KNOWN_PARAM_NAMES: &[(&str, KnownParam)] = &[
    ("broker", KnownParam::Broker),
    ("acknowledgementMode", KnownParam::AckMode),
    ("acknowledgement_mode", KnownParam::AckMode),
    /* ... one row per existing arm literal, unchanged ... */
];
```

The parse loop dispatches on `known_param(k)` (a table lookup returning
`Option<KnownParam>`). The `match` arms are `Some(KnownParam::X)` per
variant — the existing bodies (guards such as `!v.is_empty()` for
`broker`/`messageSelector`, parse/validation, last-write-wins) move over
byte-for-byte; guarded variants get an explicit empty-value companion arm
reproducing today's silent-ignore semantics (`?broker=`, `?message_selector=`
parse clean with defaults, as today). `None =>` collects the unknown key.
Because the match must be exhaustive over `KnownParam`, a variant with a
name-table row but no handler arm is a **compile error**, and a handler arm
cannot exist without a variant. A stub arm (`Some(KnownParam::X) => {}`)
that silences the compiler without handling the key is caught by a
completeness test: parse every row of `KNOWN_PARAM_NAMES` with a non-default
sample value and assert the specific config field changed (the struct has no
`Default`/`PartialEq`; per-field assertions). Drift is therefore structurally
impossible in both directions. The canonical form for suggestions comes from
the first table row per variant via an exhaustive `canonical()` method —
a new variant cannot skip it.

Bare segments without `=` keep today's `continue`. Duplicate unknown keys
are preserved per occurrence, in source order. Values are never stored.

**2. Credential-shaped keys never echo (ADR-0076 appendix §2).** Before an
unknown key is stored, it passes `camel_api::redact::
is_credential_shaped_key(k)`; a credential-shaped key (including
percent-encoded shapes such as `user%3Asecret%40host`) is stored as
`"<redacted>"` — the raw key never enters the struct, its derived `Debug`,
or any event. Non-credential keys are stored as-is. No suggestion is
computed for a redacted key. camel-api gains the predicate as a thin
`pub fn` delegating to its existing private `is_credential_shaped` +
`minimal_decode_pair` logic — the exact code path
`redact_url_with_query_allowlist` already uses to suppress such keys, so no
second predicate can drift.

**3. Near-match suggestion (`config.rs`).** Normalize with the existing
`normalize_header_name` (casefold, strip `-`/`_`/`.`), compare against every
variant's canonical form with a small hand-rolled Levenshtein (no new
dependency). Suggest when normalized distance ≤ 3 and ≤ half the normalized
unknown key's length, keys capped at 64 characters (bounded work); ties
broken by first variant in definition order. Otherwise "no close match".
Normalization is for suggestions only, never acceptance — known-key matching
stays exact.

**4. One warning per endpoint (`component.rs`, `create_endpoint`).**
Immediately after `from_uri` returns `Ok`, if `unknown_params` is
non-empty, emit one `tracing::warn!` listing the keys in source order,
per-key suggestion or "no close match", and the "ignored" hint, via
structured fields. The event carries keys/suggestions only — never values,
never the URI (query strings hold values; `redact_url` drops queries for
the same reason). Direct `from_uri` callers (tests, internal helpers) stay
silent; the warning lives at the component boundary. No cross-endpoint
deduplication: every endpoint with unknowns warns once.

`JmsEndpointConfig` keeps `#[derive(Debug, Clone)]` — it has no serde
derives today (the mission's `deny_unknown_fields` wording binds to
`BrokerTlsConfig`, untouched); Debug stays safe because raw credential keys
and all values never enter the struct. `PartialEq` may be added only if a
test needs it and all field types support it.

Workspace audit: confirm no struct-literal construction of
`JmsEndpointConfig` outside `from_uri` (additive fields break exhaustive
literals). Docs: one sentence in `docs/src/components/jms.md`.

## Affected crates

- camel-component-jms: config.rs (table, dispatch, redaction gate,
  suggestion, `unknown_params` field, tests), component.rs (warning call
  site + test), docs line.
- camel-api: additive `pub fn is_credential_shaped_key` in `redact.rs`
  (delegates to existing private logic; unit tests pin it).

## Architecture boundaries

Components-layer only (plus one additive camel-api redact helper). No
Runtime, DSL, Services, Languages, or Functions changes. The diagnostic
respects ADR-0012 log-level policy (warn, not error — an unknown parameter
must not fail boot) and ADR-0076 appendix §2 (credential-shaped keys render
as a bare `<redacted>`; the key never echoes). Endpoint creation remains
the Component's factory boundary per CONTEXT-MAP language.

## Phases

Single-phase. The change is one coherent slice: config collection + warning
surface + tests, small enough that phase grouping adds no value.
