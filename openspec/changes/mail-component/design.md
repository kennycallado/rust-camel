# Design: mail-component

## Approach

Native SMTP/IMAP component in a new crate `camel-component-mail`. One
`MailBundle` registers four schemes (`smtp`, `smtps`, `imap`, `imaps`),
the way `camel-http` registers `http` and `https`
(`camel-http/src/bundle.rs`). Third-party types stay private behind two
seams (ADR-0020, ADR-0060): `MailTransport` (send, adapter over
`lettre`) and `MailStore` (IMAP session, adapter over `async-imap`).
Adapters live under `src/adapter/`; unit tests use fakes of the seams.

Crates: `lettre` 0.11 (`default-features = false`; features
`smtp-transport`, `pool`, `builder`, `hostname`, `tokio1-rustls`, `ring`,
`webpki-roots`), `async-imap` 0.12 (`runtime-tokio`), `mail-parser` 0.11,
plus direct `tokio-rustls`, `rustls` (ring only) and `webpki-roots` with
`default-features = false` for IMAP TLS and `caFile` PEM loading.
TLS is rustls with the ring provider only; the component adds no aws-lc
edge. `async-imap` logs through `log`, not `tracing`, so the component
emits its own events. Its `thiserror` 1 duplicate is accepted
(`Cargo.lock` already carries it; no `deny.toml` change).

**URI and accounts.** URI shape: `scheme://host[:port]?options`. Host and
port are non-secret and live in the URI (defaults 587 smtp, 465 smtps,
143 imap, 993 imaps). Credentials and trust anchors live in named
accounts, `[components.mail.accounts.<name>]` with `username`, `password`
(`${env:}` placeholders allowed) and optional `caFile` (PEM, added to the
webpki roots; there is no insecure-verify flag). Account selection copies
rabbitmq brokers: explicit `?account=` must exist; omitted with exactly
one account selects it; omitted with several is an error naming
`?account=`; omitted with none means no authentication for `smtp`/`smtps`,
while `imap`/`imaps` require an account. A URI with userinfo
(`user:pw@host`) fails endpoint creation. Passwords use
`SecretString` with redacting `Debug` (ADR-0051); no server URL reaches an
error or a log line, so nothing needs `redact_url` (ADR-0076). URI options derive from
`#[derive(UriConfig)]` (ADR-0041), pattern `camel-rabbitmq/src/metadata.rs`.
Every URI option appears in metadata only in the phase whose task
implements it (no parsed-but-unused options).

**TLS posture** (ADR-0081 naming): STARTTLS is required by default for
`smtp`/`imap`. `smtps`/`imaps` use implicit TLS. Cleartext needs an
explicit `allowCleartext=true`.

**Header policy** (ADR-0093, three classes; exact CamelCase names because
the HTTP consumer copies request headers Title-Case, so a bare `To`
would be settable by any HTTP client):
- Class A, content (`CamelMailSubject`, `CamelMailContentType`, `X-*`;
  `CamelMailHtmlBody` is body content and rejects NUL only): allowed; the component rejects CR, LF
  and NUL in header values, and values over 998 bytes, in its own code.
  Known caveat, owner-fixed decision: `X-*` passes through, so an
  http-to-mail route also emits request headers such as `X-Api-Key`.
  Mitigation is `removeHeaders` in the route; ADR-0093 records it. Flagged
  for owner review (bd bd-human), not re-litigated here.
- Class B, routing (`CamelMailTo`, `CamelMailCc`, `CamelMailBcc`):
  endpoint-only unless the endpoint sets `allowedRecipients` (exact
  addresses or `@domain`, domains case-insensitive). A routing header on
  an unarmed endpoint is an ERROR. An armed fence rejects any address
  outside the list. `maxRecipients` (default 50) caps the total
  (ADR-0032). Header recipients REPLACE the endpoint's `to`/`cc`/`bcc` of
  the same kind (Camel parity); addresses configured on the endpoint are
  endpoint-owned and not fenced. The send needs `from` (endpoint option,
  required at creation) and at least one recipient, else the exchange
  fails. `CamelMailSubject` overrides the `subject` option. Bcc goes to
  the envelope only. `From`, `Sender`, `Reply-To` are endpoint-only
  (anti-spoofing).
- Class C, mutation (`delete`, `moveTo`, `copyTo`, folder):
  endpoint-only, no opt-in. Folder names are validated at endpoint
  creation. The consumer emits `CamelMailIn*` headers, so mail-to-mail
  cannot re-target.

**Body mapping.** The exchange body is the primary part: `String`, or
`bytes` that decode as UTF-8 (else the exchange fails); other types fail.
Charset is UTF-8. `contentType` (option) or `CamelMailContentType`
(header) selects `text/plain` (default) or `text/html`. When the body is
`text/plain` and header `CamelMailHtmlBody` is set, the message is
`multipart/alternative`. Attachments are a non-goal.

**Send semantics.** Never resend a non-idempotent send after an ambiguous
failure past DATA (same rule as the ADR-0023 Contract C1 and ADR-0063
Decision 7: a non-idempotent write is never re-issued). The producer has
no internal retry; a route error handler may retry the whole exchange.
Consumer side-effect failures (disposition) use
`b-prime:mail:dispose` (ADR-0012 category b-prime, ADR-0066).

**Consumer** (Phase 2/3). Poll loop with `delay`/`initialDelay`, search
`UNSEEN`, fetch `RFC822.SIZE` first and skip messages above
`maxMessageSize` before any body fetch, then `BODY.PEEK[]`,
`maxMessagesPerPoll` (applied after skips, so skipped messages cannot
starve new mail), bounded redelivery (`maxAttempts`, default 3, in-memory
per UIDVALIDITY+UID; an exhausted or oversize message is skipped for the
process lifetime, logged and counted, and stays untouched on the server).
Delivery is at-least-once: a crash between route success and `\Seen`
redelivers. Processing is `Sequential`
(ADR-0007: the consumer never restarts itself; reconnects use
`NetworkRetryPolicy`, ADR-0013). It selects on `ctx.cancelled()`. The
message is disposed only after the route completes (rabbitmq pattern):
success sets `\Seen` (always, also when only `copyTo` is set, so a
copied message is not copied again); failure leaves the message untouched.
A `contains` error from the idempotent repository does not count toward
`maxAttempts`. `contentType=text/html` with `CamelMailHtmlBody` and
`allowCleartext` on `smtps`/`imaps` are endpoint-creation or send errors
(no silent ignore). UIDVALIDITY change discards the attempt counters and
the skip set. The `send` counter ships with the producer in Phase 1; the
`poll` counter and the health check ship in Phase 2.

## Affected crates

- `crates/components/camel-mail` (new, package `camel-component-mail`; dir-vs-package split follows `camel-rabbitmq`/`camel-component-rabbitmq`).
- `crates/camel-bundles`: optional dep, `mail` feature in `default`,
  `register_bundle`, boot tests (scheme present/absent).
- `crates/camel-cli`: optional dep, `mail` feature, `flavor-regular` and
  legacy `full` lists, `register_bundle_empty!`, `CONTEXT.md` profiles
  list, tables `SLIM_FORBIDDEN_PREFIXES`/`REGULAR_REQUIRED_PREFIXES`,
  regenerated `tests/fixtures/default-deptree.txt`.
- Workspace `Cargo.toml`: dependency entries with feature comments.
- `scripts/xtask/trustpub-registrations.toml`: `new-unpublished` entry.
- Docs: `docs/src/components/mail.md` + `SUMMARY.md`, `CONTEXT-MAP.md`,
  crate `CONTEXT.md`, `docs/adr/0089-*.md`.
- No core or api crate changes.

## Architecture boundaries

Components layer only. The crate depends on `camel-component-api`,
`camel-api` and tokio; it registers through `ComponentBundle`. Producers
only send, consumers only receive (`components/CONTEXT.md`). Data plane =
exchanges through the standard Producer/Consumer seams; control plane =
`Camel.toml` resolution. `hexagonal_architecture_boundaries_test` guards
the boundaries.

## Phases

### Phase 1: Foundation and SMTP producer
- **Goal:** a registered, redaction-safe SMTP producer with the full
  header fence.
- **Dependencies:** `lettre` ring-TLS feature check; rabbitmq commit
  `513ee23f` as registration template; ADR-0051, ADR-0081.
- **Externally-visible types/interfaces:** package
  `camel-component-mail`, `MailBundle`, schemes `smtp`/`smtps`, headers
  `CamelMailSubject`, `CamelMailContentType`, `CamelMailHtmlBody`,
  `CamelMailTo/Cc/Bcc`, `CamelMailMessageId`, URI
  options `account`, `from`, `to`, `cc`, `bcc`, `subject`, `contentType`,
  `allowedRecipients`, `maxRecipients`, `allowCleartext`; `[components.mail.accounts]`.
- **Deliverable:** crate, bundle/CLI/flavor/publish registration with
  regenerated golden, fake SMTP server tier, Docker tier (Mailpit), ADR-0093.
- **Exit-criteria:** unit and fake-server tests green; `MAIL_ITEST=1`
  Mailpit round trip (incl. STARTTLS) green or `infra-unavailable`;
  `flavor_marker_table`, `default_closure_matches_golden`,
  `full_covers_universe`, `lint-gate-forwarding`,
  `lint-publish-registration`, `schema --check` green.

### Phase 2: IMAP receive
- **Goal:** IMAP consumer with at-least-once defaults and
  dispose-after-route.
- **Dependencies:** Phase 1 config, redaction and bundle.
- **Externally-visible types/interfaces:** schemes `imap`/`imaps`,
  `MailStore`, options `folder`, `delay`, `initialDelay`,
  `maxMessagesPerPoll`, `maxMessageSize`, `maxAttempts`. Headers
  `CamelMailInFrom`, `InTo`, `InCc`, `InSubject`, `InMessageId`, `InUid`,
  `InDate`, `InFolder`; body = the `text/plain` part (HTML source if there
  is none), HTML alternative in `CamelMailInHtmlBody`.
- **Deliverable:** consumer, health, metrics, GreenMail tier, consumer docs.
- **Exit-criteria:** disposition state-machine tests vs fakes green;
  failed route leaves the message `UNSEEN`; GreenMail tier green or
  `infra-unavailable`; inter-phase r_glm review clean.

### Phase 3: Disposition, dedup, pull
- **Goal:** mailbox mutation, duplicate suppression, pull consumption.
- **Dependencies:** Phase 2 consumer.
- **Externally-visible types/interfaces:** options `delete`, `moveTo`,
  `copyTo`, `idempotentRepository`; `PollingConsumer` for `pollEnrich`.
- **Deliverable:** UIDPLUS `UID EXPUNGE` (without UIDPLUS `delete` fails
  closed: a plain EXPUNGE would remove other clients' `\Deleted` mail),
  MOVE with COPY+delete fallback, `delete` and `moveTo` mutually exclusive
  (`copyTo` combines with either), UIDVALIDITY tracking, Message-ID
  idempotency (ADR-0023: `contains` skips; `add` after successful
  disposition; a skipped duplicate is marked `\Seen`; a message without
  Message-ID is processed and never deduplicated), `PollingConsumer`
  (no route-completion signal in a pull, so disposition applies right
  after fetch: at-most-once, documented), full mdBook page, CONTEXT-MAP
  entry, final ADR pass.
- **Exit-criteria:** each disposition scenario green against fakes and
  GreenMail; `docs` `mdbook test` green.

## Plan-stage clarifications

- IMAP defaults: `delay` 60000 ms, `initialDelay` 1000 ms,
  `maxMessagesPerPoll` 100, `maxMessageSize` 10 MiB. Extra consumer
  metric labels `b-prime:mail:skip` and `b-prime:mail:idempotent`.
- `idempotentRepository` names a repository. Components cannot look up
  named repositories through `ComponentContext` and this change forbids
  core/api edits, so the camel-bundles boot code hands the resolved
  repositories (`memory` plus configured names) to the component.
- `moveTo` on a server with neither MOVE nor UIDPLUS fails closed.
- Body types: `String` and UTF-8 `bytes` only; `Xml` and `Empty` fail.
- Every TLS config is built with an explicit ring provider (aws-lc-rs
  stays reachable via `camel-component-api`, bd rc-2bofp), and the crate
  declares its own `rustls` with `default-features = false`.
- `lint-publish-registration` reports exactly one finding (the
  `new-unpublished` entry) until the owner's first manual publish.

## Implementation deviations

- lettre 0.11 cannot take an explicit rustls provider. SMTP TLS uses the
  process default provider (installed as ring by `camel-cli` at startup)
  and falls back to ring. The IMAP side builds its own ring-only config.
  Other components that install aws-lc as default (camel-grpc) can make
  the process default differ; the process-level fix belongs to bd
  rc-2bofp. Recorded in ADR-0093. Re-verified at close against lettre
  0.11.23: `build_rustls` reads `CryptoProvider::get_default()` and has no
  provider or `ClientConfig` injection point.
- Closing review (e_opus):
  - The crate reaches rustls through `tokio-rustls` (workspace entry, ring
    and tls12 only) instead of declaring its own `rustls`; `webpki-roots`
    became a workspace entry. No aws-lc-rs edge either way.
  - IMAP errors split into connection-level (`Store`, `Connect`;
    retryable, the session is dropped, also after a timeout or a failed
    disposition step) and server refusals (`Refused`, tagged `NO`/`BAD`;
    the session stays). A refused poll keeps the session and retries at
    the next `delay` instead of reconnecting under backoff.
  - `maxAttempts` also counts a failed disposition step that leaves the
    message unseen, which bounds redelivery after a successful route
    (a permanent `copyTo` refusal re-ran the route on every poll). The
    "disposition failure counted" and "poison message bounded" scenarios
    hold unchanged.
  - A message expunged between SEARCH and FETCH is ignored for that poll
    instead of counting an attempt and forcing a reconnect.
  - The attempt and skip memory is pruned to the current unseen set each
    poll, so it is bounded by the folder size (UIDVALIDITY still discards
    it).
  - Push and pull share one receive pipeline (`src/poll.rs`); the pull
    therefore also bounds body-download and disposition failures by
    `maxAttempts`. Documented divergences: one message per pull, a
    repository read error ends the pull, and the pull logs without metrics.
  - A `Body::Json(Value::String)` body (what `set_body` and a DSL
    `constant` produce) is sent as text; other JSON values still fail, so
    the "unsupported body type" scenario holds.
  - Public surface narrowed to `MailBundle`, `MailComponent`,
    `MailComponentConfig`, `MailAccountConfig`, `SecretString`,
    `MailScheme`, `MailError` and the header constants; the crate joins
    the `missing_docs` ratchet at 0.
  - `examples/mail-example` added (GreenMail fixture), following the
    rabbitmq precedent.
- Production review (e_opus, owner decisions D1-D5):
  - D1: a tagged `NO`/`BAD` on `LOGIN` maps to `MailError::LoginRefused`,
    which is not retryable: the push consumer stops after one login, the
    health state records a sticky stop, and a pull returns the error
    without caching a session. The message is constant.
  - D2: the idempotent `add` moved from "after the success disposition"
    (Phase 3 plan text) to "as soon as the message is delivered" (push:
    route success; pull: hand-over), before the disposition and whatever
    its outcome. A failed `\Seen` or copy no longer lets the repository
    pass a redelivery. The spec requirement and its scenarios changed
    accordingly.
  - D3: the pull deadline bounds connect, scan and load only; a loaded
    message is always disposed (bounded by the adapter I/O timeout) and
    returned.
  - D4: `camel-config` caps the `async_imap` log target at `info`, next to
    the `camel_tracer=off` cap; an explicit `async_imap` directive wins.
  - D5: implicit single-account selection stays; ADR-0093 records it as an
    accepted risk (credential forwarding through dynamic URIs).
  - Idle drop: a proven push session failing on the first command of a
    poll reconnects at once without backoff; a pull retries a broken
    cached session once on a fresh connection within the deadline.
  - Health check gating. `register_current_route_health_check` exists
    only on `ComponentContext`, which only `create_endpoint` receives;
    `create_consumer` and `polling_consumer` get none (rabbitmq, kafka and
    jms register at endpoint creation too). Registering per consumer would
    need an api change, so the check stays registered at endpoint creation
    and reports healthy without probing until `create_consumer` attaches a
    push consumer (the http `ListenerGate` precedent, rc-hcica). A
    `pollEnrich`-only endpoint therefore never logs in periodically. A
    permanent stop is sticky: the stale probe cannot clear it; only a
    consumer that connects again does.
  - Server-derived error text is cut to 200 characters without control
    characters; async-imap parse errors (which embed the raw response
    buffer) are reported by a constant.
  - `PollReport` counters and `Cycle::skipped` were removed (tests assert
    on the fakes and metrics); `Cycle::load` returns a typed `Loaded`
    outcome instead of using `MailError::Idempotent` as a control signal.
    The pull keeps its `std::sync::Mutex` (the async-imap session is not
    known to be `Sync`).
  - `bytes`, `serde_json` and `tokio-util` became dev-dependencies.
  - `camel-config` exposes `IdempotentRepoConfig::registry_name`, shared
    by its registration code and the `camel-bundles` mail boot block.

## Non-goals (future bd under rc-ca8z, filed only after archive)

XOAUTH2 via `TokenProvider` (Gmail also needs a refresh-token/JWT flow),
IMAP IDLE, attachment model (needs its own ADR; `camel-api` has no
multipart type), POP3, Gmail/Graph REST APIs, DKIM, inbound SMTP server,
SearchTerm parity.

## Alternatives considered

- Two phases (SMTP, IMAP): rejected; disposition and dedup form a
  separate risk surface and deserve their own exit gate.
- Bare `To`/`From` headers (Camel parity): rejected; HTTP-to-mail
  header injection (ADR-0032, ADR-0034, ADR-0071).
- Opt-in for class C mutation headers: rejected, as for ADR-0034.
- `imap-proto` directly or a JVM bridge: rejected; native tokio and
  `async-imap` fit the lifecycle model.
- `allowCleartext` covers every cleartext link, stricter than ADR-0081
  (public targets only); loopback fakes in tests set the flag. ADR-0093
  records the difference.
- ADR number: bd text said 0085, but 0085 is taken (CXF). This change
  uses 0089.
