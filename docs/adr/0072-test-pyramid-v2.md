# ADR-0072: Test Pyramid v2 (Shared Matcher Algebra)

## Status

Proposed (2026-09-06). Supersedes ADR-0069 in part: vocabulary ownership
only. ADR-0069's grammar rules, tier derivation, and verdict taxonomy stand
unchanged; this ADR does not amend them. Epic rc-8zau7; e_opus consultation
ses_f880eca32ffeCbIky4WBYVB71w. The pure-crate carve this ADR ratifies landed
in the same change (tasks 1.1-1.2 of shared-matcher-core).

## Context

The two test tiers speak different assertion vocabularies with the same
intent (epic rc-8zau7). The scenario tier (ADR-0069) owns a full matcher
grammar after waves A-D: count bounds, path filters, query-subset matching,
value expectations. The unit tier's `expects` is endpoint-to-count only
(ADR-0064). Capability gravity inverted the test pyramid: authors chose the
scenario tier because the unit tier was mute, not because they wanted real
wire.

That grammar is welded to the scenario harness. Wave D landed the algebra
inside `camel-integration-test` (`document.rs`, `runner/partner_validate.rs`,
`runner.rs`), surrounded by harness concerns: `HttpWireRequest` observation,
`PartnerRouter`, redaction-coupled diagnostics (ADR-0051). Left there, the
algebra cannot serve the unit tier without dragging those concerns into it.
Extracting it while wave D is fresh prevents the weld from hardening.

Two facts frame the placement choice:

- The B2 precedent (rc-6bsf): e_opus ruled that "a new shared crate is
  over-engineering" and named camel-config as the natural home. That ruling
  addressed boot sharing, where camel-config already held the dependency
  edges.
- ADR-0055 publish topology: a crate with zero `camel-*` dependencies is
  topologically free. It publishes first, before every consumer. No cycle
  risk.

The testing story is also spread across four ADRs — ADR-0064 (contract),
ADR-0069 (crate layout), ADR-0055 (publish leaf), ADR-0070 (staged
listeners) — plus the `camel test` command and the dual-use lean
components. Coherent, but undocumented in one place. Section 5 closes that
gap.

## Decision

### 1. Placement: a dedicated `camel-matchers` crate

The shared assertion algebra lives in its own crate,
`crates/camel-matchers`, in the foundational band below `camel-api`,
alongside the pure libs. The crate is one named concept with zero ambiguity.

The B2 precedent was weighed and rejected for this case. rc-6bsf applied
where a natural home already existed for boot sharing: camel-config was the
home of the shared boot edges. Here the natural home IS the vocabulary.
Hosting matchers inside a config crate is the semantic accretion that makes
a 60+ crate workspace feel disorganized. The precedent does not transfer.

The crate-count cost is acknowledged and accepted. The cost of a crate is
semantic confusion, not the number. This crate adds no confusion: its name
and charter are one concept. The workspace is 60+ crates; this is one more,
justified on its own terms.

Rejected hosts:

- `camel-core` / `camel-api`: runtime pollution with test vocabulary.
- `camel-test`: would invert the dependency direction. The scenario kit
  (`camel-integration-test`) would depend on the unit-tier kit.

ADR-0055 topology applies cleanly: zero `camel-*` dependencies means the
crate publishes first, `lint-publish-cycles` passes trivially, and no
consumer waits on it.

### 2. Purity rule: types and pure functions, nothing else

The crate is types plus pure functions. It carries zero `camel-*`
dependencies.

Allowed foundational third-party dependencies: `regex`, `serde_json`,
`form_urlencoded`. Nothing else.

The crate has:

- No harness types.
- No wire types (`HttpWireRequest` has no home here).
- No redaction (the ADR-0051 law stays in the tier kits).
- No async runtime (no tokio).
- No Cargo features.

This buys a crate that `camel-test` (unit tier), `camel-integration-test`
(scenario tier), and `camel-cli` can consume without dragging harness or
wire concerns into one another.

### 3. One algebra, per-tier grammar, per-tier observation

"Same verbs, different subjects." Both tiers' assertion vocabularies
deserialize to, and call into, the same core types. The algebra is one; the
subjects are per-tier.

**Per-tier grammar.** Document formats stay per-tier. The ADR-0069 §2 mixing
ban stands: a `scenario:` document declares no `inputs`/`expects`/
`intercepts`, and the runner rejects the mix at load time. Grammars are
never unified.

**Per-tier observation.** What gets matched stays per-tier. The scenario
tier matches recorded wire: `HttpWireRequest`, lane keys, wire fidelity. The
unit tier matches in-process Exchange projections. These do not unify: wire
fidelity, lane keys, and redaction have no Exchange analog.

**Parameterize the algebra, not the observation.** Never introduce one
observation trait. The carve in this change is the pattern: `matching_count`
takes an iterator of projected `(method, path_and_query)` tuples. Each tier
projects its own observation into those tuples at the call site. The
scenario tier maps its wire records; a future unit tier maps its Exchange
projections.

### 4. Staged direction (future changes, not this one)

This ADR records the approved sequence. None of the steps below is this
change; this change is the pure carve only.

- **Step 2: unit-tier `expects` growth.** The unit tier grows body and
  header matchers in `camel-test`, consuming the crate.
- **Step 3: observational probes.** Step identity arrives through
  `to: mock:probe-N` probes, registry-only. These are observational and
  legal today.
- **Mutating weaving stays gated.** Skip and replace processors land only
  as a lean-set change per the ADR-0064 §5 AdviceWith Stage A/B frame.
  Observation is free; mutation is gated.
- **Wire timeouts are never virtualized.** ADR-0069 §6 stands: no virtual
  clock in core. Paused Tokio time stays a unit-harness concern.
- **`recipient_list` and dynamic-dispatch force-FULL stays static.** A
  runtime-verified closure would destroy pre-boot tier selection. The tier
  function remains a pure function of document content.

### 5. Testing-surface map

One map of the test surfaces. Each row names the surface, its role in the
pyramid, and the ADR that governs it.

| Surface | Role in the pyramid | Governing ADR |
|---|---|---|
| `camel-test` | Unit-tier kit: `CamelTestContext`, mock access, Tokio time control | ADR-0064 (unit tier), ADR-0055 (publish leaf), ADR-0070 (staged-listener helpers) |
| `camel-integration-test` | Scenario-tier kit: scenario model and parser, partner adapters, embedded FULL boot | ADR-0069 |
| `camel-matchers` | Shared assertion algebra | This ADR (0072) |
| `camel-cli test` command | Runner, tier derivation, tier filters | ADR-0069 §1 and §3, ADR-0064 |
| `camel-bundles` | Shared boot installers: bundle registration cascade, `BootHandle` | ADR-0069 §10 |
| `mock`, `direct`, `seda`, `timer`, `log` | Dual-use lean runtime components: the lean boot registers them; they are runtime components, not test-only crates | ADR-0064 §2 (closed lean set, creep-rule amendment gate), ADR-0055 |

This map closes the documentation gap. The story previously sat in
ADR-0064, ADR-0069, ADR-0055, and ADR-0070, plus the `camel-cli` command and
the component crates. This section is the single reference.

## Consequences

### Positive

- Two tiers share one semantic core without sharing grammar or observation.
- The unit tier can grow its `expects` vocabulary from the crate (step 2)
  without touching the scenario tier.
- The crate publishes first under ADR-0055 topology; no consumer waits on it.
- The test surfaces have one documented map.

### Negative

- One more crate in a 60+ workspace. Accepted and recorded in section 1.
- Grammar and observation duplication between tiers is deliberate and stays.
  Each tier keeps its own document formats and its own subjects.
- This ADR is Proposed. Section 4 records direction, not landed contract.
  Each step lands as its own change.

## Alternatives considered

- **camel-config as host (rc-6bsf B2 precedent).** Rejected in section 1.
  The precedent addressed boot sharing; the vocabulary has no natural home
  in a config crate.
- **camel-core / camel-api as host.** Rejected: runtime pollution with test
  vocabulary.
- **camel-test as host.** Rejected: inverts the dependency direction.
- **One observation trait over both tiers.** Rejected in section 3.
  `HttpWireRequest` and Exchange projections do not unify.
- **Unified grammar across tiers.** Rejected: ADR-0069 §2 stands.
