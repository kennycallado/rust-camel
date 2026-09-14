# ADR: Self-contained executable artifact format

- **Status:** Accepted — implemented through the expert-gated `cli-compile` change. Amended 2026-09-14: v2 virtual-store framing (`multidoc` change).
- **Date:** 2026-09-12
- **Supersedes:** None
- **Epic:** `rc-wp01c`

## Context

`camel run` discovers documents from the filesystem, then interpolates environment values, parses, lowers, boots components, and starts routes. A deployment preview needs one executable without making the target install a toolchain or source tree. The release matrix and immutable artifact workflow make an ELF-only section format unsuitable. Route definitions and processors are not a stable serializable artifact boundary, and pre-interpolation documents must retain `${env:}` expressions for deployment-time resolution.

## Decision

V1 uses a fixed 68-byte EOF trailer appended to a copy of the current native Linux executable. The exact footer is `CAMELTR1`, little-endian version/kind/reserved/length fields, BLAKE3, and terminal `CAMELTR1`; its checksum covers ASCII `rust-camel-trailer-v1`, one `0x00` byte, little-endian version/kind/length fields, payload, and manifest, excluding both magic fields and reserved byte. The payload is normalized document text captured before interpolation. A self-detect probe runs before Clap parsing; no recognizable footer means normal CLI behavior, while a marked footer with malformed, unsupported, or corrupt contents fails closed. If truncation removes the terminal marker, the image is indistinguishable from a trailer-free executable and follows normal CLI fallback. The embedded text re-enters the existing discovery/interpolation/lowering/runtime path. The artifact performs no extraction and disables watch.

V1 rejects unsupported compile-time assets explicitly. Runtime endpoint I/O and environment lookup remain deployment-time behavior. The manifest reports runtime version, kind, embedded components, required environment variables without defaults, and listener declarations that may be literal ports or unresolved expressions. BLAKE3 provides integrity, not authenticity; signing is deferred. V1 supports one route document or one job document; cross-compilation, AST/AOT serialization, compiled tests, multi-entry jobs, compression, and long-running route-server artifacts are deferred.

## Amendment: v2 virtual-store framing (2026-09-14)

The `multidoc` change adds trailer version 2. It keeps the `CAMELTR1` family marker. A v2 artifact is `CAMELTR1 || content || index || manifest || footer`. The footer is 76 bytes: `CAMELTR1`, little-endian `u16` version 2, `u8` kind (`1=route`, `2=job`), zero `u8` flags, little-endian `u64` content/index/manifest lengths, BLAKE3, and terminal `CAMELTR1`. The checksum covers ASCII `rust-camel-trailer-v2`, one zero byte, the encoded version/kind/three lengths, content, index, and manifest. It excludes both magic fields and the flags byte.

The content packs a virtual document store: every compile-time document (routes, jobs, `Camel.toml`, includes, selected profiles) as normalized pre-interpolation text, plus a canonical UTF-8 JSON index. Schema ownership is explicit and separate. The index carries `store_schema` (version 1); the manifest carries `manifest_schema` (version 2). The trailer version describes framing only. It never defines store or manifest schema. Readers validate each schema independently and reject unsupported values before boot.

The compiler selects sources only through the explicit `--config <Camel.toml>` and repeated `--profile <name>` options. Without `--config`, it embeds no configuration and selects no profile, and it never discovers ambient configuration. Every resolved source stays inside the selected root. Names normalize to UTF-8 relative `/` paths. Absolute paths, empty or `.` or `..` components, non-UTF-8 names, symlink escapes, duplicate canonical targets, duplicate logical paths, and out-of-root references fail before the output is created. The aggregate normalized embedded bytes stay capped at 16 MiB.

Compatibility stays within the family. A v2 reader decodes a v1 artifact as a one-entry store and preserves v1 behavior. A v1 reader recognizes the marker, does not parse v2, and may reject it as unsupported. This is a safe failure: the marked artifact is never mistaken for a trailer-free executable.

R1 keeps one logical entry point even though the store carries many documents. R3 may extend entry-point cardinality on this store without changing R1 runtime semantics, and R2 (deploy-time asset embedding) may add embedded files to it. Compression, signing, and cross-target compilation stay deferred. The runtime wall is unchanged: no source reads, no globbing, no ambient `Camel.toml`, no extraction, no watch, and artifact arguments stay limited to `--report`, `--help`, `--version`, `--manifest`, and the sanctioned R4 signature-verification surface.

## Consequences

The format is small, dependency-light, and safe for read-only roots. The same executable remains both compiler template and runtime. Compiled artifacts are final immutable outputs; later strip or mutation invalidates the trailer. The artifact trust boundary is the shipped executable, not the deployment source tree. Separate store and manifest schemas let the packed-document model and the operational manifest evolve independently of the trailer framing; unknown schema values fail closed. Boot still performs parse, interpolation, lowering, component boot, and context start, so no boot-speed claim is made before P0 measurement.
