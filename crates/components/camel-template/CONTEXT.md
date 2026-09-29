External template component for rust-camel (ADR-0047 Stage 2). It renders
MiniJinja templates from the filesystem against the body and headers of each
inbound exchange. Producer-only: it sits on the `to:` side of a route.

## Language

**TemplateComponent**:
Component for `template:file:///<absolute-path>` URIs. It compiles the
template dependency closure once at route startup and swaps the compiled
set on reload. Compilation failure prevents the route from starting
(fail-closed). A render failure keeps the original exchange body.
_Avoid_: template engine, jinja component

**ExternalTemplateLimitsConfig**:
Operator-configured acquisition limits: total source bytes, include count
and depth, single-template size, and the reload wall-clock timeout. The
template source and root directory are set at startup. No exchange header
or property can override them (zero-override).
_Avoid_: template options, template settings

**ReloadTemplates**:
Control-plane command. It re-acquires the dependency closure, recompiles
it, and swaps the compiled set atomically. A failed reload keeps the prior
set. In-flight renders are not disturbed.
_Avoid_: refresh, recompile trigger

**Template confinement**:
All file reads go through `openat`-relative handles. `..` segments,
symlinks, absolute paths, and cycles are rejected. Bare paths and
non-`file:` inner schemes are rejected at endpoint construction.
_Avoid_: sandboxing (the confinement is filesystem-path-only)

Render limits (context size, output size, fuel, recursion depth, execution
timeout) are inherited from the MiniJinja language engine
(`MinijinjaLimitsConfig`). Authority: ADR-0047.
