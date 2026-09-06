# Delta: endpoint-uri

## ADDED Requirements

### Requirement: Raw query preservation at parse

`UriComponents` MUST capture the authored query string byte-for-byte in an additive `raw_query: Option<String>` field at parse time. The structured `params` view MUST remain unchanged: duplicate keys still fail parse loudly, decoded access semantics are untouched. The raw view MUST be verbatim — no normalization, no re-encoding, no `RAW(...)` unwrapping at capture; redaction classification applies only at display surfaces.

#### Scenario: Authored bytes captured verbatim

- **Given** a component URI with query `?a=1&b=x%2Cy&c=t:1`
- **When** the URI is parsed into `UriComponents`
- **Then** `raw_query` equals the authored byte string exactly (order, escapes, and separators preserved) and `params` decodes each key for structured access

#### Scenario: Duplicate keys still rejected loudly

- **Given** a component URI with query `?a=1&a=2`
- **When** the URI is parsed
- **Then** parse fails with an `InvalidUri` error naming the duplicate key, exactly as before — the raw view introduces no silent collapse

#### Scenario: Absent query

- **Given** a component URI without a query component
- **When** the URI is parsed
- **Then** `raw_query` is `None`

#### Scenario: Empty query marker

- **Given** a component URI ending in a bare `?`
- **When** the URI is parsed
- **Then** `raw_query` is `Some("")` — the marker survives verbatim; it is not conflated with absence

#### Scenario: RAW wrapper stays as authored in the raw view

- **Given** a component URI whose query contains a `RAW(...)`-wrapped value
- **When** the URI is parsed
- **Then** `raw_query` keeps the wrapper text byte-for-byte and `params` classification follows ADR-0051 — capture never unwraps, redacts, or re-encodes
