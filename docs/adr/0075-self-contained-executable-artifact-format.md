# ADR: Self-contained executable artifact format

- **Status:** Accepted — implemented through the expert-gated `cli-compile` change.
- **Date:** 2026-09-12
- **Supersedes:** None
- **Epic:** `rc-wp01c`

## Context

`camel run` discovers documents from the filesystem, then interpolates environment values, parses, lowers, boots components, and starts routes. A deployment preview needs one executable without making the target install a toolchain or source tree. The release matrix and immutable artifact workflow make an ELF-only section format unsuitable. Route definitions and processors are not a stable serializable artifact boundary, and pre-interpolation documents must retain `${env:}` expressions for deployment-time resolution.

## Decision

V1 uses a fixed 68-byte EOF trailer appended to a copy of the current native Linux executable. The exact footer is `CAMELTR1`, little-endian version/kind/reserved/length fields, BLAKE3, and terminal `CAMELTR1`; its checksum covers ASCII `rust-camel-trailer-v1`, one `0x00` byte, little-endian version/kind/length fields, payload, and manifest, excluding both magic fields and reserved byte. The payload is normalized document text captured before interpolation. A self-detect probe runs before Clap parsing; no recognizable footer means normal CLI behavior, while a marked footer with malformed, unsupported, or corrupt contents fails closed. If truncation removes the terminal marker, the image is indistinguishable from a trailer-free executable and follows normal CLI fallback. The embedded text re-enters the existing discovery/interpolation/lowering/runtime path. The artifact performs no extraction and disables watch.

V1 rejects unsupported compile-time assets explicitly. Runtime endpoint I/O and environment lookup remain deployment-time behavior. The manifest reports runtime version, kind, embedded components, required environment variables without defaults, and listener declarations that may be literal ports or unresolved expressions. BLAKE3 provides integrity, not authenticity; signing is deferred. V1 supports one route document or one job document; cross-compilation, AST/AOT serialization, compiled tests, multi-entry jobs, compression, and long-running route-server artifacts are deferred.

## Consequences

The format is small, dependency-light, and safe for read-only roots. The same executable remains both compiler template and runtime. Compiled artifacts are final immutable outputs; later strip or mutation invalidates the trailer. The artifact trust boundary is the shipped executable, not the deployment source tree. Boot still performs parse, interpolation, lowering, component boot, and context start, so no boot-speed claim is made before P0 measurement.
