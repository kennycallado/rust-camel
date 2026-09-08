# Proposal: validate-partner-self-declare

## Why

The demo/pilot team reports (bd rc-z1cjv, verified on v0.41.0/v0.42.0)
that proxy-style scenario tests cannot express their natural assertion.
When the route-under-test dials an upstream with a per-request-varying
query (bbox/tile coordinates), no literal `receive` from-ref can match
the strict raw-wire arrival lane, so `validate {partner}` — the
framework-correct tool, per ADR-0069 §5 partner-side normative proof —
is the only usable assertion. But document cross-check (i) requires
every partner validate URI to string-equal a `send`/`receive` harness
ref, and the workarounds are toxic: a sacrificial trailing receive is
guaranteed to time out (receive-timeout verdict, exit 1, permanently-red
assertion on every real proxy test), while a sacrificial send dials the
partner for real and inflates `count` expectations.

This is a framework-level gap, not a usage error: the pattern is the
canonical proxy test shape and the tool for it exists but is
unreachable without an accompanying send/receive.

## What Changes

Adopt the papal-refined proposal **A-prime** (opt-in self-declaration;
bd's original proposal B — a no-op `declare` action — is rejected as
YAGNI):

- A partner validate target in object form (`provisioning: harness`)
  whose URI is named by a `partners:` entry **self-declares** the
  harness reference: the CLI driver wires it exactly like a
  `send`/`receive` ref (binds the partner, fills `bindVar`, feeds the
  harness-provisioned env fold).
- The doc-level cross-check (i) is relaxed accordingly, keeping a total
  typo guard: every partner validate URI must still either match a
  `send`/`receive` harness ref or be a `partners:`-named object-form
  self-declaration — otherwise `doc-validation`, exit 2, at load.
- `ScenarioAction::bindings()` returns Partner-target bindings (gated
  on `provisioning: harness`) so a validate `bindVar` reserves its env
  key like any other declaration.
- Normative spec canon amended (`integration-tier/spec.md`, "Partner
  request verification"): declaration rule prose + narrowed scenario +
  new self-declaration scenario.
- Docs: testing guide, ADR-0069 §5/§9 note, `ScenarioTarget::Partner`
  doc-comment.

Explicitly excluded: no new action grammar (no `declare`), no change to
validate read/poll semantics (`deadline`, bounds, filters untouched),
no change to send/receive behavior, no partner-script changes.

## Acceptance criteria

- A scenario whose ONLY partner reference is an object-form validate
  with `provisioning: harness` + `partners:` entry loads, binds the
  partner, fills the bindVar, exposes it via the env fold, and runs
  green with no sacrificial receive (the pilot repro).
- A plain-string (or object-form without `partners:` entry) partner
  validate URI matching no send/receive harness ref still fails at
  load with `doc-validation` naming the URI, exit 2.
- Existing documents (plain-string validate targets) behave
  byte-identically.
- `integration-tier/spec.md` canon, testing guide, and ADR-0069 all
  describe the self-declaration rule.

## Risk budget

Acceptable: narrow grammar relaxation at the parse/validate layer and
driver wiring list; small public-behavior surface (bindings()). Out of
bounds: touching validate poll/deadline semantics, partner scripting,
arrival-lane keying, or the boot/env-fold order; any behavior change
for documents that do not use object-form validate partner targets.

Affected crates: camel-integration-test (document parse/validate),
camel-cli (scenario driver wiring). Bd: rc-z1cjv.
