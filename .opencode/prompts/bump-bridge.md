### Bridge version bump prompt

Bump the version of one or more Java bridges (`cxf`, `jms`, `xml`). Each bridge is
versioned and released **independently** via tag `{bridge}-bridge-vX.Y.Z`, which triggers
`.github/workflows/{bridge}-bridge-release.yml` (native binary build → GitHub Release).

**Bridge → files to bump:**

| Bridge | gradle default version | `BRIDGE_VERSION` const(s) | Tag |
|--------|------------------------|---------------------------|-----|
| `cxf`  | `bridges/cxf/build.gradle.kts:7` | `crates/components/camel-cxf/src/lib.rs:32` | `cxf-bridge-vX.Y.Z` |
| `jms`  | `bridges/jms/build.gradle.kts:7` | `crates/components/camel-jms/src/lib.rs:38` | `jms-bridge-vX.Y.Z` |
| `xml`  | `bridges/xml/build.gradle.kts:7` | `crates/components/camel-validator/src/lib.rs:20` **and** `crates/components/camel-xslt/src/lib.rs:20` | `xml-bridge-vX.Y.Z` |

> `xml` feeds **two** Rust components (validator XSD mode + xslt) — bump **both** consts.
> The JMS const was the one missed in the 0.3.0→0.4.0 bump (fixed in `cc7951d3`); always
> confirm all 4 sites when doing a lockstep bump of the three bridges.

**Steps (per bridge; OLD=A.B.C → NEW=X.Y.Z):**

1. Update the gradle default version (line 7, the `?: "OLD"` literal — do **not** touch the
   plugin versions on lines 3-4):
   `sed -i 's/?: "OLD"/?: "NEW"/' bridges/{bridge}/build.gradle.kts`
2. Update `pub const BRIDGE_VERSION: &str` in the crate `lib.rs` file(s) listed above.
3. Sanity check: `cargo build -p camel-{bridge}` (for `xml`: `-p camel-validator -p camel-xslt`).
4. Commit (`caveman-commit`): `chore(bridge): bump version OLD to NEW`
   - Body only if non-obvious; reference `Bd: rc-XXXX` at end.
5. Local tag: `git tag {bridge}-bridge-vX.Y.Z`

**Do NOT `git push`.** The human pushes the tag to trigger the release workflow.
