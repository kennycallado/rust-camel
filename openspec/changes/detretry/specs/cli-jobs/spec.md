## ADDED Requirements

### Requirement: job send-phase transport-arm retry classification

The `camel job` send loop's OUTER transport arm (producer/endpoint
apparatus failures: component-not-registered, endpoint creation, producer
creation) SHALL classify deterministic failures and return them as
transport failures (`SendError::Transport`) on the FIRST attempt without
sleeping, while genuinely transient failures keep the existing bounded
retry window. This requirement governs transport apparatus failures (the
outer arm); the existing `job send-phase retry classification`
requirement governs pipeline-result failures (the inner arm) and is
unchanged by this delta. The arm SHALL carry failures as a typed
transport-error enum preserving the cause `CamelError` (no String
erasure), whose Display SHALL stay byte-identical to the historical
strings (`failed to send to {uri}: `{scheme}:` component not
registered`, `failed to create endpoint {uri}: {e}`, `failed to create
producer for {uri}: {e}`). A transport failure SHALL be deterministic
when: the scheme's component is not registered (the registry is frozen
after boot), or the cause is the `CamelError::InvalidUri` variant (bad
endpoint URI or parameter — the variant decides, never message text),
or the cause carries the SEDA crate's terminal-config marker (per
`camel_component_seda::is_seda_terminal_config_error`, covering both the
multipleConsumers+wait conflict and the endpoint config conflict).
Everything else — the plain `EndpointCreationFailed` variant (foreign
components' plain creation failures; the SEDA queue-full residual rides
the pipeline arm, not this one) — SHALL remain retryable for the
existing bounded window (component-owned marker doctrine: foreign
components keep the retryable default until they own a marker).
Transport failures keep exit-code 2 (send apparatus failure, the early
exit-2 class: the byte-identical Display renders to stderr and NO JSON
report is written on this path). Keycloak endpoint/role configuration
errors cannot reach this arm: job documents reject non-`direct:`/`seda:`
send targets at load (fail-closed consumer scheme allowlist
requirement).

#### Scenario: SEDA config conflict fails fast on the transport arm

- **GIVEN** a started job context whose route consumes
  `from: seda:<name>?size=10` and whose document sends to
  `seda:<name>?size=5` (the forced `waitForTaskToComplete=Always` rewrite
  keeps the size diff, so `create_endpoint` rejects the incompatible
  config)
- **WHEN** the send loop receives the typed endpoint-creation failure
  carrying the seda terminal-config marker
- **THEN** the classification reports deterministic and the send returns
  `SendError::Transport` on the first attempt without sleeping — the
  in-process send completes in under 1 second, where a retry burn would
  consume the full 3 second window

#### Scenario: invalid endpoint URI fails fast on the transport arm

- **GIVEN** a job send whose target URI carries a parameter that cannot
  parse (a non-numeric `size`, for example `seda:<name>?size=abc` —
  parameters the send rewrites, such as `waitForTaskToComplete`, cannot
  reach this class)
- **WHEN** the send loop receives the `CamelError::InvalidUri` cause
- **THEN** the classification reports deterministic and the send returns
  `SendError::Transport` on the first attempt (a bad URI never becomes
  valid on retry), pinned by a behavioral test exercising exactly that
  URI

#### Scenario: component-not-registered classifies deterministic

- **GIVEN** a transport failure whose scheme has no registered component
- **WHEN** the classification runs
- **THEN** the classification reports deterministic (the registry is
  frozen after boot; the miss can never clear within the window)

#### Scenario: plain cause without a marker stays retryable

- **GIVEN** a transport-arm failure whose cause is a plain
  `EndpointCreationFailed` (synthetic classifier characterization —
  foreign components' creation failures carry no marker; the SEDA
  queue-full residual rides the pipeline arm, governed by the existing
  `job send-phase retry classification` requirement, and is unchanged)
- **WHEN** the classification runs
- **THEN** the classification reports retryable (foreign components keep
  the default until they own a marker) and the loop keeps the existing
  bounded-window semantics

#### Scenario: transport failure Display stays byte-identical

- **GIVEN** any transport failure class
- **WHEN** the failure renders to its stderr/error text
- **THEN** the text equals the historical `format!` wording for its stage
  (component-not-registered, endpoint creation, or producer creation)
  — the typed carrier changes no user-visible text

#### Scenario: config-conflict job exits fast with apparatus failure

- **GIVEN** a job document sending to `seda:<name>?size=5` whose consumer
  route declares `from: seda:<name>?size=10`
- **WHEN** `camel job` runs the document as a subprocess
- **THEN** the process exits with code 2 (send apparatus failure, early
  exit-2 class — no JSON report is written), stderr carries the
  byte-identical conflict detail prefixed by the endpoint-creation stage
  wording, and the total process elapsed time stays below 1.5 seconds —
  half the 3 second send retry window, proving no retry burn
