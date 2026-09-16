# camel job

`camel job` runs one job from a `*.job.yaml` document. A job is a bounded execution: it boots a Camel context, sends one exchange (or drains one batch), writes a JSON report, and exits. It is not a long-running server. Use it for scheduled tasks, operational one-shots, and batch triggers.

> **Trust model.** `camel job` executes route scripts, WASM modules, and beans resolved from the current working directory, like `camel run`. Only run it from a trusted directory.

Authority: [ADR-0062](../adr/0062-reserved-test-suffix-and-placement-contract.md), the `cli-jobs` canonical spec, and [`crates/camel-cli/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md).

## Quick reference

```console
camel job                     # list the discovered jobs
camel job report-nightly      # run <root>/report-nightly.job.yaml
camel job jobs/report-nightly.job.yaml   # run by explicit path
camel job report-nightly --help          # show the declared interface
camel job report-nightly --arg env=prod --arg retries=3
```

| Flag | Description |
|------|-------------|
| `<FILE>` (positional, optional) | Job document path or bare job name. Omitted, the command lists the discovery set. |
| `--help`, `-h` | With a name, print the job's declared interface. Without a name, print usage. |
| `--arg <NAME=VALUE>` | Supply one declared argument. Repeat the flag for several pairs. |
| `--report <FILE>` | Write the JSON report to this path instead of stdout. |
| `--config <FILE>` | Path to `Camel.toml` (default: `Camel.toml`). Also read from `CAMEL_CONFIG_FILE`; an explicit `--config` wins. |

Flag definitions live in `crates/camel-cli/src/commands/job/mod.rs`.

## Document resolution

The positional argument accepts three forms:

- **Omitted.** The command lists every job document under the `[jobs].dirs` roots. Listing is a query, not an error: absent roots and empty sets exit 0. Each row shows the job name and its `description:` key.
- **Bare name** (no path separator, no `.yaml` suffix). The command probes `<name>.job.yaml` at the top level of every discovery root. A stem that matches in two roots fails with an error that names every match. A miss names every probed file.
- **Explicit path** (any path separator, or a `.yaml`/`.yml`/`.json` suffix). The command uses the path as given.

See [Jobs discovery](../configuration/jobs.md) for the `[jobs]` table and its bounded walk.

## Job documents

A job document carries one top-level `execute:` section and exactly one route source:

```yaml
description: Nightly sales report
args:
  env:
    type: enum[dev,staging,prod]
    required: true
    description: Target environment
  retries:
    type: int
    default: "3"
execute:
  mode: one-shot
  timeout: 30s
  send:
    to: "direct:report?waitForTaskToComplete=Always"
    body: "run report for ${arg:env}"
    headers:
      x-retries: "${arg:retries}"
routes:
  - from:
      uri: "direct:report"
      steps:
        - to: "log:report"
```

Grammar rules, enforced at load:

- The file suffix is `.job.yaml` or `.job.yml`. The suffix is part of the document contract ([ADR-0062](../adr/0062-reserved-test-suffix-and-placement-contract.md)).
- `execute:` carries `mode` (`one-shot` or `batch`), a mandatory `timeout`, and one `send` action. The timeout covers the whole run: boot, send, drain, and teardown.
- The `send.to` target must use `direct:` or `seda:` (in-memory, synchronous request/reply transports).
- The `send.body` accepts a string or an object/array. Explicit `body: null` is rejected.
- Exactly one route source: `routeFiles` (relative to the document), `routeFilesFromRoot` (relative to the nearest ancestor `Camel.toml`), or an inline `routes:` block.
- Route consumers are gated by a fail-closed allowlist: `from:` accepts `direct`, `seda`, `log`, and `mock`. Producer URIs in `to:` steps are unrestricted.
- `mode: batch` runs the same send as `one-shot`, then drains until every SEDA queue is empty.

## Declared arguments

A top-level `args:` map declares the job's interface. Each entry admits four keys:

| Key | Value | Meaning |
|-----|-------|---------|
| `type` | `string` (default), `int`, `bool`, or `enum[a,b,c]` | The declared type of the argument. |
| `required` | `true` or `false` | The CLI must supply a value. |
| `default` | string | The value applied when the CLI omits the argument. |
| `description` | string | Author documentation, shown by `--help`. |

Argument names match `[A-Za-z_][A-Za-z0-9_]*`. Unknown keys fail at load.

Typed values are coerced to a canonical form before use:

- `int` parses as `i64` and canonicalizes to plain decimal (`007` becomes `7`, `+5` becomes `5`).
- `bool` accepts `true`/`false` case-insensitively, never `1`/`0`, and canonicalizes to lowercase.
- `enum` members match exactly, case-sensitive.
- `string` keeps the value verbatim.

Resolution order: an unknown `--arg` name fails first, then a missing `required` argument, then a coercion failure. A repeated `--arg` takes the last value. An explicit pair always wins over a `default`.

Resolved values reach the document through `${arg:NAME}` tokens in `to`, `body`, `headers`, and `timeout`:

- `${arg:NAME}` consults only the resolved argument values. It never reads the environment.
- `${env:NAME}` consults only the ambient environment.
- A fallback form `${arg:NAME:-value}` is rejected.

### Legacy documents

A document without an `args:` map keeps the legacy behavior. Each `--arg NAME=VALUE` pair becomes a message header at send time, applied after the document headers. The command prints a deprecation note to stderr. Declare `args:` in new documents.

## camel job \<name\> --help

The `--help` flag with a job name renders the declared interface: the description, the mode, the send target, and one row per argument with its type, requirement flag, default, and description. The renderer flattens line breaks so each row stays on one line.

## Reports and exit codes

The run writes a JSON report to stdout, or to `--report`. The report records the outcome and timing. With `capture-reply` set, it also carries the reply exchange body and headers.

| Exit code | Outcome |
|-----------|---------|
| 0 | `Completed` |
| 1 | `Failed` (the route pipeline failed) |
| 2 | `Timeout` (the overall budget expired) or `Interrupted` (first SIGINT/SIGTERM) |

The first SIGINT or SIGTERM interrupts the send. Teardown always runs under a bounded budget.

## Embedded jobs in compiled artifacts

A compiled artifact (see [`camel compile`](compile.md)) can carry a job document as its entry point. A v2 artifact loads its routes from the embedded virtual store; a legacy v1 artifact runs the single embedded document inline. An embedded run uses the declaration defaults with no `--arg` surface. A required argument without a default fails before boot with exit code 2. Authority: [ADR-0075](../adr/0075-self-contained-executable-artifact-format.md).

## See also

- [Jobs discovery](../configuration/jobs.md) for the `[jobs]` table.
- [CLI reference](index.md) for the full command surface.
- [Testing](../testing/index.md) for the reserved-suffix family (`*.test.yaml`).

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md)
