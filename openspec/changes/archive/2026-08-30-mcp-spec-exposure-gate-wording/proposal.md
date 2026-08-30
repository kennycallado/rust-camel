# Proposal: mcp-spec-exposure-gate-wording

## Why

The canonical spec `openspec/specs/mcp-component/spec.md` still says the
MCP server endpoint SHALL refuse to bind unless a `security_policy` is
configured, and carries a "bind refused without security policy"
scenario. That component-local gate was removed (consumer.rs Task 2.9
comment; test `policy_less_config_no_longer_refused_at_validate`).
ADR-0061 Rule 9 superseded ADR-0060 Rule 8: the kernel per-bind
exposure gate owns the public-exposure decision. The spec text is
drift against code, tests, and two ADRs (bd rc-63aj, flagged twice
during fix-mcp-dead-registry-entry reviews).

## What Changes

- One MODIFIED requirement ("MCP server fail-closed authentication"):
  the first paragraph states the ADR-0061 exposure-gate semantics
  (fail-closed on empty `allow_public_exposure`; loopback binds start
  without a policy; non-loopback needs a policy or an explicit ack).
- The stale "bind refused without security policy" scenario is
  replaced by two scenarios matching the tested behavior: loopback
  policy-less allowed, non-loopback policy-less without ack refused
  by the kernel gate.
- No code, no tests, no behavior change. Spec-only correction.

Excluded: any wording beyond this requirement; the ADR-0068 ownership
paragraphs and route-level enforcement text stay as archived.

## Acceptance criteria

- `openspec validate mcp-spec-exposure-gate-wording --type change`
  passes.
- After archive, the canonical spec contains no "refuse to bind
  unless" clause and the old scenario name is gone.

## Risk budget

- Spec text only. The main risk is wording that over- or
  under-states the kernel gate; the delta cites the exact tests that
  pin the behavior.

Bd: rc-63aj
