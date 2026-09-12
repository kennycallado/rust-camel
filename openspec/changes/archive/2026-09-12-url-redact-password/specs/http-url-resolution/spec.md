## MODIFIED Requirements

### Requirement: CamelHttpUri host fence

The endpoint URI SHALL accept an `allowedUriHosts` option: comma-separated
exact host entries, each optionally `host:port`. Empty comma segments are
dropped; a declared option that yields zero valid entries SHALL fail
endpoint creation, as SHALL any other malformed entry. DNS names SHALL
compare case-insensitively; IPv6 literals compare in bracketed canonical
form; a host-only entry permits any port; a `host:port` entry matches only
the override's effective port. When the option is declared and a
`CamelHttpUri` override resolves to a host not matching any entry, or the
override URI fails to yield a host, the producer SHALL fail resolution with
an error that renders the rejected URL only through the diagnostics
redaction path. When the option is absent, override behavior is unchanged.
The option SHALL be consumed: it never appears in the outbound query.
When the producer follows redirects, every hop target SHALL satisfy the
same fence, using the same entries as override resolution (ADR-0071): a
redirect to a host not matching any entry, or whose URL fails to yield a
host, SHALL fail the request closed with an error naming the fence, and no
request SHALL be sent to the unlisted host. (Per-hop enforcement landed in
83cc7e5f, bd rc-sxe1x; hop host classification via `classify_host`, bd
rc-uwaj.) The diagnostics redaction path SHALL mask userinfo whenever the
rejected URL carries any credential byte in userinfo, including
password-only userinfo (`http://:pass@host/`, valid RFC 3986), which SHALL
render with the same `***@` masking shape as username-bearing userinfo.

#### Scenario: armed fence rejects unknown host with redacted error

- **GIVEN** an endpoint declaring `allowedUriHosts=api.internal:8443,cdn.example.com`
  and an exchange carrying
  `CamelHttpUri=http://user:pass@evil.example.com/x?token=s3cret`
- **WHEN** the producer resolves the outbound URL
- **THEN** resolution fails, and the error text contains neither `pass` nor
  `s3cret` — the rejected URL is rendered only through the diagnostics
  redaction path

#### Scenario: armed fence rejects password-only userinfo with redacted error

- **GIVEN** an endpoint declaring `allowedUriHosts=api.internal:8443,cdn.example.com`
  and an exchange carrying
  `CamelHttpUri=http://:passwordonly@evil.example.com/x?token=querysecret`
- **WHEN** the producer resolves the outbound URL
- **THEN** resolution fails, and the error text contains neither
  `passwordonly` nor `querysecret` — the rejected URL is rendered only
  through the diagnostics redaction path, with the password-only userinfo
  masked as `***@`

#### Scenario: armed fence allows listed host

- **GIVEN** the same endpoint and an exchange carrying
  `CamelHttpUri=http://cdn.example.com/x`
- **WHEN** the producer resolves the outbound URL
- **THEN** the outbound URL is `http://cdn.example.com/x`

#### Scenario: host-only entry permits any port

- **GIVEN** an endpoint declaring `allowedUriHosts=cdn.example.com` and an
  exchange carrying `CamelHttpUri=http://cdn.example.com:9443/x`
- **WHEN** the producer resolves the outbound URL
- **THEN** the override is honored

#### Scenario: unarmed endpoint unchanged

- **GIVEN** an endpoint without `allowedUriHosts` and an exchange carrying
  `CamelHttpUri=http://any.example.com/path`
- **WHEN** the producer resolves the outbound URL
- **THEN** the override is honored exactly as before the fence existed

#### Scenario: redirect hop to an unlisted host fails closed

- **GIVEN** an endpoint declaring `allowedUriHosts=api.internal:8443`
  whose request to `api.internal:8443` is answered with a redirect to
  `http://intranet.evil.example/x`
- **WHEN** the producer follows redirects
- **THEN** the request fails closed with an error naming the fence, and no
  connection is made to the unlisted host

#### Scenario: redirect hop to a listed host is followed

- **GIVEN** the same endpoint whose request is answered with a redirect to
  `http://api.internal:8443/next`
- **WHEN** the producer follows redirects
- **THEN** the hop is followed and the exchange completes
