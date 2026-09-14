# Proposal: jobhelp

## Why

`camel job` declares an interface (A2 top-level `args:` with `required`,
`default`, and `description`), but operators cannot see it. clap intercepts
`--help`, so `camel job <name> --help` prints clap's subcommand usage instead
of the job's contract. Bd issue `rc-mlzuq` (A3 of epic `rc-d5cgc`, RULING
section 2c and section 6 trap 5) requires a job-scoped help path that renders
the declared interface from the document, before any boot.

## What Changes

- Suppress clap's automatic `--help`/`-h` on the `job` subcommand and accept
  `--help` as a regular flag.
- With a job name present: resolve the name through the existing A1
  discovery, parse the document with a projection parser that enforces
  every structural and declaration check but resolves no arguments and
  validates no execution values, and print the declared interface — job
  file stem, description, execution mode, send target, and one line per
  declared argument (name, type, required/optional, default,
  description). Exit 0.
- A document without an `args:` block prints `(no arguments)` in place of
  the argument table.
- Without a job name: `--help` prints the `camel job` usage text; bare
  `camel job` keeps the A1 listing unchanged.
- Help is a pure projection: no route boot, no report write, no signal
  handling. Unknown or ambiguous name and malformed documents fail loud with
  exit 2.

Out of scope: typed arguments (A4), new exit codes, changes to `camel-dsl`
or `camel-config`, and any change to the listing behavior.

## Acceptance criteria

- `camel job <name> --help` prints description plus the argument table from
  `args:`; exit 0.
- A job with no `args:` block prints description plus `(no arguments)`.
- clap does not intercept `--help` when a job name is present.
- Help renders without booting the route pipeline (no side effects).
- Unknown job name with `--help` fails loud with exit 2.

## Risk budget

Acceptable risk is limited to `crates/camel-cli` (`commands/job` clap
surface, one render function, dispatch wiring) and the cli-jobs delta spec.
No runtime, DSL, config, or exit-code taxonomy changes. The top-level
`camel --help` and every other subcommand's help must stay clap-rendered.
