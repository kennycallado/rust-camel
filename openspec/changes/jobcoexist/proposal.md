# Proposal: jobcoexist

## Why

`camel job` cannot run beside a live `camel run` in the same pod. The job
inherits the ambient `Camel.toml` wholesale: the durable runtime journal
(redb, single writer) fails with `Database already open`, and enabled
Prometheus or health listeners fail their port bind. Both exit 2. A real
Kubernetes report from the camel-cache demo team (bd rc-va2jl) shows the
primary incident path — `kubectl exec` job operations against a live pod —
is blocked.

The journal and the diagnostic listeners are server-side control-plane and
observability surfaces. A one-shot job does not need them: no report field
reads journal data, and job outcome construction has no exporter dependency.
The e_gpt survey (`.opencode/fleet/orders/104-e_gpt-rc-va2jl-findings.md`)
confirmed all four surfaces — journal, OTel, Prometheus, health — are
optional side effects inherited from server configuration.

## What Changes

Ruling (e_opus 2026-09-16, final): ship Option D — command-purpose
projection at the `execute_job` seam.

- Project the parsed, validated config into a job-effective config at the
  top of `execute_job`, before `configure_context_with_beans`: set
  `runtime_journal = None` and `observability = default()`. The allowlist
  is exact — components, repositories, security, platform, beans,
  supervision, timeouts, and log level survive.
- Apply to ALL job forms: argv filesystem jobs, embedded single-document
  jobs, and compiled artifacts. No flag; no telemetry opt-in now.
- Parse, validate, then project: malformed ambient configuration still
  fails loud with exit 2.
- Compiled job manifests stop declaring listeners the projection
  suppresses (`TrailerKind::Job` filter); route artifacts unchanged.
- Spec deltas: `cli-jobs` (composition-root wording, coexistence
  scenarios, isolated one-shot operator naming), `job` (stability
  exemption for inherited diagnostic Lifecycles), `cli-compile`
  (projection through job boot policy, truthful manifests).
- Docs: `crates/camel-cli/CONTEXT.md` job boot prose and the
  `CONTEXT-MAP.md` job entry drop full-inheritance claims.

Excluded: job-in-app submission into a live context (epic rc-u991y, own
trigger); any telemetry flag; changes to `camel-config` generic context
semantics, `camel-core`, `camel-bundles`, `camel run`, or ambient
repository (`idempotent_repo`, `cache_repo`) behavior.

## Acceptance criteria

- A `camel job` against a config with live journal + Prometheus + health
  starts no diagnostic listener, opens no journal, exits 0 with report
  (real-binary coexistence test beside `job_one_shot_test`).
- Two concurrent `camel job` processes on one ambient config collide on
  nothing beyond documented ambient repositories.
- Compiled job manifests list no suppressed listeners; route artifacts
  unchanged.
- A structural test proves the projection touches only `runtime_journal`
  and `observability`.
- `camel-config` `context_config_test` stays green untouched.

## Risk budget

Small code surface, one crate (`camel-cli`). Dominant risks: subprocess
coexistence test flakiness (bounded by readiness polling and free-port
selection) and manifest drift between declared and effective listeners
(guarded by kind-filtered derivation tests).
