# Proposal: sedaretry

## Why

bd rc-rif19: a `camel job` send to `seda:foo?multipleConsumers=true`
force-injects `waitForTaskToComplete=Always` (verdict-fidelity rule,
cli-jobs spec), and the SEDA producer deterministically rejects the
combination: `multipleConsumers=true with waitForTaskToComplete != Never
is not supported`. The reject is a plain `CamelError::EndpointCreationFailed`,
which `is_direct_startup_race` blanket-classifies RETRYABLE — so the job
send loop sleeps 20 ms and replays the send for the full 3 s
`SEND_RETRY_WINDOW` on an error that can never clear. A configuration
error is terminal: retrying it wastes the window and re-executes route
side effects, the same defect family as the no-active-consumers gate
(rc-ucemm) and the connect-timeout classification (rc-rif19's parent,
mission 211 discipline: typed provenance markers, never Display text).

## What Changes

- `camel-component-seda`: the multipleConsumers+wait reject at the
  producer call site becomes a typed terminal-config rejection — an
  `EndpointCreationFailedWithSource` whose source chain carries a new
  crate-private marker (same doctrine as `NoActiveConsumersGate`,
  rc-3px7o). Outer detail text stays byte-identical. New pub predicate
  `is_seda_terminal_config_error` (bounded 8-hop source walk).
  `is_direct_startup_race` excludes the marker alongside the gate.
- `camel-cli`: no behavior change (classification delegates to the seda
  crate); doc comments updated; characterization tests extended.
- Regression: behavioral test proves the config error exits the send
  loop on the FIRST failure (no sleep, no replay); subprocess e2e pins
  exit code 1, outcome `Failed`, and the pinned error text, completing
  far below the 3 s window.
- Inventory of sibling misclassifications: queue-full/enqueue-timeout
  stay retryable (documented residual — they can clear); the send
  loop's outer transport arm still burns the window on deterministic
  apparatus failures (different seam — follow-up bd), including
  Keycloak's endpoint/role configuration errors, which ride
  `EndpointCreationFailed` but surface through that outer arm
  (String-erased), never through the inner classifier; Keycloak
  runtime admin-auth failures use `ProcessorError` and are already
  non-retryable.

Excluded: changing `seda_send_uri`'s Always injection (would break the
pinned synchronous-verdict requirement) and the outer-arm retry policy.

## Acceptance criteria

- Genuine multipleConsumers+wait reject reports
  `is_seda_terminal_config_error` true and `is_direct_startup_race`
  false; job send loop returns `SendError::Pipeline` on first attempt.
- Subprocess `camel job` run against the invalid combination exits 1
  with outcome `Failed` and the pinned config error text, well under
  the 3 s window (no retry burn).
- Foreign imitations (plain variant with byte-exact wording, typed
  source without the marker, marker deeper than 8 hops) stay retryable.
- Gate classification, direct-race retryability, and queue-full
  residual behavior unchanged (existing tests pass).

## Risk budget

Low: one reject site changes variant (Display unchanged); classifier
narrows by one typed exclusion. Acceptable risk: none beyond the seda
crate's classification seam. Out of bounds: any change to retry timing,
the gate discipline, or `seda_send_uri` semantics.
