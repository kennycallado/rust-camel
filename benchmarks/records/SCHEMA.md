# Records schema

This document defines the canonical schema for benchmark records in
`benchmarks/records/`. It is the single source of truth for `run.json`
and `index.json`. Consumers and producers MUST follow it.

## Canonical public path

Records are served verbatim under the docs site at
`benchmarks/records/`. A client fetches the index with the relative
path `records/index.json` (no execution environment required; the file
is static JSON). Each published run lives in its own directory under
`records/`, and `index.json` points to it with a relative `path`.

## `run.json` — schema v1

`run.json` is the per-run record. It is deterministic: keys are sorted
and floats use a fixed representation, so two invocations over the same
inputs are byte-identical.

The fixed representation is: `repr(float(x))` for every numeric value,
`sort_keys=True`, `indent=2`, and LF line endings. Emitters MUST use
exactly this representation.

Array ordering is part of the contract. `cells` MUST be emitted sorted
by `(scenario, contender, variant, payload_class, metric)`; `ratios`
MUST be emitted sorted by `(numerator, denominator, metric)`;
`expected_cells` and `m2_attempted_cells` MUST be sorted
lexicographically. Two-invocation byte-identity depends on this order.

Top-level fields:

| Field | Type | Description |
| --- | --- | --- |
| `schema_version` | integer | Schema version. `1` for this document. |
| `run_id` | string | `<YYYYMMDDTHHMMSSZ>` — the launch timestamp, plain and chronological (records sort lexicographically). No sequence numbering (retired 2026-08-31: named subsets and `-v<N>` ids drifted from the owner's product definition — one command, one complete run, one record). Legacy era-1/era-2-pre ids keep the `<YYYYMMDD>-v<N>` shape. |
| `era` | string | `"1"` or `"2"`. A string, not an integer, so the vocabulary can grow without a type change. |
| `date` | string | ISO-8601 date of the run. |
| `git_commit` | string | 40-hex commit the run was measured against. |
| `container_digest` | string \| null | `sha256:<64hex>` image digest, or `null` for non-container runs. |
| `host_provenance` | object | Host snapshot (see below). |
| `protocol` | object | Run protocol (see below). |
| `cells` | array | Per scenario/contender cell measurements (see below). |
| `expected_cells` | array | Sorted list of `<scenario>/<contender>` identities — the EXPECTED registered roster, derived at build time from the run's `meta.json` `scenarios` (legacy `subset`). Identities, not a count: a wholly absent cell (or scenario) stays nameable. Derivation mirrors the harness asymmetry (`SCENARIO_ARTIFACT_SET` in `benchmarks/harness/run.sh`): 8 contenders per full scenario, 6 per bridge scenario (`xsd-validation-bridge`, `xslt-bridge` — core 4 + node 2; YAML variants carry no bridge-tax signal). |
| `m2_attempted_cells` | array | Sorted identities whose m2 attempt produced records but no usable ok summary (`m2-summary.txt` `status=failed reason=insufficient-samples` with `observed>0`). LEGACY shape: since the adaptive m2 window (bench-consol-tick task 3.2, bd rc-tpig) the harness no longer emits this status with `observed>0` — slow-ticking cells get an extended collection window and, if still short of the nominal count, an ok summary with a `note=slow-tick` line. The field + summarize.py recognition stay for pre-fix run dirs; such cells count as PRESENT m2 data by the publish completeness gate. Post-fix `status=failed … observed=0` (dead cell) is NOT present data. |
| `ratios` | array | Contender comparison ratios (see below). |

Completeness rule (pinned): a record is COMPLETE iff every
`expected_cells` identity has m1 data, plus m2 data when the
scenario's warm concept applies. Warm applicability is declared
scenario vocabulary: `startup-minimal` is cold-only (`warm: n/a` —
absence is not a gap); `http-server`, `t2-json`, `split-aggregate`,
`t2-realistic-eip`, `xsd-validation-bridge` and `xslt-bridge` are
warm-applicable. `bench publish` fails closed on incomplete records:
nonzero exit listing every missing `<scenario>/<contender>/<metric>`.

### `host_provenance`

Object capturing the host the run executed on:

| Field | Type | Description |
| --- | --- | --- |
| `cpu_model` | string | CPU model name. |
| `cores` | integer | Number of cores. |
| `kernel` | string | Kernel version. |
| `containerized` | boolean | Whether the run executed inside a container. |
| `load` | object | Load snapshot at run start. |

The `load` object:

| Field | Type | Description |
| --- | --- | --- |
| `one` | number | 1-minute load average captured at run start. |
| `five` | number | 5-minute load average captured at run start. |
| `fifteen` | number | 15-minute load average captured at run start. |

### `protocol`

Object describing the run protocol:

| Field | Type | Description |
| --- | --- | --- |
| `rounds` | integer | Number of measurement rounds. |
| `duration_secs` | number | Total run duration in seconds. |
| `warmup_secs` | number | Warmup duration in seconds. |
| `order_seed` | integer | Seed controlling measurement order. |

### `cells`

Array of per-cell measurements. Each cell:

| Field | Type | Description |
| --- | --- | --- |
| `scenario` | string | Scenario name. |
| `contender` | string | Contender (system under test) name. |
| `variant` | string | Variant of the contender. |
| `payload_class` | string | Payload class measured. |
| `metric` | string | Metric name. Opaque string; vocabulary owned by the harness and `CONTEXT.md` (no enum in this schema). |
| `round_values` | array | Array of numbers: per-round values in measurement order where the metric is per-round; otherwise the metric's native aggregation series (e.g. m4 `delta_distribution`). |
| `median` | number | `statistics.median` of `round_values`. |
| `unit` | string | Unit of the metric. Opaque string; vocabulary owned by the harness and `CONTEXT.md` (no enum in this schema). |
| `input_sha256` | string or null | Canonical input digest via the loadgen payload contract (`sha256:` + lowercase hex). `null` when the scenario has no canonical payload contract — its measurement input is not a canonical body. |

### `ratios`

Array of contender comparison ratios. Each ratio:

| Field | Type | Description |
| --- | --- | --- |
| `numerator` | string | Numerator contender. |
| `denominator` | string | Denominator contender. |
| `metric` | string | Metric the ratio applies to. |
| `point` | number | Point estimate. |
| `ci_lo` | number | Lower confidence bound. |
| `ci_hi` | number | Upper confidence bound. |
| `method` | string | Method used to compute the ratio and CI. |

Pairing rule (pinned): within a scenario, the numerator contender is
`rust-camel-lib` whenever that contender was measured; otherwise it is
the alphabetically first contender. The numerator is paired against
each remaining contender in alphabetical order. `rust-camel-lib` is
pinned as numerator so the headline row always reads
`rust-camel-lib / <baseline>` regardless of lexical order.

## Forward compatibility

Consumers MUST ignore unknown fields. Producers MUST bump
`schema_version` on any breaking change. Additive fields are minor and
do not bump the version within v1.

## `index.json` — versioned object

`index.json` is an OBJECT, not an array:

```json
{
  "index_schema_version": 1,
  "runs": [entry, entry]
}
```

Each entry:

| Field | Type | Description |
| --- | --- | --- |
| `run_id` | string | The run's `run_id`. |
| `date` | string | ISO-8601 date of the run (`YYYY-MM-DD`). |
| `era` | string | `"1"` or `"2"`. |
| `git_commit` | string | 40-hex commit of the run. |
| `scenarios` | string | Scenarios the run covers, comma-joined and sorted (derived from the cells). Pre-2026-08-31 entries carry `subset` with the same shape (legacy vocabulary). |
| `path` | string | Relative to `records/`, pointing at the run DIRECTORY (e.g. `20260905T142601Z/`); consumers append `run.json`. |

`runs` is ordered by date ascending. The object shape is versioned via
`index_schema_version` so the index can evolve without breaking
existing consumers.
