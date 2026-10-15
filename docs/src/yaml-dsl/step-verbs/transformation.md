# Transformation and enrichment

Convert, marshal, script, and enrich the message body.

## `transform`
Alias for `set_body`. Same forms and fields, including the top-level `${}`
value gate. A bare top-level string containing `${...}` compiles to a Simple
template. A nested literal string stays literal. See
[Basic steps](basic.md) for the gate.

```yaml
- transform:
    simple: "${body.field}"
```

> **Migration (BREAKING)**. Replace `script:` that uses
> simple/jsonpath/xpath/minijinja with `transform: {language, source}`.
> The behavior is identical.

## `marshal`
Serialize the body to a data format.

| Field | Type | Required | Description |
|---|---|---|---|
| `marshal` | string | yes | Format name (`json`, `protobuf`, ...) |
| `config` | object | no | Format-specific config |

```yaml
- marshal: "json"
```

## `unmarshal`
Parse the body from a data format. An optional `schema` validates the parsed
JSON and rejects mismatches.

| Field | Type | Required | Description |
|---|---|---|---|
| `unmarshal` | string | yes | Format name |
| `schema` | object | no | JSON Schema for validation |
| `config` | object | no | Format-specific config |

```yaml
- unmarshal: "json"
```

## `convert_body_to`
Convert the body type.

| Field | Type | Required | Description |
|---|---|---|---|
| `convert_body_to` | string | yes | Target type (`json`, ...) |

```yaml
- convert_body_to: json
```

## `script`
Run a mutating script inline. The script writes the Exchange back.

| Field | Type | Required | Description |
|---|---|---|---|
| `language` | string | yes | Script language; must support mutating expressions (`rhai`, `js`) |
| `source` | string | yes | Script source |

```yaml
- script:
    language: "rhai"
    source: "1 + 1"
```

## `enrich`
Enrich the exchange by requesting data from an endpoint.

| Form | Syntax |
|---|---|
| Short | `enrich: "http:..."` |
| Full | `enrich: { uri: "...", strategy: "...", timeout: 5000 }` |

The full form takes `uri` (required), `strategy`, `timeout`, and an optional `parameters` map merged into the URI query.

```yaml
- enrich: "http:my-service/api/data"
- enrich:
    uri: "http:my-service/api/data"
    strategy: "use_enriched_body"
    timeout: 5000
```

## `poll_enrich`
Enrich the exchange by polling an endpoint. Same fields as `enrich`, including the `parameters` map.

```yaml
- poll_enrich: "file:data"
```
