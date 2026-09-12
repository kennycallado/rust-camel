# Proposal: cli-compile

## Why

Issue `rc-wp01c` needs a deployment artifact that runs a single Camel route or job without a source tree, route files, or a target-side toolchain. Current discovery depends on filesystem paths and glob expansion. This prevents a small production deployment from shipping one immutable executable and leaves `${env:}` semantics easy to misunderstand.

## What Changes

Add `camel compile <document> -o <artifact>` for a native Linux deployment preview. The command copies the current Camel executable and appends a fixed EOF trailer containing normalized pre-interpolation document text, an operational manifest, format metadata, and a mandatory BLAKE3 checksum. The executable detects its own trailer before normal CLI parsing and feeds the embedded text through the existing runtime discovery, interpolation, lowering, boot, and start path.

Compiled artifacts reject unsupported compile-time assets explicitly, preserve `${env:}` for artifact-time resolution, disable watch behavior, use no temporary extraction, and support read-only root filesystems. Artifact arguments are limited to `--report <path>`, `--help`, `--version`, and `--manifest`; other arguments exit 2. `--manifest` prints the operational manifest. Cross-compilation, AST/AOT serialization, compiled tests, multi-entry jobs, signing, and long-running route-server artifacts remain out of scope. V1 supports one route document or one job document.

Affected areas: `camel-cli`, `camel-dsl`, `CONTEXT-MAP.md`, `crates/camel-cli/CONTEXT.md`, and ADR 0075. The P0 benchmark owned by the fleet is a prerequisite for performance claims, not part of this implementation.

## Acceptance criteria

- `camel compile` produces a runnable native Linux single-document artifact with a valid trailer and manifest.
- Marked trailer corruption, unsupported version, and unsupported assets fail closed with named diagnostics; terminal-marker loss is indistinguishable from a trailer-free executable and follows normal CLI fallback.
- Build-time environment values never appear because payload capture is pre-interpolation; artifact-time environment resolution remains unchanged.
- Trailer-free binaries retain normal CLI behavior; artifact argument and exit-code rules are deterministic.
- Artifact runtime performs no extraction or writes except an explicitly requested report.
- Unit, CLI, and focused integration tests cover codec integrity, compile rejection, self-detection, runtime interpolation, manifest output, and read-only-root behavior.
- ADR 0075 and cross-cutting context terms document the format and trust boundary.

## Risk budget

Accept format-agnostic append-only trailer behavior for the native Linux preview and unchanged downstream route boot semantics. Do not add compression, a serialization format, a sandbox, cross-compilation, signing, or a second runtime. Checksum provides integrity only, not authenticity. Do not claim boot-speed improvement before P0 evidence lands.
