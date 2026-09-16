# camel compile

`camel compile` packs one route or job document into a self-contained executable artifact. The artifact is a copy of the current Camel executable with an appended trailer that carries the documents. It runs on a target host without a source tree, route files, or a toolchain, and it performs no extraction.

The format is a native Linux preview: the only accepted target is the native Linux triple of the compiling executable.

> **Trust model.** The artifact is a copy of this Camel executable plus embedded authoring text. It runs with the deployment's process capabilities. Compile-time assets that the format does not support fail closed.

Authority: [ADR-0075](../adr/0075-self-contained-executable-artifact-format.md) and [`crates/camel-cli/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md).

## Usage

```console
camel compile routes.yaml -o my-routes
camel compile jobs/report.job.yaml -o nightly-report
camel compile routes.yaml -o my-routes --config Camel.toml --profile production
./my-routes                      # run the artifact
./my-routes --manifest           # print the embedded manifest, no boot
```

| Flag | Description |
|------|-------------|
| `<DOCUMENT>` (positional) | Route document (`*.yaml`/`*.yml`/`*.json`) or job document (`*.job.yaml`/`*.job.yml`) to embed as the entry point. |
| `-o`, `--output <ARTIFACT>` | Path of the executable artifact to write. |
| `--target <TRIPLE>` | Requested target triple. v2 accepts the native Linux triple only. |
| `--config <CONFIG>` | Explicit `Camel.toml` to resolve and embed (ordered includes, selected profiles, route patterns). Its directory becomes the confinement root. Without this flag, no configuration is embedded and none is discovered. |
| `--profile <NAME>` | Configuration profile to select. Repeatable, order preserved. Requires `--config`. |

Flag definitions live in `crates/camel-cli/src/commands/compile.rs`.

## What gets embedded

- **Without `--config`.** Only the entry-point document.
- **With `--config`.** The entry-point document plus the resolved configuration chain: ordered includes, the selected profiles, and the route patterns the document's source plan references. The config directory becomes the confinement root for these reads.

The compile-side policy rejects unsupported asset classes before writing anything:

- A `Camel.toml` in the compile working directory without an explicit `--config` fails. The artifact must not silently capture ambient configuration.
- `routeFilesFromRoot` in the entry document requires `--config`: the config directory is its anchor. Without the flag, compile fails.
- Any `CAMEL_*` environment variable present at compile time fails. Compile requires a clean environment.
- Nested documents embedded through the source plan may not declare further route sources.

The entry document's own `routeFiles` patterns are supported: compile resolves them relative to the document's directory (`routeFilesFromRoot` relative to the `--config` root), reads them under the confinement root, and embeds them as store entries. A v2 job artifact can therefore carry its route files inside the store.

Documents are normalized before embedding: UTF-8 only, one leading BOM removed, CRLF and lone CR converted to LF. The aggregate embedded payload is capped at 16 MiB.

## Artifact format (v2)

The encoded image is `CAMELTR1 || content || index || manifest || footer`:

- **Content.** The normalized bytes of every embedded document.
- **Index.** The canonical store index: one entry per document with its path, kind (`route`, `job`, `config`, `include`, `profile`), byte range, and references. The entry point names the `route` or `job` entry of the artifact's own kind.
- **Manifest.** A canonical JSON manifest (schema 2): artifact kind, entry-point name, runtime version, the component schemes used, the `${env:}` names without defaults, the listener endpoints, and one `embedded_files` entry per document with a BLAKE3 content digest.
- **Footer.** 76 bytes: the `CAMELTR1` magic, format version 2, kind, flags, section lengths, and a BLAKE3 checksum over the domain-separated sections.

Legacy v1 artifacts carry one document in a 68-byte footer format. The version-aware reader accepts both.

The write is atomic. Both the executable copy and the trailer go to a sibling `<output>.tmp` first, then the complete file is renamed onto the output path. A rejected or failed compile never truncates an existing artifact.

## Running an artifact

The artifact self-detects its trailer before any CLI parsing. Without the trailer magic it behaves as an ordinary `camel` executable. With a corrupt trailer it fails closed with an integrity diagnostic and exit code 2.

The artifact argument surface is deliberately narrow:

| Argument | Description |
|----------|-------------|
| (none) | Run the embedded route or job. |
| `--help` | Print artifact help and exit 0, without booting. |
| `--version` | Print version information and exit 0, without booting. |
| `--manifest` | Print the embedded manifest JSON and exit 0, without booting. |
| `--report <FILE>` | Write the run report to this path. |

`--arg` is unsupported and rejected as unknown. A job artifact resolves its declared arguments from the embedded declarations alone: declaration defaults apply, and a required argument without a default fails before boot with exit code 2.

A v2 run uses the merged embedded configuration and the ordered source-plan routes. `${env:}` tokens resolve against the deployment environment. The artifact reads no ambient `Camel.toml` and honors no `CAMEL_*` overrides. Watch mode is always off.

Exit codes for an artifact run: 0 for graceful completion (or a completed job); 1 for a failed job pipeline (job artifacts); 2 for argument misuse, validation, discovery, configuration, boot, or report-write failure.

## See also

- [`camel job`](job.md) for job documents and declared arguments.
- [Configuration](../configuration/index.md) for `Camel.toml`, includes, and profiles.
- [Jobs discovery](../configuration/jobs.md) for the source-tree counterpart of job discovery.

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md) | [ADR-0075](../adr/0075-self-contained-executable-artifact-format.md)
