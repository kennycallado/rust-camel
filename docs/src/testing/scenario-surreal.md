# SurrealDB state assertions

Scenario testing has two state branches. The TRAFFIC branch asserts messages on the wire: `send`, `receive`, and the `partner` and `lastReceived` validate targets. The STATE branch asserts data at rest. The state families are `sql` for relational rows and `surreal` for SurrealDB records. Writes go through the `surreal:` prepare action. Reads go through the `validate` surreal target. The vocabularies never mix: a prepare statement that reads fails the load, and a validate query that mutates fails the load too. The contract is pinned in [ADR-0069](../adr/0069-integration-tier-testing-contract.md).

The datasource itself lives in `Camel.toml` and is steered through `env:` interpolation. Read [Datasource steering](scenario-documents.md#datasource-steering) and [Isolation and teardown](scenario-documents.md#isolation-and-teardown) first. This page covers only the document-side grammar.

Actions run in declaration order, so `surreal:` actions compose with `send`, `receive`, `sleep`, and `sql:` in one list.

## The `surreal:` prepare action

A `surreal:` action prepares datasource state before the assertions run:

```yaml
scenario:
- surreal:
    datasource: statedb
    prepare:
    - REMOVE TABLE IF EXISTS user
    - DEFINE TABLE user SCHEMALESS
    - CREATE user SET name = 'alice'
```

`datasource` names a key under `[datasources]` in `Camel.toml`. `prepare` is an ordered list of SurrealQL write statements: define schema, seed records, or clean prior state. The statements run in order over the datasource's client before the next action starts. The run stops at the first failing statement and names its index. Failure diagnostics name the datasource, never its URL.

The parser enforces the write-only vocabulary at load:

- An empty `prepare` list fails.
- A statement whose text starts with `select` fails, after whitespace trimming and unwrapping of a leading parenthesis group. `(SELECT * FROM user)` fails like `SELECT * FROM user`. SurrealQL has no other read prefix, so `select` is the only banned one.

The error names the action index, the statement index, and the rule: reads belong to the `validate` surreal target. These parse-time rules run in every build. A `surreal:` action also requires the `surreal` Cargo feature: a build without the feature reports a named demand-gate error for a `surreal:` action that passes those rules. The `integration-surreal` CLI feature carries the family into `camel test` builds.

## The `validate` surreal target

A `validate` action with a `surreal` target runs a read against a datasource and asserts the returned records:

```yaml
- validate:
    target:
      surreal:
        datasource: statedb
        query: SELECT id, name FROM user ORDER BY name
    expectation: <rows-expectation>
    deadline: 5s
```

`datasource` follows the same identifier rule as the prepare action. `query` is the read text, run verbatim. The query must be exactly one read, and two load-time rules enforce that:

- The text must start with `select`, in any letter case, after whitespace trimming and leading-parenthesis unwrapping. Any other prefix fails the load. The prefix law is grammar admission, the read/write family split. It is not proof that the execution is read-only.
- A `;` separator followed by further non-whitespace text fails the load. Appended statements are rejected.

The `;` check is textual, and it can false-positive: a `;` inside a string literal in the query trips it. Restructure such a query. Move the literal out of the statement text, or split the projection.

`deadline` is a humantime string. It is valid on the `partner`, `sql`, and `surreal` validate targets. On any other target it is a grammar error.

### Rows expectation

The expectation holds exactly one shape: concrete row patterns (`rows`) or a row-count bound (`count`, `atLeast`, `atMost`). Declaring both fails the load.

- `rows` is a non-empty list of rows. Each row is a list of cell expectations, one per projected field.
- `columns` names the projection. It is required with `rows` on a surreal target: the driver returns records as key-sorted objects, so the query's selection order is not recoverable. A surreal `rows` expectation without `columns` fails the load. `columns` projects each record by field name, in declaration order. A name the record does not carry fails closed. Row-count bounds do not require `columns`.
- `unordered` defaults to `false`: rows match positionally in declaration order. `true` matches rows in any order through a perfect pairing.

```yaml
expectation:
  columns: [id, name]
  rows:
  - ['user:1', 'alice']
  - ['user:2', 'bob']
```

A row-count bound replaces `rows`. The forms mirror the partner count grammar, and exactly one bound form is allowed. `count: 2` is exact. `atLeast: 1` is a floor. `atMost: 4` is a ceiling. `atLeast` together with `atMost` is the inclusive range. `atLeast` above `atMost` fails the load.

### Cell verbs

Each cell uses the matcher dual grammar. A bare value is a literal `equals`. A map with one recognized key selects that verb. Any other object is a literal value.

| Verb | Payload | Meaning |
|------|---------|---------|
| `equals` | any JSON value | structural equality |
| `regex` | pattern string | unanchored regex match, compiled at load |
| `contains` | string | substring containment |
| `startsWith` | string | prefix match |
| `endsWith` | string | suffix match |
| `exists` | none | the cell is present |
| `ignore` | none | the wildcard: matches anything, including `null` |
| `jsonSubset` | JSON object | recursive subset match |

`exists` and `ignore` take no argument and are written `exists: null` and `ignore: null`. A stored null reads as JSON `null`, so a cell that must be null writes `equals: null`. A projected field the record does not carry fails closed. It never reads as a silent null.

## Record ids and value mapping

Every projected cell passes one mapping law:

| SurrealQL value | Cell value |
|-----------------|------------|
| `none`, `null` | JSON `null` |
| `bool` | boolean |
| `int`, finite `float` | number |
| `string` | string |
| `uuid`, `datetime` | string form |
| record id | `table:key` string |
| `object`, `array` | structured value, mapped recursively |

The law fails closed. Decimal numbers, non-finite floats, `bytes`, `duration`, `geometry`, `table`, `file`, `range`, `regex`, and `set` have no exact cell form. Any of them fails the action naming the field and its SurrealQL type. Recursion never launders an unsupported kind: one nested inside an object or array fails naming the field path. There is no silent null and no lossy coercion.

The record id projects as its `table:key` string. A query that selects `id` yields cells like `user:1`.

## Deadline and settle semantics

Without a `deadline`, one immediate snapshot decides the assertion. With a `deadline`, the runner takes a fresh snapshot every 100 ms until the deadline passes, and the final snapshot decides.

The surreal poll never settles early, even when a snapshot matches. Record sets are non-monotone: a concurrent `DELETE` can shrink a set that an earlier snapshot already matched. This matches the sql poll and deviates from the partner poll, which settles as soon as its count holds. The one mid-window exit is an upper-bound breach: an `atMost` or range bound observed above its ceiling is an immediate terminal failure. The runner does not wait to see whether a later snapshot dips back under the ceiling.

## The ORDER BY advisory

An ordered `rows` assertion over a query without `ORDER BY` depends on the driver's record return order. The load-time advisory warns about that combination, the same rule the sql target applies: a surreal target, a `rows` expectation, `unordered` false, and no `ORDER BY` in the query text (a case-insensitive search). The warning names the action index and says to declare `unordered: true` or add `ORDER BY`. The advisory only warns. It never rejects the document. The search can trip on a string literal that contains the words. A warning on a correct document is the accepted cost. A wrong rejection is not.

## The `mem://` convention

The scenario-tier convention for this family is the embedded mem tier:

```toml
[datasources.statedb]
provider = "surrealdb"
db_url = "mem://"
```

`mem://` boots an embedded SurrealDB instance inside the scenario process. Pin `provider = "surrealdb"` to name the factory explicitly. No credentials are needed: the factory skips signin on the `mem` scheme, because a fresh embedded instance has no root user. `namespace` and `database` are optional and default to `test` and `test`. Every `mem://` connect builds a fresh, isolated embedded instance, so the SQLite per-connection `:memory:` hazard does not exist here. No boot lint guards the form.

## Remote backends and the clean-first idiom

A remote `db_url` (`ws://`, `wss://`, `http://`, or `https://`) addresses a real SurrealDB instance through the same datasource steering. Remote schemes keep the mandatory credentials and the signin step. Their state is durable and outside the boot freshness guarantee. Cleanup is the author's job, and the clean-first idiom is the tool: the first `surreal:` prepare statement deletes prior state.

Teach `REMOVE TABLE IF EXISTS user`, not plain `REMOVE TABLE`. On surrealdb 3.x a plain `REMOVE TABLE` is not absence-tolerant: it fails when the table does not exist, and the first boot against a clean instance would die on statement 0. `IF EXISTS` makes the idiom idempotent. A `DELETE user` on an existing table serves the same role.

## Isolation

Each scenario boot owns its datasource catalog and its clients, and the teardown releases both. The surreal close hook invalidates the boot's clients. A `mem://` datasource dies with its boot: the embedded instance has no other holder, and a later document booting the same alias starts empty. A remote backend keeps its records across boots. Its cleanup is the author's job, through the clean-first idiom. See [Isolation and teardown](scenario-documents.md#isolation-and-teardown).

## Worked example

The example boots one `direct:` route, seeds records with `surreal:`, and asserts the seeded records. The datasource is the mem convention above. The document:

```yaml
routeFiles: [routes.yaml]
scenario:
- surreal:
    datasource: statedb
    prepare:
    - REMOVE TABLE IF EXISTS user
    - DEFINE TABLE user SCHEMALESS
    - CREATE user SET num = 1, name = 'alice'
    - CREATE user SET num = 2, name = 'bob'
- validate:
    target:
      surreal:
        datasource: statedb
        query: SELECT id, num, name FROM user ORDER BY num
    expectation:
      columns: [id, num, name]
      rows:
      - ['user:1', 1, 'alice']
      - ['user:2', 2, 'bob']
```

An unordered variant drops `ORDER BY` from the query, declares `unordered: true`, and lists the rows in any order. A deadline variant adds `deadline: 5s`; the poll runs through the deadline and the final snapshot decides.

The e2e shape is pinned by [`crates/camel-integration-test/tests/surreal_state_test.rs`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/tests/surreal_state_test.rs) (`surreal_state_e2e_prepare_route_validate`, the `mem://` datasource shape), the prepare and catalog rules by [`src/runner_test.rs`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/src/runner_test.rs) (`surreal_prepare_seeds_and_proceeds`, `surreal_statement_failure_stops_and_redacts`, `surreal_single_catalog_invariant`), the target grammar by [`src/doc_parse_test.rs`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/src/doc_parse_test.rs) (`surreal_target_parses`, `surreal_rows_without_columns_is_load_error`), and the projection and poll rules by [`src/runner/surreal_validate_test.rs`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/src/runner/surreal_validate_test.rs) (`ordered_rows_pass_immediately`, `record_id_projects_as_string`, `deadline_poll_passes_when_record_appears`, `no_early_settle_final_snapshot_decides`).
