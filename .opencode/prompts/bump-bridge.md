### Bridge version bump prompt

`bridges/versions.toml` is the single version source for the `cxf`, `jms`, and
`xml` bridges. The tag `{bridge}-bridge-vX.Y.Z` is the release trigger for
`.github/workflows/{bridge}-bridge-release.yml`, which builds the native binaries
and publishes the GitHub Release.

**Bridge → registry and constants:**

| Bridge | Registry key | `BRIDGE_VERSION` const(s) | Tag |
|--------|--------------|---------------------------|-----|
| `cxf`  | `cxf` | `crates/components/camel-cxf/src/lib.rs` | `cxf-bridge-vX.Y.Z` |
| `jms`  | `jms` | `crates/components/camel-jms/src/lib.rs` | `jms-bridge-vX.Y.Z` |
| `xml`  | `xml` | `crates/components/camel-validator/src/lib.rs` **and** `crates/components/camel-xslt/src/lib.rs` | `xml-bridge-vX.Y.Z` |

> `xml` feeds **two** Rust components (validator XSD mode + xslt) — its single
> registry key drives both constants, and both constants move together.

**Procedure (per bridge; OLD=A.B.C → NEW=X.Y.Z):**

1. Run `cargo xtask bump-bridge <bridge> <version>`. It atomically writes
   `bridges/versions.toml` and the registered `BRIDGE_VERSION` constant(s) — for
   `xml` both constants — and restores every target byte-for-byte on failure.
2. Run `cargo xtask lint-bridge-versions`; require a clean exit before
   committing.
3. Commit (`caveman-commit`): `chore(bridge): bump version OLD to NEW`
   - Body only if non-obvious; reference the tracked issue at the end
     (`Bd: rc-bxjzq`).
4. Stop.

The agent never tags, pushes, or triggers a release.
The human creates and pushes the tag {bridge}-bridge-vX.Y.Z.
The tag push is what starts the release workflow.

## After the tag is pushed (human-run verification)

1. Run `cargo xtask lint-bridge-versions --check-releases`; it must exit 0.
2. Confirm the release carries every matrix asset:
   - `{bridge}-bridge-{version}-linux-musl-x86_64.tar.gz`
   - `{bridge}-bridge-{version}-macos-aarch64.tar.gz`
   - `{bridge}-bridge-{version}-windows-x86_64.zip`
   - `{bridge}-bridge-{version}-SHA256SUMS`
3. Confirm the published tag equals the registry version.

`--check-releases` fails before the release exists. That is expected and is not a bump failure.
The bump is not complete until the release exists and the check is green.

The published JMS binary is smoke-tested by
`.github/workflows/published-jms-bridge-smoke.yml`; use it as the artifact check.
