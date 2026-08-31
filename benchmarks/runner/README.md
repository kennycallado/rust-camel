The runner's identity is the image digest recorded in `DIGEST` (see `pin.sh`); tags are convenience labels for local use only and never appear in records or canonical run configuration — every canonical run consumes the `sha256:` digest from `DIGEST`.

## Digest pins

`pin.sh` is the single source of truth for the literals below; the Dockerfile `ARG` defaults mirror them so a bare `docker build` still works. `bash benchmarks/runner/pin.sh --report` prints the record without building.

| Pin | Value | Artifact |
| --- | --- | --- |
| `NODE_VERSION` | `22.14.0` | <https://nodejs.org/dist/v22.14.0/node-v22.14.0-linux-x64.tar.gz> |
| `NODE_SHA256` | `9d942932535988091034dc94cc5f42b6dc8784d6366df3a36c4c9ccb3996f0c2` | sha256 of the tarball above |
| `DOCKER_CLI_VERSION` | `28.5.2` | <https://download.docker.com/linux/static/stable/x86_64/docker-28.5.2.tgz> (static CLI; upstream publishes no `.sha256` — hash computed from the official artifact) |
| `DOCKER_CLI_SHA256` | `ea90cfd12e1eeb12aa1c971741adb8bd4ed88e2a574eaac13f5029a1dbc6300d` | sha256 of the tgz above |

The Node tarball is SHA256-verified at image build BEFORE extraction (fail closed on mismatch) — stronger than the JVM toolchains, whose Maven/Gradle archives get only the implicit format check of extraction (a corrupt archive fails to untar, but no digest is compared).
