# camel compile

`camel compile` packs one route or job document into a self-contained executable artifact. The artifact is a copy of the current Camel executable with an appended trailer that carries the documents. It runs on a target host without a source tree, route files, or a toolchain, and it performs no extraction; the TLS, xslt, xsd, and sql classes materialize into a confined per-boot directory under the system temp directory.

The format is a native Linux preview: the only accepted target is the native Linux triple of the compiling executable.

> **Trust model.** The artifact is a copy of this Camel executable plus embedded authoring text. It runs with the deployment's process capabilities. Compile-time assets that the format does not support fail closed.

Authority: [ADR-0075](../adr/0075-self-contained-executable-artifact-format.md) and [`crates/camel-cli/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md).

## Usage

```console
camel compile routes.yaml -o my-routes
camel compile jobs/report.job.yaml -o nightly-report
camel compile routes.yaml -o my-routes --config Camel.toml --profile production
camel compile routes.yaml -o my-routes --sign --signing-key key.bin
./my-routes                      # run the artifact
./my-routes --manifest           # print the embedded manifest, no boot
./my-routes --verify             # verify the signature envelope, no boot
```

| Flag | Description |
|------|-------------|
| `<DOCUMENT>` (positional) | Route document (`*.yaml`/`*.yml`/`*.json`) or job document (`*.job.yaml`/`*.job.yml`) to embed as the entry point. |
| `-o`, `--output <ARTIFACT>` | Path of the executable artifact to write. |
| `--target <TRIPLE>` | Requested target triple. v2 accepts the native Linux triple only. |
| `--config <CONFIG>` | Explicit `Camel.toml` to resolve and embed (ordered includes, selected profiles, route patterns). Its directory becomes the confinement root. Without this flag, no configuration is embedded and none is discovered. |
| `--profile <NAME>` | Configuration profile to select. Repeatable, order preserved. Requires `--config`. |
| `--sign` | Sign the complete final artifact bytes and write `<ARTIFACT>.sig` beside the artifact. |
| `--signing-key <PATH>` | File with the 32-byte Ed25519 seed. Requires `--sign`. The `CAMEL_COMPILE_SIGNING_KEY` environment variable is the fallback; the flag wins when both are present. |
| `--require-signature` | Record in the signed manifest that boot must verify an envelope. Requires `--sign`. |

Flag definitions live in `crates/camel-cli/src/commands/compile.rs`.

## What gets embedded

- **Without `--config`.** Only the entry-point document.
- **With `--config`.** The entry-point document plus the resolved configuration chain: ordered includes, the selected profiles, and the route patterns the document's source plan references. The config directory becomes the confinement root for these reads.

The compile-side policy rejects unsupported asset classes before writing anything:

- A `Camel.toml` in the compile working directory without an explicit `--config` fails. The artifact must not silently capture ambient configuration.
- `routeFilesFromRoot` in the entry document requires `--config`: the config directory is its anchor. Without the flag, compile fails.
- Any `CAMEL_*` environment variable present at compile time fails. Compile requires a clean environment. The one exception is `CAMEL_TRUSTSTORE`, which is benign at compile time (see [Signing artifacts](#signing-artifacts)).
- Nested documents embedded through the source plan may not declare further route sources.

The entry document's own `routeFiles` patterns are supported: compile resolves them relative to the document's directory (`routeFilesFromRoot` relative to the `--config` root), reads them under the confinement root, and embeds them as store entries. A v2 job artifact can therefore carry its route files inside the store.

Documents are normalized before embedding: UTF-8 only, one leading BOM removed, CRLF and lone CR converted to LF. The aggregate embedded payload is capped at 16 MiB.

## Artifact format (v2)

The encoded image is `CAMELTR1 || content || index || manifest || footer`:

- **Content.** The normalized bytes of every embedded document.
- **Index.** The canonical store index: one entry per document with its path, kind (`route`, `job`, `config`, `include`, `profile`), byte range, and references. The entry point names the `route` or `job` entry of the artifact's own kind.
- **Manifest.** A canonical JSON manifest (schema 3; a signed artifact carries schema 5): artifact kind, entry-point name, runtime version, the component schemes used, the `${env:}` names without defaults, the listener endpoints, and one `embedded_files` entry per document with a BLAKE3 content digest.
- **Footer.** 76 bytes: the `CAMELTR1` magic, format version 2, kind, flags, section lengths, and a BLAKE3 checksum over the domain-separated sections.

Legacy v1 artifacts carry one document in a 68-byte footer format. The version-aware reader accepts both.

The write is atomic. Both the executable copy and the trailer go to a sibling `<output>.tmp` first, then the complete file is renamed onto the output path. A rejected or failed compile never truncates an existing artifact.

## Signing artifacts

`--sign` writes a detached signature envelope beside the artifact: `<ARTIFACT>.sig`, exactly 148 bytes. The envelope carries the public key, the signature, and a BLAKE3 envelope checksum. It never carries key material.

The signature is Ed25519ph (RFC 8032 prehash). It covers the complete final artifact bytes: the executable copy plus the trailer. The compiler hashes that stream once while it writes the artifact, so signing does not re-read or buffer the artifact. Verification streams the artifact the same way.

The key file holds exactly 32 bytes: an Ed25519 seed. Supply it with `--signing-key <PATH>` or the `CAMEL_COMPILE_SIGNING_KEY` environment variable. The flag wins when both are present. `--sign` with no key source exits 2. Compile still rejects every other `CAMEL_*` variable except the benign `CAMEL_TRUSTSTORE`, and rejects a stray `CAMEL_COMPILE_SIGNING_KEY` when `--sign` is absent.

A signed compile moves the manifest to schema 5 and adds a `signing` block: the algorithm (`ed25519ph`), the `key_fingerprint` (`blake3:` plus 64 lowercase hex characters over the 32-byte public key), a `required` boolean, and a mandatory `freshness` marker (a u64 unix-seconds value, signed with the manifest bytes). Schema 4 artifacts (pre-keypin compiles) remain readable. Unsigned compiles stay schema 3 and stay byte-identical. The private seed never appears in the artifact, the envelope, the manifest, or a log.

`--require-signature` sets `required: true`. At boot, a required signature that is absent exits 2. Without the flag, a present envelope is still verified at boot, but an unsigned artifact without an envelope still boots (a supplied truststore is stricter; see below).

Verify an artifact without booting:

```console
./my-routes --verify
```

Exit 0 prints two lines: `algorithm: ed25519ph` and `key_fingerprint: blake3:<hex>`. Any failure exits 2 and names the failing step: envelope, fingerprint, signature, or truststore. `--verify` accepts `--truststore` and is exclusive with every other artifact argument.

**Pinning a producer.** A truststore pins the keys an artifact may verify against. Supply it with the artifact argument `--truststore <path>` or the `CAMEL_TRUSTSTORE` environment variable; the argument wins. `--truststore` is a modifier, not a mode: it pairs with a boot and with `--verify`, and `--help`, `--version`, and `--manifest` reject it. The variable is benign at compile time. The file holds one pin per line, a `blake3:` value plus 64 lowercase hex characters, with an optional decimal floor column; `#` comments and blank lines are ignored, and malformed input fails closed.

Under a supplied truststore:

- The manifest `key_fingerprint` must be pinned; an unpinned key exits 2 with `truststore-pin`.
- A manifest signing block without its envelope exits 2 (the strip rule), whether or not the manifest marked the signature required.
- A pinned schema-5 artifact whose freshness marker is below the recorded floor exits 2 with `freshness-rollback`; a pinned schema-4 artifact below an existing floor fails the same way (it carries no marker).
- Boot records each accepted key's floor in the truststore under an advisory lock on `<truststore>.lock`, merging maxima, so a floor never decreases.
- `--verify` is a dry run: it applies the same policy and never writes.

`--manifest` (and `--help`/`--version`) also run the boot-side trust policy: on first sight of a pinned key they record its floor, and a rolled-back artifact fails them too. Authority: [ADR-0083](../adr/0083-artifact-signing-envelope.md).

## Running an artifact

The artifact self-detects its trailer before any CLI parsing. Without the trailer magic it behaves as an ordinary `camel` executable. With a corrupt trailer it fails closed with an integrity diagnostic and exit code 2.

The artifact argument surface is deliberately narrow:

| Argument | Description |
|----------|-------------|
| (none) | Run the embedded route or job. |
| `--help` | Print artifact help and exit 0, without booting. |
| `--version` | Print version information and exit 0, without booting. |
| `--manifest` | Print the embedded manifest JSON and exit 0, without booting. |
| `--verify` | Verify the signature envelope and exit 0, without booting. Accepts `--truststore`; exclusive with every other artifact argument. |
| `--truststore <PATH>` | Deployment truststore pin file for the boot-side trust policy. A modifier: pairs with a boot and with `--verify`, and is rejected by `--help`, `--version`, and `--manifest`. `CAMEL_TRUSTSTORE` is the fallback; the argument wins. |
| `--report <FILE>` | Write the run report to this path. |

`--arg` is unsupported and rejected as unknown. A job artifact resolves its declared arguments from the embedded declarations alone: declaration defaults apply, and a required argument without a default fails before boot with exit code 2.

A v2 run uses the merged embedded configuration and the ordered source-plan routes. `${env:}` tokens resolve against the deployment environment. The artifact reads no ambient `Camel.toml` and honors no `CAMEL_*` overrides. Watch mode is always off.

Exit codes for an artifact run: 0 for graceful completion (or a completed job); 1 for a failed job pipeline (job artifacts); 2 for argument misuse, validation, discovery, configuration, boot, or report-write failure.

A route artifact runs like `camel run --no-watch`: it binds every listener its embedded documents and configuration declare and serves until the first SIGINT/SIGTERM. The first signal starts teardown: in-flight work drains within the configured drain budget (`drain_timeout_ms`, default 10 s) and the process exits 0. A second signal during teardown force-exits 1. Job artifacts stay bounded and never serve (bd `rc-zs7au`).

## TLS in compiled artifacts

The bootable TLS shape is the `https://` URI parameters `tlsCert` and `tlsKey`: the listener parses the materialized PEMs at boot. Route-level `tls:` blocks (document fields) compile, embed, and materialize — the compile-side promise holds — but a route-level `tls:` key is an unknown DSL field at discovery, so an artifact whose route carries one fails at discovery and never boots. A bootable client-CA site does not exist yet, because the `https` listener has no URI-parameter site for one; a listener TLS authoring shape is future work ([ADR-0075](../adr/0075-self-contained-executable-artifact-format.md) R2 amendment, bd `rc-7mzdu`).

## See also

- [`camel job`](job.md) for job documents and declared arguments.
- [Configuration](../configuration/index.md) for `Camel.toml`, includes, and profiles.
- [Jobs discovery](../configuration/jobs.md) for the source-tree counterpart of job discovery.

**Reference**: [CLI crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-cli/CONTEXT.md) | [ADR-0075](../adr/0075-self-contained-executable-artifact-format.md)
