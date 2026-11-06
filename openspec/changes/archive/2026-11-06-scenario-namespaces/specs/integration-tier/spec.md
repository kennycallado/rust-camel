# Delta: integration-tier

## ADDED Requirements

### Requirement: Document namespaces

A scenario document MAY declare a top-level `namespaces:` list of
`{bindVar: NAME, base: BASE}` entries. The harness SHALL mint one namespace
value per entry per document with the fixed grammar
`BASE "~it~" RUN "~" ORD "~"`, where `BASE` matches
`[A-Za-z0-9][A-Za-z0-9_-]{0,31}`, `RUN` is four bytes from the
OS CSPRNG rendered as exactly eight lowercase hex characters, generated
once per `camel test` invocation and shared by every document of that run
(never derived from time, pid, or hostname), and `ORD`
is the 1-based zero-padded four-digit index of the document in the
deterministic sorted discovered-document list, before every filter (tier,
name, or explicit-set). An example
mint is `112~it~3f9a1c0e~0007~`; the maximum length is 50 bytes. The set of
minted values SHALL be prefix-free by two independent guarantees — the
trailing `~` terminator and the fixed-width zero-padded fields — and both
guarantees are load-bearing: no minted value is a prefix of another, so the
prefix of document 10's value never matches document 100's keys.

At load, the parser SHALL reject, with `doc-validation` errors naming the
offending entry: a `bindVar` duplicate within the document, a `base`
duplicate within the document (two entries with the same base would mint
identical values), a `base` outside the `BASE` grammar, and a `bindVar`
colliding with any document `env:` key, passthrough name, or partner
`bindVar` (checked in both directions). More than 9999 discovered documents
in one invocation SHALL be a load error. The minted value SHALL bind in the
harness-provisioned layer of the layered environment — the same layer as
partner `bindVar` values — so `${env:NAME}` resolves to it with harness
precedence and the reserved-key rule keeps `NAME` out of the document env
map. Namespace grammar, validation, and minting SHALL run in every build
(no feature gate); they are harness-side and touch no datasource.

#### Scenario: mint is prefix-free across documents 10 and 100

- **GIVEN** the same run id and base, with ordinals 10 and 100
- **WHEN** the harness mints both values
- **THEN** neither literal string is a prefix of the other

#### Scenario: namespace is minted per document and visible to env

- **GIVEN** one document declaring a `namespaces:` entry
- **WHEN** the harness runs an invocation with two scenario documents
- **THEN** each document's mint differs, matches the grammar, and
  `${env:NAME}` resolves to that document's mint in the harness layer

#### Scenario: run id is per invocation and shared

- **GIVEN** one `camel test` invocation over several documents
- **WHEN** every document's namespace is minted
- **THEN** the run segment is 8 lowercase hex and identical across all of
  them

#### Scenario: duplicate bindVar or base is a load error

- **GIVEN** a document whose `namespaces:` declares two entries with the
  same `bindVar`, or two entries with the same `base`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the duplicated field and
  entry

#### Scenario: bindVar colliding with env is a load error

- **GIVEN** a document whose `namespaces:` declares a `bindVar` equal to a
  document `env:` key, a passthrough name, or a partner `bindVar`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the colliding key

#### Scenario: ordinal is zero-padded and bounded

- **GIVEN** documents at ordinal positions 3 and 10000 of one invocation
- **WHEN** the mints render
- **THEN** ordinal 3 renders `0003`, and the 10000th document is a load
  error because more than 9999 documents were discovered

### Requirement: Redis unlinkPrefix teardown action

The scenario grammar SHALL accept a namespace-scoped pattern delete under
the existing `redis:` action key as
`redis: {datasource: <name>, unlinkPrefix: "<template>"}`, in the
`finally:` list ONLY; an `unlinkPrefix` entry in the `scenario:` list SHALL
fail at load with a `doc-validation` error naming the action and its
position. A raw `redis:` action item SHALL carry exactly one of `del` or
`unlinkPrefix`, never both; a malformed or absent namespace declaration
SHALL keep the load error named. The `unlinkPrefix` template SHALL be
exactly `${env:NAME}` plus an optional literal suffix, where `NAME` SHALL
be one of the same document's declared namespace `bindVar`s. A template
carrying a `:-default`, a second `${`, an unknown or undeclared `NAME`, or
a suffix containing a Redis glob metacharacter (`* ? [ ] \ ^`) SHALL fail
at load. The `datasource` keeps the identifier law (a configured datasource
name, never env-interpolated). Load-time grammar rules SHALL run in every
build before the `redis` demand gate (the `sql:` precedent); when the
feature is off, a valid entry SHALL fail at load with a named demand-gate
error. The boot selection scan SHALL count an `unlinkPrefix` entry as a
redis action. The general pattern-delete ban outside this exception, and
`KEYS`, `FLUSHDB`, `FLUSHALL`, and generic command runners, remain
forbidden; the `del` contract is unchanged.

#### Scenario: unlinkPrefix requires a declared binding

- **GIVEN** documents whose `unlinkPrefix` templates are a literal prefix,
  a `${env:DOCUMENT_ENV_KEY}` value, a `${env:NS:-default}` default, and an
  unknown `${env:NOT_DECLARED}` name respectively
- **WHEN** each document loads
- **THEN** every load fails `doc-validation` naming the template defect

#### Scenario: unlinkPrefix is rejected outside finally

- **GIVEN** a document whose `scenario:` list carries an `unlinkPrefix`
  entry
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming `unlinkPrefix` and the
  scenario position

#### Scenario: unlinkPrefix rejects glob metacharacters in the suffix

- **GIVEN** an `unlinkPrefix` template `${env:ns}orders:*`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the suffix and the
  metacharacter

#### Scenario: unlinkPrefix selects the full boot

- **GIVEN** a document whose only teardown is an `unlinkPrefix` entry
- **WHEN** `camel test` selects the execution path
- **THEN** the document boots with the datasource catalog, not the smoke
  path

### Requirement: unlinkPrefix runtime deletion scope

At run time (feature `redis`) the executor SHALL resolve the template's
`NAME` from the harness-minted namespace layer ONLY — never the merged
environment, document env, or ambient values. It SHALL re-check that the
resolved value matches the mint grammar AND is byte-equal to the value
minted for this document, and SHALL fail without issuing any SCAN when
either check fails. It SHALL build the SCAN MATCH pattern as the escaped
concatenation of the namespace value and suffix followed by exactly one
unescaped `*`, and SHALL scan with `COUNT 500` (a hint — a reply may carry
more keys). The executor SHALL validate the ENTIRE scan reply with
byte-exact `starts_with(namespace || suffix)` BEFORE any UNLINK chunk
executes; if any key in the reply fails the check — including a key beyond
the first 500 entries — the batch SHALL fail as an infrastructure failure
and NOTHING from that reply SHALL be unlinked. It
SHALL UNLINK in batches of at most 500 keys inside a 10-second deadline and
a 100 000-key cap; a deadline or cap breach, or an unaligned scan result,
SHALL be reported under the existing `teardown-failure` apparatus class
with the reason and the number of keys deleted so far — no new failure
class. Cluster datasources SHALL be refused (at load when the config says
cluster, otherwise at run time). Keys outside the namespace SHALL survive.

#### Scenario: unlinkPrefix deletes only under the namespace

- **GIVEN** a document with an `unlinkPrefix` teardown, with keys seeded
  under the document's namespace and an out-of-namespace sentinel whose
  name differs from a namespace key only after the ordinal segment
- **WHEN** the teardown runs
- **THEN** every in-namespace key is gone and the sentinel survives

#### Scenario: unaligned scan result stops the batch

- **GIVEN** a scan result containing a key that does not byte-prefix-match
  the namespace-plus-suffix
- **WHEN** the executor classifies the batch
- **THEN** the batch fails with an infrastructure failure and none of its
  keys are unlinked

#### Scenario: violating key beyond the first 500 entries still refuses the reply

- **GIVEN** a scan reply whose violating key appears after the first 500
  entries
- **WHEN** the executor validates the full reply
- **THEN** no UNLINK chunk executes and the failure names the violating
  position

#### Scenario: deadline and cap report teardown failure

- **GIVEN** an executor run whose limits (deadline, key cap) are exceeded
- **WHEN** the breach is detected
- **THEN** the failure renders as `teardown-failure` carrying the reason
  and the number of keys deleted so far

## MODIFIED Requirements

### Requirement: Redis delete prepare action

The scenario grammar SHALL accept a delete-only redis prepare action under the key
`redis: {datasource: <name>, del: [<keys>...]}`, in BOTH the `scenario:` list and the
`finally:` list. The `datasource` SHALL be a configured datasource name and every key
SHALL be a document-authored literal; neither is env-interpolated (the identifier law).
The `del` list SHALL be non-empty. A key containing a glob metacharacter (`* ? [ ] { }`)
SHALL fail at load naming the key. The action MUST be visibly distinct from the `validate`
redis target grammar (`datasource/key/type/ttl`): the prepare form carries `datasource` and
`del`. There SHALL be no `FLUSHDB`, no `FLUSHALL`, no pattern delete of authored keys, and
no generic command runner; the only pattern-shaped delete is the namespace-scoped
`unlinkPrefix` teardown exception (its own requirement, finally-only).

Load-time grammar rules SHALL run in every build before the `redis` demand gate (the
`sql:` precedent); when the feature is off, a valid entry SHALL fail at load with a named
demand-gate error, and a directly-constructed document SHALL fail closed at action time.

Execution (feature `redis`) SHALL resolve the datasource through the same
`DatasourceCatalog` the booted composition root built (one datasource name, one connection
handle; the executor SHALL NOT construct its own), SHALL issue one `DEL` over the literal
keys, and SHALL treat missing keys as success. A driver error SHALL surface as an
apparatus-class failure carrying the datasource name and ADR-0051-sanitized driver text,
with the resolved URL redacted. The boot selection scan SHALL route a document whose
`scenario:` or `finally:` needs the catalog (including this action) to the full boot.

#### Scenario: finally redis del removes exactly the listed keys

- **GIVEN** a document whose `finally:` carries a redis del action listing key `k1`
- **WHEN** the teardown runs against the catalog datasource with keys `k1` and `sentinel`
  seeded
- **THEN** `k1` is gone and `sentinel` is untouched

#### Scenario: redis del rejects a pattern key

- **GIVEN** a redis del action whose `del` list contains `orders:*`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the key and the metacharacter

#### Scenario: empty del list is a load error

- **GIVEN** a redis del action with `del: []`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming the action

#### Scenario: redis del in the scenario list selects the full boot

- **GIVEN** a document whose only action is a redis del (no wired endpoints, no validate
  target)
- **WHEN** `camel test` selects the execution path
- **THEN** the document boots with the datasource catalog, not the smoke path

### Requirement: Document teardown with finally

A scenario document MAY declare a document-level `finally:` key, sibling of `scenario:`,
carrying an ordered teardown action list. The allowed set is exactly: `sleep`, `sql`,
`surreal`, the `redis` delete prepare action, and the `redis` `unlinkPrefix`
teardown action (namespace-scoped pattern delete, bd rc-ql8eq; grammar under
its own requirement). A `send`, `receive`, or `validate` entry
in `finally:` SHALL fail at load with a `doc-validation` error naming the action key and
the `finally:` position. A declared-but-empty `finally:` list SHALL fail at load, mirroring
the empty-`prepare` law. A document without the key SHALL behave exactly as before. Grammar
validation of `finally:` entries SHALL run before the feature demand gate (the `sql:`
precedent): a featureless build reports the grammar error for a malformed entry and the
named demand-gate error for a valid one.

Teardown SHALL run after the scenario verdict is fixed, before the report finalises, on
BOTH verdict paths. Each item SHALL run inside panic containment at the document boundary:
a panicking item SHALL be reported as the apparatus class `teardown-failure` (exit 2) and
SHALL NOT abort the process; the next item and the next document still run. A teardown
failure SHALL be recorded as `teardown-failure` after the recorded verdict and SHALL NEVER
mask or rewrite it: a passed scenario stays a pass with a `teardown-failure` record; a
failed scenario keeps its verdict-class failure. When teardown and boot shutdown both
fail, both records SHALL render.

The document outcome SHALL record an execution-order trace: one event per completed
scenario action, the verdict-fixed marker when a pass verdict is fixed, the logs-evaluated
marker after the logs evaluation, and one event per completed teardown item, strictly in
execution order. A failed scenario SHALL record no verdict-fixed marker.

#### Scenario: finally key parses and accepts prepare actions

- **GIVEN** a document whose `finally:` carries a `redis` del action and a `sleep` action
- **WHEN** the document loads
- **THEN** both entries parse and the document loads

#### Scenario: finally rejects a verdict action at load

- **GIVEN** a document whose `finally:` contains a `send` action
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming `send` and the `finally:` position

#### Scenario: finally is optional

- **GIVEN** a document without a `finally:` key
- **WHEN** the scenario runs
- **THEN** behavior is unchanged from the current runner

#### Scenario: declared-but-empty finally list is a load error

- **GIVEN** a document declaring `finally: []`
- **WHEN** the document loads
- **THEN** the load fails `doc-validation` naming `finally`

#### Scenario: finally runs after the scenario verdict

- **GIVEN** a passing scenario with two `finally:` items
- **WHEN** the scenario runs
- **THEN** the recorded trace reads verdict-fixed after the last action event and teardown
  events after it, in declared order

#### Scenario: finally failure does not mask a pass

- **GIVEN** a passing scenario whose single `finally:` item fails
- **WHEN** the run reports
- **THEN** the verdict stays recorded and a `teardown-failure` record renders after it,
  with exit 2

#### Scenario: finally panic is contained and reported

- **GIVEN** a `finally:` item that panics
- **WHEN** the teardown runs
- **THEN** the panic is reported as `teardown-failure` and the runner still executes the
  next document

#### Scenario: finally runs when the scenario fails

- **GIVEN** a scenario whose `validate` action asserts a variable target with an
  expectation no extract ever satisfies
- **WHEN** the scenario fails its verdict
- **THEN** every `finally:` item still runs after the failure and the recorded trace
  carries no verdict-fixed marker

#### Scenario: grammar validation precedes the demand gate

- **GIVEN** a build without the `sql` feature and a document whose `finally:` carries a
  malformed `sql:` entry
- **WHEN** the document loads
- **THEN** the load fails with the grammar error naming the `finally:` position, not the
  demand-gate error

#### Scenario: teardown failure and shutdown failure both render

- **GIVEN** a scenario whose `finally:` item fails and whose boot shutdown also fails
- **WHEN** the run reports
- **THEN** the `teardown-failure` record renders first and the `shutdown-failure` record
  renders after it, both apparatus-class

#### Scenario: finally accepts an unlinkPrefix entry

- **GIVEN** a document declaring a namespace and whose `finally:` carries a valid
  `unlinkPrefix` entry
- **WHEN** the document loads
- **THEN** the entry parses and the document loads
