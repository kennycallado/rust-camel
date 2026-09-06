# Proposal: dsl-parity-json-valid

## Why

Assurance-monthly calibration run 33984285881 (fuzz-deep, `dsl_parity` target,
900 s leg) found a panic in the parity oracle: `parity divergence: yaml rejects
json-valid document` (`fuzz/src/lib.rs:139`). The minimized document embeds a
raw DEL byte (U+007F) inside a JSON string. `serde_json` accepts the document;
the YAML front-end rejects it because a YAML stream forbids non-printable
characters.

This is not a parser defect. JSON permits DEL in strings; YAML prohibits it in
the raw stream (YAML 1.2 c-printable, enforced by libyaml-class scanners). The
oracle contract "every JSON-valid document is YAML-valid" is too strong: the
rejection is spec-correct, and the panic is a false-positive divergence that
re-fires on every future fuzzing leg.

## What Changes

- Refine the `dsl_parity` oracle (`fuzz/src/lib.rs`): when a `serde_json`-valid
  document contains characters outside the YAML printable set, assert the
  expected YAML rejection instead of panicking; keep the strict panic for all
  other rejections.
- Add one shared classifier `yaml_stream_has_non_printable(&str) -> bool` in
  `camel-dsl` (`src/yaml.rs`), exported so the fuzz crate and camel-dsl tests
  use a single definition of the YAML printable set.
- Promote the minimized input to a committed `#[test]` regression per assurance
  policy: `camel-dsl/tests` gains a parity regression pinning both production
  front-ends (JSON accepts; YAML rejects with a `YAML DSL error:` annotated
  message).

Affected crates: `camel-dsl`, `camel-fuzz` (fuzz workspace, excluded from the
root workspace).

bd: rc-crpk (P1, claimed).

## Acceptance Criteria

- `dsl_parity` oracle unit tests: the harness consumes the minimized document
  without panic; the carve-out path asserts the YAML front-end error.
- camel-dsl regression test: `parse_json_to_declarative` returns `Ok` on the
  minimized document; `parse_yaml_to_declarative` returns `Err` whose message
  starts with `YAML DSL error:`.
- Escaped DEL (`\u007f` on the wire) remains under strict parity: both
  front-ends accept the escaped form and produce equal route steps; the
  carve-out is not triggered when the stream is printable.
- `cargo fmt` / `cargo clippy` clean; camel-dsl tests green; camel-fuzz lib
  tests green.

## Risk Budget

Low. No production parse semantics change: both front-ends keep their current
behavior for this input class. The only behavioral change is inside the fuzz
oracle (a false-positive class is removed). The main risk is an over-broad
classifier that excuses real divergences — mitigated by keeping the prohibited
set exactly the YAML non-printable classes and pinning each class in unit
tests.
