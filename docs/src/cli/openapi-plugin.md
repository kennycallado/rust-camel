# OpenAPI and plugin subcommands

The CLI ships two helper families for route work outside the running context. One reads `rest:` blocks and emits an OpenAPI 3.0.3 document. The other scaffolds and compiles WASM plugins for the runtime.

## Quick reference

| Command | Purpose | Example |
|---------|---------|---------|
| `openapi generate` | Emit an OpenAPI 3.0.3 document from `rest:` blocks | `camel openapi generate routes.yaml` |
| `plugin new` | Scaffold a WASM plugin project from a template | `camel plugin new my-plugin` |
| `plugin build` | Compile a WASM plugin and install the artifact | `camel plugin build` |

## camel openapi generate

Emit an OpenAPI 3.0.3 document from the `rest:` blocks of a YAML or JSON route file. The CLI reads the file, lowers the blocks, and prints a pretty JSON document to stdout.

```console
camel openapi generate routes.yaml
```

### Input file

A route file with one or more `rest:` blocks. Each block maps to a path entry. Each operation maps to a verb.

```yaml
rest:
  - host: 0.0.0.0
    port: 9090
    path: /api/users
    operations:
      - method: GET
        operation_id: listUsers
        to: direct:listUsers
      - method: POST
        operation_id: createUser
        consumes: application/json
        produces: application/json
        success_status: 201
        to: direct:createUser
        request_schema:
          type: object
          properties:
            name:
              type: string
          required: [name]
```

### Flags

| Flag | Description |
|------|-------------|
| `<file>` (positional) | Path to the route file (`.yaml`, `.yml`, or `.json`) |
| `--title <TITLE>` | API title for the `info` section (default: `Generated API`) |
| `--version <VER>` | API version for the `info` section (default: `1.0.0`) |

Flag definitions live in `crates/camel-cli/src/commands/openapi.rs`.

### Expected output

The command prints a pretty JSON document. The top-level `openapi` field is `3.0.3`. Each `rest:` block becomes a path entry. Each operation becomes a verb under that path. The success response carries a description and a content schema under `produces`. Body verbs carry a `requestBody` with a content schema under `consumes`.

```json
{
  "openapi": "3.0.3",
  "info": {
    "title": "Generated API",
    "version": "1.0.0"
  },
  "paths": {
    "/api/users": {
      "get": {
        "operationId": "listUsers",
        "responses": {
          "200": {
            "description": "OK",
            "content": {
              "application/json": { "schema": { "type": "object" } }
            }
          }
        }
      },
      "post": {
        "operationId": "createUser",
        "requestBody": {
          "content": {
            "application/json": {
              "schema": {
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
              }
            }
          }
        },
        "responses": {
          "201": {
            "description": "Created",
            "content": {
              "application/json": { "schema": { "type": "object" } }
            }
          }
        }
      }
    }
  }
}
```

### How generation works

The CLI selects a parser by file extension. `.yaml` and `.yml` files go through `extract_rest_blocks`. `.json` files go through `serde_json`. Unknown extensions fall back to YAML parsing.

Generation fails fast on three conditions. A file with no `rest:` blocks exits with an error. Lowering errors fail before generation runs. Duplicate route ids fail before generation runs. Validation warnings print to stderr and do not abort the command.

A body verb with no `request_schema` produces a warning and a weak stub. A non-204 verb with no response schema does the same. Regenerate the document whenever the `rest:` blocks change. The document mirrors the runtime contract.

## camel plugin new

Create a WASM plugin project from a template. The CLI writes a Cargo workspace member with the right target and the right dependencies. It also writes a sample `lib.rs` for the chosen plugin type.

```console
camel plugin new my-plugin
```

| Flag | Description |
|------|-------------|
| `<name>` (positional) | Plugin name (letters, digits, hyphens, underscores) |
| `--type <TYPE>` | `processor` (default), `bean`, or `authorization-policy` |
| `--force` | Overwrite files if the directory already exists |

Flag definitions live in `crates/camel-cli/src/commands/plugin.rs`.

### Plugin types

| Type | Purpose |
|------|---------|
| `processor` | Custom pipeline step. The plugin runs inside a Route as a `Service<Exchange>`. |
| `bean` | Named function called by name from a route. |
| `authorization-policy` | Security policy decision source. Routes reference it from a `security_policy` block. |

The default is `processor`. Pick the type that matches the plugin role. The template files differ by type. A processor template exports a function that takes an exchange. A bean template exports a named function. An authorization-policy template exports a decision function that returns allow or deny.

### Expected output

```text
Created camel processor plugin 'my-plugin'

Next steps:
  cd my-plugin
  camel plugin build
```

## camel plugin build

Compile a plugin to the `wasm32-wasip2` target. The CLI copies the compiled `.wasm` into the project plugins directory.

```console
camel plugin build
```

Run this from inside the plugin directory, or pass a path:

```console
camel plugin build ./my-plugin
```

| Flag | Description |
|------|-------------|
| `<path>` (positional, optional) | Plugin directory (default: current directory) |
| `--debug` | Build without `--release` |

The build calls `cargo build --target wasm32-wasip2`. Install the target with `rustup target add wasm32-wasip2` before the first build.

### Plugins directory resolution

The CLI resolves the destination in two steps. First, it walks up from the plugin directory to find the project root. The root holds a `Camel.toml` or a workspace `Cargo.toml`.

Second, it reads `[default.components.wasm].plugins_dir` from `Camel.toml`. The default is `plugins`. The value must be a relative path with no `..` segments. The CLI rejects absolute paths and paths that resolve outside the project root. These checks catch symlink escapes.

### Expected output

```text
Built and installed plugin 'my-plugin'
  source: /path/to/my-plugin/target/wasm32-wasip2/release/my_plugin.wasm
  installed: /path/to/camel-root/plugins/my-plugin.wasm
```

The CLI converts hyphens in the plugin name to underscores for the wasm artifact name. The `my-super-plugin` package compiles to `my_super_plugin.wasm`.

## See also

- [CLI usage](../getting-started/cli.md) for the rest of the CLI surface.
- [YAML DSL](../yaml-dsl/index.md) for the full `rest:` block reference.
- [WASM component](../components/index.md) for runtime plugin loading.

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md) | [DSL crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-dsl/CONTEXT.md) | [OpenAPI generator source](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-dsl/src/openapi.rs) | [Plugin command source](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/src/commands/plugin.rs)
