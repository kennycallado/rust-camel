# Design: scenario-namespaces

## Approach

Mirror the landed `finally:`/`del` architecture (bd rc-6wodc): grammar and
validation ungated in every build, execution demand-gated behind the
`redis` feature, one `ScenarioAction` variant landing together with its
`run_action` arm.

**Mint (new `namespace.rs` in camel-integration-test, ungated).**
`mint_namespace(base, run_id, ordinal) -> String` builds
`base~it~run~%04ord~` (max 50 bytes). `RunId` is four bytes from the OS
CSPRNG (`getrandom`, already in the lockfile) rendered as exactly eight
lowercase hex characters, generated once per `camel test` invocation, never
from time/pid/hostname. `validate_namespaces` enforces the load rules:
bindVar unique in the document, reserved against `env:` keys, passthrough
names, and partner bindVars (both directions); base matches `BASE` and is
unique per document; >9999 discovered documents is a load error.

**Document model.** `ScenarioDocument` gains two fields: `namespaces:
Vec<NamespaceBinding>` (declared, load-validated) and `minted_namespaces:
BTreeMap<String, String>` (harness-written after parse; the parser never
reads it from YAML; empty by default, so a directly-constructed document
resolves no namespace). `parse_scenario_document` parses and validates the
raw `namespaces:` entries with the same reserved-key walk the inbound
bindVar uses, extended in both directions.

**Variant.** `ScenarioAction::RedisUnlinkPrefix { datasource, template }`,
raw shape `redis: {datasource, unlinkPrefix: "..."}`, validated by
`validate_redis_unlink_prefix` in `redis_action.rs`: template is exactly
`${env:NAME}` plus an optional suffix, NAME is one of this document's
declared namespace bindVars, the NAME segment carries no default construct
(`${env:ns:-fallback}` fails; a literal `:-` in the suffix is valid), no
second `${`, suffix free of Redis glob
metacharacters (`* ? [ ] \ ^`), one-of with `del` (the raw `redis`
item carries exactly one of the two fields — both raw fields are
`Option`, so an unlink-only item deserializes and the validation owns the
one-of rule), and the variant is accepted
only in `finally:` (a post-build position check rejects it for `scenario:`
items, the mirror of the `finally_from_raw` verdict-action rejection).
Grammar runs in every
build; the boot-selection scan counts the variant as a redis action.

**Runtime (feature `redis`).** `execute_redis_unlink_prefix(catalog,
datasource, prefix, limits)` in `redis_action.rs`: resolve the datasource
through the boot catalog's steering axis (never a self-constructed
connection; the `MultiplexedConnection` downcast inherently refuses cluster
handles, and the pool factory already rejects `redis+cluster://` /
`redis+sentinel://` URLs at catalog build). Steps: build pattern as
`escape(prefix_suffix) + "*"` with exactly one unescaped `*`; SCAN
`COUNT 500` (a hint — a reply may carry more keys); validate the ENTIRE
scan reply byte-exact `starts_with(prefix_suffix)` BEFORE any UNLINK chunk
executes — a violating key anywhere in the reply fails the batch as an
infrastructure failure with nothing in that batch unlinked; UNLINK in
batches of at most 500 inside a 10 s deadline and a
100 000-key cap (`UnlinkLimits` with `Default`). Any breach or unaligned
key surfaces as an apparatus failure carrying the reason and the keys
deleted so far; the `finally:` loop already wraps it as `teardown-failure`
(exit 2), so no new failure class.

**Resolution seam.** The `run_action` unlink arm resolves NAME from
`doc.minted_namespaces` ONLY — never the merged `LayeredEnv` lookup — so a
document-env or ambient value can never scope a delete. Before issuing any
SCAN it re-checks BOTH that the resolved value matches the mint grammar AND
that it is byte-equal to the value minted for this document; either check
failing stops the action without a SCAN. The CLI driver
(`test.rs`) generates the run id once per invocation, assigns each
scenario document its 1-based ordinal from the complete deterministic
sorted discovered-document list (a byte-sorted clone used ONLY for ordinal
assignment — execution order and the expansion contract stay untouched),
before every filter (tier, name, or explicit-set; the ordinal map is keyed
by canonical path and read after parse, independent of the admitted loop),
mints each declared binding, and enforces the >9999 discovery cap at that
same point. `run_scenario_full_boot` folds the minted values into the
harness tier of `LayeredEnv` (`harness_provisioned`), which makes them
visible to `${env:NAME}` with the same precedence as partner bindVars; the
fake-smoke path has no `LayeredEnv` and needs no fold (scenario strings
stay literal there; every `unlinkPrefix` document full-boots). Unit-tier
documents are untouched.

**ADR/docs.** ADR-0069 §4 gains the namespace paragraph (grammar + the
"two independent guarantees / defence in depth" rationale); §8.4 gains the
unlinkPrefix exception after the rc-ql8eq deferral (finally-only, declared
binding, no default, byte-equality, escaping, SCAN 500 + byte re-check +
UNLINK ≤500, 10 s / 100k cap, `teardown-failure`, no cluster, and the
"only a harness-minted prefix-free namespace makes the delete scope
provable" sentence). `CONTEXT-MAP.md` ADR-0069 entry records the amendment.
`docs/src/testing/scenario-documents.md` documents `namespaces:`;
`scenario-redis.md` documents `unlinkPrefix`.

## Affected crates

- camel-integration-test: `namespace.rs` (new), `document.rs` (raw key,
  model fields, reserved-key walk, finally/scenario set), `redis_action.rs`
  (raw/typed unlink twin, validation, executor), `runner.rs` (run_action
  arm), `lib.rs` exports.
- camel-cli: `commands/test.rs` (run id, ordinals, >9999 load error),
  `commands/test/scenario.rs` (mint threading into both paths, harness
  env tier, action label).

## Architecture boundaries

Harness-side only: camel-core, camel-processor, and camel-component-redis
are untouched. The namespace is a test-harness isolation device per
ADR-0069 §4/§8; the runner's failure mapping rides the existing §7
taxonomy. No process-environment mutation (the layered-env law); ambient
state enters only through the injected lookup. Del contract unchanged.

Single-phase change: no `## Phase` headings; tasks run as one ordered flat
list (ADR first, then grammar/model, variant, runtime, driver, docs).
