# egress-cleartext-policy Specification

## ADDED Requirements

### Requirement: Public cleartext egress rejected by default

Every outbound egress path in camel-http and the MCP producer MUST
reject cleartext `http://` to a PUBLIC target (an address that
`camel_api::is_ssrf_blocked_ip` classifies as not blocked) unless the
operator sets `allow_cleartext` on that target. The rejection MUST be
independent of `allow_internal`.

#### Scenario: camel-http rejects a public IP literal over http by default

- **Given** an `HttpEndpointConfig` with `allow_internal=false` and `allow_cleartext=false`
- **When** `validate_url_for_ssrf` validates `http://93.184.216.34/api`
- **Then** validation fails with an error naming the cleartext refusal and the `allowCleartext` remedy

#### Scenario: camel-http rejects a public hostname over http by default

- **Given** a hostname URL `http://example.com/` with `allow_internal=false` and `allow_cleartext=false`
- **When** `resolve_initial_url_for_ssrf` resolves the host to at least one public address
- **Then** resolution fails with an error naming the public cleartext refusal and the `allowCleartext` remedy

#### Scenario: camel-http rejects a public cleartext redirect hop by default

- **Given** a redirect target URL with scheme `http` whose host is a public IP literal, or resolves to a public address, with `allow_internal=false` and `allow_cleartext=false`
- **When** `validate_redirect_target_for_ssrf` validates the hop
- **Then** the hop is rejected with a cleartext-refusal error

#### Scenario: MCP rejects a public IP-literal remote over http at config load by default

- **Given** an `McpRemoteConfig` with `url = http://93.184.216.34/mcp`, `allow_internal=false`, `allow_cleartext=false`
- **When** `validate_url` runs at config load
- **Then** validation fails with an error naming the cleartext refusal and the `allow_cleartext` remedy

#### Scenario: MCP rejects a public hostname remote over http at connect by default

- **Given** an MCP remote resolved to addresses containing at least one public IP, scheme `http`, `allow_internal=false`, `allow_cleartext=false`
- **When** `validate_resolution` runs at connect time
- **Then** validation fails with a cleartext-refusal error naming the `allow_cleartext` remedy

#### Scenario: camel-http rejects a public hostname over http under allow_internal

- **Given** a hostname URL `http://example.com/` with `allow_internal=true` and `allow_cleartext=false`
- **When** `resolve_initial_url_for_ssrf` resolves the host to at least one public address
- **Then** resolution fails with the public cleartext-refusal error

#### Scenario: camel-http rejects a public cleartext redirect hop under allow_internal

- **Given** a redirect target with scheme `http` whose host is a public IP literal or resolves to a public address, `allow_internal=true`, `allow_cleartext=false`
- **When** `validate_redirect_target_for_ssrf` validates the hop
- **Then** the hop is rejected with the cleartext-refusal error

### Requirement: Public cleartext stays rejected under allow_internal

`allow_internal=true` MUST NOT reopen public cleartext egress. The
ADR-0079 cell (public cleartext under `allow_internal=true`) MUST keep
its rejection verdict in all four paths.

#### Scenario: camel-http public literal over http rejected under allow_internal

- **Given** an `HttpEndpointConfig` with `allow_internal=true` and `allow_cleartext=false`
- **When** `validate_url_for_ssrf` validates `http://93.184.216.34/api`
- **Then** validation fails with the cleartext-refusal error

#### Scenario: MCP public literal over http rejected under allow_internal

- **Given** an `McpRemoteConfig` with `url = http://93.184.216.34/mcp`, `allow_internal=true`, `allow_cleartext=false`
- **When** `validate_url` runs at config load
- **Then** validation fails with the cleartext-refusal error

#### Scenario: MCP hostname resolving public over http rejected under allow_internal

- **Given** an MCP remote resolution containing a public address, scheme `http`, `allow_internal=true`, `allow_cleartext=false`
- **When** `validate_resolution` runs
- **Then** validation fails with the cleartext-refusal error

### Requirement: allow_cleartext escape hatch permits public cleartext per endpoint and per remote

A per-endpoint (camel-http `allowCleartext` URI option) and per-remote
(MCP `allow_cleartext` TOML field) opt-in, both default `false`, SHALL
be the only mechanism that permits cleartext `http://` to a public
target. It MUST be independent of `allow_internal` and MUST NOT affect
internal-target reachability or https behavior. On camel-http, one
endpoint's consent covers its configured URL, `CamelHttpUri` override
targets, and redirect targets; host confinement beyond that requires
`allowedUriHosts`.

#### Scenario: camel-http allows public literal over http with allowCleartext

- **Given** an `HttpEndpointConfig` with `allow_cleartext=true` parsed from the `allowCleartext` URI option
- **When** `validate_url_for_ssrf` validates `http://93.184.216.34/api` with `allow_internal=false`
- **Then** validation succeeds

#### Scenario: camel-http allows public hostname and redirect hops over http with allowCleartext

- **Given** `allow_cleartext=true` with `allow_internal=false`
- **When** a hostname URL resolving only to public addresses is validated as an initial URL and as a redirect hop
- **Then** both validations succeed

#### Scenario: camel-http allows public cleartext with both flags set

- **Given** an `HttpEndpointConfig` with `allow_internal=true` and `allow_cleartext=true`
- **When** `validate_url_for_ssrf` validates `http://93.184.216.34/api`
- **Then** validation succeeds (explicit named consent overrides the ADR-0079 cell)

#### Scenario: CamelHttpUri override to public cleartext follows the endpoint flags

- **Given** an endpoint with `allow_cleartext=false`, unarmed `allowedUriHosts` fence, and an exchange carrying `CamelHttpUri=http://93.184.216.34/exfil`
- **When** the producer resolves the override URL
- **Then** the request fails with the cleartext-refusal error; with `allow_cleartext=true` on the endpoint, the same override resolves and dispatches

#### Scenario: MCP allows public literal and hostname remotes over http with allow_cleartext

- **Given** `McpRemoteConfig` with `allow_cleartext=true`
- **When** `validate_url` checks `http://93.184.216.34/mcp` (literal, either `allow_internal` value) and `validate_resolution` checks a public-only resolution over `http` (either `allow_internal` value)
- **Then** both validations succeed

#### Scenario: absent allow_cleartext deserializes to false

- **Given** an MCP remote TOML block and a camel-http endpoint URI that omit the new field/option
- **When** both configs load
- **Then** loading succeeds with `allow_cleartext=false`, and `deny_unknown_fields` behavior is unchanged

### Requirement: Internal-target and https behavior unchanged

The blocked-IP policy, `allow_internal` reachability semantics, and
https acceptance MUST keep their ADR-0079 behavior in all four paths.

#### Scenario: internal targets keep requiring allow_internal

- **Given** a URL pointing at a blocked/internal address with `allow_internal=false`
- **When** validated on any of the four paths
- **Then** the blocked-IP rejection fires (unchanged)

#### Scenario: internal cleartext under allow_internal stays allowed

- **Given** `http://127.0.0.1:8000` with `allow_internal=true` and `allow_cleartext=false`
- **When** validated on any of the four paths
- **Then** validation succeeds (cleartext to internal targets is not governed by the new rule)

#### Scenario: https to public targets stays allowed everywhere

- **Given** `https://` URLs to public literals and public hostnames with both flags false
- **When** validated on any of the four paths
- **Then** validation succeeds

#### Scenario: https-to-http redirect downgrade keeps stripping sensitive headers

- **Given** a cross-origin redirect from `https://` to `http://` carrying `Authorization`/`Cookie`/custom credential headers
- **When** the downgrade hop is followed (with `allow_cleartext=true`, since the hop is cleartext)
- **Then** the sensitive headers are stripped exactly as before (ADR-0057/F2-5 stripping is not relaxed by `allow_cleartext`)

#### Scenario: cleartext-refusal errors never echo credentials

- **Given** a URL rejected by the public-cleartext rule that carries userinfo (`http://user:pass@93.184.216.34/`) or query credentials
- **When** the rejection error is rendered on any of the four paths
- **Then** the message names the target host/class and the remedy only; userinfo and query values never appear (ADR-0051 redaction)
