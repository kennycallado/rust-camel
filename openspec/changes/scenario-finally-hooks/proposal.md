# Proposal: scenario-finally-hooks

## Why

Scenario documents own state that outlives a boot: durable datasources, partner state, Redis
keys. The only post-hook today is the head of the action list; there is no guaranteed teardown,
and a scenario that fails mid-list leaves its seed state behind. Downstream camel-112 leaves
~85 run-time-named Redis keys per document and asked for a supported way to clean them. A Redis
flush is wrong once documents run in parallel (bd rc-gcf9n), so cleanup must be per-key and
author-declared. bd rc-6wodc.

## What Changes

- Document-level `finally:` key, sibling of `scenario:` (ADR-0069 amendment first, then code):
  an ordered teardown list that runs after the scenario verdict is fixed, on both verdict paths,
  before the report finalises.
- Restricted action set: exactly `sleep`, `sql`, `surreal`, and the new delete-only
  `redis: {datasource, del: [keys]}` prepare action. Verdict/assertion actions (`send`,
  `receive`, `validate`) in `finally:` fail at load, naming the key and the `finally:` position.
- New `redis` del prepare action, accepted in BOTH `scenario:` and `finally:` lists: literal
  keys only, resolved through the datasource catalog; glob metacharacters rejected at load.
  No `FLUSHDB`, no `FLUSHALL`, no pattern delete, no generic command runner (pattern delete is
  deferred to rc-ql8eq).
- Teardown failures surface as a new apparatus class `ScenarioFailure::TeardownFailure`
  (`teardown-failure`, exit 2) recorded through the existing `DocumentOutcome::final_failure`
  post-verdict slot plus a per-item record; they never mask the recorded verdict. Panic
  containment via `catch_unwind` at the document boundary.
- Execution-order trace on `DocumentOutcome` (`Action`, `LogsEvaluated`, `VerdictFixed`,
  `Teardown`), strictly in execution order; a failed scenario records no `VerdictFixed`.
- `camel-cli` boot selection extended: `finally:` prepare actions and the redis del action
  select the full boot like their `scenario:` counterparts.
- Docs: ADR-0069 §7/§8/§9/§11 amendment, CONTEXT-MAP.md index entry,
  scenario-documents.md "Teardown with `finally:`", scenario-redis.md `del` action, CHANGELOG.

## Affected crates

`camel-integration-test`, `camel-cli`, docs.

## Acceptance criteria

- `finally:` parses; verdict actions and glob keys fail at load; the key is optional.
- Teardown runs after the verdict on pass and fail; a teardown failure (or panic) adds a
  `teardown-failure` record after the verdict without rewriting it.
- `redis del` removes exactly the listed keys and touches nothing else.
- Boot selection routes catalog-needing `finally:` documents to the full boot.
- ADR-0069 amendment and context docs land before code (task 1).

## Risk budget

No `before:` semantics change; no existing validate target changes; sql/surreal prepare
actions only gain `finally:` acceptance. Demand gates and the mixed-vocabulary ban stand.
