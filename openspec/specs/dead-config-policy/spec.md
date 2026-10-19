# dead-config-policy Specification

## Purpose
Fail-closed handling of configuration fields. The runtime rejects fields it
does not consume, keeps `proxy_url` only for deserialization and rejects any
value, enforces the WebSocket client send timeout, and resolves `${env:}`
placeholders over every string leaf with a shared syntax.
## Requirements
### Requirement: No silently ignored config fields

The system SHALL NOT parse a config field from URI parameters or TOML configuration unless the field is consumed by runtime logic. When a removed parameter is present in a URI, the system SHALL reject it with an error indicating the parameter is not supported.

**Exception:** A field may be retained on a config struct for serde deserialization compatibility if `validate()` rejects any non-default value with an explicit error (fail-closed). Such a field is not considered silently ignored because the operator receives an error at validation time.

#### Scenario: Removed xj transformDirection rejected

- **GIVEN** a `camel-xj` endpoint URI with `transformDirection=XML2JSON`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `transformDirection` is not supported (use `direction` instead)

#### Scenario: Removed xj resourceUri rejected

- **GIVEN** a `camel-xj` endpoint URI with `resourceUri=classpath:extra.xslt`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `resourceUri` is not supported

#### Scenario: Removed http cookieHandling rejected

- **GIVEN** a `camel-http` endpoint URI with `cookieHandling=InMemory`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `cookieHandling` is not supported

#### Scenario: Removed direct block rejected

- **GIVEN** a `camel-direct` endpoint URI with `block=true`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `block` is not supported

#### Scenario: Removed direct exchange_pattern rejected

- **GIVEN** a `camel-direct` endpoint URI with `exchange_pattern=InOnly`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `exchange_pattern` is not supported

#### Scenario: Removed direct exchangePattern (camelCase) rejected

- **GIVEN** a `camel-direct` endpoint URI with `exchangePattern=InOnly`
- **WHEN** the URI is parsed
- **THEN** parsing fails with an error indicating `exchangePattern` is not supported

### Requirement: proxy_url validation rejection

The system SHALL retain the `proxy_url` field on `HttpConfig` for serde deserialization compatibility but SHALL reject any non-None value at validation time with an SSRF-specific error.

#### Scenario: proxy_url set to valid URL rejected at validation

- **GIVEN** an `HttpConfig` with `proxy_url` set to `Some("http://proxy:8080")`
- **WHEN** `validate()` is called
- **THEN** validation fails with an error stating proxy_url is incompatible with SSRF DNS pinning

#### Scenario: proxy_url None passes validation

- **GIVEN** an `HttpConfig` with `proxy_url` set to `None`
- **WHEN** `validate()` is called
- **THEN** validation succeeds

#### Scenario: proxy_url from TOML rejected at validation

- **GIVEN** TOML config with `proxy_url = "http://proxy:8080"` (deserializes successfully because the field is retained)
- **WHEN** `validate()` is called
- **THEN** validation fails with the SSRF incompatibility error

### Requirement: WebSocket send timeout enforcement (client mode)

The system SHALL enforce `send_timeout` on the client-mode WebSocket send path (`ws_stream.send`). Server-send mode (internal mpsc `try_send_with_backpressure`) SHALL NOT be affected. When the timeout elapses before the client-mode send completes, the system SHALL return an error.

#### Scenario: Client send completes within timeout

- **GIVEN** a `camel-ws` client endpoint with `sendTimeoutMs=5000`
- **WHEN** a message is sent via `ws_stream.send` and the sink accepts it within 5 seconds
- **THEN** the send succeeds with no error

#### Scenario: Client send exceeds timeout

- **GIVEN** a `camel-ws` client endpoint with `sendTimeoutMs=100`
- **WHEN** a message is sent via `ws_stream.send` and the sink does not accept it within 100 milliseconds
- **THEN** the send returns a timeout error

#### Scenario: Default send timeout

- **GIVEN** a `camel-ws` endpoint with no `sendTimeoutMs` specified
- **WHEN** the endpoint configuration is resolved
- **THEN** the default send timeout is 30 seconds

### Requirement: Removal of never-consumed native issuer surface

The `token_issuer` and `clients` config fields and their backing runtime code
(`native_issuer`, `native_client_store`, `native_jwks`, `ApiKeyAuthenticator`,
`camel-http::auth` wrapper) SHALL be removed. `deny_unknown_fields` SHALL reject
stale configs loudly. The scalar `api_key` field SHALL remain and SHALL be wired
as a single-entry credential.

#### Scenario: Stale token_issuer config fails loudly

- **GIVEN** a Camel.toml containing `[security.native.token_issuer]`
- **WHEN** the CLI starts
- **THEN** startup fails with an unknown-field configuration error rather than silently ignoring the block

#### Scenario: Dead code is gone from the workspace

- **GIVEN** the merged main branch
- **WHEN** searching the workspace for `NativeTokenIssuer`, `M2mClientStore`, `NativeJwksProvider`, `ApiKeyAuthenticator`
- **THEN** no definitions or references remain outside git history

### Requirement: No documented placeholder recipe ships a literal credential

Documentation for `security.*` credential fields SHALL NOT present placeholder
syntax that the resolver does not support. Where a placeholder form is
documented for a credential field, the documented behavior SHALL match the
fail-closed semantics: unset variable means startup failure, not a live
literal-credential. The ambiguous `{{env:VAR:-default}}` double-dash form SHALL
be documented as rejected.

#### Scenario: Docs recipes match resolver behavior

- **GIVEN** `docs/src/configuration/schema.md` and `crates/camel-config/README.md` after this change
- **WHEN** a reader follows any placeholder recipe for `bearer_token`, `api_key`, or `client_secret` exactly as written
- **THEN** the recipe either resolves the secret from the environment or fails closed with a `ConfigError` — no recipe produces a config where the literal placeholder string is the accepted credential

#### Scenario: Double-dash default form is documented as rejected

- **GIVEN** the configuration docs after this change
- **WHEN** a reader writes `{{env:X:-changeme}}` in a security credential field with `X` unset
- **THEN** the documented and actual behavior is a `ConfigError` at startup, and the docs state this explicitly in the syntax-boundary note

### Requirement: cache_repo cross-backend field rejection

The `cache_repo` configuration section SHALL fail validation, per the dead-config-policy
fail-closed principle, when fields that do not apply to the configured `backend` are set
to non-default values: with `backend = "memory"`, any of `path`, `stale_retention`,
`max_entries`, `cache_size`, or `sweep_interval` set SHALL be rejected; with
`backend = "redb"`, `max_capacity` set SHALL be rejected. An omitted `stale_retention`
SHALL deserialize as `None` (the serde default SHALL NOT materialize a value for an
absent field), and the 7-day fallback SHALL apply in wiring only after validation
passes for the redb backend — so memory-backend configs that omit the field validate
unchanged.

#### Scenario: cache_size on memory backend rejected

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and
  `cache_repo.cache_size = "512MiB"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.cache_size` as not applicable
  to the `memory` backend

#### Scenario: path on memory backend rejected

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and
  `cache_repo.path = "data/cache.redb"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.path` as not applicable to the
  `memory` backend

#### Scenario: max_capacity on redb backend rejected

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` with valid redb fields
  and `cache_repo.max_capacity = 5000`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.max_capacity` as not
  applicable to the `redb` backend

#### Scenario: omitted stale_retention stays None on memory backend

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` with only
  `max_capacity` set (stale_retention omitted)
- **WHEN** the config is deserialized and validated
- **THEN** `stale_retention` deserializes as `None` and validation succeeds

#### Scenario: omitted stale_retention falls back in wiring for redb

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` with a path and
  `cache_size`, stale_retention omitted
- **WHEN** the context is built from that config
- **THEN** the `"persistent"` repository is constructed with a 7-day stale retention
  applied by wiring, and validation did not treat the field as set

### Requirement: Config placeholder resolution is exhaustive over all string leaves

Camel.toml placeholder resolution MUST walk every string leaf of the configuration — typed
fields and untyped component/bean value maps — with no per-field allowlist. A newly added
config section's string leaves MUST resolve through the existing walk without any resolver
code change. Resolution MUST be prefix-gated on `${env:` so component-owned expressions
(`${body}`, `${file:...}`, `${1}`) pass through untouched.

#### Scenario: new section resolves without code change

- **Given** a Camel.toml carrying a synthetic `[future_section] value = "${env:SOME_VAR}"` — a section whose fields appear nowhere in the resolver or the typed config struct
- **When** the config loads with `SOME_VAR` set
- **Then** the leaf resolves to the env value with zero resolver code changes required (asserted at the raw-tree stage or via the struct's extra-fields capture)

#### Scenario: component expressions pass through

- **Given** a `[components.*]` value containing `${body}` or `${file:name}` (no `env:` prefix)
- **When** the config loads
- **Then** the value is unchanged — no resolution attempt, no error

### Requirement: Single interpolation syntax across routes and config

Camel.toml MUST use the same placeholder syntax and resolver semantics as route files:
`${env:NAME}` and `${env:NAME:-default}` resolved by the shared `interpolate_env` engine.
Legacy `{{...}}` placeholders in Camel.toml MUST be rejected at load with an actionable error
naming the field and the `${env:}` replacement — they MUST never resolve, warn, or pass
through silently. The STANDALONE `$$` escape MUST produce `$` on ALL string leaves (routes,
non-security config, security, datasource). The full escaped form `$${env:...}` MUST produce
the literal text `${env:...}` on the route surface and on non-security config leaves;
strict-gate leaves (security, datasources, idempotent_repo, cache_repo) reject that form via the
residual-marker gate — credentials have
no legitimate literal-placeholder content.

#### Scenario: legacy braces rejected with guidance

- **Given** any Camel.toml string leaf containing `{{env:FOO}}`
- **When** the config loads
- **Then** `load_config` returns `Err` whose message names the field and states the `${env:NAME}` / `${env:NAME:-default}` replacement forms

#### Scenario: escape yields literal on successful surfaces

- **Given** a Camel.toml non-security value `$${env:FOO}` and a route value `$${env:FOO}`
- **When** each loads through its pipeline
- **Then** both yield the literal text `${env:FOO}`

#### Scenario: standalone dollar escape on every leaf class

- **Given** values `a$$b` on a route, a non-security config leaf, a security leaf (`[security.keycloak] realm = "a$$b"`), a datasource leaf, and repo-section leaves (`[idempotent_repo] backend`, `[cache_repo] backend`)
- **When** each loads through its pipeline
- **Then** all values yield `a$b` — the standalone escape leaves no prohibited marker

#### Scenario: escaped placeholder rejected on security leaves

- **Given** `[security.native] bearer_token = "$${env:FOO}"` (escaped form on a credential leaf)
- **When** the config loads
- **Then** `load_config` returns `Err` via the residual-marker rejection — the literal placeholder text never reaches a credential store

#### Scenario: route interpolation unchanged

- **Given** a route file with `to: "log://${env:ROUTE_VAR}"` and `ROUTE_VAR` set
- **When** the route loads
- **Then** the endpoint resolves to the env value with semantics identical to before this change

### Requirement: Uniform fail-closed on missing environment variables

A `${env:NAME}` placeholder with the variable unset and no `:-default` MUST abort config load
with `ConfigError` naming the field — on every string leaf, security or not. Optional values
MUST declare `:-default`. A declared default MUST be used when the variable is unset.

#### Scenario: optional endpoint without default aborts

- **Given** `[observability.otel] endpoint = "${env:OTEL_EP}"` with `OTEL_EP` unset and no default
- **When** the config loads
- **Then** `load_config` returns `Err` naming `observability.otel.endpoint`

#### Scenario: default declared is used

- **Given** `[observability.otel] endpoint = "${env:OTEL_EP:-http://localhost:4317}"` with `OTEL_EP` unset
- **When** the config loads
- **Then** the endpoint resolves to `http://localhost:4317` and load succeeds

#### Scenario: security credential fails closed under new syntax

- **Given** `[security.native] bearer_token = "${env:APP_TOKEN}"` with `APP_TOKEN` unset
- **When** the config loads
- **Then** `load_config` returns `Err` — the literal placeholder never becomes a credential

#### Scenario: CLI surfaces load errors instead of silent defaults

- **Given** a `Camel.toml` that fails to load (parse error, broken include, or unresolved `${env:...}` without default)
- **When** `camel run` starts with that file
- **Then** the command aborts with an error naming the file and cause — it never boots on empty-config defaults silently

### Requirement: cxf signature knobs are applied or rejected

The cxf bridge sidecar SHALL apply the parsed signature configuration
(`SIGNATURE_ALGORITHM`, `SIGNATURE_DIGEST_ALGORITHM`,
`SIGNATURE_C14N_ALGORITHM`) on BOTH signing paths — the producer Dispatch
out-interceptor and the consumer `processOutbound` signed-response path —
whenever the profile's out-actions include `Signature`.
`SIGNATURE_PARTS` SHALL be applied on the producer path only: consumer
endpoint construction SHALL fail when the consumer's profile sets
`SIGNATURE_PARTS` (enforced at Rust `create_consumer`), and the Java
consumer path SHALL refuse a PARTS-configured profile at runtime,
because consumer coverage (Body plus Timestamp) is the fixed
replay-defense invariant. Profile construction SHALL fail with
a diagnostic naming the offending environment variable when: any of
`SIGNATURE_ALGORITHM`, `SIGNATURE_DIGEST_ALGORITHM`,
`SIGNATURE_C14N_ALGORITHM`, or `SIGNATURE_PARTS` is set while out-actions
lack `Signature`; any of those knobs is set without a signing keystore;
`SIGNATURE_PARTS` violates the strict grammar (WSS4J canonical order:
`;`-separated segments, each a bare non-empty localName or
`{modifier}{namespace}localName` with modifier empty or exactly
`Element`/`Content` and non-empty localName); or an algorithm knob is not
an absolute URI. A profile that sets none of these knobs SHALL behave
identically to pre-change builds.

The `ALLOWED_SIGNATURE_ALGORITHMS` and `ALLOWED_DIGEST_ALGORITHMS`
settings (`cxf.security.allowed.signature.algorithms` /
`cxf.security.allowed.digest.algorithms`, sidecar env
`CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS` /
`CXF_PROFILE_<NAME>_ALLOWED_DIGEST_ALGORITHMS`) are INBOUND verification
knobs with replacement semantics: when set, the comma-separated list of
absolute algorithm URIs IS the inbound allow-list for that axis
(signature method URIs, digest method URIs respectively). They are EXEMPT
from the signing preconditions above — a profile MAY set them without a
signing keystore and without a `Signature` out-action — but they are NOT
unconditional: profile construction SHALL fail naming the offending
environment variable when an `ALLOWED_*` value is set and is not a
non-empty, non-blank, comma-separated list of absolute URIs (an empty or
blank-only list MUST NOT degenerate to allow-all), or when `ALLOWED_*` is
set on a profile with NO inbound verification material (neither
truststore nor keystore configured: `canVerifyInbound()` is false, both
inbound paths skip WS-Security processing entirely, and the knob would be
silently inert). When both `ALLOWED_*` knobs are unset, the default
allow-list applies: signature rsa-sha256/rsa-sha384/rsa-sha512, digest
sha256/sha384/sha512 — SHA-1 signature and digest URIs are therefore
REJECTED on inbound verification by default, on JVM and native alike, and
so is everything outside those six URIs (the ecdsa, rsa-pss, hmac and
sha224 families included); this tightening of today's JVM behaviour is a
declared breaking change and MUST be stated in the changelog, README and
ADR, including its effect on consumer profiles with `Encrypt`-only
in-actions that receive signed messages (the consumer path verifies every
signature in the header regardless of the required in-actions). Inbound
verification SHALL reject any message whose signature method
or digest method is not on the profile's effective allow-list for that
axis, on BOTH inbound paths: the consumer `processInbound` path and the
producer response-verification in-interceptor. The allow-list is enforced
by ONE shared allow-list source with per-path installation: a signature
`Processor` wrapper pins the Santuario implementation explicitly (`new
SignatureProcessor(new XMLDSigRI())`, no runtime `Security` provider
registration) and installs the profile's `AlgorithmSuite` — built solely
from the two lists, with every key-length and derived-key-length bound
set to a
non-rejecting value (minimum 0, maximum `Integer.MAX_VALUE`) so the suite
constrains ONLY the signature-method and digest axes and never narrows
decryption, key-wrap or key-length behaviour relative to a suite-less
run, independent of security-header element order — into the `RequestData`
on the consumer engine path; on the CXF producer path WSS4J
mechanically seeds an empty (permissive) suite, so the SAME profile
suite from the SAME single source is installed via the in-interceptor's
`setAlgorithmSuites` hook — so acceptance is
deterministic in JVM and native builds and identical across both paths.
Setting the outbound signing knobs (`SIGNATURE_ALGORITHM`,
`SIGNATURE_DIGEST_ALGORITHM`) to legacy SHA-1 URIs does NOT opt any
profile into inbound SHA-1 acceptance. Startup SHALL log a WARN naming
the profile whenever the effective allow-list contains a SHA-1 signature
or digest URI — detected among `xmldsig#rsa-sha1`, `xmldsig#dsa-sha1`,
`xmldsig#hmac-sha1`, `xmldsig-more#ecdsa-sha1`, `xmldsig#sha1` — stating
that inbound SHA-1 acceptance is enabled for that profile. Startup SHALL
additionally log a WARN, on profiles with inbound verification material
(a truststore or keystore configured), when a signing knob names a SHA-1 URI that the profile's
effective inbound allow-list excludes, directing the operator to the
`ALLOWED_*` knobs. Outbound signing defaults are unchanged by
these knobs.
#### Scenario: algorithm lands on the producer out-interceptor

- **GIVEN** a profile with keystore, out-actions `Signature`, and `SIGNATURE_ALGORITHM` set to the rsa-sha384 URI
- **WHEN** the producer out-interceptor is created
- **THEN** its WSS4J configuration carries the rsa-sha384 URI verbatim under the literal `signatureAlgorithm` property key (the signature bytes this key produces are WSS4J's documented contract — the consumer path, where this repo's code calls `WSSecSignature` directly, is the behavioral twin of this scenario)

#### Scenario: algorithm takes effect on signed consumer responses

- **GIVEN** a consumer profile with keystore, out-actions including `Signature` and `Timestamp`, and `SIGNATURE_DIGEST_ALGORITHM` set to the sha-384 digest URI
- **WHEN** `processOutbound` signs the response
- **THEN** the emitted signature's `DigestMethod` is sha-384 and its coverage still includes Body and Timestamp

#### Scenario: parts land on the producer out-interceptor

- **GIVEN** a producer profile with signing keystore, out-actions `Signature`, and `SIGNATURE_PARTS` naming only a header element
- **WHEN** the producer out-interceptor is created
- **THEN** its WSS4J configuration carries the `SIGNATURE_PARTS` value verbatim under the literal `signatureParts` property key (the reference-narrowing behavior behind that key is WSS4J's documented contract)

#### Scenario: parts-configured profile cannot serve a consumer endpoint

- **GIVEN** a consumer endpoint whose selected profile sets `SIGNATURE_PARTS`
- **WHEN** the Rust `create_consumer` constructs the endpoint
- **THEN** construction fails naming `SIGNATURE_PARTS` and the Body-plus-Timestamp replay invariant, and the Java consumer path would also refuse the profile at runtime

#### Scenario: knob without matching action aborts construction

- **GIVEN** a profile whose out-actions omit `Signature`
- **WHEN** any of `SIGNATURE_ALGORITHM`, `SIGNATURE_DIGEST_ALGORITHM`, `SIGNATURE_C14N_ALGORITHM`, or `SIGNATURE_PARTS` is set at construction
- **THEN** construction fails naming the knob's environment variable

#### Scenario: malformed knob values abort construction

- **GIVEN** a `SIGNATURE_PARTS` segment with an empty localName or a braced modifier other than empty/`Element`/`Content`, an algorithm knob that is not an absolute URI, or any of those knobs set without a signing keystore
- **WHEN** the profile is constructed
- **THEN** construction fails naming the offending environment variable

#### Scenario: unset knobs preserve defaults

- **GIVEN** a profile with out-actions `Signature` and no `SIGNATURE_*` knobs set
- **WHEN** either path signs a message
- **THEN** WSS4J default algorithms and (consumer) Body+Timestamp coverage apply, byte-identical to pre-change builds

#### Scenario: inbound legacy sha1 rejected by default on both inbound paths

- **GIVEN** a profile with a truststore, in-actions `Signature`, and no `ALLOWED_*` settings
- **WHEN** a message signed with rsa-sha1 and sha1 digest is verified on the consumer `processInbound` path, and the same message is verified through the producer response-verification in-interceptor
- **THEN** verification fails on BOTH paths with an algorithm-forbidden error

#### Scenario: inbound legacy sha1 accepted when allow-listed on both inbound paths

- **GIVEN** a profile with a truststore, in-actions `Signature`, and `ALLOWED_SIGNATURE_ALGORITHMS`/`ALLOWED_DIGEST_ALGORITHMS` including the rsa-sha1 and sha1 URIs
- **WHEN** a message signed with rsa-sha1 and sha1 digest is verified on the consumer `processInbound` path, and the same message is verified through the producer response-verification in-interceptor
- **THEN** verification succeeds on BOTH paths

#### Scenario: inbound verification is provider-deterministic

- **GIVEN** a profile allow-listing the rsa-sha1 and sha1 URIs, with the `ApacheXMLDSig` security provider absent from `java.security.Security` (the GraalVM native condition)
- **WHEN** a message signed with rsa-sha1 and sha1 digest is verified on the inbound path
- **THEN** verification succeeds (the pinned processor makes provider availability independent of runtime registration)

#### Scenario: inbound modern algorithms unchanged

- **GIVEN** a profile with a truststore, in-actions `Signature`, and no `ALLOWED_*` settings
- **WHEN** a message signed with rsa-sha256 and sha256 digest is verified on the inbound path
- **THEN** verification succeeds exactly as before this change

#### Scenario: three-reference partner shape verifies

- **GIVEN** an opted-in profile with a truststore and in-actions `Signature`
- **WHEN** a message whose single signature carries three references — BinarySecurityToken, Timestamp and SOAP Body — with exclusive c14n, sha1 digest on ALL three references, and a SecurityTokenReference KeyInfo is verified on the inbound path
- **THEN** verification succeeds (coverage checks are required-contains: extra references neither fail nor narrow the signer extraction)

#### Scenario: non-sha1 algorithm outside the default list rejected then accepted when allow-listed

- **GIVEN** a profile with a truststore, in-actions `Signature`, and no `ALLOWED_*` settings
- **WHEN** a message signed with ecdsa-sha256 and sha256 digest is verified on the inbound path
- **THEN** verification fails with an algorithm-forbidden error; when the profile additionally allow-lists the ecdsa-sha256 URI, the same message verifies

#### Scenario: signed and encrypted inbound is unaffected by the default suite

- **GIVEN** a profile with truststore and keystore, in-actions `Signature Encrypt`, and no `ALLOWED_*` settings
- **WHEN** a message signed with rsa-sha256 and encrypted with an AES-128/rsa-oaep combination is verified and decrypted on the inbound path — in BOTH security-header orders (signature before EncryptedKey and EncryptedKey before signature)
- **THEN** verification and decryption succeed exactly as before this change in BOTH orders (the suite constrains only the signature-method and digest axes; all key-length and derived-key-length bounds are non-rejecting, so no check appears or disappears based on header element order)

#### Scenario: encrypt-only consumer with allow-list accepts signed and encrypted legacy message

- **GIVEN** a profile with truststore and keystore, in-actions `Encrypt` only, and `ALLOWED_SIGNATURE_ALGORITHMS`/`ALLOWED_DIGEST_ALGORITHMS` including the rsa-sha1 and sha1 URIs
- **WHEN** a message signed with rsa-sha1 and sha1 digest and then encrypted is verified and decrypted on the consumer path
- **THEN** verification and decryption succeed (the consumer path verifies every signature in the header regardless of the required in-actions, so Encrypt-only consumers can opt in)

#### Scenario: allowed-list flags exempt from signing preconditions

- **GIVEN** a verify-only profile with a truststore, in-actions `Signature`, no signing keystore, and out-actions lacking `Signature`
- **WHEN** `ALLOWED_SIGNATURE_ALGORITHMS` or `ALLOWED_DIGEST_ALGORITHMS` is set at construction
- **THEN** construction succeeds (the exemption is what lets a verify-only consumer opt in)

#### Scenario: allowed-list without inbound verification material aborts construction

- **GIVEN** a profile with neither truststore nor keystore configured (inbound WS-Security processing is skipped entirely)
- **WHEN** `ALLOWED_SIGNATURE_ALGORITHMS` or `ALLOWED_DIGEST_ALGORITHMS` is set at construction
- **THEN** construction fails naming the offending environment variable

#### Scenario: empty or blank allowed-list aborts construction

- **GIVEN** an `ALLOWED_SIGNATURE_ALGORITHMS` or `ALLOWED_DIGEST_ALGORITHMS` value that is empty, whitespace-only, or only separators
- **WHEN** the profile is constructed
- **THEN** construction fails naming the offending environment variable (an empty effective list would bypass the suite checks entirely and accept everything)

#### Scenario: malformed allowed-list values abort construction

- **GIVEN** an `ALLOWED_SIGNATURE_ALGORITHMS` or `ALLOWED_DIGEST_ALGORITHMS` value containing an entry that is not an absolute URI
- **WHEN** the profile is constructed
- **THEN** construction fails naming the offending environment variable

#### Scenario: legacy signing knob does not opt in to inbound sha1

- **GIVEN** a profile with a signing keystore, out-actions `Signature`, a truststore, in-actions `Signature`, and `SIGNATURE_ALGORITHM`/`SIGNATURE_DIGEST_ALGORITHM` set to the rsa-sha1 and sha1 URIs, with no `ALLOWED_*` settings
- **WHEN** a message signed with rsa-sha1 and sha1 digest is verified on the inbound path
- **THEN** verification fails with an algorithm-forbidden error, and a startup WARN has pointed the operator at the `ALLOWED_*` knobs

#### Scenario: startup warn names profiles allowing sha1

- **GIVEN** a profile whose effective allow-list contains a SHA-1 signature or digest URI via an explicit `ALLOWED_*` list
- **WHEN** the bridge starts
- **THEN** a WARN-level log names the profile and the SHA-1 algorithms it allows

### Requirement: cxf security out-actions fail loud without their crypto material

The bridge-side producer security profile SHALL reject, at build time,
explicitly configured outbound actions whose security effect would
otherwise be a silent no-op: a `Timestamp` action without `Signature`
(strippable decorative security), and any of `Signature`/`Encrypt`/
`Timestamp` without a configured keystore. Profiles that did not
configure outbound actions (falling back to the default action set) are
outside this rule. Checks are ordered deterministically: action
composition first, crypto material second.

#### Scenario: Timestamp without Signature is rejected

- **GIVEN** a security profile builder with out-actions "Timestamp" and a keystore
- **WHEN** build() runs
- **THEN** an IllegalArgumentException is thrown whose message names the Timestamp action and the missing Signature action

#### Scenario: security actions without keystore are rejected

- **GIVEN** a security profile builder with out-actions "Signature" and no keystore configured
- **WHEN** build() runs
- **THEN** an IllegalArgumentException is thrown whose message names the missing keystore

#### Scenario: Encrypt without keystore is rejected

- **GIVEN** a security profile builder with out-actions "Encrypt" and no keystore configured
- **WHEN** build() runs
- **THEN** an IllegalArgumentException is thrown whose message names the missing keystore

#### Scenario: composed actions without keystore follow composition-first precedence

- **GIVEN** a security profile builder with out-actions "Signature Timestamp" and no keystore configured
- **WHEN** build() runs
- **THEN** an IllegalArgumentException is thrown naming the missing keystore (the action composition is valid, so the material check fires)

#### Scenario: explicitly blank actions are unaffected

- **GIVEN** a security profile with no out-actions configured and no keystore
- **WHEN** build() runs
- **THEN** the profile builds successfully (no security interceptors; default action resolution is not subject to the material check)

