# CLI reference

The `camel` CLI runs, scaffolds, inspects, and compiles integration routes. Install it with `cargo install camel-cli`, or `cargo binstall camel-cli` for a prebuilt binary (see [Distribution flavors](../operations/distribution-flavors.md)).

Each command has a reference page in this section. `run`, `new`, and `journal inspect` also appear in the [Getting started guide](../getting-started/cli.md) as walkthroughs.

## Command surface

| Command | Purpose | Reference |
|---------|---------|-----------|
| `camel run` | Start a Camel context from route files | [Getting started: CLI usage](../getting-started/cli.md) |
| `camel new <NAME>` | Scaffold a new project | [Getting started: CLI usage](../getting-started/cli.md) |
| `camel journal inspect <FILE>` | Read events from a runtime journal file | [Getting started: CLI usage](../getting-started/cli.md) |
| `camel lint <PATH>` | Lint a route file or a directory tree | this page |
| `camel test <FILE\|DIR>...` | Run declarative mock tests from `*.test.yaml` documents | [Declarative camel test](../testing/camel-test.md) |
| `camel job [<FILE>]` | Run or list jobs from `*.job.yaml` documents | [camel job](job.md) |
| `camel compile <DOCUMENT>` | Pack a route or job into a self-contained artifact | [camel compile](compile.md) |
| `camel plugin new` / `camel plugin build` | Scaffold and build WASM plugins | [OpenAPI and plugin subcommands](openapi-plugin.md) |
| `camel openapi generate <FILE>` | Emit an OpenAPI 3.0.3 document from `rest:` blocks | [OpenAPI and plugin subcommands](openapi-plugin.md) |
| `camel lsp` | Start the Language Server Protocol server over stdio | this page |

Flag definitions for the command enum live in `crates/camel-cli/src/main.rs`.

## camel lint

Lint a route file or a directory tree against the production component catalog. The command prints diagnostics with byte-exact spans.

```console
camel lint routes/my-route.yaml
camel lint routes/
```

Pass a directory to lint every route document below it. The walker collects `.yaml`, `.yml`, and `.json` files (extensions are case-sensitive). It prunes hidden entries and `target/` directories below the root, and never descends into symlinked directories; symlinked files are linted. Reserved documents (`*.test.yaml`, `*.test.yml`, `*.job.yaml`, `*.job.yml`) are skipped with an info line. Files are visited in deterministic depth-first order, sorted by name. The command prints one summary line on stdout: `linted N file(s), M skipped, F failed, K diagnostic(s)`.

Exit codes aggregate across the tree: 2 when the root cannot be read or any file fails to read, else 1 when any file has an Error diagnostic, else 0. Documents that are not route definitions are linted too, so they can raise `R-SCHEMA` errors, exactly as when named individually.

| Exit code | Outcome |
|-----------|---------|
| 0 | Clean. No Error diagnostics. |
| 1 | At least one Error diagnostic. |
| 2 | CLI misuse, or a directory root or file that cannot be read. |

Warnings do not change the exit code. For example, `camel lint` warns `R-MOCK-IN-PRODUCTION` on inline `to: mock:` sends in route files; see [Declarative camel test](../testing/camel-test.md). Reserved documents are skipped with an informational message: `*.test.yaml` test documents and `*.job.yaml` job documents.

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
