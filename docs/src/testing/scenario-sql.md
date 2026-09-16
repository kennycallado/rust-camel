# SQL state assertions

Scenario testing has two state branches. The TRAFFIC branch asserts messages on the wire: `send`, `receive`, and the `partner` and `lastReceived` validate targets. The STATE branch asserts data at rest. Writes go through the `sql:` prepare action. Reads go through the `validate` sql target. The two vocabularies never mix: a prepare statement that reads fails the load, and a validate query that mutates fails the load too. The contract is pinned in [ADR-0069](../adr/0069-integration-tier-testing-contract.md).

The datasource itself lives in `Camel.toml` and is steered through `env:` interpolation. Read [Datasource steering](index.md#datasource-steering) and [Isolation and teardown](index.md#isolation-and-teardown) first. This page covers only the document-side grammar.

Actions run in declaration order, so `sql:` actions compose with `send`, `receive`, and `sleep` in one list.

## The `sql:` prepare action

An `sql:` action prepares datasource state before the assertions run:

```yaml
scenario:
- sql:
    datasource: appdb
    prepare:
    - DELETE FROM orders
    - INSERT INTO orders VALUES ('seed-a')
```

`datasource` names a key under `[datasources]` in `Camel.toml`. `prepare` is an ordered list of non-SELECT statements: seed rows, clean prior state, or apply DDL. The statements run in order over the datasource's pool before the next action starts. The run stops at the first failing statement and names its index. Failure diagnostics name the datasource, never its URL.

The parser enforces the write-only vocabulary at load:

- An empty `prepare` list fails.
- A statement whose text starts with `select` or `with` fails, after whitespace trimming and unwrapping of a leading parenthesis group. `(SELECT 1)` fails like `SELECT 1`.

The error names the action index, the statement index, and the rule: reads belong to the `validate` sql target. These parse-time rules run in every build. A `sql:` action also requires the `sql` Cargo feature: a build without the feature reports a named demand-gate error for a `sql:` action that passes those rules.

## The `validate` sql target

A `validate` action with an `sql` target runs a read against a datasource and asserts the returned rows:

```yaml
- validate:
    target:
      sql:
        datasource: appdb
        query: SELECT id, sku FROM orders ORDER BY id
    expectation: <rows-expectation>
    deadline: 5s
```

`datasource` follows the same identifier rule as the prepare action. `query` is the read text, run verbatim. The query must be a read: the same `select`/`with` prefix rule applies, and a mutating query fails the load. `deadline` is a humantime string. It is valid only on the `partner` and `sql` validate targets; on any other target it is a grammar error.

### Rows expectation

The expectation holds exactly one shape: concrete row patterns (`rows`) or a row-count bound (`count`, `atLeast`, `atMost`). Declaring both fails the load.

- `rows` is a non-empty list of rows. Each row is a list of cell expectations, one per projected column.
- `columns` optionally names the projection. The runner narrows each row to these columns by name, in declaration order. A name the result does not carry fails closed.
- `unordered` defaults to `false`: rows match positionally in declaration order. `true` matches rows in any order through a perfect pairing.

```yaml
expectation:
  columns: [id, sku]
  rows:
  - [1, 'abc']
  - [2, 'def']
```

A row-count bound replaces `rows`. The forms mirror the partner count grammar, and exactly one bound form is allowed. `count: 2` is exact. `atLeast: 1` is a floor. `atMost: 4` is a ceiling. `atLeast` together with `atMost` is the inclusive range; `atLeast` above `atMost` fails the load.

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

`exists` and `ignore` take no argument and are written `exists: null` and `ignore: null`. SQL NULL reads as JSON `null`, so a cell that must be null writes `equals: null`.

## Deadline and settle semantics

Without a `deadline`, one immediate snapshot decides the assertion. With a `deadline`, the runner takes a fresh snapshot every 100 ms until the deadline passes, and the final snapshot decides.

The sql poll never settles early, even when a snapshot matches. Row sets are non-monotone: a concurrent `DELETE` can shrink a set that an earlier snapshot already matched. This is a deliberate deviation from the partner poll, which settles as soon as its count holds. The one mid-window exit is an upper-bound breach: an `atMost` or range bound observed above its ceiling is an immediate terminal failure. The runner does not wait to see whether a later snapshot dips back under the ceiling.

## The ORDER BY advisory

An ordered `rows` assertion over a query without `ORDER BY` depends on the database's row return order. A load-time advisory warns about that combination: an `sql` target, a `rows` expectation, `unordered` false, and no `ORDER BY` in the query text (a case-insensitive search). The warning names the action index and says to declare `unordered: true` or add `ORDER BY`. The advisory only warns; it never rejects the document. The search can trip on a string literal that contains the words. A warning on a correct document is the accepted cost; a wrong rejection is not.

## sqlite in-memory recap

A bare `sqlite::memory:` URL gives every pooled connection its own private database. An INSERT on one connection and a SELECT on another can hit different databases, and a validation can pass against state the document never seeded. The boot rejects that URL with the `sql-memory-not-shared` lint. Two forms are accepted:

- the named shared-memory URI `sqlite:file:<name>?mode=memory&cache=shared`, the scenario-tier convention. Pin `provider = "sqlx"`, because `sqlite:file:` matches no automatic datasource factory prefix.
- the bare `sqlite::memory:?cache=shared`. Pin `max_connections = 1`, because pooled connections can hold private databases there.

The full recipe lives in [Datasource steering](index.md#datasource-steering).

## Isolation

Each scenario boot owns its datasource catalog and its pools, and the teardown closes them. A memory datasource starts empty at every boot. A durable datasource, such as file-backed sqlite or a service-container Postgres, keeps its rows across boots, and cleanup is the author's job: the clean-first idiom deletes prior state in the first `prepare` statement. See [Isolation and teardown](index.md#isolation-and-teardown).

## Worked example

The example boots one `direct:` route, seeds a table with `sql:`, and asserts the seeded rows. The datasource:

```toml
[datasources.appdb]
provider = "sqlx"
db_url = "sqlite:file:memdb_orders?mode=memory&cache=shared"
```

The document:

```yaml
routeFiles: [routes.yaml]
scenario:
- sql:
    datasource: appdb
    prepare:
    - CREATE TABLE orders (id INTEGER, sku TEXT)
    - INSERT INTO orders VALUES (1, 'abc')
    - INSERT INTO orders VALUES (2, 'def')
- validate:
    target:
      sql:
        datasource: appdb
        query: SELECT id, sku FROM orders ORDER BY id
    expectation:
      columns: [id, sku]
      rows:
      - [1, 'abc']
      - [2, 'def']
```

An unordered variant drops `ORDER BY` from the query, declares `unordered: true`, and lists the rows in any order. A deadline variant adds `deadline: 5s`; the poll runs through the deadline and the final snapshot decides.

No shipped fixture yet combines an http partner write with a sql validation in one document, so the example stays inside the verified surface. The `sql:`-prepare document with a `direct:` boot route is the e2e shape of `sql_prepare_seeds_and_proceeds`, and the grammar pieces in this example are pinned by [`crates/camel-integration-test/src/runner_test.rs`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/src/runner_test.rs) (`sql_prepare_seeds_and_proceeds`, the `SQL_E2E_DATASOURCE` datasource shape), [`src/doc_parse_test.rs`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/src/doc_parse_test.rs) (`sql_target_parses`), and [`src/sql_validate_test.rs`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-integration-test/src/sql_validate_test.rs) (`ordered_rows_pass_immediately`, `unordered_rows_match_reorder`, `deadline_poll_passes_when_row_appears`).
