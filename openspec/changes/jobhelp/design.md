# Design: jobhelp

## Approach

Clap intercepts `--help` before application code runs, so the `job`
subcommand disables clap's automatic help flag and accepts `--help`/`-h` as
an ordinary boolean flag on `JobArgs`. Dispatch then branches on the
positional name:

- **Name present, `--help` set.** Resolve the name through the existing A1
  seam (`resolve_job_path` over `[jobs].dirs`), read the file, and parse it
  with a new projection parser, `parse_job_document_for_help`. Help must
  project every schema-valid document, including documents whose `to` or
  `timeout` carry unresolved `${arg:NAME}` tokens — the bare
  `parse_job_document` seam value-validates those fields (duration parse,
  send scheme) against literal token text and would reject valid A2
  documents, and `parse_job_document_with_args` with empty pairs fails on
  unsatisfied required arguments. The projection parser therefore runs
  every structural and declaration check (suffix contract, section
  exclusivity, strict serde shape, route-source conflict, strict
  per-argument declarations, mode spelling, `timeout` presence) and returns
  a small projection struct carrying the mode spelling, the raw `to` text,
  and the normalized declarations — with no pair resolution and no
  execution-value validation (duration, send scheme). Unresolved tokens
  render as authored (`Sends to: ${arg:target}`): help shows what the
  author wrote, not a resolved run. The description comes from the A1
  `JobListProbe` serde over the in-memory text (the strict document model
  does not carry `description`). Print to stdout, exit 0.
- **No name, `--help` set.** Print clap's usage text for the `job`
  subcommand (rendered from the command tree), exit 0. Bare `camel job`
  still lists the discovery set (A1, untouched).
- **Name present, no `--help`.** Existing execution path, untouched.

Help precedes `--report` handling: `--help --report FILE` renders help and
writes nothing. Signal streams are armed only when a document run follows,
so the arming predicate gains `&& !args.help`. A malformed `--arg` pair
still fails in clap's value parser with exit 2, consistent with the usage
taxonomy.

The rendered job name is the file stem of the resolved document
(`daily-sync.job.yaml` renders `daily-sync`), for both bare-name and
explicit-path invocation. Output layout (exact-string pinned by tests;
`JobArgumentDeclarations` is a `BTreeMap`, so rows are ordered lexically by
argument name):

```
<file stem>

<description or "(no description)">

Mode:      one-shot
Sends to:  direct:ingest

Arguments:
  feed        string  required             Feed identifier
  region      string  optional  default=eu-west-1
```

A document without `args:` — absent or present-but-empty (`args: {}`) —
prints `Arguments:` followed by the single line `(no arguments)`. The
`string` type column is fixed in v1 and anticipates A4 typed arguments.
Each argument renders on one line: any CR or LF inside a `default` or
`description` renders as a single space (multiline values stay one row).
The summary pins only mode and send target (`to`), per the RULING's
projection scope; `timeout`, `body`, and `headers` stay out.

## Affected crates

- `camel-cli`: `commands/job` clap surface (`disable_help_flag`, own
  `--help` flag), `parse_job_document_for_help` projection parser,
  `render_job_help` pure function, `run_job` dispatch wiring, unit and
  subprocess tests.
- `openspec/specs/cli-jobs` (delta): new job-scoped help requirement.

## Architecture boundaries

Help stays operator configuration at the CLI boundary. It reads the
declaration surface A2 already produces and adds no schema, no runtime
registry, no trait, and no route-pipeline interaction — the render path
returns before boot. The projection parser reuses the existing strict
serde model and `JobDocError` diagnostics, so structural and declaration
failures keep the existing loud exit-2 behavior; it adds no new error
variants. Discovery and bare-name resolution reuse the A1 seams (ADR-0062
jobs discovery set, `[jobs].dirs`), so resolution failure modes (unknown
name, ambiguous stem) inherit the existing loud exit-2 behavior. The
job-scoped help path follows RULING-camel-job-orientation section 2c and
section 6 trap 5; exit codes stay inside the existing taxonomy
(load/validation/usage = 2). camel-dsl and camel-config are untouched.

## Alternatives considered

- **`allow_hyphen_values` on the positional.** Does not stop clap's help
  interception; rejected.
- **Pre-clap manual `--help` scan in `main`.** Fragile, drifts from the
  clap grammar; rejected in favor of `disable_help_flag` scoped to the
  `job` subcommand only.
- **Render from `parse_job_document_with_args` with empty pairs.** That
  path validates required arguments and would fail help for documents with
  unsatisfied required args; rejected.
- **Render from the bare `parse_job_document`.** That path leaves
  `${arg:...}` tokens unresolved but still value-validates `timeout`
  (duration parse) and `to` (send scheme) against the literal token text,
  so valid A2 documents with interpolated targets fail help with exit 2;
  rejected in favor of the projection parser.
- **Token-sniffing validation (skip a value check only when the raw text
  contains `${`).** Two validation modes per field add spec surface for
  no operator value; rejected — help validates structure and declarations
  strictly, and execution values not at all.
- **`--help` without a name printing the A1 listing.** Rejected: the
  listing is already the no-flag default, and usage text is the
  least-surprise help for the subcommand itself.
