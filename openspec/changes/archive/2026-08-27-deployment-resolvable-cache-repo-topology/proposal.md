# Proposal: deployment-resolvable-cache-repo-topology

## Why

The demo team deployed rust-camel 0.35.0 as N nodes × M environments over one
image, selecting topology only via `CAMEL_PROFILE` + deployment env vars. Two
verified gaps block the natural model ("profile = structure axis, deployment =
resolve environment via env vars"):

- **FR1 (bd rc-5kyu)**: `${env:X:-}` expands to `Some("")`, but redis topology
  validation (`validate_redis_topology_fields`, config.rs:871) decides by
  `Option::is_some()`. A `[cache_repo]` section that carries both `url` and
  `sentinel_nodes` keys — one expanded empty — always trips the mutual
  exclusion, regardless of env state. Standalone-vs-sentinel topology is not
  parametrizable by env vars within one section.
- **FR2 (bd rc-2o9e)**: `ALLOWED_ENV_OVERRIDES` (config.rs:2363) covers only 5
  `cache_repo` fields. Non-credential tuning fields (`db`, `key_prefix`,
  `sweep_interval`, …) cannot be set per-deployment without profile
  proliferation.

Upstream report verified by escalation research (e_glm, 2026-08-27); no
duplicates existed in bd or openspec.

## What Changes

- **Empty topology values resolve to absent (FR1)**: after placeholder
  expansion, empty string values (and all-blank `sentinel_nodes` arrays)
  in cache_repo AND idempotent_repo topology fields are normalized to `None`
  before validation. Precedent: `payload_dir` treats empty as absent.
  In scope fields: `url`, `sentinel_nodes`, `master_name`, `username`,
  `sentinel_username`, `key_prefix`. Both repos share the validator, so both
  get the normalization.
- **Non-credential env overrides (FR2)**: `ALLOWED_ENV_OVERRIDES` gains
  `CAMEL_CACHE_REPO_PAYLOAD`, `CAMEL_CACHE_REPO_PAYLOAD_DIR`,
  `CAMEL_CACHE_REPO_CACHE_SIZE`, `CAMEL_CACHE_REPO_SWEEP_INTERVAL`,
  `CAMEL_CACHE_REPO_MASTER_NAME`, `CAMEL_CACHE_REPO_KEY_PREFIX`,
  `CAMEL_CACHE_REPO_DB`, `CAMEL_CACHE_REPO_SENTINEL_NODES` (CSV →
  `Vec<String>` coercion; empty CSV → unset via FR1). Empty SCALAR overrides
  are a no-op (file value preserved — empty strings must never reach typed
  deserialization).
- **Operator docs**: guide configuration chapter gains the new override vars
  and the empty-means-unset rule.

### Explicitly excluded

- `url`, `username`, `password`, `sentinel_username`, `sentinel_password`
  NEVER become env-overridable (L-C2 invariant: no security-sensitive field
  can be overridden via env; connection strings belong in `${env:}`
  placeholders, which FR1 makes topology-switchable).
- No change to additive profile-merge semantics or cross-backend validation
  (documented as intended design; per-profile complete `cache_repo` sections
  remain the supported backend-switching pattern).
- Credential VALUES are never dropped: only whitespace-only `password` /
  `sentinel_password` normalize to unset (a blank placeholder selecting the
  other topology must not trip sentinel-only validation); non-blank
  credentials pass through untouched; `validate_credentials` domain
  untouched.

## Acceptance criteria

- A `[cache_repo]` section with `url = "${env:REDIS_URL:-}"`,
  `sentinel_nodes` and `master_name` set, and `REDIS_URL` unset, validates as
  sentinel topology.
- The same section with `REDIS_URL` set AND the sentinel keys
  (`sentinel_nodes` entries, `master_name`) expanding empty validates as
  standalone. (Both-present-populated remains a mutual-exclusion error, as
  today.)
- `CAMEL_CACHE_REPO_DB=3` and `CAMEL_CACHE_REPO_SENTINEL_NODES="a:26379,b:26379"`
  override the deserialized config; an EMPTY scalar override var (e.g.
  `CAMEL_CACHE_REPO_DB=`) preserves the file value; an empty CSV var clears a
  populated file `sentinel_nodes` to unset (standalone then validates).
- A blank-expanded `key_prefix` selects the repository default prefix; a
  non-blank invalid prefix stays rejected by keyspace validation.
- `CAMEL_CACHE_REPO_URL` (and every credential var) is still ignored with the
  existing "not in override allowlist" warning.
- All existing topology validation errors still fire for genuinely present
  conflicting values.

## Risk budget

- No security-boundary relaxation of any kind; the allowlist stays
  deny-by-default and credential-free.
- Empty-normalization is semantics-tightening only: `Some("")` previously
  either failed validation or produced a broken client; no working config can
  regress (empty `key_prefix` was already invalid, empty `url` already failed
  topology or construction).
- `idempotent_repo` receives the same normalization (shared validator);
  accepted scope widening, documented in design.
- Programmatic construction and direct deserialization bypassing the loader
  pipeline are out of scope (no behavior change there).
