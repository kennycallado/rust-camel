# Changelog

## Unreleased

### Added

- feat(components/jms): durable topic subscribers end to end — `clientId`/`subscriptionName` URI parameters (snake_case aliases, pair-required, topic-only, `concurrentConsumers=1`), `client_id`/`subscription_name` on `SubscribeRequest`, and the fail-closed `durable-subscribers-v1` startup/attach capability gate; the bridge pins Artemis `verifyHost=true` explicitly for secure schemes (2.36 connector default, now explicit), and the JMS READMEs document the durable contract plus a Classic `ssl://` example (bd rc-c4fbo).
- feat(testing): scenario documents accept a document-level `finally:` teardown list (sleep, sql, surreal, and a delete-only redis `del` prepare action) that runs after the verdict on both verdict paths; teardown failures report as the new `teardown-failure` apparatus class without masking the verdict (bd rc-6wodc).

### Changed

- change(components/http,rest-dsl): non-request-borne type-conversion failures now map to HTTP 500 (conversion_failed) instead of 400 (bad_request), and the REST binding-injected unmarshal/schema check reclassifies its failures to ValidationError (400, validation_error) per ADR-0096 fault blame; mid-route user unmarshal and authored marshal keep their conversion error identity but now report 500; metric label at the binding seam moves type_conversion → validation; no opt-in window (ADR-0090) (bd rc-zwd3l).

### Fixes

- fix(cli): sealed artifacts boot without a writable system temp directory for the xslt, xsd, and sql asset classes (in-memory resolution), and the TLS classes fall back to a writable artifact-adjacent directory with a typed error when none exists (bd rc-l39dj).
- fix(components/http): a route that sets `Content-Type` now always emits it verbatim on the reply, including when it equals the request value — inheritance is tracked by the `CamelHttpContentTypeInherited` provenance marker that route header writes clear, replacing value equality; replies to requests with a `Content-Type` whose route only rewrites the body no longer emit a request-dependent inferred type, and archive/stream fragment replies and `useOriginalMessage` restorations no longer depend on the request value either (bd rc-tf0k4).
- fix(languages/simple): interpolating a `Bytes` body as a direct `${body}` slot into a template string with literal text now renders valid UTF-8 as text instead of a byte list, and fails with a `conversion` error on invalid UTF-8; a bare `${body}` keeps the byte array (bd rc-0v855).
- fix(languages/rhai): a JSON integer above i64::MAX in a header, property, or body no longer fails every later rhai script step — it binds as a json number carrying the exact decimal token, and fails only later, at the operation, if used in a refused comparison or converted outbound as a non-image token (bd rc-141om).
- fix(components/mail): with one account and no `?account=`, an endpoint no longer logs in to an unlisted host — it needs the URI host in the account's `allowedHosts` (case-insensitive), fails at endpoint creation when the list is empty, the host does not match or the URI has no host, and never carries the host in the error; migrate with `?account=<name>` or `allowedHosts` (bd rc-bcstf).

### Performance

- perf(languages/rhai): write-back change detection compares values natively instead of compiling a script expression per scalar leaf, and each evaluation holds one snapshot and one working copy per container — multi-thousand-entry properties no longer cost hundreds of thousands of script compilations per mutating eval (bd rc-m01r9).
