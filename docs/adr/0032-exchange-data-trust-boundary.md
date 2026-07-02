# ADR-0032: Exchange-Data Trust Boundary

**Date:** 2026-07-02
**Status:** Accepted
**Amends:** none
**Cross-refs:** ADR-0010 (route SecurityPolicy pre-pipeline authz)

## Decision

Operator configuration is **trusted**. Exchange data — message headers, body, properties, and
correlation keys set by the data plane at runtime — is **untrusted, adversary-controlled**. No
untrusted exchange datum may drive a control-plane action, an unbounded numeric or resource
decision, or an executable/interpretable sink without validation, bounding, or a capability check.

This is one architectural principle violated 8 times in the pre-1.0 audit (H12 delayer,
R3-C1 aggregator, H13 recipient-list, H7 SQL header, D-M8 throttler, H1 auth principal,
R3-H1 CSV marshal, R4-H1 ControlBus). One rule; uniformly testable; uniformly enforced.

## Context

Apache Camel's legacy in-process model trusts everything once inside the JVM. The
pre-1.0 audit (`docs/audit/SECURITY-AUDIT-v1-pre-stabilization.md`) showed that this
trust-everything model turns every EIP that reads an exchange-derived value into a
DoS, injection, or authz-bypass surface. ADR-0010 covers one slice — the principal
that drives route authz — but no ADR generalizes "untrusted exchange data must not
cross into control, numeric, or interpretable decisions."

## Considered Options

### Trust everything in-process (status quo)

Rejected. Every EIP becomes a DoS / injection surface; reproducing a fix in one EIP
does not generalize. The audit surfaced 8 instances of the same bug-class.

### Validate ad-hoc per component

Rejected. This is the current state. It produced 8 identical bugs in 5 different
crates. Reviewers cannot recognize the pattern because the codebase has no shared
vocabulary for it.

### Document a cross-cutting boundary with per-site enforcement (chosen)

One principle, one review checklist, one regression-test shape (untrusted exchange
datum → bounded / neutralized / denied / capability-required). Future components
inherit the checklist; existing fixes re-state the same boundary case consistently.

## Consequences

- Every EIP that touches exchange-derived numerics or sinks gains a bound, a
  validation, or a capability check. Six boundary cases are fixed in Batch 1
  (R3-C1, H12, H13, R4-H1, H1, R3-H1). Two more (H7 SQL, D-M8 throttler) are fixed
  by the same principle but live in Disposition-3 (Require-Explicit-Choice) and
  Disposition-4 (Safety-Primitive) respectively.
- New components MUST answer: "does any exchange-derived value cross into a
  control / numeric / executable sink?" If yes, the boundary is enforced at the
  crossing.
- Diverges permanently from Camel's in-process trust model. Operators cannot opt
  out of the boundary per-route; they can only narrow it (more specific bindings).
- The CONTEXT-MAP glossary adds the term "Exchange-data trust boundary" so the
  rule is searchable in code review.
