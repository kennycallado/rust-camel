# ADR-0033: Security Defaults & Fail-Closed Startup Validation

**Date:** 2026-07-02
**Status:** Accepted
**Amends:** none
**Cross-refs:** ADR-0017 (DSL snake_case naming — ADR-0033 owns the `deny_unknown_fields`
fail-closed policy), ADR-0032 (Exchange-data trust boundary — this ADR is the *enforcement*
arm), ADR-0010 (SecurityPolicy pre-pipeline authz)

## Decision

A single startup-validation phase enforces the 5-disposition security-defaults policy
(Intent-Violation, Intent-Declaration, Require-Explicit-Choice, Safety-Primitive,
Untrusted-Data-Validation) before any route starts. In v1.0.0 only the safety-critical
Require-Explicit-Choice members refuse to start when unset (SQL dynamic query, WASM
per-world capability). Intent-Violation fixes fail closed (gRPC TLS). Safety-Primitive
bounds are asserted at construction (throttler `max_requests != 0`, aggregator has
≥1 completion bound, loop Count clamped). `deny_unknown_fields` rejects typo'd config.

The phase is designed so deferred Require-Explicit-Choice flips (broker/transport
posture) slot in as additional checks in 1.0.x without re-architecting.

## Context

Insecure defaults are documented ad-hoc per component. There is no unified policy
for "what does the operator have to explicitly opt into?" — every Component
CONTEXT.md files its own warning. Reviewers cannot tell which defaults are load-bearing
for a v1.0.0 security posture. The pre-1.0 audit surfaced 28 blocker / near-blocker
findings, of which 8 land in Batch 1 as DoS caps + CRITICALs.

Without a single enforcement arm, a future component could ship an insecure default
and the policy would silently miss it. With the startup-validation phase, every
default that requires explicit operator choice is one trait impl away from being
enforced.

## Considered Options

### Warn-and-continue everywhere

Rejected. Leaves the insecure default live; operators miss the warning; no single
audit point. The audit shows that ad-hoc warnings (`tracing::warn!` at config parse)
are insufficient: gRPC has warned since v0.x that `tls=true` is ignored, and the
audit still found plaintext credentials traveling over h2c.

### Flip every default silently

Rejected. Breaks existing deployments with no migration path and no operator
visibility. Kafka / MQTT / Redis plaintext-by-default configs would refuse to start
overnight with no doctor command to flag the upcoming breakage.

### Fail-closed + `camel doctor` preflight + per-item escape hatch (chosen)

A single preflight scan (`camel doctor` / `xtask preflight`) enumerates every
opt-in now required and every default that changed. Each hardened default has its
own per-item flag — no global "disable hardening" switch. The startup phase
enforces; the doctor command warns; the operator chooses per item. This makes the
policy legible and the migration mechanical.

## Consequences

- Some existing deployments will refuse to start until config is made explicit
  (e.g. gRPC plaintext requires `tls=false` / `transport=plaintext`; SQL dynamic
  query requires `allow_dynamic_query=true`).
- Migration is mechanical: run `camel doctor` once; flip the flags it lists; restart.
- Defaults are sticky post-1.0. Once an operator declares an intent, that intent
  is enforced in subsequent runs; the policy does not silently re-flip.
- The startup-validation phase is implemented as a single trait `ConfigCheck` and
  a `run_startup_validation()` entry point. Each Require-Explicit-Choice member
  becomes one `ConfigCheck` impl. New members do not require a new architecture.
- `deny_unknown_fields` is the highest-friction flip (39 DSL structs). The `doctor`
  command diffs the operator's config against the known schema and lists every
  currently-ignored key that will become a hard error, because typo'd keys fail
  silently today.
- **Batch 1 implementation:** the phase is delivered as a *skeleton* (types +
  `pub fn run_startup_validation() -> Result<(), CamelError>` returning `Ok(())`
  until later batches register checks). The skeleton compiles, the trait is
  public, and the `ConfigCheck` impls that Batch 1 *would* register (gRPC TLS,
  aggregator completion bound) are in their respective components. Wiring
  `CamelContext::start()` to call the phase is Batch 5+ work; the skeleton is
  the first step.
