## MODIFIED Requirements

### Requirement: URL diagnostics redaction fails closed on unparseable URLs

The camel-http diagnostics redaction path
(`redact_url_for_diagnostics`, composed over the canonical
`camel_api::redact` helpers) SHALL fail closed when the internal URL
parser rejects the input: the output SHALL NOT contain any byte of the
input's authority region when that region may carry userinfo. Authority
windows follow the canonical scan: every maximal run of `/` and `\`
characters opens a candidate window from the run's end to the next
`/`, `?`, or `#`; a slash-only run opens a window when it is two or
more characters long; a backslash-bearing run opens a window only when
an RFC 3986 scheme prefix sits immediately before it — a backslash run
of two or more characters needs any such prefix, a single-backslash
run needs a prefix of at least two characters or a one-character
prefix with credential-shaped content (`:` before the last `@` in the
candidate window). When any window contains
`@`, the path SHALL return the constant sentinel `[redacted]` and no
other content. When no window exists or no window carries `@`, the
input SHALL remain visible for diagnostics, with everything from the
earliest `?` or `#` dropped and one `?[redacted]` and/or `#[redacted]`
sentinel per distinct introducer appended in first-occurrence order,
the sentinel byte budget reserved before the result is capped at 256
bytes floored to a UTF-8 character boundary; no byte of the original
query or fragment SHALL survive, and each sentinel SHALL render
complete whenever it fits within the cap. These parse-failure
guarantees apply to every diagnostics render of URL text, including the
`allowedUriHosts` fence rejection of a raw `CamelHttpUri` override
whose predicate rejects unparseable inputs. The parse-success arm
SHALL keep its accessor userinfo mask with `***` (including
password-only userinfo, rc-u4jk6) and then compose over the canonical
strict variant: the render keeps query and fragment set, and the
canonical window mask, compose-both sentinels, and 256-byte cap apply
to the rendered string — so a fragment that itself contains `?`
renders both sentinels (`#[redacted]?[redacted]`), never fewer
sentinel bytes than the introducers present.

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

#### Scenario: backslash authority on a non-special scheme is masked

- **GIVEN** `foo:\\user:pass@evil/` (non-special scheme: the url crate
  does not normalize the backslashes, the string carries no `//` run)
  and `foo:\\clean/path`
- **WHEN** each passes through the diagnostics redaction path
- **THEN** the first renders no credential byte — its scheme-prefixed
  backslash run opens a window and the credentials are suppressed or
  masked — and the second stays visible

#### Scenario: drive and UNC shaped inputs stay visible

- **GIVEN** `C:\Users\x@corp\file` (single backslash after a
  one-character scheme, no `:` in the candidate window) and
  `\\server\x@y` (UNC: no scheme prefix)
- **WHEN** each passes through the diagnostics redaction path
- **THEN** neither opens an authority window — the strings render under
  the query-redaction and cap rules only, keeping diagnosability for
  inputs that are not URL-shaped

#### Scenario: one-character scheme with credential-shaped content is masked

- **GIVEN** `x:\user:pass@evil` — a single backslash after the
  one-character scheme `x:`, whose candidate window carries `:` before
  its last `@`
- **WHEN** it passes through the diagnostics redaction path
- **THEN** the credential bytes never render — the window opens and the
  userinfo is suppressed or masked

#### Scenario: parse-success fragment sentinel composes

- **GIVEN** `https://h/p#access_token=x` and `https://h/cb#f?state=x`
- **WHEN** each passes through the diagnostics redaction path
- **THEN** the first renders `https://h/p#[redacted]` and the second
  `https://h/cb#[redacted]?[redacted]` — fragment credential bytes never
  render, and sentinels compose for every introducer present
