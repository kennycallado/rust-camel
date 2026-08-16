## ADDED Requirements

### Requirement: EndpointUri typed seam with fail-closed merge

camel-api SHALL provide an `EndpointUri` value type (`#[non_exhaustive]`) holding `scheme`, `path`, and `params: BTreeMap<String,String>`, constructed via `try_from_uri_and_params(base: &str, params: BTreeMap<String,String>)`.

Accepted base-URI grammar: `<scheme>:<path>` optionally followed by `?` and a query of `&`-separated pairs. `scheme` SHALL be the non-empty substring before the first `:`. Query pairs SHALL split on the FIRST `=` only (subsequent `=` characters are literal value bytes); a pair with no `=` has an empty value. `#` SHALL NOT be treated as a fragment marker (matching the string-based runtime, which does not special-case it). Construction SHALL fail, returning an `EndpointUriError` (convertible into `CamelError::EndpointUri`) naming the offending input, when: the scheme is missing or empty; a query pair has an empty key; or a `params` map key violates the key policy — non-empty and containing none of the bytes `&`, `=`, `%`, `#`, `?`, `+`, or space. Repeated keys WITHIN the query string SHALL be accepted and preserved verbatim (list-valued options use repeated keys). Construction SHALL fail closed when the same raw key string appears both in the base URI query string and in the `params` map — never silently merging or overriding.

#### Scenario: uri and parameters merge into a canonical string

- **GIVEN** base URI `kafka:orders` and params `{brokers: my-host:9092, acks: all}`
- **WHEN** `EndpointUri::try_from_uri_and_params` succeeds and `to_canonical_string()` is called
- **THEN** the result is `kafka:orders?acks=all&brokers=my-host:9092` (params sorted by BTreeMap order; `:` is not in the encoding set and passes through)

#### Scenario: duplicate key across query string and parameters fails closed

- **GIVEN** base URI `kafka:orders?brokers=a` and params `{brokers: b}`
- **WHEN** `EndpointUri::try_from_uri_and_params` is called
- **THEN** construction returns an `EndpointUriError::DuplicateKey` naming the conflicting key `brokers` and both sources

#### Scenario: repeated query keys are preserved, not conflated

- **GIVEN** base URI `list:demo?item=a&item=b` and empty params
- **WHEN** construction succeeds and `to_canonical_string()` is called
- **THEN** the output contains both pairs in their original order (`item=a&item=b`)

#### Scenario: malformed bases are rejected with named errors

- **GIVEN** each of `noscheme`, `:pathonly`, `timer:tick?=1` (empty query key), and params containing key `a&b`
- **WHEN** `try_from_uri_and_params` is called with each
- **THEN** each returns an `EndpointUriError` variant identifying the malformed part (missing/empty scheme, empty query key, invalid param key)

### Requirement: Deterministic canonical rendering for source_hash stability

`EndpointUri::to_canonical_string()` SHALL be deterministic. Rendering contract: the existing query string of the base URI SHALL be preserved byte-for-byte in its original position and order (no re-encoding of existing pairs); when `params` is non-empty, its entries SHALL be appended after the existing query (or after a newly introduced `?` when none existed) in BTreeMap sorted order; each appended key SHALL be emitted verbatim (keys are pre-validated by the construction key policy to contain no reserved bytes) and each value SHALL be percent-encoded with uppercase hex over the UTF-8 bytes, encoding exactly the reserved set `& = % # ? +` and space (space as `%20`, never `+`); all other bytes — including `:` and multi-byte UTF-8 — SHALL pass through unchanged. The same inputs SHALL always produce byte-identical output. Round-trip stability SHALL hold: the canonical string re-parses (via representative component `from_uri` parsers) to the same scheme, path, and parameter set.

#### Scenario: Deterministic output across constructions

- **GIVEN** params inserted in different orders (e.g. `{b: 2, a: 1}` then `{a: 1, b: 2}`) against the same base URI
- **WHEN** both `EndpointUri` values render `to_canonical_string()`
- **THEN** the two output strings are byte-identical

#### Scenario: Existing query order is preserved when no parameters are given

- **GIVEN** base URI `timer:tick?period=1000&repeatCount=6` and empty params
- **WHEN** `to_canonical_string()` is called
- **THEN** the output is byte-identical to the input base URI

#### Scenario: Golden rendering with reserved characters and mixed sources

- **GIVEN** base URI `http:srv?a=1&flag` and params `{z: "100%", q: "a b+c"}`
- **WHEN** `to_canonical_string()` is called
- **THEN** the output is exactly `http:srv?a=1&flag&q=a%20b%2Bc&z=100%25` (existing pairs untouched and first, params sorted, space `%20`, `+` `%2B`, `%` `%25`)

#### Scenario: Canonical string round-trips through a component parser

- **GIVEN** an `EndpointUri` built from `timer:tick` + `{period: 2500}` (a NON-default value, so a dropped parameter cannot coincide with the parser default)
- **WHEN** `to_canonical_string()` output is parsed by the timer component's `from_uri`
- **THEN** the parsed config carries `period == 2500` (the non-default value) and matches the config parsed from the literal `timer:tick?period=2500`

### Requirement: Redacting wrapper classification for EndpointUri

`EndpointUri` SHALL comply with the ADR-0051 `redacting-wrapper` class: it SHALL NOT derive `Serialize` (it never crosses a persistence boundary), and its `Debug` implementation SHALL mask ALL parameter values (Debug has no catalog access; fail-safe masking) AND SHALL omit the raw query bytes; if the path carries RFC 3986 userinfo (`//user:pass@...`), the userinfo credential segment SHALL be masked in `Debug` and the redacted rendering (the canonical string stays byte-faithful, mirroring the raw-query treatment). `to_redacted_string(&dyn ComponentMetadataCatalog)` SHALL produce the canonical shape with a value masked UNLESS the catalog affirmatively resolves the option (by name or alias; an option carrying `pattern: Some(_)` does not match by its anchor name) for the scheme AND marks it non-secret; unknown schemes and unresolved options SHALL be masked (fail-safe redaction).

#### Scenario: Debug never leaks any parameter values

- **GIVEN** an `EndpointUri` built from `http://srv?password=clear` with params `{delay: 1000}`
- **WHEN** the value is formatted with `{:?}`
- **THEN** the output contains a redaction placeholder for every param value and contains neither `1000` nor `clear`

#### Scenario: Debug and redacted rendering mask userinfo credentials

- **GIVEN** an `EndpointUri` built from `http://admin:hunter2@srv/path`
- **WHEN** the value is formatted with `{:?}` and with `to_redacted_string(catalog)`
- **THEN** neither output contains `hunter2`, while `to_canonical_string()` remains byte-identical to the input

#### Scenario: to_redacted_string masks secret-flagged values and passes known non-secrets

- **GIVEN** an `EndpointUri` for scheme `http` with params `{password: hunter2, timeout: 5000}` and a catalog whose `http` metadata marks `password` secret and `timeout` non-secret
- **WHEN** `to_redacted_string(catalog)` is called
- **THEN** the output equals the canonical string with the `password` value replaced by a placeholder and `timeout=5000` rendered in clear

#### Scenario: Aliases resolve like names, pattern anchors do not

- **GIVEN** a catalog whose `http` metadata defines an option `token` (alias `apikey`, non-secret, no pattern) and a prefix-pattern option anchored `cfg` (non-secret, `pattern: Some(_)`)
- **WHEN** `to_redacted_string` renders params `{apikey: abc}` and `{cfg.foo: bar}`
- **THEN** `apikey` resolves through its alias and renders clear; `cfg.foo` does NOT resolve via the pattern anchor and renders masked

#### Scenario: Unknown scheme degrades to fail-safe masking

- **GIVEN** an `EndpointUri` with scheme `not-a-scheme` absent from the catalog and params `{token: abc}`
- **WHEN** `to_redacted_string(catalog)` is called
- **THEN** the output shows `token` with a masked value (no error, no clear-text leak)
