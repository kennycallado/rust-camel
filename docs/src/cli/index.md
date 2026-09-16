# CLI reference

The `camel` CLI runs, scaffolds, inspects, and compiles integration routes. Install it with `cargo install camel-cli`.

Each command has a reference page in this section. `run`, `new`, and `journal inspect` also appear in the [Getting started guide](../getting-started/cli.md) as walkthroughs.

## Command surface

| Command | Purpose | Reference |
|---------|---------|-----------|
| `camel run` | Start a Camel context from route files | [Getting started: CLI usage](../getting-started/cli.md) |
| `camel new <NAME>` | Scaffold a new project | [Getting started: CLI usage](../getting-started/cli.md) |
| `camel journal inspect <FILE>` | Read events from a runtime journal file | [Getting started: CLI usage](../getting-started/cli.md) |
| `camel lint <FILE>` | Lint a route file against the production component catalog | this page |
| `camel test <FILE\|DIR>...` | Run declarative mock tests from `*.test.yaml` documents | [Testing](../testing/index.md) |
| `camel job [<FILE>]` | Run or list jobs from `*.job.yaml` documents | [camel job](job.md) |
| `camel compile <DOCUMENT>` | Pack a route or job into a self-contained artifact | [camel compile](compile.md) |
| `camel plugin new` / `camel plugin build` | Scaffold and build WASM plugins | [OpenAPI and plugin subcommands](openapi-plugin.md) |
| `camel openapi generate <FILE>` | Emit an OpenAPI 3.0.3 document from `rest:` blocks | [OpenAPI and plugin subcommands](openapi-plugin.md) |
| `camel lsp` | Start the Language Server Protocol server over stdio | this page |

Flag definitions for the command enum live in `crates/camel-cli/src/main.rs`.

## camel lint

Lint one route file against the production component catalog. The command prints diagnostics with byte-exact spans.

```console
camel lint routes/my-route.yaml
```

| Exit code | Outcome |
|-----------|---------|
| 0 | Clean. No Error diagnostics. |
| 1 | At least one Error diagnostic. |
| 2 | CLI misuse: missing or unreadable file. |

Warnings do not change the exit code. For example, `camel lint` warns `R-MOCK-IN-PRODUCTION` on inline `to: mock:` sends in route files; see [Testing](../testing/index.md). Reserved documents are skipped with an informational message: `*.test.yaml` test documents and `*.job.yaml` job documents.

## camel lsp

Starts the Language Server Protocol server over stdio. Editors drive it; it takes no arguments. The server provides route-file diagnostics from the same lint engine.

## Conventions

- **Config file.** Commands that read configuration accept `--config <FILE>` (default: `Camel.toml`) and the `CAMEL_CONFIG_FILE` environment variable. An explicit `--config` wins.
- **Profiles.** Set the active profile with `CAMEL_PROFILE`.
- **Trust model.** `camel run`, `camel test`, and `camel job` execute route scripts, WASM modules, and beans resolved from the current working directory. Only run them from a trusted directory.
- **Exit codes.** 0 success; 1 execution failure (a failed route pipeline, a failing test, an Error diagnostic); 2 usage, boot, timeout, or apparatus failure. See the per-command pages for their exact taxonomies.

## See also

- [Getting started: CLI usage](../getting-started/cli.md) for the walkthroughs.
- [Configuration](../configuration/index.md) for `Camel.toml`, profiles, and environment interpolation.

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md)
