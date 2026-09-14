## ADDED Requirements

### Requirement: URL diagnostics redaction fails closed on unparseable URLs

The camel-http diagnostics redaction path
(`redact_url_for_diagnostics`) SHALL fail closed when the internal URL
parser rejects the input: the output SHALL NOT contain any byte of the
input's authority region when that region may carry userinfo. When the
authority window — the substring beginning immediately after the first
`//` and ending at the next `/`, `?`, or `#` searched from that offset —
contains `@`, the path SHALL return the constant sentinel
`[redacted]` and no other content. When no `//` authority window exists or
the window carries no `@`, the input SHALL remain visible for diagnostics,
with any query string dropped at the first `?` and replaced by
`?[redacted]` before any length cap is applied, and the result capped at
256 bytes floored to a UTF-8 character boundary; no byte of the original
query SHALL survive, and the `?[redacted]` suffix SHALL be present
whenever it fits within the cap. These parse-failure
guarantees apply to every diagnostics render of URL text, including the
`allowedUriHosts` fence rejection of a raw `CamelHttpUri` override whose
predicate rejects unparseable inputs. The parse-success arm
SHALL remain unchanged: userinfo masked with `***` (including
password-only userinfo, rc-u4jk6), query rendered `?[redacted]`, same
256-byte cap.

#### Scenario: unparseable authority with credentials is fully suppressed

- **GIVEN** a URL string that fails `url::Url::parse` and whose authority
  window contains `@`, such as `http://u:secretpw@/x` (empty host) or
  `http://u:secretpw@host:99999/x` (invalid port)
- **WHEN** the string passes through the diagnostics redaction path
- **THEN** the output is exactly `[redacted]`, containing neither
  `secretpw` nor any other byte of the input's authority region

#### Scenario: bd-repro credential string never leaks regardless of arm

- **GIVEN** the bd rc-2i5c5 repro string `http://user:pa%ss@host/path`
  whose userinfo carries credentials
- **WHEN** the string passes through the diagnostics redaction path
- **THEN** whichever arm processes it, the output contains neither
  `user:pa%ss` nor `pa%ss` — credential bytes cannot survive diagnostics

#### Scenario: unparseable credential-free string stays visible, capped on char boundary

- **GIVEN** a string that fails `url::Url::parse` and whose authority
  window carries no `@`, such as 1000 `x` characters or a multibyte-UTF-8
  string longer than 256 bytes
- **WHEN** the string passes through the diagnostics redaction path
- **THEN** the output is the input capped at 256 bytes, the cut aligned to
  a UTF-8 character boundary, and the render does not panic

#### Scenario: unparseable query string is redacted before truncation

- **GIVEN** a string that fails `url::Url::parse`, carries no `@` in its
  authority window, and contains `?token=shortsecret`, in both a short
  (<256 bytes) and a long (>256 bytes pre-query text) form
- **WHEN** the string passes through the diagnostics redaction path
- **THEN** no byte of the original query appears in the output in either
  form — the query is dropped before the cap; in the short form the
  output ends with `?[redacted]`; in the long form the output is capped
  at 256 bytes and the suffix appears only if it fits within the cap

#### Scenario: at-sign outside the authority window is not suppressed

- **GIVEN** a string whose `@` lies outside the authority window, such as
  `http://host:99999/x@y` (fails `url::Url::parse` on the port), or a
  scheme-only string like `mailto:user@example.com` with no `//` at all
  (parses; the render contract is byte-identical under either arm)
- **WHEN** the string passes through the diagnostics redaction path
- **THEN** the string is not suppressed to the sentinel — it renders under
  the query-redaction and cap rules only

#### Scenario: fence rejection never echoes credentials of an unparseable override

- **GIVEN** an endpoint declaring `allowedUriHosts` and an exchange whose
  raw `CamelHttpUri` header value fails to yield a host — the fence
  predicate rejects unparseable inputs — while carrying credentials in
  its authority region
- **WHEN** the producer resolves the outbound URL and the fence rejects
  the override
- **THEN** the fence error naming the fence contains no credential byte of
  the rejected URL — the rejected URL is rendered only through the
  fail-closed diagnostics redaction path

#### Scenario: parse-success arm behavior is unchanged

- **GIVEN** the existing parse-success fixtures (userinfo+query,
  password-only userinfo, clean URL, query-bearing URL)
- **WHEN** they pass through the diagnostics redaction path
- **THEN** the existing golden outputs hold: `***`-masked userinfo,
  `?[redacted]` queries, clean URLs visible, 256-byte cap
