# Design: mcp-spec-exposure-gate-wording

## Approach

Spec-only correction of one canonical requirement. The current text
mandates a component-local bind refusal that no longer exists. The
replacement states the ADR-0061 Rule 9 behavior, which the tests pin:

- `validate_server_policy` classifies binds only: a loopback
  policy-less config returns `Ok(None)`; a non-loopback policy-less
  config returns `Ok(Some(BindPolicyWarning::NonLoopback))`
  (server_config_test.rs `policy_less_config_no_longer_refused_at_validate`).
- The kernel per-bind exposure gate runs at consumer start over the
  plans snapshot. Fail-closed default: the `allow_public_exposure`
  ack map starts empty, so a non-loopback route without a policy and
  without an ack is refused
  (server_config_test.rs `mcp_old_bind_gate_removed`;
  `enforce_bind_exposure_gate` call at consumer.rs start step b2).

The corrected paragraph keeps the trust-boundary rationale (ADR-0032)
and scopes the `warn!` to acknowledged public non-loopback exposure
(ADR-0061 Rule 4, `bind_gate.rs:58-72`: secure non-Public routes do
not warn). Route-level enforcement and the ADR-0068 ownership
paragraphs are unchanged.

## Affected crates

- None. `openspec/specs/mcp-component/spec.md` only.

## Architecture boundaries

No code planes touched. The change aligns the spec canon with
ADR-0061 (Unified Transport Authentication Kernel), which amended
ADR-0060 Rule 3 and superseded Rule 8.

## Alternatives considered

- Direct edit of the canonical spec: rejected. Canonical specs
  change through OpenSpec deltas so archive history stays coherent.
- Waiting for the next mcp feature change to fold the fix in:
  rejected. Drift between a security requirement and shipped
  behavior is the kind of debt reviewers already flagged twice.
