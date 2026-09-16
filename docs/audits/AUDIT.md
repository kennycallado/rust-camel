# Auditoría de Fidelidad y Calidad — rust-camel v0.x → v1.0.0

> **Source of truth (v4.2).** Este documento es el prompt maestro y la base de datos de seguimiento de la auditoría. **Cualquier agente sin contexto previo puede continuar la auditoría leyendo solo este archivo** (más los tracking tables al final).

> **▶ AUDITORÍA REANUDADA (2026-08-04).** La pausa por rc-iq7 se levantó: rc-iq7 cerró 2026-06-26 (epic completo, final `00c5f6ef`, incl. ADR-0026 + ADR-0017 amend). Todos los crates T1 antes `deferred/blocked` son ahora auditables. v4.2 añade **lens L7** (behavioral parity gate, ADR-0046). Source of truth sigue siendo este archivo.

> **Versión v4.2 (e_opus ronda-2 reconciliation, 2026-08-04).** Cambios: (1) lens **L7** — behavioral parity gate que invoca el Protocolo de ADR-0046 (Apache Camel = inspiración, no conformance); (2) finding class `FC-BEHAVIORAL-PARITY-GAP`; (3) cierre del blind spot #10; (4) reset de crates T1 `deferred/blocked` tras cierre de rc-iq7; (5) camel-core marcado `covered-by-rc-d0pu` + L7 delta. Flujo sin cambio: audit trabaja en main; oráculo aprueba proposals Y las materializa+commitea; sin fixer agent separado; cláusula tests+target dir compartido.

## Propósito

El proyecto entra en fase de **estabilización hacia v1.0.0**. Aún faltan features y fixes, pero necesitamos **marcar el camino**: detectar tempranamente (a) drift de las premisas fundacionales (ADRs) y (b) degradación de calidad producida por el ritmo de entrega reciente.

Esta auditoría es **observacional**: el audit no toca código del repo. Solo documenta hallazgos y proposals. La única excepción: el **oráculo** materializa y commitea las documentation proposals aprobadas (ADRs y CONTEXT.md updates).

## Alcance

**Todos los crates del workspace** (~55 crates), más `bridges/`, `xtask/`, y `examples/` como secundario.

### Tiers

| Tier | Criterio | Crates |
|---|---|---|
| **T1 Critical** | Carga arquitectónica; drift duele más | camel-core, camel-api, camel-processor, camel-dsl, camel-config, camel-cli, camel-builder |
| **T2 Important** | Churn reciente / dependencia externa / lógica sustativa | camel-health, camel-endpoint, camel-endpoint-macros, camel-bean, camel-bean-macros, camel-test, camel-auth, camel-function, camel-otel, camel-component-api, camel-component-llm, camel-component-wasm, camel-component-grpc, camel-component-surrealdb, camel-component-keycloak, camel-jms, camel-http, camel-kafka, camel-sql, camel-opensearch, camel-redis, camel-bridge, camel-prometheus, camel-cxf, camel-xslt, camel-xj, camel-language-js, camel-language-rhai, camel-language-jsonpath, camel-language-xpath |
| **T3 Simple** | Componentes/utiles pequeños | camel-log, camel-mock, camel-timer, camel-direct, camel-file, camel-master, camel-validator, camel-ws, camel-controlbus, camel-container, camel-dataformat-protobuf, camel-language-api, camel-language-simple, camel-platform-kubernetes, camel-proto-compiler, camel-bench, camel-wit, camel-component-seda |

### Orden de ejecución (T1-first, optimiza riesgo v1.0)

1. **T1 en orden**: camel-processor → camel-core → camel-api → camel-dsl → camel-config → camel-cli → camel-builder.
2. **T2 en batches 2-3** con `dispatching-parallel-agents`.
3. **T3 en batches 3-5** con `dispatching-parallel-agents`.

## Skills involucradas

- **`thermo-nuclear-code-quality-review`** (full): abstracciones honestas, giant files, spaghetti. **Aplicar sin filtro.**
- **`ponytail`** (lite): cuestiona YAGNI, reach for stdlib antes que custom.
- **`self-grill-proposals`** (disponible en `~/.agents/skills/self-grill-proposals/`): copia no-interactiva de `grill-with-docs`. El auditor la invoca en paso final para refinar proposals L6 contra el domain model antes de enviarlas al oráculo.

### ⚠ Caveat ponytail

rust-camel busca parity con Apache Camel 4.x. Components/EIPs/languages individuales son **parity-driven** — NO son YAGNI violations.

Ponytail aplica a: abstracciones internas, boilerplate, dependencias overreach, over-engineering en factores internos.

Ponytail NO aplica a: componentes/EIPs/languages individuales.

## Lenses por tipo de crate (OBLIGATORIOS)

Cada lens es una checklist. El auditor aplica todos los lenses relevantes al crate (mencionar en "Lens observations").

### L1 — API stability / semver v1.0
Aplica a crates con API pública, feature flags, URI options. ¿Tipos marcados estables? ¿`#[non_exhaustive]` donde v1.0 lo amerita? ¿Breaking changes sin bump? ¿Paridad con Apache Camel 4.x?

### L2 — Concurrency / runtime safety
Aplica a crates con async/await, spawned tasks, `poll_ready`, `ArcSwap`, `CancellationToken`. ¿Shutdown respeta in-flight (ADR-0004)? ¿`poll_ready` incondicional `Ready(Ok(()))` donde ADR-0019 aplica? ¿`Consumer::stop()` no en crash path (ADR-0007)? ¿Spawned tasks con supervision?

### L3 — Security
Aplica a auth/http/sql/jms/kafka/redis/wasm. ¿Credenciales redacted en `Debug`? ¿Authz pre-pipeline (ADR-0010)? ¿SQL parameterized? ¿Trust boundaries?

### L4 — Performance hot-path
Aplica a processors, routing loops, allocation-heavy. ¿Allocations por Exchange? ¿Locks contended? ¿`Clone` innecesario? ¿Async blocks sin `Send`?

### L5 — Dependency boundary
Aplica a crates con deps externas churnosas (siumai, sqlx, kafka). ¿Dep confinada a N archivos (ej: siumai → 2+1, ADR-0020)? ¿Test de boundary enforced? ¿Feature leakage?

### L6 — Architectural documentation coherence (siempre, default)
Aplica a TODOS los crates. ¿CONTEXT.md existe y al día? ¿Decisiones implícitas merecen ADR? ¿ADRs citados correctamente? ¿Drift con CONTEXT-MAP? ¿README coincide con código?

> **L6 proposals van al oráculo, que las materializa + commitea.** Ver "Oracle workflow" abajo.

### L7 — Behavioral parity gate (Apache Camel test mining, gated by ADR-0046)

> **L7 es un GATE, no un método.** Toda la metodología (dosis 3-tests, tests nativos no traducidos, KPI divergencias/EIP, 5 anti-patrones) vive en **ADR-0046 §Decision "Protocolo de consulta"**. L7 solo decide SI disparar y dónde va el resultado. **No dupliques** la dosis/KPI/anti-patrones aquí — cítalos. Resuelve el blind spot #10.

**Aplicabilidad:** L7 aplica solo a crates que **implementan EIPs** (camel-processor, camel-core, camel-bean, componentes con lógica EIP). Crates de infraestructura pura (macros, config, cli, schemas) → **L7 N/A**.

**Trigger (por EIP del crate, no por crate):** para cada EIP que el crate implementa, clasifícalo:

- **Stateless casi-idéntico a Camel** (Filter, Content-Based Router, Throttle, SetBody/SetHeader, similares sin estado ni temporización): **L7 = N/A.** Coverage puro. **NO leer Apache Camel.** Registrar "L7 N/A (stateless)".
- **≥1 marcador de divergencia ADR-0046** (stateful; completion/timeout; control-flow divergente ADR-0019/0024/0025; trust-boundary ADR-0032; backpressure/admission ADR-0044): aplicar la **regla de dos verbos**:
  1. **VERIFICAR (default, barato):** ¿las divergencias conocidas del EIP están pineadas en un tracked doc (ADR / crate `CONTEXT.md` / bd con label `divergence`|`pin-invariant`)? Si SÍ → registrar "L7 parity verified, N divergencias tracked", **cero lectura de Camel**.
  2. **ESCALAR (solo si hueco genuino):** si el EIP es divergente PERO ninguna divergencia está documentada → **NO resolver inline.** Emitir finding `FC-BEHAVIORAL-PARITY-GAP`, abrir `bd create "<EIP> parity gap" --deps discovered-from:rc-ca8z` (label `gap-coverage`) y encolar el **Protocolo ADR-0046 completo** (dosis 3-tests, tests nativos, KPI) como trabajo derivado post-audit, resuelto con oráculo. **El audit DETECTA; el Protocolo RESUELVE.**

**Scope guard (honra ADR-0046 §no-retroactivo) — la no-retroactividad exime lo CARO, no lo BARATO.** La **detección** (verbos 1+2) aplica a **TODOS los EIPs sin importar edad**. L7 nunca re-diseña una divergencia ya decidida ni re-abre un ADR estable; solo verifica el pin (verb 1) o detecta su ausencia (verb 2). El **dating** (pre/post ADR-0046, aceptado `2026-07-17` `7cfd5fe7`) es paso requerido PERO determina **CÓMO se resuelve** un hueco, no **SI se detecta**: EIP **post**-ADR-0046 sin divergencia documentada → GAP + Protocolo **obligatorio**; EIP **pre**-ADR-0046 sin divergencia documentada → **SIGUE siendo GAP**, resolución **voluntaria** (documentar desde comportamiento existente, **sin leer Camel**). Leer Apache Camel es exclusivo del verbo 2 (escalación). **⚠ Failure mode conocido (re-run camel-processor 2026-08-04):** aplicar la edad como escudo blanket ("pre-ADR-0046 → 0 GAPs") produce **falsos 0-GAPs** — el auditor lo hizo y era incorrecto; dating sostenido ≠ ausencia de gaps de documentación. Ver blind spot #16. **Resolución voluntaria (pre-ADR-0046) = cosificar el comportamiento existente en tracked doc; NUNCA implica leer Camel ni abrir bd `gap-coverage` con Protocolo obligatorio** (anti-patrón #4 ADR-0046). Ejemplar canónico: DP-4 §"Stateful repository EIPs".

**Riesgo vigilado:** L7 convirtiéndose en el `cargo xtask port-camel-test` rechazado por ADR-0046 (verde inválido + rojo espurio + pérdida de invariantes, demostrado por el spike `rc-spt-camel-splitter-spike` `8d31e74a`). Mitigación: default = cero lectura de Camel; stateless = N/A; el audit nunca porta inline.

**Output (inline en el reporte del crate):** cada EIP produce una línea:
`<EIP> | L7: {N/A-stateless | verified(N div) | GAP→bd-xxx}`. Los GAP alimentan la finding class `FC-BEHAVIORAL-PARITY-GAP`.

## Roles — v4.2

| Rol | Agente | Función |
|---|---|---|
| **AUDITOR** | `reviewers/r_glm5.2` | Análisis semántico profundo en main: lee código + ADRs + CONTEXT.md, aplica skills + lenses L1-L7 (L7 solo si el crate implementa EIPs), hace `cargo check` + reproducers + test-file inspection, produce reporte con findings + quotes + evidence + **documentation proposals L6** (preliminares, con self-grill manual o skill). |
| **VALIDATOR** | `workers/w_deep4-flash` | Verificación mecánica en main: spot-check 5+ citations, ejecuta tests reales (con target dir dedicado + cláusula 2/2 + grep Blocking lock), hace **negative search** (T1/T2), valida 9 reglas anti-falso-positivo, aprueba/desviación. |
| **ORACLE** | `experts/e_gpt` / `e_glm` / `e_opus` | Gatekeeper arquitectural. Aprueba/rechaza/needs-info cada proposal L6 con **contenido final literal** (diff/ADR body completo). **Materializa los cambios** (edita CONTEXT.md / crea/edita ADR) y **commitea** (porque `docs/adr/*.md` y `crates/<crate>/CONTEXT.md` están tracked). |
| **ORQUESTADOR** (tú) | main agent | Enruta: lanza auditor, lanza validator, acumula proposals, lanza oracle calls. **DEBE despachar al oráculo al cerrar la sesión si hay proposals pendientes** (≥3/fin-de-tier es GUÍA, no gate — ver blind spot #17). **No edita archivos**: el oráculo materializa + commitea. |

## Workflow por módulo

```
┌─────────────────────────────────────────────────────────┐
│ AUDITOR (r_glm5.2) — trabaja en main                     │
│  1. Lee AUDIT.md + AGENTS.md + CONTEXT-MAP               │
│  2. Lee CONTEXT.md del crate + ADRs relevantes           │
│  3. Invoca thermo-nuclear + ponytail (con caveat)        │
│  4. Aplica lenses L1-L7 (L7 gated by ADR-0046)           │
│  5. Lee TODO el código del crate en main                 │
│  6. Ejecuta cargo check -p <crate>                       │
│  7. Reproducers targeted (sandbox) para findings runtime │
│  8. Test-file inspection antes de claimar "untested"     │
│  9. Escribe docs/audits/modules/<crate>-quality-DATE.md  │
│     (untracked, backlog local)                           │
│ 10. Documentation proposals L6 preliminares              │
│ 11. Self-grill: invoca self-grill-proposals skill sobre proposals L6 │
│     preliminares → las grilladas van al reporte final           │
│ 12. Edita el reporte con proposals grilladas             │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│ VALIDATOR (w_deep4-flash) — trabaja en main              │
│  1. Lee AUDIT.md + output del auditor                    │
│  2. Spot-check mecánico: 5+ citations + formato          │
│  3. Captura HEAD + git status (ver cláusula tests)       │
│  4. Ejecuta tests con CARGO_TARGET_DIR dedicado:         │
│     cargo test -p <crate>, camel-test, bench --list      │
│  5. Aplica regla 2/2 reproducibilidad                    │
│  6. Para T1/T2: NEGATIVE SEARCH (3-5 patrones por ADR)   │
│  7. Valida 9 reglas anti-falso-positivo                  │
│  8. Aprueba / approved-with-minor-fixes / needs-rework   │
└─────────────────────────────────────────────────────────┘
                          ↓
            Orquestador actualiza tracking tables
            Acumula documentation proposals L6/L7
            Oracle call cuando ≥3, fin de tier, O fin de sesión con proposals pendientes
            (NO congelar proposals esperando count — blind spot #17)
                          ↓
┌─────────────────────────────────────────────────────────┐
│ ORACLE (experts/e_gpt default / e_glm / e_opus)          │
│  Por cada proposal: approve / reject / needs-info +      │
│  contenido final literal (diff/ADR body)                 │
│                                                          │
│  Para approved:                                          │
│   - Edita crates/<crate>/CONTEXT.md (si context update)  │
│   - Crea/edita docs/adr/XXXX-*.md (si new-adr/amendment) │
│   - git add + git commit con mensaje descriptivo         │
│   - NO git push (per AGENTS.md)                          │
│  Puede combinar proposals cross-crate                    │
└─────────────────────────────────────────────────────────┘
                          ↓
            Orquestador actualiza tracking (proposals → committed)
```

DATE = `2026-06-22` para esta pasada.

## Autonomous audit loop (conductor-audit)

Para runs multi-día autónomos, el agente **`.opencode/agents/conductor-audit.md`** (primary) orquesta este workflow crate-por-crate sobre MAIN. Lee este archivo como master prompt. Su loop: resume-check → pick batch (HEAD frozen) → auditor→validator→index→accumulate → oracle trigger per-batch → tier-gate sweep. **No worktree, no merge, no push.** Ver su §"Core invariants" (especialmente **R-A materialization freeze** e **index-not-hold MANDATORY**).

Shakedown obligatorio: correr **T1 completo semi-supervisado (interactive) ANTES de autopilot en T2/T3** (T1 = serial + high blast-radius).

## Autonomous failure handling

Política para el conductor autónomo. **Invariante: un fallo de UN crate NUNCA aborta el loop** — se marca en tracking y se continúa. El loop solo PARA por budget cap o `pending` vacío.

| Fallo | Detección | Acción del conductor |
|---|---|---|
| Subagente retorna vacío | output < N chars / sin secciones | Re-dispatch 1×. Si 2º vacío → tracking `needs-rework (empty-return ×2)`, SKIP crate, continúa. |
| Agente (no test) se cuelga | timeout de dispatch | Marca `needs-rework (<role>-timeout)`, continúa. |
| `cargo test` hang/flake | grep `Blocking waiting for file lock`; regla 2/2 | Env-failure ≠ finding (ya en prompt validator). Conductor NO re-dispatcha por esto. |
| Crate no compila | `cargo check` falla | Critical automático EN EL REPORTE (no aborta loop). Tracking `drafted` con Critical. Continúa. |
| Falso-positivo grosero | validator 9-reglas / oráculo reject | validator → needs-rework; oráculo → `reject (false-positive)`. No es fallo del loop. |
| HEAD movió bajo batch paralelo | oráculo stale-check | **Materialización congelada durante batch in-flight (R-A).** Oráculo commitea solo post-batch. HEAD capturado una vez por batch. |
| Budget cap excedido | contador escalaciones/vacíos | STOP, reporta estado, espera humano. |

## Resume protocol (multi-sesión / post-compactación)

El **tracking table ES el checkpoint durable** (como tasks.md en conductor-light). Un conductor fresco reconstruye SIN estado propio:

1. Lee AUDIT.md (este archivo) + tracking-por-crate + tracking-de-proposals.
2. Reconstruye progreso: `done/committed` = hechos; `drafted` sin validar = retomar en validator; `in_progress` = re-dispatch auditor (el trabajo previo se perdió, no hay checkpoint intra-crate); `pending` = cola.
3. Recupera reportes previos vía KB: `ctx_search(source: "audit-<crate>")` — NO releas los `.md` completos (matan el contexto). One-line pointers bastan.
4. Proposals pendientes sin materializar (tracking proposals `pending-oracle`): dispatch oráculo antes de avanzar (blind spot #17 — nunca las dejes colgadas).
5. Determina tier activo: primer tier con crates `pending`. Si un tier tiene `pending`=0 pero su sweep no corrió → corre el sweep antes de cruzar.
6. Resume el loop en el primer `pending`/`drafted`.

**Granularidad del checkpoint = 1 crate.** No hay resume intra-crate: si una sesión cae con un auditor a mitad, ese crate se re-audita entero. Aceptable (un crate ≠ 40).

## Output & correction flow (two-stream)

La auditoría es **observacional**: NO corrige código. Produce **dos streams de output**, cada uno con su flujo de resolución distinto:

| Stream | Qué produce | Quién resuelve | Flujo |
|---|---|---|---|
| **Docs** | Proposals L6/L7 (DP-N: ADR/CONTEXT.md drift, parity GAPs) | **ORACLE** (e_gpt/e_opus) | reporte → oracle materializa+commitea (§"Oracle workflow") |
| **Código** | Findings C/I/M (F-\<crate\>-N: bugs, perf, design) | **conductor-light** (vía OpenSpec SDD) | reporte → **triage post-auditoría** → bd issues/epics → `/opsx:propose` → conductor-light worktree |

**El stream de código NO se resuelve durante la auditoría.** Los findings viven en el reporte hasta el **triage interactivo** (owner + orquestador, al completar auditoría/tier/crate): agrupar en bd issues (`discovered-from:rc-6z4`), clusters → bd epics, cada uno → OpenSpec change → conductor-light implementa en worktree.

> Detalle del stream de código + handoff a conductor-light: ver §"Findings de código — output de corrección" más abajo.

## Reglas anti-falso-positivo (9 reglas, OBLIGATORIAS)

El AUDITOR debe cumplir OBLIGATORIAMENTE:

1. **Quote literal del código** (2-3 líneas) en findings de performance / orden de ejecución / semantics. **Cita por símbolo** (`fn name`, `impl Trait`, `struct Name`) como referencia primaria — los line-ranges drift entre auditoría y corrección (semanas, worktree distinto); `path:line` solo como suplemento.
2. **Antes de claimar "untested"**, abre el archivo de test, busca el guard/spec, cita `path:line`.
3. **`grep -c '#\[test\]\|#\[tokio::test\]'`** en Cobertura. Comando + count.
4. **Declara `Depends on:`** entre findings en cascada.
5. **ADR provenance:** todo claim ADR cita `docs/adr/XXXX-*.md` + quote breve del ADR.
6. **ACK/dedupe check:** antes de marcar drift, buscar en `bd list` + `docs/audits/` + `CONTEXT-MAP.md`.
7. **Causal preconditions:** si afirmas "puede pasar X", listar input/runtime path mínimo.
8. **Workspace blast radius:** si finding toca public API/trait/URI/lifecycle/feature, contar callers cross-crate.
9. **Severity calibration:** Critical solo si compile fail / data loss / security hole / panic common path / ADR betrayal central.

El VALIDATOR debe verificar explícitamente que las 9 reglas se cumplieron.

## Reglas L6 para documentation proposals (4 reglas, OBLIGATORIAS)

> Lección del primer oracle call (e_gpt 2026-06-22, pilot camel-processor).

1. **Cita ADRs related/conflicting + justifica "why not amendment":** toda `new-adr` proposal debe listar ADRs relacionados (Related) y explicar por qué NO es amendment.
2. **Additive diff para CONTEXT.md:** las `context-md-update` deben ser **additivas** — preservar glossary, añadir secciones/campos, no reemplazar ciegamente.
3. **Counts/claims verificados mecánicamente:** toda claim cuantitativa (LOC, exports, tests) acompañada del comando `rg`/`wc` ejecutado y su output. No estimar a ojo.
4. **Cross-crate semver/API → workspace ADR default:** proposals que tocan política API/semver se proponen como **un ADR workspace-wide** + crate-local exceptions.

## Test reliability under concurrent worktrees (CLÁUSULA CRÍTICA)

> **Lección e_opus:** el owner trabaja en paralelo en otros worktrees. `CARGO_TARGET_DIR=/home/shared/rust-camel-target` es **global, compartido**. Worktree aísla source, NO target.

El VALIDATOR debe:

1. **Capturar al inicio de test execution:**
   - `git rev-parse HEAD` (commit hash de referencia).
   - `git status --porcelain` (si no está limpio → caveat en el reporte).

2. **Usar target dir dedicado por crate:**
   ```
   export CARGO_TARGET_DIR=/home/shared/rust-camel-target-audit/<crate>
   ```
   Ej: `/home/shared/rust-camel-target-audit/camel-processor`. Aislamiento total entre crates auditados y con el owner. Mitigado por `RUSTC_WRAPPER=sccache` (content-addressed, concurrency-safe).

3. **Regla 2/2 reproducibilidad (árbitro binario):**
   - Un test failure es `test-failure (bug del crate)` **solo si reproduce 2/2** corridas consecutivas del mismo binario (`cargo test -p <crate> <testname> --exact` ×2, sin recompilar en medio).
   - Falla 1ª, pasa 2ª = `test-environment-failure` (flaky/race externa). NO es finding.
   - Falla 2/2 = bug real → Critical finding.

4. **Detector objetivo de contención:**
   - Guarda el log completo con `tee` (no solo `tail`): `cargo test -p <crate> --no-fail-fast 2>&1 | tee /tmp/audit-<crate>-tests.log`.
   - Grep **completo** del log por `Blocking waiting for file lock on build directory` (literal). Si aparece → hay un cargo concurrente → marcar sospecha de environment-failure.

5. **Compile failure ≠ environment:**
   - Si `cargo test --no-run` falla a compilar (con source estable + sccache), eso **NO puede ser environment** → Critical automático.

6. **Distinguir en Validator notes:**
   - `test-failure (bug)` → Critical finding.
   - `test-environment-failure (concurrent worktree)` → note + retry recomendado, NO finding.
   - Auditor **no puede** abrir Critical por test failure sin pasar regla 2/2.

## Negative search (validator T1/T2, OBLIGATORIO)

> Lección e_opus: para T1/T2 el validator no debe solo confirmar citations — debe buscar activamente lo que el auditor pudo missing.

Para cada crate T1/T2, el validator define **3-5 patrones peligrosos** ligados al crate y sus ADRs, y los busca con `rg`. Reportar findings aunque el auditor no los haya flaggeado.

**Catálogo de patrones por ADR:**

| ADR | Patrón peligroso | rg hint |
|---|---|---|
| 0001 | lifecycle op via `Service::call` | `rg 'fn (start|stop|suspend|resume)_route' crates/` |
| 0007 | `Consumer::stop()` en crash path | `rg 'consumer\.?stop\|Consumer::stop' crates/<crate>/` |
| 0010 | `SecurityPolicy` como Step | `rg 'YamlStep::Security\|DeclarativeStep::Security'` |
| 0012 | `error!` sin `// log-policy:` | `rg -B1 'error!' crates/<crate>/src/` |
| 0019 | `poll_ready` devolviendo `Err` | `rg 'fn poll_ready' crates/<crate>/src/` |
| 0024 | `CamelError::Stopped` en sitios nuevos | `rg 'CamelError::Stopped' crates/<crate>/src/` |
| Phase C WS2 | `.unwrap()/.expect()` en hot path | `rg '\.unwrap\(\)\|\.expect\(' crates/<crate>/src/` |

Auditor + validator acuerdan los 3-5 patrones específicos por crate (basados en lenses L1-L5 aplicados).

## Oracle workflow — proposals L6

> Regla inalienable: las proposals L6 (cambios a CONTEXT.md / ADRs / CONTEXT-MAP) **solo pueden ser aprobadas por el oráculo**. El oráculo también las **materializa y commitea**.

### Flujo

```
AUDITOR detecta L6 drift o decisión implícita
   → Documentation proposal en el reporte del crate
   → status: pending-oracle-approval
        ↓
ORQUESTADOR acumula proposals (≥3 o fin de crate)
        ↓
ORACLE (e_gpt / e_glm / e_opus)
   approve / reject / needs-info + contenido final literal
   MATERIALIZA:
   - context-md-update → edita crates/<crate>/CONTEXT.md
   - new-adr → crea docs/adr/XXXX-*.md (con el body completo)
   - adr-amendment → edita docs/adr/XXXX-*.md
   - context-map-update → edita CONTEXT-MAP.md
   COMMITEA con mensaje descriptivo (NO git push)
```

### Oracle sweep obligatorio al final de T1

> Lección e_gpt (2026-06-22): T1 tiene blast radius cross-crate alto; proposals pueden interactuar/contradecirse entre crates del mismo tier.

Antes de pasar de T1 a T2, el orquestador debe lanzar un **oracle sweep** que:
- Revisa TODAS las proposals acumuladas de T1 (no solo ≥3 por crate).
- Detecta merges/splits cross-crate (ej: múltiples crates pidiendo ADRs sobre la misma política).
- Detecta contradicciones (ej: camel-core propone X, camel-processor propone anti-X).
- Aprueba materializaciones conjuntas donde aplique.

Solo tras el sweep de fin de T1 se puede arrancar T2.

### Stale check del oráculo contra HEAD actual

> Lección e_gpt (2026-06-22): entre audit.HEAD y el oracle call, main puede haber avanzado (owner trabaja en paralelo).

El oráculo **trabaja contra HEAD actual**, no contra `audit.HEAD`. Para cada proposal:

1. **Comparar** `git diff audit.HEAD..HEAD -- crates/<crate>/ docs/adr/ docs/audits/` (archivos relevantes al target de la proposal).
2. **Si NO cambió evidencia relevante:** aprobar/materializar normalmente.
3. **Si cambió evidencia relevante:**
   - Revalidar la proposal contra el código actual (no el del audit).
   - Si la proposal sigue siendo válida → aprobar.
   - Si la proposal ya no aplica o quedó desactualizada → `needs-info` con razón, devuelve al auditor para re-grill.
4. **Documentar en el oracle call** el diff check (`audit.HEAD=X, current HEAD=Y, diff on targets=<files>`).

### Skill `self-grill-proposals` (disponible) — y autoridad grill = oráculo

Copia no-interactiva de `grill-with-docs` ubicada en `~/.agents/skills/self-grill-proposals/`. Creada 2026-06-22. El auditor la invoca en paso final para refinar proposals L6: genera **4 questions mínimas (una por técnica)**, contesta con citations, produce outcome (confirm/refine/merge/split/drop/open-question), documenta Q&A como evidence.

Si por alguna razón el skill no coopera, fallback manual con los 4 principios (consistency con CONTEXT-MAP, conflicto con ADRs existentes, redundancia con ADRs implícitos, numeración correcta) y documenta `"self-grill": "manual"`.

> **B7 (corregido 2026-08-05, test w_fast ses_02e9a0326):** verificación confirmó que los subagentes **SÍ pueden cargar skills** via la herramienta `skill` (`self-grill-proposals` + `ponytail` cargaron completo en w_fast). El reporte del auditor r_glm de que el skill "no cooperaba" fue **error de ejecución del auditor**, no limitación de plataforma. **El auditor DEBE invocar el skill** `self-grill-proposals` en Paso 7 (no fallback manual prematuro). La grill **autoritativa final** sigue siendo el **oráculo** (stale-check + 4 reglas L6 + sweep cross-crate) — pero el self-grill del auditor es un paso real de refinamiento, no "best-effort".

### Tipos de proposals

- `context-md-update`: diff textual para `crates/<crate>/CONTEXT.md` (tracked).
- `new-adr`: ADR nuevo con número tentativo (0027+ siguiente libre; **0026 reservado para rc-iq7**), título, contexto, decisión tentativa. Se escribirá en `docs/adr/XXXX-*.md` (tracked por `.gitignore` exception `!docs/adr/*.md`).
- `adr-amendment`: enmienda a ADR existente.
- `context-map-update`: cambios al `CONTEXT-MAP.md` global (tracked en raíz).

### Política de files (recordatorio)

- `docs/*` está gitignored.
- `!docs/adr/*.md` exception → **ADRs son tracked y commiteable**.
- `docs/audits/` untracked → **reportes de auditoría son backlog local** (no viajan por git).
- `crates/<crate>/CONTEXT.md` tracked → **CONTEXT.md es commiteable**.

### Findings de código — output de corrección (two-stream)

Los Critical/Important/Minor (no absurdos) de código **NO van a beads automáticamente, NI se corrigen durante la auditoría** (es observacional). Viven en `docs/audits/modules/<crate>-quality-DATE.md` (untracked, backlog local) con:
- **ID estable** `F-<crate>-<severity><N>` (ej: `F-camel-processor-C1`, `F-camel-processor-I3`) — referenciable sin ambigüedad en bd/OpenSpec.
- **Citation por símbolo** (`fn poll_ready`, `impl ConsumerSegment`) — **OBLIGATORIO (B4)**: las correcciones ocurren semanas después en worktree distinto; los line-ranges drift, los símbolos no.
- **Correction direction** — pista de fix (1-3 líneas) para alimentar el `design.md` del OpenSpec change futuro. No es el fix completo, es la dirección.

**Flujo de corrección (stream de código):**
1. Auditoría registra findings en el reporte (formato arriba).
2. Al completar auditoría / tier / crate → **triage interactivo** (owner + orquestador):
   - Agrupar findings relacionados en **clusters**.
   - Cada cluster → **bd epic**; findings individuales → **bd issues** (`discovered-from:rc-6z4`, `--design`/`--acceptance` cuando aplique).
3. Cada bd issue/epic → `/opsx:propose` → **OpenSpec change** (design + specs delta + tasks).
4. **conductor-light** implementa cada change en worktree aislado (`.worktrees/audit-fix-<crate>`), con review checkpoints, merge a main.

> La auditoría (conductor-audit) **termina su trabajo al producir findings limpios**. La corrección es un proyecto separado orquestado por conductor-light.

---

## PROMPT CANONICAL — AUDITOR (r_glm5.2)

> Copiar este prompt íntegro al lanzar el subagente. Sustituir `<CRATE>`, `<TIER>`, y `<LENSES>`.

```
Eres un AUDITOR de código senior para el proyecto rust-camel (Rust, Tower-native async, reimplementación de Apache Camel 4.x). Fase: estabilización hacia v1.0.0.

Tu análisis será validado por un validator mecánico (w_deep4-flash) que spot-checkeará citations, ejecutará tests reales (con target dir dedicado + regla 2/2 reproducibilidad), Y (para T1/T2) hará negative search activo. **Escribe sabiendo que cada afirmación será verificada.**

## Tu tarea

Auditar el crate `<CRATE>` (tier `<TIER>`), produciendo un reporte de calidad profundo. **Lenses aplicables:** `<LENSES>` + **L6 siempre**.

## Paso 1 — Lectura obligatoria (en orden)

1. `/home/kenny/dev/rust-camel/AGENTS.md`.
2. `/home/kenny/dev/rust-camel/docs/audits/AUDIT.md` — léelo COMPLETO.
3. `/home/kenny/dev/rust-camel/CONTEXT-MAP.md`.
4. `/home/kenny/dev/rust-camel/crates/.../<CRATE>/CONTEXT.md` y `README.md`.
5. ADRs relevantes (abre `ls /home/kenny/dev/rust-camel/docs/adr/`). **Nota:** `docs/` está gitignored — `ctx_index(docs/adr)` retorna 0 archivos (respeta .gitignore). Lee los ADRs individualmente con Read, o usa `ctx_index(path, respectGitignore: false)` si quieres indexarlos en bloque.
6. TODO el código del crate bajo `crates/.../<CRATE>/src/`.
7. Tests: `crates/.../<CRATE>/tests/` y `#[cfg(test)]` inline.

## Paso 2 — Invocar skills

Invoca **`thermo-nuclear-code-quality-review`** (full) y **`ponytail`** (lite con caveat parity-driven).

## Paso 3 — Aplicar lenses

L1-L5 relevantes + L6 (siempre). L6 findings van en sección "Documentation proposals (pending oracle approval)", NO en Findings.

## Paso 4 — Tests mínimos del auditor

- `cargo check -p <CRATE>` — si falla, Critical automático.
- Reproducer targeted para findings runtime/panic/ordering.
- Test-file inspection antes de claimar "untested".

## Paso 5 — Análisis con 9 reglas anti-falso-positivo + 4 reglas L6

Para cada finding: severidad, tipo, cita ADR con path+quote, evidence `path:line`, quote del código, ACK check, causal preconditions, workspace blast radius, depends-on, fix. Critical solo si compile fail / data loss / security hole / panic common path / ADR betrayal central.

## Paso 6 — Output preliminar

Escribe `/home/kenny/dev/rust-camel/docs/audits/modules/<CRATE>-quality-2026-06-22.md` con el template (ver sección "Formato de output").

## Paso 7 — Self-grill con `self-grill-proposals` (OBLIGATORIO)

Una vez escrito el reporte preliminar, **invoca el skill `self-grill-proposals`** (disponible en `~/.agents/skills/self-grill-proposals/`) sobre las proposals L6 preliminares.

El skill aplica los 4 cuestionamientos (glossary challenge, sharpen language, scenarios, cross-reference code) en modo auto-entrevista. Por cada proposal: confirm / refine / merge / split / drop / open-question. Documenta el Q&A como evidence (el validator/oráculo lo inspeccionará).

**Edita el reporte** para reflejar el resultado final del grilling (proposals refinadas, fusiones, drops razonados, open-questions documentadas con campo `Open questions (for oracle)`).

Si por alguna razón el skill no coopera, fallback manual: aplica los 4 principios del grilling manualmente (consistency con CONTEXT-MAP, conflicto con ADRs existentes, redundancia con ADRs implícitos, numeración correcta) y documenta `"self-grill": "manual"`.

## Reglas generales

- NO toques código del repo (sí puedes correr snippets sandbox).
- Cita `path:line` siempre.
- Coverage total del crate (justifica muestreo si >2000 LOC).
- No inventes ADRs.
- L6 proposals: cite related ADRs, justify "why not amendment", additive diff, counts verificados mecánicamente, workspace-wide default para política semver.

## Mensaje final al orquestador

Devuelve: (1) Path. (2) Conteo findings + lens observations. (3) Documentation proposals L6 (lista con ID/tipo/target/rationale/self-grill outcome). (4) Verdict. (5) Tests relevantes para el validator. (6) Tickets beads. (7) Problemas. (8) Feedback proceso.
```

---

## PROMPT CANONICAL — VALIDATOR (w_deep4-flash)

> Copiar este prompt íntegro. Sustituir `<CRATE>`, `<TIER>`. Para T1/T2, ajustar negative search con patrones acordados.

```
Eres un VALIDATOR mecánico para rust-camel. Verificas que el reporte del auditor sobrevive escrutinio externo. Tu valor: cazar falsos positivos, evidence débil, omisiones objetivas, ejecutar tests reales, Y (T1/T2) hacer negative search.

## Paso 1 — Lectura obligatoria

1. `/home/kenny/dev/rust-camel/docs/audits/AUDIT.md` — léelo COMPLETO, especialmente "Test reliability under concurrent worktrees", "Reglas anti-falso-positivo", "Negative search".
2. `/home/kenny/dev/rust-camel/docs/audits/modules/<CRATE>-quality-2026-06-22.md` — el reporte.
3. `/home/kenny/dev/rust-camel/CONTEXT-MAP.md`.
4. CONTEXT.md y README.md del crate.
5. Spot-check del código: abre archivos citados en Critical/Important. Mínimo 5 citations verificadas.

## Paso 2 — Validar (checklist)

- Formato (template, secciones, metadata).
- Evidence (`path:line` + quote literal).
- 9 reglas anti-falso-positivo.
- 4 reglas L6 (si hay proposals).
- Rigor (thermo-nuclear, ponytail contextualizado, ADRs correctos).
- Verdict justificado.

## Paso 2.5 — Ejecución de tests (OBLIGATORIO)

**Setup:**
- `export CARGO_TARGET_DIR=/home/shared/rust-camel-target-audit`
- Captura: `git rev-parse HEAD` + `git status --porcelain`

**Secuencia:**
1. `cargo test -p <CRATE> --no-run 2>&1 | tail -30` — verifica compilación. Si falla → Critical automático.
2. `cargo test -p <CRATE> --no-fail-fast 2>&1 | tail -100` — full test suite.
3. **Para cada test que falla:** regla 2/2 reproducibilidad:
   - `cargo test -p <CRATE> <testname> --exact 2>&1 | tail -20`
   - Correlo 2 veces consecutivas sin recompilar en medio.
   - Pasa 1ª, falla 2ª (o viceversa) → `test-environment-failure`, NO finding.
   - Falla 2/2 → `test-failure (bug)` → Critical finding.
4. **Grep por `Blocking waiting for file lock`** en stderr. Si aparece → sospecha environment-failure.
5. **camel-test integration:** `rg -l '<crate_name>' crates/camel-test/tests/` → para cada match, `cargo test -p camel-test --test <file>`.
6. `cargo bench -p <CRATE> --list` (report-only).
7. Coverage: solo si `coverage.toml` existe + tooling disponible.

## Paso 2.6 — Negative search (T1/T2 OBLIGATORIO, T3 opcional)

Define 3-5 patrones peligrosos ligados al crate + ADRs. Ejecuta cada `rg`, reporta hits con `path:line`, verifica si auditor flaggeó.

## Paso 3 — Output

Edita el reporte. Llena `Validator:`, cambia `Status:` a approved / approved-with-minor-fixes / needs-rework. Añade `## Validator notes` con template.

## Reglas

- NO toques código del repo.
- Spot-check mínimo 5+ citations.
- Tests OBLIGATORIOS con target dir dedicado.
- Negative search OBLIGATORIO T1/T2.
- Si insufficient, marca needs-rework.

## Mensaje final al orquestador

(1) Status. (2) Citations verificadas. (3) 9 reglas. (4) Tests ejecutados (con distinción test-failure vs environment-failure). (5) Negative search. (6) Hallazgos omitidos. (7) Feedback proceso.
```

---

## PROMPT CANONICAL — ORACLE (experts/e_gpt default)

> Copiar este prompt cuando el orquestador acumule ≥3 proposals L6 o fin de tier.

```
Eres el oráculo escalation-expert del proyecto rust-camel. Apruebes/rechazas/needs-info documentation proposals L6 y **materializas los cambios commiteables**.

## Lecturas obligatorias

1. `/home/kenny/dev/rust-camel/docs/audits/AUDIT.md` — especialmente "Oracle workflow", "Reglas L6 para proposals".
2. Audit files relevantes (con las proposals).
3. `/home/kenny/dev/rust-camel/CONTEXT-MAP.md`.
4. ADRs relacionados con cada proposal.

## Tu mandato

Por cada proposal: approve / reject / needs-info / merge / split / renumber.

**Stale check (OBLIGATORIO antes de aprobar):**
- Compara `git diff audit.HEAD..HEAD -- <target_paths>`.
- Si cambió evidencia relevante (código o ADRs del target): revalidar contra HEAD actual. Si la proposal ya no aplica → `needs-info` al auditor.
- Si no cambió: aprobar normalmente.
- Documenta en el verdict: `audit.HEAD=X, current HEAD=Y, diff=<files or "none">`.

**Sweep al final de T1:** si este oracle call es el último de T1 (o el orquestador lo marca como sweep), revisa TODAS las proposals acumuladas del tier, detecta merges/splits/contradicciones cross-crate.

Para **approve**:
1. Da el contenido final literal (diff para CONTEXT.md, body completo para ADR).
2. **Materializa el cambio**:
   - Edita `crates/<crate>/CONTEXT.md` o crea/edita `docs/adr/XXXX-*.md`.
    - Numeración: respeta 0022/0023=rc-blw, 0024=PipelineOutcome, 0025=runtime-migration rc-b5h, **0026=reservado para rc-iq7 (Slice E1 JSON canonical Authoring)**, 0027+=siguiente libre.
   - **Materialization completeness (B6, OBLIGATORIO):** al tocar un archivo, valida **TODAS** las secciones que citan código de ese archivo, no solo la sección trigger. Un CONTEXT.md con 43 filas de catalog: si la proposal toca la fila de `FilterService`, verifica también las filas relacionadas (`Splitter`, `StreamingSplitter`) por consistencia. Mitiga el drift reintroducido (riesgo R4 — un commit que toca solo 1 sección deja las otras stale).
3. **Commitea** con mensaje descriptivo (formato Conventional Commits, ej: `docs(adr): 0027 wire-tap fire-and-forget semantics`). **NO git push** (per AGENTS.md).

Para **reject/needs-info/merge/split**: documenta razón sin tocar archivos.

## Reglas L6 (ya aplicadas por el auditor, el oráculo verifica)

1. New ADR proposal cita related ADRs + "why not amendment".
2. CONTEXT.md proposals: additive diff.
3. Counts verificados mecánicamente.
4. Cross-crate semver/API → workspace ADR default.

## Mensaje final al orquestador

(1) Por cada proposal: verdict + contenido final si approve + commit hash si commiteado. (2) Cross-crate merges detectados. (3) Ajustes al workflow sugeridos.
```

---

## Formato de output por módulo (template)

```markdown
# Quality Audit — <CRATE>

**Date:** 2026-06-22
**Auditor:** r_glm5.2
**Validator:** _(filled by validator)_
**Status:** draft
**Tier:** <TIER>
**HEAD at audit time:** <git rev-parse HEAD>
**Working tree:** clean / dirty (<details>)
**LOC src (sin tests):** <n> _(wc -l)_
**LOC tests inline:** <n>
**LOC tests dir (tests/):** <n>
**Test count:** <n> _(grep -c)_
**ADRs relevantes leídas:** <lista>
**Lenses aplicados:** <L1/L2/L3/L4/L5 subset> + L6

## Resumen ejecutivo

<2-4 frases>

## Premisas ADR relevantes

- **ADR-XXXX** — <one-line>. Cita: `docs/adr/XXXX-*.md`.

## Lens observations

### L1 — API stability / semver v1.0
...

### L2 — Concurrency / runtime safety
...

### L3 — Security
...

### L4 — Performance hot-path
...

### L5 — Dependency boundary
...

### L6 — Architectural documentation coherence
...

## Findings

### Critical
- **[F-<crate>-C1]** <título>
  - **Tipo:** <tipo>
  - **Símbolo:** `<fn name> / <impl Trait> / <struct Name>` (referencia primaria, B4)
  - **Path:** `path:line` (suplemento, puede drift)
  - **Descripción:** ...
  - **Quote:**
    ```
    <2-3 líneas del archivo>
    ```
  - **Causal preconditions:** ...
  - **Workspace blast radius:** <N callers cross-crate>
  - **ACK check:** ...
  - **Correction direction:** <pista de fix 1-3 líneas, para OpenSpec design.md>
  - **Cita ADR:** "Viola ADR-XXXX" (`docs/adr/XXXX-*.md`)

### Important / Minor / Ponytail / Thermo-nuclear
...

## Documentation proposals (pending oracle approval)

### DP-1: <título>
- **Tipo:** context-md-update | new-adr | adr-amendment | context-map-update
- **Target:** <path>
- **Rationale:** ...
- **Proposed change:** <diff/contenido>
- **Related ADRs:** <lista> (regla L6 #1)
- **Why not amendment:** ... (regla L6 #1, si new-adr)
- **Counts verificados:** <comando + output> (regla L6 #3, si aplica)
- **Self-grill outcome:** confirm | refine | merge | split | drop | open-question
- **Self-grill mode:** self-grill-proposals skill | manual (fallback)
- **Open questions (for oracle):** <opcional>
- **Status:** pending-oracle-approval

## Acknowledged gaps (no son Findings)

...

## Cobertura

- **Archivos .rs en src/:** <n>
- **Archivos leídos:** <n>
- **Test count ejecutado:** `<grep>` → <n>
- **Tests (unit + integration):** <n>
- **camel-test integration:** `<n>` — `rg -l ...` output
- **Benchmarks definidos:** `<n>`
- **`cargo check -p <CRATE>`:** ✅ pass / ❌ fail (`<error>`)
- **`wc -l src/*.rs` total:** <n>
- **¿CONTEXT.md existe y al día?:** yes/no/missing

## Reproducers (sandbox)

- **[R1]** para finding [XX]: <descripción + resultado>

## Verdict

- **Drift real:** <n> critical, <n> important
- **Gaps ACK:** <lista>
- **Health:** green / yellow / red
- **Recomendación v1.0.0:** blocks / nice-to-have / post-v1.0

## Cross-references

- Módulos relacionados: ...
- Tickets beads creados: <rc-XXX o "ninguno">
- **Finding classes (for cross-crate tracking):** <lista FC-* tags>
- Sesión/origen: rc-6z4
```

## Cómo retomar tras interrupción

1. Lee este AUDIT.md.
2. Abre tracking tables (crate + proposals).
3. Busca módulos con status `pending` / `needs-rework` / `in_progress`.
4. Para empezar un módulo:
   - Determina tier y lenses.
   - Lanza auditor (reviewers/r_glm5.2) con el prompt canonical.
5. Auditor termina → tracking status `drafted`.
6. Lanza validator (workers/w_deep4-flash).
7. Validator termina → tracking status final.
8. Acumula proposals L6. Cuando ≥3 o fin de tier → lanza oracle (experts/e_gpt) con prompt canonical.
9. Oracle commitea → tracking proposals → `committed`.

## Tracking por crate

> L6 (architectural documentation coherence) aplica a todos. Orden: T1 → T2 → T3.

| Crate | Tier | Status | Auditor | Validator | File | Lenses (sin L6) | Notes |
|---|---|---|---|---|---|---|---|
| camel-processor | T1 | reviewed (approved-with-minor-fixes) | r_glm (v4.2) | w_fast (v4.2) | modules/camel-processor-quality-2026-{06-22,08-05}.md | L1,L2,L3(narrow),L4,L7 | **run v4.1 (2026-06-22):** 0C/2I/5M; validator 388/388 tests, 12 citations, neg-search 5/5; oracle DP-1+DP-2 committed (`272120c2`,`3613eb3d`); DP-3 (FilterService→ADR-0019) quedó pendiente. **re-run v4.2 (2026-08-04, test del flujo + L7):** auditor r_glm 0C/4I/6M; validator w_fast 600/600 tests 2/2 (no flake, 0 locks), 7/7 citations, 9/9 reglas, dating 0-GAP sostenido (EIPs pre-ADR-0046), approved-with-minor-fixes; **L7 = 0 GAPs** (scope guard §no-retroactivo — correcto); DP-3 re-confirmado ausente; M7 nuevo (CONTEXT.md catalog stale); I4 thermo-nuclear trend (aggregator 1270→1953, error_handler 1544→1904). EnrichService producer.poll_ready open question persiste (defer sweep) |
| camel-core | T1 | covered (rc-d0pu/ADR-0045) + L7 delta | — | — | (ver rc-d0pu) | L7,L2(delta) | Clean-Arch estabilizado fuera de v4.1 por rc-d0pu (3 tiers, closed 2026-07-18); audit v4.2 = solo L7 parity + git-log delta desde `00c5f6ef` + L2(delta). Máxima densidad de marcadores ADR-0046 (control-flow ADR-0024/0025, backpressure ADR-0044). **DP-3 (FilterService→ADR-0019) ya commiteada** (`1df205cb`, e_opus ronda-3) — nota previa "cerrar antes DP-3" stale, actualizada 2026-08-05 |
| camel-api | T1 | reviewed (approved-with-minor-fixes) | r_glm (ses_02e77a27) | w_fast (ses_02e6ed23) | modules/camel-api-quality-2026-08-05.md | L1 | **Run v4.3 (2026-08-05, BATCH_HEAD cf19034a):** 0C/2I/5M; health yellow, nice-to-have before freeze. I1 ClaimCheckRepository rustdoc cita `CamelError::NotFound` (variante inexistente), I2 enums contrato CQRS + Canonical* sin `#[non_exhaustive]` (semver v1.0), M1 thermo-nuclear (runtime.rs 1069/xml_convert.rs 1012 >1k), M2 ponytail DRY is_ssrf_blocked_ip, M3 Principal Debug sobre claims (ADR-0032), M4 ExponentialBackoff since="0.1.0", M5 TODO(API-006). **Validator:** 7/7 citations, 9/9 reglas, 507/507 tests (0 locks, regla 2/2 N/A), neg-search 6 patrones (non_exhaustive 3/13 enums, CamelError::NotFound 0 hits confirma I1, poll_ready 4/4 compliant, unwrap solo en tests); 2 TODOs internos omitidos no materiales. **DPs L6:** DP-5 context-md-update (ClaimCheck/Idempotent glossary, self-grill confirm), DP-6 open-question new-adr 0049 workspace `#[non_exhaustive]` v1.0 (self-grill refine→open-question, auditor no decide política workspace). rc-iq7 cerró, API estable post-`RouteDsl*` |
| camel-dsl | T1 | reviewed (approved-with-minor-fixes) | r_glm (ses_02e3770c8) | w_fast (ses_02e2c6c65) | modules/camel-dsl-quality-2026-08-05.md | L1 | **Run v4.3 (2026-08-05, BATCH_HEAD ec65888d):** 0C/1I/2M; health yellow, nice-to-have before freeze. I1: 6 public types sin `#[non_exhaustive]` decision pre-freeze (DeclarativeRoute struct 10fields + DeclarativeStepKind 41var, DeclarativeStep 36var, DeclarativeSecurityPolicy 5var, DeclarativeConcurrency 2var, RouteDslStep 38var) — camel-dsl **OUT de ADR-0049 scope** → caso-por-caso L1. M1 thermo-nuclear (compile.rs 4622 LOC, yaml.rs 4612 LOC, workspace-wide pattern cf. camel-processor I4), M2 README step tables under-represent (13 variants sin mención). rename `Yaml*`→`RouteDsl*` verificado clean (0 stale). **Validator:** 16/16 citations, variant counts I1 verificados manualmente (todos correctos post-self-corrección auditor), 9/9 reglas, 600/600 tests (0 locks, regla 2/2 N/A), camel-test integration 8/8 pass, neg-search 9 patrones (NS9 gap menor: 13 camel-test files vs 1 reportado, no material). **DPs L6:** DP-7 context-md-update (log-policy table stale + missing 3rd site `extract_rest_blocks`, self-grill confirm), DP-8 context-md-update skeleton `#[non_exhaustive]` posture table (open-question, self-grill refine, oracle decide amend-0049 vs crate-local). L7 N/A (DSL/schema crate, no EIP). |
| camel-config | T1 | reviewed (approved-with-minor-fixes) | r_glm (ses_02e03c8a4) | w_fast (ses_02ded8d36, 1er dispatch ses_02df7639e empty-return) | modules/camel-config-quality-2026-08-05.md | L1 | **Run v4.3 (2026-08-05, BATCH_HEAD 98ace84e):** 0C/0I/4M; **health GREEN**, proceed. M1 (FC-DOC-DRIFT) TODO(CONFIG-004) stale ×3 (`config.rs:19,45`, `context_ext.rs:140`) — afirman hot-reload "not implemented" pero SÍ implementado+consumido (`camel-cli/run.rs:498,507` → `camel-core::reload_watcher`); cazado vía anti-falso-positivo #2. M2 thermo-nuclear (config.rs 3375 LOC, context_ext.rs 1507). M3 (FC-API-INSTABILITY-adjacent) 2/27 structs non_exhaustive inconsistente (`NativeIssuerConfig`, `NativeM2mClientConfig`) — NARROW: structs config no deben. M4 ponytail `PropertiesResolver`+`ResolveError` re-export pub, 0 consumidores. L1: 0 non_exhaustive gaps en 4 pub enums (todos closed-set/parity-mirror/monitor/config-value, postura NARROW aplicada correcta). rc-iq7 cleanup limpio (0 `Yaml*` residuales). L5 narrow N/A (deps intrínsecas config). L7 N/A (no EIP). **Validator:** 14/14 citations, ACK-check CONFIG-* bds = 0 (M1/M4 nuevos, no duplicados), 9/9 reglas, 229 tests pass (228+1 ignored, 0 failures, 0 locks, regla 2/2 N/A), check --no-default-features sin sccache = pass (auditor sccache failure era env-only), 8 neg-search. **DPs L6:** DP-9 context-md-update NEW FILE `crates/camel-config/CONTEXT.md` (único T1 sin CONTEXT.md; posture non_exhaustive selective + 2 decisiones implícitas; self-grill refine). Empty-return resuelto (1/2, no skip). JSON re-exports estables post-rc-iq7. |
| camel-cli | T1 | reviewed (approved-with-minor-fixes) | r_glm (ses_02ddc3659) | w_fast (ses_02dc5e3fd) | modules/camel-cli-quality-2026-08-05.md | L1 | **Run v4.3 (2026-08-05, BATCH_HEAD 0a720767, AUTOPILOT):** 0C/1I/2M; health yellow. **I1** (error-handling): SQL/SurrealDB bundle loaders tragan config errors (`tracing::error!`+continue) mientras JMS/CXF fail-fast (`return Err`) — fail-late en `camel run`. Símbolo `fn run` (run.rs:333-346 vs :382-412). **M1** dead helper `report_cli_failure_msg_and_exit` + inline `eprintln!+exit(1)` 45 veces (validator: 45 no "30+"). **M2** (FC-DOC-DRIFT) README drift. L1: non_exhaustive **N/A confirmado (0 ocurrencias, calibration rc-vmrr aplicada correcta — no sobre-reportado)**; `camel run` rc-ca8z limpio. **Validator:** 7/7 citations, ACK-check I1 = 0 bds (finding nuevo), causal-path I1 verificado (Camel.toml [components.sql] malformado → arranca → fail-late), 9/9 reglas, 96/96 tests (69 lib + 27 integration, 0 failures/locks, 2/2 N/A), clippy -D warnings gate pass, 6 neg-search (incl. reproducer I1 spot-check). **DPs L6:** DP-10 context-md-update log-policy table stale (claima 7, real 10; patrón DP-7/DP-A, 4º crate FC-DOC-DRIFT → umbrella epic candidate sweep). `camel run` shipped rc-ca8z. |
| camel-builder | T1 | reviewed (approved-with-minor-fixes) | r_glm (ses_02db2724e) | w_fast (ses_02d975e1d) | modules/camel-builder-quality-2026-08-05.md | L1 | **Run v4.3 (2026-08-05, BATCH_HEAD 7f9d8a03, AUTOPILOT):** 0C/1I/4M+1P; health yellow, nice-to-have. **I1** (panic-vs-Result inconsistency): `do_finally()` (double-call) + `disposition(Continued)` panican (`do_try.rs:106,153`) mientras `build()`/`build_canonical()`/`marshal()`/`unmarshal()` retornan Result — pre-freeze momento de unificar (signature change post-1.0 = breaking). **M1** thermo-nuclear lib.rs 4211 LOC (umbrella candidate w/ camel-processor I4 + camel-dsl), **M2** TODO BUILDER-003/006 numeración colisiona `docs/archived/audits/taxonomy-*`, **M3** `StepAccumulator` trait público no sellado (riesgo residual mitigado por default-only), **M4** string "canonical v1" stale post-v2, **P1** ponytail. L1: **0 pub enums → non_exhaustive N/A confirmado** (calibration rc-vmrr correcta). **Validator:** 7/7 citations (corrigió typo `StepAccumulation`→`StepAccumulator` vía replaceAll), ACK I1/M3 = 0 bds, causal-path I1 = misuse paths `#[should_panic]` (no common path, severity I correcta, reproducer 2/2 pass), 8/9 reglas (regla 6 ⚠️ por typo citation corregido), 160/160 tests (0 failures/locks, 2/2 N/A), camel-test 25 files (auditor dijo 20, subestimación menor), 7 neg-search. **DPs L6:** DP-11 NEW CONTEXT.md committed (`c4a791f7`), DP-12 CONTEXT-MAP split committed (`ac55f1a6`). **I1 → bd rc-0lhn** (P2, panic-vs-Result unification). Fluent API, rc-iq7 lifted. |

**▶ T1 SWEEP DONE (2026-08-05, e_gpt ses_02d86840b — e_opus ses previo cancelled por timeout):** T1→T2 crossing **APPROVED**. Commits sweep: `2831e6f6` (CONTEXT-MAP añade Processor+Config entries), `95d0886b` (camel-dsl declarative boundary clarify). 0 contradicciones cross-crate en DP-1..12. Umbrella epics filed: rc-w5yo (doc-drift, P2) + children rc-bwbg/rc-9h5a; rc-xctv (thermo-nuclear, P3 post-v1.0). ADR-0050 (panic-vs-Result) **NO** — rc-0lhn es decisión crate-local. Condiciones cruce: rc-3pw3 + rc-ierl cerrar antes de **freeze** (no antes de T2). T2: L3 completo en auth/http/sql, vigilar trust boundaries + fail-closed + outside-contract. **Autopilot T2/T3 aprobado** (pausa solo en C/I, contradicción ADR, nueva decisión cross-crate). **Budget-cap conductor-audit reached (5/5 consultas) → pausa humano antes de T2.**
| camel-health | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_029b7ba4a) | w_fast (ses_029b15fb1) | modules/camel-health-quality-2026-08-05.md | L2 | **T2 batch4 (BATCH_HEAD 8a7fab95, AUTOPILOT):** YELLOW 0C/1I/3M+2P. Validator 8/8 citations, **I1 race REAL** (5s shutdown vs 6s handler timeout, 0 `.abort()`), 18/18 tests, 9/9 reglas. **I1→bd rc-7wus** (P2 FC-ASYNC-LIFECYCLE), M1 FC-DOC-DRIFT→rc-w5yo, M2 line-ref drift, M3 dup closures. DP-1 enrich CONTEXT.md fail-closed contract + timeout relation (refine) pending oracle. |
| camel-endpoint | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_029b7ba2) | w_fast (ses_029b15f85) | modules/camel-endpoint-quality-2026-08-05.md | L1 | **T2 batch4 (BATCH_HEAD 8a7fab95, AUTOPILOT):** GREEN 0C/0I/1M+1P. Validator 1/1 citation, **M1 README confirmed non-compiling** (`#[uri]` not recognized, real=`#[uri_param]`+`#[uri_scheme]`), 77/77 tests, 0 pub enums→ADR-0049 N/A, 9/9 reglas. M1→rc-w5yo (6º crate). DP-1 NEW CONTEXT.md (posture-table, B5 honored) pending oracle. |
| camel-endpoint-macros | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_029a68bcc) | w_fast (ses_0299f8409) | modules/camel-endpoint-macros-quality-2026-08-05.md | L1 | **T2 batch5 (BATCH_HEAD 08d103b4, AUTOPILOT):** GREEN 0C/1I/2M+1P. Validator 6/6 citations (count corrected ~30→15 error sites, 6→5 branches non-Option), 9/9 reglas, 77/77 transitive tests. **I1→bd rc-7ka6** (P2, **new FC-PROC-MACRO-TEST-GAP** trybuild), **M3→bd rc-omb8** (P3 FC-CODE-DUPLICATION collapse branches), M2 self-contained rustdoc drift. DP-1 NEW CONTEXT.md thin pointer (refine) pending oracle. |
| camel-bean | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_029b7ba1) | w_fast (ses_029b15f73) | modules/camel-bean-quality-2026-08-05.md | L1 | **T2 batch4 (BATCH_HEAD 8a7fab95, AUTOPILOT):** YELLOW 0C/1I/2M+1P. Validator 7/7 citations, **I1 BeanError valid** (pub enum API pública, lacks non_exhaustive) PERO match-eado overstated (external solo construye), broader API-stability principle holds, 23/23 tests, 9/9 reglas. **I1→bd rc-sfy1** (P2, **1ª FC-API-INSTABILITY fuera del set pre-bound → rc-3pw3/rc-ierl scope revisit**), M1→rc-w5yo, **M2→bd rc-x2gy** (P3 dead derive). **L7 N/A** (Bean EIP stateless, 0 divergence markers). DP-1 NEW CONTEXT.md pending oracle. |
| camel-bean-macros | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_029a68bb7) | w_fast (ses_0299f83f6) | modules/camel-bean-macros-quality-2026-08-05.md | L1 | **T2 batch5 (BATCH_HEAD 08d103b4, AUTOPILOT):** GREEN 0C/0I/2M+0P. Validator 5/5, **e8ea9e54 = INTENTIONAL scaffold** (git show confirmed), bean_impl exercised en camel-test (NOT stale), 16/16 tests, 9/9 reglas. M1→rc-w5yo, **M2→bd rc-2nds** (P3 loose matching). **DP-0 drop** agreed (CONTEXT-MAP:171 exempts). |
| camel-test | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_02d383120) | w_fast (ses_02d310922) | modules/camel-test-quality-2026-08-05.md | L1 | **T2 batch1 (BATCH_HEAD 95d0886b, AUTOPILOT):** GREEN 0C/0I/2M+1P. Validator 2/2 citations, 9/9 reglas, compile pass, 0 pub enums→ADR-0049 OUT confirmed. M1 FC-DOC-DRIFT→rc-w5yo, **M2→bd rc-597a** (P3 chore, camel-master dup Cargo.toml). DP-1 NEW CONTEXT.md pending oracle. |
| camel-auth | T2 | reviewed (approved) | r_glm (ses_02d383133) | w_fast (ses_02d31094d) | modules/camel-auth-quality-2026-08-05.md | L2,L3 | **T2 batch1 (BATCH_HEAD 95d0886b, AUTOPILOT):** YELLOW **1C/2I/1M**. Validator 9/9 citations, 9/9 reglas, **229/229 tests 0 locks**, C1=Critical justified (security hole), C1+I1 complete Debug-leak set, 0 prod spawns. **C1→bd rc-c9xo** (P1 security, blocks freeze), **I1→rc-fvl5** (P2, blocked-by rc-c9xo), **I2→rc-h6yv** (P2 HOL, FC-ASYNC-LIFECYCLE), M1 FC-DOC-DRIFT→rc-w5yo. **New FC: FC-DEBUG-SECRET-LEAK.** DP-1 CONTEXT.md Zeroize caveat pending oracle. |
| camel-function | T2 | reviewed (approved) | r_glm (ses_029a68ba3) | w_fast (ses_0299f83e5) | modules/camel-function-quality-2026-08-05.md | L2 | **T2 batch5 (BATCH_HEAD 08d103b4, AUTOPILOT):** YELLOW 0C/1I/0M+0P. Validator 5/5 citations, **I1 CONFIRMED** (service.rs:298-304 `?` short-circuit vs rollback_start:185 `let _ =`, both providers Ok-always → latent), 33/33 tests, 9/9 reglas. **I1→bd rc-b50f** (P2 FC-ASYNC-LIFECYCLE, ADR-0004). **L7 N/A** (function: native ADR-0005, no Camel EIP). **DP-0 drop** agreed (ADR-0005 carries invariant). |
| camel-otel | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_0292a4611) | w_fast (ses_029209723) | modules/camel-otel-quality-2026-08-06.md | L4 | **T2 batch7 (BATCH_HEAD 9e44f814, AUTOPILOT):** YELLOW 0C/3I/6M+1P. I1 OnceLock meter stale-binding (FC-LAZY-CACHE-STALE-BINDING new), I2 no Drop/global-state-leak (FC-GLOBAL-STATE-LEAK new), **I3 ADR-0051 violation** OtelConfig derive(Debug) cred-capable. DP-1 expand CONTEXT.md (refine) + open-Q global-singleton ADR. |
| camel-component-api | T2 | reviewed (approved) | r_glm (ses_02d383153) | w_fast (ses_02d310960) | modules/camel-component-api-quality-2026-08-05.md | L1 | **T2 batch1 (BATCH_HEAD 95d0886b, AUTOPILOT):** YELLOW 0C/1I/0M+1P. Validator 10/10 citations, 9/9 reglas, compile pass. I1=rc-3pw3 (verified 2 enums, no extras). DP-1 NEW CONTEXT.md pending oracle. Calibration T2 OK. |
| camel-component-llm | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_0292a2132) | w_fast (ses_029209710) | modules/camel-component-llm-quality-2026-08-06.md | L4,L5 | **T2 batch7 (BATCH_HEAD 9e44f814, AUTOPILOT):** YELLOW 0C/1I/1M+1P. **Siumai boundary STRICTLY HELD** (1 prod file, tighter than ADR-0020 doc). I1 unbounded untrusted deserialize headers DoS (FC-UNBOUNDED-UNTRUSTED-DESERIALIZE new). M1 FinishReason non_exhaustive advisory. DP-1 ADR-0020 amend "2 files"→1 (refine). |
| camel-component-wasm | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_029971b83) | w_fast (ses_0298d16f7) | modules/camel-component-wasm-quality-2026-08-05.md | L3,L5 | **T2 batch6 (BATCH_HEAD 60da8447, AUTOPILOT):** YELLOW 0C/4I/2M+1P+1T. Validator 13/15 citations (I2 2 sites mischar'd, **I3 count 2→3 corrected**), I3 airtight (real API keys, latent), StateStore=only leak, 9/9 reglas, ADR-0047 warranted. **I3→bd rc-zb1b** (P1, **FC-DEBUG-SECRET-LEAK 3ª OCURRENCIA** → ADR-revisit trigger), **I1→rc-466y** (P2 WASI linker), **I2→rc-dzd7** (P2 inherit_stderr, depends ADR-0047), **I4→rc-cgc8** (P2 host DoS). DP-1 expand CONTEXT.md + DP-2 NEW ADR-0047 (WASM sandbox) pending oracle. |
| camel-component-grpc | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_029971b6) | w_fast (ses_0298d16c3) | modules/camel-component-grpc-quality-2026-08-05.md | L3 | **T2 batch6 (BATCH_HEAD 60da8447, AUTOPILOT):** YELLOW 0C/1I/3M+0P. Validator 7/7 citations, **198/198 tests**, I1 busy-spin REAL (continue no sleep server.rs:355), **FC-DEBUG-SECRET-LEAK NO** (counter-example, manual redacting Debug, paths not bytes), 9/9 reglas. **I1→bd rc-5qao** (P2 availability), M1→rc-w5yo, M2 inject_headers reserved keys, M3 ADR-0044 concurrency inconsistency. DP-1+DP-2 pending oracle. |
| camel-component-surrealdb | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_029971b5d) | w_fast (ses_0298d16b2) | modules/camel-component-surrealdb-quality-2026-08-05.md | L3 | **T2 batch6 (BATCH_HEAD 60da8447, AUTOPILOT):** YELLOW 0C/1I/2M+0P. Validator 7/7 citations, **I1 REAL GAP** (ident-valid no defiende DDL/DML, only warn! not gate), **SurQL CLEAN** (all builders validate_identifier, $body/$vector bound), **FC-DEBUG-SECRET-LEAK NO** (counter-example holds), 9/9 reglas. **I1→bd rc-iom7** (P2 FC-TRUST-BOUNDARY-GAP, mirror camel-sql H7), M1 dead code, M2 producer.rs ~1100→rc-xctv. DP-1 NEW CONTEXT.md + DP-2 CONTEXT-MAP entry pending oracle. |
| camel-component-keycloak | T2 | reviewed (approved) | r_glm (ses_029ce8173) | w_fast (ses_029c62cf4) | modules/camel-component-keycloak-quality-2026-08-05.md | L3 | **T2 batch3 (BATCH_HEAD 49755624, AUTOPILOT):** GREEN 0C/0I/1M+1P. Validator 9/9 citations, **FC-DEBUG-SECRET-LEAK set completo confirmado** (6+10 split, no misses), 106/106 tests, lints clean, 9/9 reglas. M1→rc-w5yo (8º). DP-1 CONTEXT.md log-policy line→symbol rewrite (refine) pending oracle. Thin integration delegates crypto to camel-auth. |
| camel-jms | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_029ce8150) | w_fast (ses_029c62cb6) | modules/camel-jms-quality-2026-08-05.md | L2,L3 | **T2 batch3 (BATCH_HEAD 49755624, AUTOPILOT):** YELLOW 0C/2I/3M+2P. Validator 7/7 citations, **I1 ADR-0024 = REAL** (blind-spot #12: route_compiler:425+dsl/compile.rs:745,770 usan ConsumerStopping, ProcessorError no matchea → reply code 500 vs 503 + exception policy miss), 115/115 tests, 9/9 reglas. **I1→bd rc-0zsm** (P2, ADR-0024 betrayal — LazyJmsProducer missed in migration list), I2=CONTEXT.md anémico→DP, M1 unsupervised spawn, M2 component.rs 2056 LOC→rc-xctv, M3 comment typo. **FC-DEBUG-SECRET-LEAK NO** (BrokerConfig/Redacted/redact_url), no JNDI, ADR-0007-compliant. DP-1+DP-2 pending oracle. |
| camel-http | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_02c86e357) | w_fast (ses_029e07acd) | modules/camel-http-quality-2026-08-05.md | L3 | **T2 batch2 (BATCH_HEAD c30029c5, AUTOPILOT):** YELLOW 0C/2I/4M. Validator 6/6 citations, **clippy gate CONFIRMED FAILS** (3 `await_holding_lock`), 9/9 reglas, neg-search clean. **I1→bd rc-4vx8** (P1, clippy gate blocks CI), **I2→rc-aj1h** (P2→**P3 reclassify NON-VIOLATION per ADR-0051**: TlsConfig holds file PATHS not credential bytes; latent footgun only). M1-M4 Minors (M1/M2 dead config→rc-w5yo, M3 cred-hygiene, M4 lib.rs 6649 LOC→rc-xctv thermo). DP-1 CONTEXT.md trust-boundary pending oracle, **DP-2 dropped** (premature ADR). **L3 POSITIVE:** SSRF fail-closed+DNS pinning, redirect strip, TLS fail-closed. |
| camel-kafka | T2 | reviewed (approved) | r_glm (ses_029ce8161) | w_fast (ses_029c62ce0) | modules/camel-kafka-quality-2026-08-05.md | L2,L3 | **T2 batch3 (BATCH_HEAD 49755624, AUTOPILOT):** GREEN-YELLOW 0C/1I/3M. Validator 7/7 citations, **separate clippy gate PASS** + xtask lints OK, I1=Serialize-leak **fully latent** (0 dump/diagnostic paths), Important correct, 9/9 reglas. **I1→bd rc-xbl1** (P2, **FC-SERIALIZE-SECRET-LEAK 1ª ocurrencia** note-and-watch), M1 dup Debug, M2/M3 config>1k LOC→rc-xctv. Debug vector hardened (Batch 6). DP-1 CONTEXT.md Secret-redaction subsection pending oracle. |
| camel-sql | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_02c86e33d) | w_fast (ses_029e07ab3) | modules/camel-sql-quality-2026-08-05.md | L3 | **T2 batch2 (BATCH_HEAD c30029c5, AUTOPILOT):** GREEN 0C/0I/2M+2P. Validator 7/7 citations, **SQL injection CLEAN confirmed** (0 vulns, all bound, default-deny), compile pass, 9/9 reglas. **M1→bd rc-7rup** (P3 security-latent redact fallback), M2→rc-w5yo (Parameters docs), P2 config.rs 2104 LOC→rc-xctv thermo. DP-1+DP-2 (dep-boundary + allow_dynamic_query/ADR-0032) pending oracle. |
| camel-opensearch | T2 | reviewed (approved) | r_glm (ses_0292a211a) | w_fast (ses_0292096ff) | modules/camel-opensearch-quality-2026-08-06.md | L3 | **T2 batch7 (BATCH_HEAD 9e44f814, AUTOPILOT):** YELLOW 0C/1I/3M+1P. **Query injection CLEAN** (11/11 bound, 0 interpolated). I1 FC-TRUST-BOUNDARY-GAP doc_id unvalidated (3rd crate→epic). FC-DEBUG-SECRET-LEAK NO counter-example. DP-1 expand CONTEXT.md (refine). M1 FC-DOC-DRIFT→rc-w5yo (9º), M3 MULTISEARCH half-shipped. |
| camel-redis | T2 | reviewed (approved) | r_glm (ses_02c86e32b) | w_fast (ses_029e07a9c) | modules/camel-redis-quality-2026-08-05.md | L3 | **T2 batch2 (BATCH_HEAD c30029c5, AUTOPILOT):** GREEN 0C/0I/3M+2P. Validator 9/9 citations, **451/451 tests, 3 xtask lints pass**, L3 neg-search 3/3 clean, 9/9 reglas. M1→rc-w5yo (doc-drift 7º), **M2→bd rc-0xzt** (P3 dead sentinel feature), M3→rc-xctv thermo (config.rs 1875). **No FC-DEBUG-SECRET-LEAK** (counter-example, both config structs redact). DP-1+DP-2 (log-policy refresh + 3 additive sections) pending oracle. |
| camel-bridge | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_028c21895) | w_fast (ses_028b40530) | modules/camel-bridge-quality-2026-08-06.md | L1,L4 | **T2 batch10 (BATCH_HEAD 4a2cd4e9, AUTOPILOT):** YELLOW 0C/1I/4M+1P. Lifecycle CLEAN (no FC-ASYNC-LIFECYCLE). **I1 ADR-0051 violation** env_vars raw passwords in Debug struct. DP-1 expand CONTEXT.md (refine). |
| camel-prometheus | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_028c2186e) | w_fast (ses_028b4051f) | modules/camel-prometheus-quality-2026-08-06.md | L3,L4 | **T2 batch10 (BATCH_HEAD 4a2cd4e9, AUTOPILOT):** YELLOW 0C/3I/5M+3P. **I1 FC-METRICS-EXPOSURE** (no auth/TLS), **I2 FC-METRICS-CARDINALITY** (unbounded labels), I3 status lies. Cross-crate (otel, health). DP-1a amend ADR-0032, **DP-1b NEW ADR-0052**, DP-2 CONTEXT.md. |
| camel-cxf | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_0290e4032) | w_fast (ses_02905a600) | modules/camel-cxf-quality-2026-08-06.md | L3 | **T2 batch8 (BATCH_HEAD b6d4d7e7, AUTOPILOT):** GREEN 0C/1I/2M+1P. **XXE CLEAN by delegation** (no XML parser in Rust crate; bytes opaque to Java bridge). I1 ADR-0051 regression-test gap (redaction works, no test). DP-1 CONTEXT.md security-posture section (refine). |
| camel-xslt | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_0290e4021) | w_fast (ses_02905a5ef) | modules/camel-xslt-quality-2026-08-06.md | L3,L4 | **T2 batch8 (BATCH_HEAD b6d4d7e7, AUTOPILOT):** GREEN 0C/0I/4M+1P. **XSLT/XXE CLEAN by delegation** (zero parse; bytes to Saxon sidecar; input bounded 10MiB ADR-0040). M1 hot-path clone perf, M2 doc-drift. DP-1 CONTEXT.md expand (confirm). **Open-Q: sidecar unaudited.** |
| camel-xj | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_0290e4011) | w_fast (ses_02905a5df) | modules/camel-xj-quality-2026-08-06.md | L3,L4 | **T2 batch8 (BATCH_HEAD b6d4d7e7, AUTOPILOT):** GREEN 0C/1I/2M+1P. **XXE CLEAN** (zero in-process parse; bytes to Saxon bridge). I1 dead config options transformDirection/resourceUri silently ignored (FC-DEAD-CONFIG new). DP-1 CONTEXT.md expand (confirm). |
| camel-language-js | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_028f9dc54) | w_fast (ses_028d47456) | modules/camel-language-js-quality-2026-08-06.md | L3,L5 | **T2 batch9 (BATCH_HEAD 6207ea45, AUTOPILOT):** GREEN 0C/0I/4M+3P. **SANDBOXED** (boa 0 host APIs, 5/5 limits, null-proto hardening, no injection path). Boa boundary CONFINED. DP-1 NEW CONTEXT.md + DP-2 README + DP-3 parent list. |
| camel-language-rhai | T2 | reviewed (approved) | r_glm (ses_028f9dc43) | w_fast (ses_028d46090) | modules/camel-language-rhai-quality-2026-08-06.md | L3,L5 | **T2 batch9 (BATCH_HEAD 6207ea45, AUTOPILOT):** GREEN 0C/1I/4M+1P. **SANDBOXED** (no_module, max_ops=100k, 5s timeout). Rhai boundary CLEAN. I1 max_call_levels unconfigurable (8 dbg/64 rel). DP-1 NEW CONTEXT.md. |
| camel-language-jsonpath | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_028c21859) | w_fast (ses_028b4050e) | modules/camel-language-jsonpath-quality-2026-08-06.md | L1,L4 | **T2 batch10 (BATCH_HEAD 4a2cd4e9, AUTOPILOT):** YELLOW 0C/1I/2M+2P. **FC-LANG-RECOMPILE confirmado 2/3** (igual que xpath: parse→discard→recompile per Exchange). DP-1 parent list + DP-2 NEW CONTEXT.md. |
| camel-language-xpath | T2 | reviewed (approved-with-minor-fixes) | r_glm (ses_028f9dc33) | w_fast (ses_028d4607b) | modules/camel-language-xpath-quality-2026-08-06.md | L3 | **T2 batch9 (BATCH_HEAD 6207ea45, AUTOPILOT):** YELLOW 0C/1I/4M+2P. **XPath-injection/XXE CLEAN** (sxd pure-Rust, no ENTITY parser, document() disabled). I1 recompile-per-Exchange perf regression (FC-LANG-RECOMPILE new). DP-1 parent list + DP-2 NEW CONTEXT.md. |
| camel-log | T3 | reviewed (approved-with-minor-fixes) | r_glm (ses_0289c1560) | w_fast (ses_02891b7be) | modules/camel-log-quality-2026-08-06.md | L2,L4 | **T3 batch1 (BATCH_HEAD 0e2b5ca5):** YELLOW 0C/2I/3M+1P. I1 truncate multibyte panic, I2 gating untested. ADR-0012 COMPLIANT. |
| camel-mock | T3 | reviewed (approved-with-minor-fixes) | r_glm (ses_0289c154f) | w_fast (ses_02891b7ac) | modules/camel-mock-quality-2026-08-06.md | L1 | **T3 batch1 (BATCH_HEAD 0e2b5ca5):** YELLOW 0C/2I/5M+0P. I1 dead feature (fail_fast never set), I2 clone drops Body::Stream. |
| camel-timer | T3 | reviewed (approved) | r_glm (ses_0289c153f) | w_fast (ses_02891b79c) | modules/camel-timer-quality-2026-08-06.md | L2 | **T3 batch1 (BATCH_HEAD 0e2b5ca5):** GREEN 0C/0I/3M+0P. Lifecycle PASS. M2 u32 overflow ~49d. |
| camel-direct | T3 | reviewed (approved-with-minor-fixes) | r_glm (ses_0289c1530) | w_fast (ses_02891b78d) | modules/camel-direct-quality-2026-08-06.md | L1 | **T3 batch1 (BATCH_HEAD 0e2b5ca5):** GREEN 0C/0I/5M+1P. FC-DEAD-CONFIG 3ª occ (block/exchange_pattern silent drop). DP-1 line-ref fix. |
| camel-file | T3 | reviewed (approved-with-minor-fixes) | r_glm (ses_0289c1520) | w_fast (ses_02891b77e) | modules/camel-file-quality-2026-08-06.md | L2 | **T3 batch1 (BATCH_HEAD 0e2b5ca5):** GREEN 0C/0I/4M+1P. Path-traversal SOUND. Thermo 3743 LOC. DP-1 line-ref drift. |
| camel-master | T3 | reviewed (approved) | r_glm (ses_028227b26) | w_fast (ses_0281b77d8) | modules/camel-master-quality-2026-08-06.md | L2 | **T3 batch2:** YELLOW 0C/1I/3M+1P. I1 stop_delegate error-path skip drain. FC-ASYNC-LIFECYCLE 4º crate. | leadership |
| camel-validator | T3 | reviewed (approved) | r_glm (ses_028227b03) | w_fast (ses_0281b77c4) | modules/camel-validator-quality-2026-08-06.md | L1 | **T3 batch2:** YELLOW 0C/1I/4M+2P. I1 README claims bridge cleanup but 0 impl Lifecycle. | |
| camel-ws | T3 | reviewed (approved) | r_glm (ses_02764b098) | w_fast (ses_0275b5aa8) | modules/camel-ws-quality-2026-08-06.md | L2,L3 | **T3 batch3:** YELLOW 0C/2I/5M+1P. I1 send_timeout vestigial (FC-DEAD-CONFIG 3ª occ), I2 TLS readiness asymmetry. Lifecycle+security CLEAN. | |
| camel-controlbus | T3 | reviewed (approved-with-minor-fixes) | r_glm (ses_02764b083) | w_fast (ses_0275b5a98) | modules/camel-controlbus-quality-2026-08-06.md | L1 | **T3 batch3:** YELLOW 0C/1I/1M+2P. I1 README drift security-adjacent. | |
| camel-container | T3 | reviewed (approved) | r_glm (ses_027545d79) | w_fast (ses_0274d640b) | modules/camel-container-quality-2026-08-06.md | L5 | **T3 batch4:** YELLOW 0C/2I/4M+3P. L5 CONFINED. I1 cleanup ignora docker_host, I2 thermo. | |
| camel-dataformat-protobuf | T3 | reviewed (approved) | r_glm (ses_027545d69) | w_fast (ses_0274d63fa) | modules/camel-dataformat-protobuf-quality-2026-08-06.md | L1 | **T3 batch4:** GREEN 0C/0I/4M+0P. Leaf crate, 0 DPs. | |
| camel-language-api | T3 | reviewed (approved) | r_glm (ses_027442ef5) | w_fast (ses_024d153ef) | modules/camel-language-api-quality-2026-08-06.md | L1 | **T3 batch5:** YELLOW 0C/2I/3M+3P. ADR-0049 compliant (lint exit 0). SPI contract doc-drift (phantom symbols). DP-1 README+DP-2 CONTEXT.md+DP-3 lib.rs doc. |
| camel-language-simple | T3 | reviewed (approved) | r_glm (ses_027442ee2) | w_fast (ses_024d153ca) | modules/camel-language-simple-quality-2026-08-06.md | L1 | **T3 batch5:** GREEN 0C/0I/0M+1P. FC-LANG-RECOMPILE CACHES (not affected). | |
| camel-platform-kubernetes | T3 | reviewed (approved) | r_glm (ses_024c73ffd) | w_fast (ses_024be4dcf) | modules/camel-platform-kubernetes-quality-2026-08-06.md | L1,L2,L5 | **T3 batch6:** GREEN 0C/0I/4M+1P. Lifecycle CLEAN. Dep-boundary PARTIAL (kube::Client 2 pub ctors). DP-1 CONTEXT.md. |
| camel-proto-compiler | T3 | reviewed (approved-with-minor-fixes) | r_glm (ses_024c73fed) | w_fast (ses_024be4dbf) | modules/camel-proto-compiler-quality-2026-08-06.md | L1 | **T3 batch6:** YELLOW 0C/1I/2M+1P. I1 shared temp_dir process-local counter → clobber parallel builds. |
| camel-bench | T3 | reviewed (approved) | r_glm (ses_024b572de) | w_fast (ses_024acaa5d) | modules/camel-bench-quality-2026-08-06.md | L1 | **T3 batch7:** GREEN 0C/0I/3M+0P. Trivial (0 pub items, publish=false). |
| camel-wit | T3 | reviewed (approved) | r_glm (ses_024b572cd) | w_fast (ses_024acaa4b) | modules/camel-wit-quality-2026-08-06.md | L1,L5 | **T3 batch7:** YELLOW 0C/2I/3M+2P. I1 dead code en contract crate (causa dep camel-api), I2 host duplica .wit (filediff frágil). |
| camel-component-seda | T3 | reviewed (approved) | r_glm (ses_024a3a717) | w_fast (ses_0249aaab8) | modules/camel-component-seda-quality-2026-08-06.md | L1,L2 | **T3 batch8:** YELLOW 0C/1I/4M+2P. I1 concurrentConsumers>1 no entrega concurrencia (1 forwarder serializa). Lifecycle PASS-with-notes. Oracle DP-16/DP-17 committed (`4a2cbdc0`, `c555fef4`). |

**Status legend:** pending → in_progress → drafted → reviewed (approved | approved-with-minor-fixes | needs-rework) → done.

## Tracking de documentation proposals

> Una fila por proposal detectada en cualquier crate. Status cuando el oráculo commitea → `committed`.

| DP-ID | Crate | Tipo | Target | Status | Oracle call | Commit | Notes |
|---|---|---|---|---|---|---|---|
| DP-1 | camel-processor | context-md-update | CONTEXT.md | committed | ses_10ee94925 (e_gpt) | 272120c2 | 3 secciones aditivas (EIP catalog, Public API surface v1.0, poll_ready contract); self-grill: refine; validator approved técnico |
| DP-2 | camel-processor | adr-amendment | ADR-0019 | committed (refined) | ses_10ee94925 (e_gpt) | 3613eb3d | Enumeración normativa de processors bound by Ready(Ok(())) contract; oracle refinement: Splitter+StreamingSplitter movidos a pending-fix (auditor los tenía como migrated); self-grill: confirm; validator approved técnico |
| DP-3 | camel-processor | adr-amendment | ADR-0019 + CONTEXT.md mirror | committed | ses_0326933dbffe (e_opus ronda-3) | 1df205cb | Añadir `FilterService` a la enumeración pending-fix (mismo patrón que Splitter: propaga sub_pipeline.poll_ready, filter.rs:45-47). Hallado post-oracle por orquestador review. Ver I3 en audit report. **Re-confirmado en re-run v4.2 (2026-08-04):** validator w_fast verifica FilterService ausente (0 hits). Congelado ~6 semanas. Oracle ronda-3 provee literal + fila espejo en CONTEXT.md (`:109-119`) — **mismo commit** para no reintroducir drift (riesgo R4). |
| DP-4 | camel-processor | context-md-update | CONTEXT.md | committed | ses_0326933dbffe (e_opus ronda-3) | 1df205cb | M7+M9 (re-run v4.2): catálogo EIP stale — EIPs nuevos (claim_check, idempotent_consumer, json_schema_validate, resequencer, sampling, sort, validate) ausentes; fila `IdempotentConsumerSegment` falta; nueva §"Stateful repository EIPs (ADR-0046 retro-exempt)" documentando contratos de trait desde comportamiento existente (ejemplar canónico de resolución voluntaria, blind spot #16). Reporte decía "35 módulos", real 46 `pub mod`. Oracle provee 3 ediciones literales (A/B/C). |

| DP-A | camel-processor | context-md-update | CONTEXT.md | committed | ses_02e86e091 (e_gpt) | 0b472757 | Re-run v4.3 (2026-08-05, shakedown end-to-end): catalog aggregator stale ("auto-spawned at construction" → lazy-on-first-poll_ready) + tabla poll_ready line-refs drifted → **citation-by-symbol (B4)**. Self-grill: refine. Open-Q resuelta por oracle: símbolos, no líneas. |
| DP-B | camel-processor | context-md-update | CONTEXT.md | committed (corrected) | ses_02e86e091 (e_gpt) | 0b472757 | §ADR-0012 log-policy sites: auditor dijo 8, **oracle corrigió a 11** (7 handler-owned + 3 system-broken + 1 post-ack) — B6 materialization completeness funcionó como diseñado. Self-grill: confirm. Mismo commit que DP-A (R4). |

| DP-5 | camel-api | context-md-update | crates/camel-api/CONTEXT.md | committed | ses_02e5d4308 (e_opus) | 193a959f | Añadir entradas glosario `ClaimCheckRepository` + `IdempotentRepository` (explicitar key-only vs payload-bearing, ADR-0028). Self-grill: confirm. Additivo, cita ADR-0023/0028. Oracle refinement: rewrote Spanish-mixed draft → English (file language), added line refs (`idempotent.rs:21`, `claim_check.rs:27`), canonical-impl cross-links to camel-core. B6 revalidación full-section: 0 drift adicional. Reporte local: DP-1. |
| DP-6 | camel-api | new-adr | docs/adr/0049-*.md | committed | ses_02e5d4308 (e_opus) | ec65888d | **Política workspace `#[non_exhaustive]` v1.0 contract enums.** Oracle APPROVE as ADR-0049 (not deferrable — freeze es el deadline). Scope: 3 contract crates (camel-api, camel-component-api, camel-language-api); exceptions: `PipelineOutcome` (ADR-0024), `ExchangePattern`; variant-name-guarded enums keep non_exhaustive. Execution (13 enums) → code stream vía I2. Self-grill: refine→open-question; oracle decide política. **Pre-bound:** camel-component-api + camel-language-api audits aplican 0049 + filen su I2-equivalente, no re-litigan. Reporte local: DP-2. |
| DP-7 | camel-dsl | context-md-update | crates/camel-dsl/CONTEXT.md | committed | ses_02e135aea (e_opus) | 8c9f95aa | Log-policy table stale: line numbers drift (75/1277 → 100/137/1786) + missing 3rd site (`extract_rest_blocks`). 3 `error!` sites confirmados en `yaml.rs`. Self-grill: confirm. Additivo. Oracle: approve; **corrigió auditor** — dropped "(c)" category letter (ADR-0012 (c)=route-lifecycle; DSL parse failures son class `system-broken`, no letter-code). B6 completeness: validó todas las same-crate citations, 0 drift adicional. **Workflow lesson:** future log-policy proposals citan ADR-0012 *class* (system-broken/outside-contract/handler-owned), no letter-code. Reporte local: DP-1. |
| DP-8 | camel-dsl | context-md-update (posture-table) | crates/camel-dsl/CONTEXT.md | committed | ses_02e06c7a6 (e_opus) | 98ace84e | Posture-table crate-local `#[non_exhaustive]` para 6 types I1 (todos **yes**): DeclarativeRoute, DeclarativeStepKind, DeclarativeStep, DeclarativeSecurityPolicy, DeclarativeConcurrency, RouteDslStep. Cita ADR-0049 §Rule 3 como framework, SIN extender scope obligatorio 0049. **+27 líneas additivo**, B6 completeness OK. Resolución temprana de rc-vmrr (pre-sweep) per oracle ses_02e0d55d3 — signal completo (config=selectivo, cli=N/A, builder=N/A). Mechanical attribute app → code stream rc-3pw3. Reporte local: DP-2. |
| DP-9 | camel-config | context-md-update (NEW FILE) | crates/camel-config/CONTEXT.md | committed | ses_02ddeca64 (e_opus) | 0a720767 | Crear `crates/camel-config/CONTEXT.md` (único T1 sin CONTEXT.md). 95 LOC: header + 4 terms glossary (CamelConfig, ComponentsConfig, PropertiesResolver, DiscoveryError disambiguated de camel-dsl) + posture table `#[non_exhaustive]` "selective" (4 rows) framed contra ADR-0049 §Rule 3 + Exceptions clause + camel-config OUT scope binding + camel-dsl DP-8 precedent + architecture notes (enum split, hot-reload wiring M1, discovery delegation L7 N/A). Oracle corrigió line-refs L599/L615 → L597/L613. Counts rg-verificados: 27 pub struct, 2 non_exhaustive, 4 pub enum. **M3 routed code stream** (remove non_exhaustive 2 native-auth structs — non-breaking widening, aligns 27/27). M1/M2 post-v1.0. Self-grill: refine. Reporte local: DP-1. |
| DP-10 | camel-cli | context-md-update | crates/camel-cli/CONTEXT.md | committed | ses_02db49bb6 (e_opus) | 7f9d8a03 | Log-policy table stale: 7→**10 sitios**, code (c)→**(d)** per ADR-0012 l.19 (CLI/bootstrap = system-broken class, lead con class no letter-code per workflow lesson ses_02e135aea). 10 rows citados por símbolo (`async fn run` ×9, `fn maybe_instrument_routes` ×1). Counts rg-verificados. Patrón DP-7/DP-A. **B6 halló dead citation `TODO(PROC-004)` l.31 (0 markers en tree) → follow-up sweep/umbrella FC-DOC-DRIFT.** Self-grill: refine. Reporte local: DP-10. |
| DP-11 | camel-builder | context-md-update (NEW FILE) | crates/camel-builder/CONTEXT.md | committed | ses_02d907713 (e_opus) | c4a791f7 | Crear `crates/camel-builder/CONTEXT.md` (132 LOC, English). Header + glossary 5 terms (`RouteBuilder` w/ camel-dsl disambiguation, `StepAccumulator` default-only/un-sealed + B4 non-symbol warning, `BuilderStep` re-export, child-builder typestate-via-parent-ownership, build/build_canonical v2) + non_exhaustive posture N/A (0 pub enums, ADR-0049 §Scope) + architecture notes (consuming-self, panic-vs-Result decision-noted-not-prescribed → I1 code stream, non-Clone BUILDER-003, thread-safety, L7 N/A) + related decisions. Oracle corrigió counts 17→**16 pub struct**. L6 #2 additive, B6 OK. Reporte local: DP-1. |
| DP-12 | camel-builder | context-map-update | CONTEXT-MAP.md:7 + camel-dsl/CONTEXT.md:80 | committed | ses_02d907713 (e_opus) | ac55f1a6 | **Option 1 scope-extended (3-way drift):** CONTEXT-MAP.md:7 era factual error (atribuía "fluent builder API" al DSL crate). Split en **Builder** entry (programático `RouteBuilder`) + corrected **DSL** entry (declarativo `RouteDslRoute`); reconcile camel-dsl/CONTEXT.md:80 → apunta al real owner (camel-builder/src/lib.rs:358) para evitar glossary-ownership collision. Authority order CONTEXT-MAP.md:135 (source-is-truth). **No otras líneas T1 ambiguas** (oracle full-scan). Non-blocking sweep flag: Contexts list missing camel-config/camel-processor entries (completeness, T2/T3). Self-grill: refine→open-Q; oracle Option 1. Reporte local: open-Q. |
| DP-13 | camel-otel | context-md-update | crates/services/camel-otel/CONTEXT.md | committed | current session (e_gpt) | bb3ab93a | Glosario + invariantes global-provider/meter-cache + postura ADR-0049 case-by-case. Oracle decide no ADR workspace: constraint inherente de OpenTelemetry y uso productivo confined al crate. Self-grill: refine. Reporte local: DP-1. |
| DP-14 | camel-component-llm | adr-amendment | ADR-0020 + tracked prose | committed | current session (e_gpt) | b867f673 | Corrige boundary factual de 2→1 production file; actualiza lib.rs, README y CONTEXT-MAP. Decisión arquitectural sin cambio. Self-grill: refine. Reporte local: DP-1. |
| DP-15 | camel-opensearch | context-md-update | crates/components/camel-opensearch/CONTEXT.md | committed | current session (e_gpt) | b6d4d7e7 | Glosario + postura ADR-0049 OUT + arquitectura + trust boundary ADR-0032. 11 paths: 10 typed requests, MULTISEARCH fails pre-request; doc_id gap queda pending en rc-25j3. Self-grill: refine. Reporte local: DP-1. |
| DP-16 | camel-component-seda | context-md-update (NEW FILE) | crates/components/camel-component-seda/CONTEXT.md | committed | current session (e_gpt) | 4a2cbdc0 | SEDA staging model, InOut single-forwarder limit (`rc-exa2`), honored shutdown contract, and ADR-0049 N/A posture. Self-grill: confirm. Reporte local: DP-1. |
| DP-17 | camel-component-seda | context-md-update | crates/components/camel-component-seda/README.md | committed | current session (e_gpt) | c555fef4 | Features text no longer promises parallel processing from `concurrentConsumers`; it states the InOut single-forwarder limit. Self-grill: confirm. Reporte local: DP-2. |

**Status legend:** pending-oracle-approval → approved → committed (con hash) / rejected → archived / needs-info → returned-to-auditor.

## Tracking por finding class (cross-crate patterns)

> Si N crates fallan por la misma clase → issue umbrella en beads.

| Finding class | Descripción | Crates afectados | Tickets |
|---|---|---|---|
| FC-URI-PARITY | URI options que no matchean Apache Camel 4.x | — | — |
| FC-UNWRAP-HOT | `.unwrap()`/`.expect()` en hot path sin `// allow-unwrap` | — | — |
| FC-LOG-POLICY | `error!` sin `// log-policy:` annotation | — | — |
| FC-CAMEL-ERROR-STOPPED | `CamelError::Stopped` en sitios no-grandfathered | — | — |
| FC-DRIFT-ADR | Traición directa de un ADR | camel-processor (I1 WireTap, I2 LoadBalancer, I3 Filter — todos ADR-0019) | — |
| FC-COVERAGE-GAP | Path/test no cubierto significativo | — | — |
| FC-CONTEXT-STALE | CONTEXT.md desactualizado vs código | camel-processor (DP-1 → committed) | — |
| FC-DEP-LEAKAGE | Dependencia externa fuera de su jaula | — | — |
| FC-CROSS-CRATE-INVARIANT | Invariante cross-crate rota | — | — |
| FC-API-INSTABILITY | API pública sin `#[non_exhaustive]`/breaking change | camel-api (I2 enums contrato, M4 since drift), camel-dsl (I1 6 public types sin non_exhaustive decision pre-freeze), camel-config (M3 2/27 structs non_exhaustive inconsistente — NARROW: structs config no deben), camel-bean (I1 BeanError pub enum sin non_exhaustive → rc-sfy1, 1ª fuera del set pre-bound), camel-component-api (I1 → rc-3pw3) | rc-3pw3 (execute ADR-0049), rc-ierl (xtask lint gate) |
| FC-ASYNC-LIFECYCLE | Spawned task/shutdown leak | camel-processor (I1 WireTap spawn+poll_ready redundante, I3 Filter sub_pipeline propagation), camel-health (I1 5s shutdown vs 6s handler timeout, 0 `.abort()` → rc-7wus), camel-function (I1 `?` short-circuit vs rollback `let _` → rc-b50f), camel-jms (I1 ADR-0024 error-match miss, LazyJmsProducer → rc-0zsm), camel-master (T3 I1 stop_delegate error-path skips epoch-bump drain → rc-97gf) — **4 crates, individualmente ticketeados; heterogéneos (NO epic — decisión T2 sweep mantenida en T3: shutdown-race / rollback-short-circuit / ADR-0024-match-miss / stop_delegate-skip no comparten shape)**. camel-bridge/camel-timer/camel-function CLEAN-with-notes. | rc-7wus, rc-b50f, rc-0zsm, rc-97gf (individuales; sin epic — fixes no comparten shape) |
| FC-DEBUG-SECRET-LEAK | Type con bytes de credencial expone secreto vía `Debug`/`Serialize` (ADR-0051 governs) | **4 violaciones:** camel-auth (C1 → rc-c9xo, active P1), camel-component-wasm (I3 StateStore → rc-zb1b, latent P1), camel-otel (I3 OtelConfig → rc-2g5v, P2), camel-bridge (I1 env_vars raw passwords → rc-4tbt, P2). **1 reclasificado NON-violation:** camel-http (rc-aj1h, TlsConfig paths≠bytes per ADR-0051 §sharpen). **1 test-gap:** camel-cxf (rc-ryl0, redacción funciona, falta regression test). **4 counter-examples** (redacción manual correcta): redis, keycloak, grpc, opensearch. | ADR-0051 (governs); rc-c9xo (P1 blocks freeze), rc-zb1b, rc-2g5v, rc-4tbt, rc-ryl0 |
| FC-SERIALIZE-SECRET-LEAK | Config type con bytes de credencial deriva `Serialize` (ADR-0051 §Serialize rule) | camel-kafka (I1 → rc-xbl1, fully latent, note-and-watch) | rc-xbl1 (P2) |
| FC-TRUST-BOUNDARY-GAP | Header/body de exchange no-confiable maneja path/query segment sin validar mientras config confiable SÍ se valida (ADR-0032 + ADR-0033 Require-Explicit-Choice) | camel-sql (H7, audit report), camel-component-surrealdb (rc-iom7), camel-opensearch (rc-25j3) — **3 crates → epic** | **rc-be30 (epic, sweep-filed)** ← rc-iom7, rc-25j3 children; sql H7 referenced |
| FC-LANG-RECOMPILE | Expresión de lenguaje parseada→descartada→recompilada por Exchange (perf) | camel-language-xpath (rc-jla3), camel-language-jsonpath (JPT), camel-language-rhai (rc-qaom) — **3 crates → epic**. minijinja CACHES (no afectado); js re-parsea per eval (sandbox-driven, distinto) | rc-flny (epic, existente) |
| FC-METRICS-EXPOSURE | Endpoint de diagnóstico sin auth/TLS, bind-default 0.0.0.0 | camel-prometheus (I1 → rc-asm9), compartido con camel-health (mismo health_router) | **rc-asm9; ADR-0052 (sweep-committed 0e2b5ca5)** |
| FC-METRICS-CARDINALITY | Label values de métricas sin cap/eviction/sanitización (DoS latente ADR-0032) | camel-prometheus (I2 → rc-0pyv); contract workspace-wide en MetricsCollector (afecta camel-otel) | **rc-0pyv; ADR-0032 amendment (sweep-committed 0e2b5ca5)** |
| FC-PROC-MACRO-TEST-GAP | Proc-macro sin trybuild/compile-fail coverage | camel-endpoint-macros (I1 → rc-7ka6) | rc-7ka6 (P2) |
| FC-UNBOUNDED-UNTRUSTED-DESERIALIZE | Deserialización de dato no-confiable sin bound (DoS) | camel-component-llm (I1 headers → rc-wvty) | rc-wvty |
| FC-LAZY-CACHE-STALE-BINDING | `OnceLock`/lazy cache liga un recurso stale tras reconfiguración | camel-otel (I1 meter → rc-z0y3) | rc-z0y3 |
| FC-GLOBAL-STATE-LEAK | Estado global sin `Drop`/teardown (leak entre lifecycles) | camel-otel (I2 → rc-3ixr) | rc-3ixr |
| FC-SCRIPT-SANDBOX-COHERENCE | Knobs de sandbox de script no uniformes entre engines | camel-language-js, camel-language-rhai (engine knobs divergentes) | — (advisory) |
| FC-DEAD-CONFIG | Opción de config parseada+testeada+documentada pero nunca consumida/enforced (silenciosamente ignorada en runtime) | camel-xj (I1 transformDirection/resourceUri → rc-1v0s), camel-ws (I1 send_timeout vestigial → rc-yaep), camel-direct (M3 block/exchange_pattern Option fields silent drop, lib.rs:74,78 — doc-only, fold en epic) — **3 instances → EPIC (homogéneo: fix shape = implement-or-remove per field)** | **rc-p0ta (epic, T3-sweep-filed)** ← rc-1v0s, rc-yaep children; direct M3 folds in |
| FC-DEAD-CODE/SCAFFOLDING | Código muerto / aspiracional shipped en crate publicado (feature nunca activada, struct/machinery no consumida, contract-crate con runtime code) | camel-mock (I1 fail_fast_error never assigned → rc-zx30), camel-dataformat-protobuf (M2 dead ProtobufConfig struct, lib.rs:34), camel-wit (I1 WitHost dead runtime code en contract crate → rc-m9nn), camel-component-seda (I1 dead Vec<JoinHandle> machinery → rc-exa2) — **4 instances → NO epic (heterogéneo: dead feature flag / dead config struct / dead contract-crate runtime / dead JoinHandle machinery no comparten shape — mismo razonamiento que FC-ASYNC-LIFECYCLE). Track, individualmente ticketeados.** Nota: FC-PONYTAIL-DEAD-CODE (file M2/M3, direct, protobuf M3, bench M1/M3) es sub-patrón ponytail relacionado (dead code en production paths), no re-epic'd. | rc-zx30, rc-m9nn, rc-exa2 (individuales; protobuf M2 doc-only). Sin epic. |
| FC-DUPLICATE-CONTRACT-TYPE | Crate redefine `pub enum` que sombrea el tipo canónico de `camel-api` en vez de re-exportarlo | camel-component-seda (M1 `pub enum ExchangePattern` lib.rs:50 sombrea `camel_api::ExchangePattern` exchange.rs:47), camel-jms (config.rs:155 mismo smell) — **2 instances → WATCH (no epic aún; umbral ≥3). 3ª ocurrencia dispara epic.** | — (note-and-watch) |
| FC-UNTRACKED-TODO | TODO en código sin bd tracking | camel-language-xpath (XPH-*), camel-language-jsonpath (JPT-004), camel-direct (DIR-001/005), camel-dataformat-protobuf (VAL-013) — patrón continúa de T2 | — (fold en rc-w5yo doc-drift) |
| FC-XSLT-SIDECAR-SECURITY-GAP | Crate XML delega parse a sidecar Saxon/Java no auditado (XXE/XSLT/entity) | camel-cxf, camel-xslt, camel-xj — **3 crates delegan → epic** | rc-krpx (epic, existente) |
| FC-DOC-DRIFT | README/CONTEXT.md no coincide con código | **T1+T2:** camel-api, camel-config, camel-dsl, camel-cli, camel-builder, camel-health (M1), camel-endpoint (M1), camel-bean (M1), camel-component-grpc (M1), camel-kafka, camel-opensearch (M1 9º), camel-keycloak (M1 8º), camel-redis (M1 7º), camel-prometheus (M3), camel-jms (I2). **T3 (~14 crates):** camel-log, camel-mock, camel-timer, camel-direct, camel-file, camel-validator (I1 README claims bridge cleanup, 0 impl), camel-ws, camel-controlbus (I1 README drift security-adjacent), camel-container, camel-dataformat-protobuf, camel-language-api (SPI phantom symbols), camel-platform-kubernetes, camel-proto-compiler, camel-component-seda. — **~24+ crates cross-tier** | rc-w5yo (epic), rc-bwbg (baseline child), rc-9h5a (xtask lint-context-citations child) |
| FC-THERMO-NUCLEAR-FILE-SIZE | Módulos >1k LOC (maintainability, no correctness) | **T1:** camel-processor, camel-dsl, camel-builder, camel-config. **T2:** camel-jms (component.rs 2056), camel-http (lib.rs 6649), camel-sql (config.rs 2104), camel-redis (config.rs 1875), camel-kafka, camel-component-surrealdb (producer.rs ~1100). **T3:** camel-file (3743 LOC), camel-ws (3198), camel-container (2632), camel-component-seda (1717). | rc-xctv (epic, P3 post-v1.0) |
| FC-BEHAVIORAL-PARITY-GAP | EIP con marcador de divergencia ADR-0046 cuya divergencia NO está en tracked doc al auditar (≠ FC-DRIFT-ADR traición de ADR; ≠ FC-COVERAGE-GAP test faltante). Cada fila enlaza su bd (`gap-coverage`). Disparador del verbo-2 de L7. | — (T2: L7 N/A o verified en todos los crates auditados) | — |

Cuando una clase acumula ≥3 crates → `bd create "Issue umbrella: <class>" -t epic --deps discovered-from:rc-ca8z` (epic de positioning activo). **Corrección v4.2:** umbrellas de clases de parity/premisa escalan a `rc-ca8z`, no a `rc-6z4` (rc-6z4 es observacional, consume findings como evidencia, no los posee).

> **T2 SWEEP (2026-08-06, oracle e_opus, HEAD `0f3a067`→commit `0e2b5ca5`).** T2→T3 crossing **APPROVED**. Decisiones: (A) **ADR-0052** creado (diagnostic-endpoint exposure posture, FC-METRICS-EXPOSURE/rc-asm9); (B) **ADR-0032 amendment** (metric label values → unbounded-sink roster, FC-METRICS-CARDINALITY/rc-0pyv) — ambos commit `0e2b5ca5` + prometheus CONTEXT.md B6 coherence. Epic nuevo: **rc-be30** (FC-TRUST-BOUNDARY-GAP, 3 crates, children rc-iom7+rc-25j3, sql H7 ref). **FC-ASYNC-LIFECYCLE: NO epic** — 3 crates individualmente ticketeados (rc-7wus/rc-b50f/rc-0zsm) pero fixes heterogéneos (shutdown-race / rollback-short-circuit / ADR-0024-match-miss) sin shape común; un epic sería bolsa de fixes inconexos. **0 contradicciones cross-crate.** ADR-0051 gobierna las 6 FC-DEBUG/SERIALIZE-SECRET-LEAK (4 violaciones + 1 reclasif + 1 test-gap); postura non_exhaustive coherente (service crates N/A, ADR-0049 respetado). Umbrellas ya existentes verificados: rc-w5yo (doc-drift), rc-flny (lang-recompile), rc-krpx (xslt-sidecar), rc-xctv (thermo). Cruce condicional: rc-c9xo (P1 auth secret-leak) **blocks freeze**.

> **T3 SWEEP (2026-08-07, oracle e_opus, HEAD `c555fef4`, tree clean) — AUDITORÍA COMPLETA (55 crates: T1=7, T2=30, T3=18).** No hay tier siguiente; este es el **gate final**. **Epic decisiones:** (A) **FC-DEAD-CONFIG → EPIC `rc-p0ta`** filed (3 instances xj/ws/direct, homogéneo: fix shape = implement-or-remove per field; children rc-1v0s + rc-yaep, direct M3 folds in). (B) **FC-DEAD-CODE/SCAFFOLDING → NO epic** (4 instances rc-zx30/protobuf-M2/rc-m9nn/rc-exa2 heterogéneos: dead feature / dead config struct / dead contract-crate runtime / dead JoinHandle machinery — mismo criterio que FC-ASYNC-LIFECYCLE; track individual). **Nuevas FC:** FC-DEAD-CODE/SCAFFOLDING, FC-DUPLICATE-CONTRACT-TYPE (seda M1 + jms, 2 instances → WATCH, 3ª dispara epic). **Consolidación:** FC-DOC-DRIFT +14 T3 crates (~24 total → rc-w5yo); FC-THERMO-NUCLEAR +4 T3 files (file 3743/ws 3198/container 2632/seda 1717 → rc-xctv); FC-ASYNC-LIFECYCLE +camel-master (4º, decisión NO-epic mantenida → rc-97gf); FC-UNTRACKED-TODO +direct/protobuf. **Contradicción check (T1+T2+T3):** 0 contradicciones. Ningún CONTEXT.md/ADR de T3 contradice T1/T2. Postura `#[non_exhaustive]` coherente cross-tier (contract crates ADR-0049; component/config crates N/A o selective — 0 conflicto). **ADR-0053 (WIT versioning)** no contradice nada (camel-wit contract-crate scope; rc-m9nn es dead-code, no versioning conflict). **Patrones cross-tier (55 crates):** (1) FC-DOC-DRIFT es el patrón #1 universal (~24 crates, todos → rc-w5yo — señal de que doc-maintenance no escaló con delivery velocity); (2) FC-THERMO-NUCLEAR persiste en todos los tiers (giant files son estructural, no tier-specific → rc-xctv post-v1.0); (3) dead-code/dead-config aparece recién en T3 (componentes pequeños shipping aspirational surface). **FREEZE GATE STATE:** rc-3pw3 + rc-ierl **CERRADOS** (ADR-0049 ejecutado + lint gate live) → condición de cruce T1-sweep satisfecha. **Open P1s al cierre de sweep (5, `bd list --status open -p 1`):** (1) **rc-c9xo** camel-auth signed-JWT Debug leak — el único que se auto-declara "Blocks v1.0.0 freeze" (P1 security, active); (2) **rc-zb1b** camel-component-wasm StateStore secrets leak (FC-DEBUG-SECRET-LEAK 3ª, latent P1); (3) **rc-4vx8** camel-http clippy gate FAILS 3× await_holding_lock (CI-breaking → de-facto freeze-relevant, un quality-gate rojo no debe congelarse); (4) **rc-aaxe** WIT package versioning pre-v1.0 (ADR-0053 scope); (5) **rc-ca8z** roadmap epic de positioning (NO freeze blocker — es el paraguas observacional). **Verdict freeze:** cerrar rc-c9xo + rc-zb1b + rc-4vx8 (los 3 P1-bug técnicos) antes de freeze; rc-aaxe según decisión ADR-0053. rc-ca8z permanece abierto por diseño. **AUDITORÍA OBSERVACIONAL CERRADA** — findings de código limpios listos para triage post-auditoría → conductor-light.

## Known blind spots

1. **Cross-crate invariants profundas** — audit es crate-local; solo parcial vía negative search.
2. **Public API / semver freeze** — L1 ayuda pero no es análisis formal (necesitaría cargo-semver-checks).
3. **Feature flag matrix** — no probamos todas las combinaciones.
4. **Async task lifecycle leaks** — requeriría tokio-console.
5. **Security trust boundaries** — L3 es heurístico, no audit formal.
6. **Performance regressions under load** — no corremos benches ni load tests.
7. **Dead code / workspace orphan exports** — parcial via thermo-nuclear.
8. **External dependency churn** — L5 verifica jaula, no monitorea upstream.
9. **CONTEXT.md consistency semántica** — verificamos si existe/al día, no precisión.
10. ~~**Apache Camel parity semantics vs option-count parity** — verificamos opción-count, no behaviorial.~~ → **CERRADO por L7 + ADR-0046 (v4.2).** Verificamos parity behavioral vía gate, no solo option-count. Residual: L7 verifica que la divergencia está *documentada*, no re-valida la *corrección* de la decisión de divergencia (eso fue el oráculo en su diseño original).
11. **Audit stale vs HEAD** — cuando alguien vaya a trabajar un finding, debe comparar audit.HEAD vs main actual; si diverge en archivos del crate → re-audit.
12. **Oracle ack para fixes T1 que tocan semántica ADR** — un fix de código que cambia comportamiento descrito por un ADR es decisión arquitectural de facto; aunque el audit no lo toca, quien lo arregle debe consultar al oráculo si la semántica ADR se ve afectada.
13. **`rg 'fn poll_ready'` lista definiciones, no cuerpos** — el auditor y el validator de camel-processor usaron este patrón y listaron 2 sitios sospechosos (wire_tap, load_balancer). El oráculo añadió Splitter/StreamingSplitter al refinar DP-2. Revisión post-oracle del orquestador encontró un 5to sitio (Filter) y un open question (EnrichService producer.poll_ready) que TODOS habían omitido. **Lección:** para lenses L2/concurrency, usar `rg -A 3 'fn poll_ready' | rg -B 3 '\.poll_ready\('` que captura el BODY. Añadir como regla #10 en reglas anti-falso-positivo.
14. **Oracle no está obligado a re-grepear** — el oráculo e_gpt confió en la enumeración del auditor/validator y solo añadió Splitter/StreamingSplitter porque ya estaban en el reporte. No re-ejecutó el grep independiente. Considerar pedirle al oráculo un independent grep verification como parte del stale check.
15. **Audit concurrente con epic P1 que renombra** — pausar auditoría de crates en scope de un epic P1 (caso rc-iq7 rename `Yaml*` → `RouteDsl*` en camel-dsl/camel-config). Auditar pre-rename produce findings sobre superficie API que va a desaparecer, desperdiciando esfuerzo y contaminando el backlog. **Regla:** si un epic P1 toca el crate, defer hasta que el epic se cierre; marcar en tracking table como `deferred (rc-XXXX)`.
16. **L7 edad como escudo blanket (false 0-GAPs)** — en el re-run camel-processor v4.2 (2026-08-04), el auditor trató "EIP pre-ADR-0046" como exención total de L7 → emitió 0 GAPs. Razonamiento **incorrecto**: la no-retroactividad de ADR-0046 exime el Protocolo (leer Camel), NO la verificación de documentación. **Regla:** la detección L7 (verbos 1+2) aplica a todos los EIPs sin importar edad; el dating determina la resolución (obligatoria vs voluntaria), no la detección. Para pre-ADR-0046 con divergencia no documentada → flaguear GAP con resolución voluntaria (documentar desde comportamiento existente, sin leer Camel). **Aterrizaje del GAP pre-ADR-0046:** se registra como **L6 observation → DP context-md-update**, NO como `FC-BEHAVIORAL-PARITY-GAP` (esa class es verbo-2 / post-ADR-0046 únicamente). Riesgo simétrico: emitir `FC-BEHAVIORAL-PARITY-GAP` espurios sobre pre-ADR-0046. Ver L7 §Scope guard.
17. **Materialización es trabajo activo del orquestador (cuello de botella estructural)** — los subagentes (auditor/validator) **NO pueden despachar a otros subagentes**; solo el orquestador dispara el oracle call. La regla "≥3 o fin de tier" es **GUÍA de batching, NO un gate duro**: si proposals están listas y la sesión cierra, el orquestador **DESPACHA al oráculo** (no las congela esperando el count). DP-3 estuvo congelado ~6 semanas (2026-06-22 → 2026-08-04) porque ninguna sesión de orquestador hizo follow-up. **Regla:** al cerrar una sesión de audit con proposals pendientes, **SIEMPRE** dispatch oracle (o registrar explícitamente por qué se difiere). **Agrupación:** todas las proposals pendientes de una sesión van en **un solo oracle call** (no uno por proposal). El **sweep de fin-de-T1 sigue siendo gate duro** para cruzar a T2 (necesita ver todas las proposals cross-crate juntas para detectar merges/contradicciones).
18. **Two-stream output (docs vs código) — la auditoría es observacional** — el output tiene DOS destinos distintos: proposals L6/L7 → **oracle** (docs); findings C/I/M → **conductor-light vía OpenSpec** (código). Mezclarlos (ej. que el audit intente "arreglar" code findings) rompe el aislamiento observacional y duplica trabajo. **Regla:** el audit TERMINA al producir findings limpios con ID estable + símbolo + correction direction. La corrección de código es proyecto separado: triage post-auditoría → bd issues/epics (`discovered-from:rc-6z4`) → `/opsx:propose` → conductor-light worktree. Ver §"Output & correction flow" + §"Findings de código".
19. **Citation por símbolo, no line-range (B4)** — las correcciones de código ocurren **semanas después** de la auditoría, en **worktree distinto**. Los line-ranges drift constantemente (caso: aggregator.rs 1270→1953 entre runs). Un finding citado como `aggregator.rs:1270` es irreconocible al corregir. **Regla:** todo finding de código cita **símbolo** (`fn merge`, `impl AggregatorService`) como referencia primaria; `path:line` es suplemento informativo únicamente.
20. **Materialization completeness del oráculo (B6)** — cuando el oráculo edita un archivo (CONTEXT.md/ADR), debe validar **TODAS** las secciones que citan código de ese archivo, no solo la sección trigger. Riesgo R4 (drift reintroducido): un commit que toca 1 sección deja las otras stale. Caso: DP-4 sincronizó el catalog pero no la tabla poll_ready → M1 FC-CONTEXT-STALE al re-auditar. **Regla:** el oráculo verifica consistencia cross-sección del archivo tocado antes de commitear.
21. ~~**Autoridad grill = oráculo, no self-grill del auditor (B7)** — el skill `self-grill-proposals` no es cargable en el entorno del subagente.~~ → **CORREGIDO 2026-08-05 (test w_fast `ses_02e9a0326`):** los subagentes **SÍ cargan skills** via tool `skill` (`self-grill-proposals` + `ponytail` cargaron completos). El reporte de r_glm era error de ejecución del auditor, no limitación de plataforma. **Regla:** el auditor DEBE invocar `self-grill-proposals` en Paso 7 (no fallback manual prematuro). El oráculo sigue siendo la grill autoritativa final (stale-check + L6 + sweep), pero el self-grill del auditor es paso real de refinamiento, no best-effort.

Para v1.0, considerar audits especializados separados: cargo-audit (security), cargo-semver-checks (API), loom/shuttle (concurrency), criterion (perf).

## Origen y vinculaciones

- **Ticket beads:** `rc-6z4` — "Auditoría de fidelidad a las premisas fundacionales de rust-camel".
- **Síntesis de premisas:** oracle `e_glm`, sesión `ses_1102343a1ffeUhOLUu8vofbLWV` (2026-06-22).
- **Trial v1 (descartado):** worker=w_deep4-flash + reviewer=r_glm5.2 sobre camel-log. Reviewer cazó falso positivo Important (I2 lectura invertida). Condujo a inversión de roles.
- **Trial v2 (descartado):** auditor=r_glm5.2 + validator=w_deep4-flash sobre camel-log. Sin falsos positivos. Detectó 2 bugs reales (I1 String::truncate multi-byte panic, I2 regression test vacuo), ambos confirmados con tests reales.
- **Pilot v3.x/v4 (descartado):** auditor=r_glm5.2 + validator=w_deep4-flash + oracle=e_gpt sobre camel-processor. 2 bugs (I1 ThrottleStrategy::Drop, I2 WireTap poll_ready). Oracle e_gpt aprobó 4 DPs (DP-1 CONTEXT.md, DP-2 ADR-0019 amendment, DP-3 ADR-0026 WireTap, DP-4 ADR-0027 enum policy). **Pilot descartado por owner** para correr audit oficial con v4.1 limpio. En el run oficial v4.1 (2026-06-22), DP-1 y DP-2 re-emergieron y fueron commiteadas (272120c2, 3613eb3d); ADR-0026 WireTap y ADR-0027 enum policy NO re-emergieron (oracle determinó que DP-2 amendment era suficiente).
- **Run oficial v4.1 (2026-06-22):** primer run oficial sobre camel-processor. Auditor=r_glm5.2 (0C/2I/5M), validator=w_deep4-flash (388/388 tests, 12 citations, 5/5 negative search), oracle=e_gpt (DP-1+DP-2 commiteados con refinement: Splitter/StreamingSplitter movidos a pending-fix). Post-oracle review del orquestador detectó coverage gap: Filter (I3) y EnrichService (open question) omitidos por todos — añadidos al audit report + DP-3 pendiente para T1 sweep. Lecciones 13+14 añadidas a blind spots.
- **Oracle e_gpt blessing (2026-06-22):** lenses + 9 reglas + cargo check + reproducers + negative search + escalamiento T1-first + tracking por finding class + blind spots. Aplicado.
- **Oracle e_gpt primer oracle call (2026-06-22):** 4 reglas L6 (cite related, additive diff, counts verificados, workspace-wide default). Aplicado.
- **Oracle e_opus deep review (2026-06-22):** detectó 4 hechos de campo (CARGO_TARGET_DIR compartido, sccache activo, docs/* gitignored con exception !docs/adr/*.md, 6 worktrees vivas). Pidió: cláusula tests+target dir, regla 2/2 reproducibilidad, grep Blocking lock. Aplicado.
- **Owner steering v4.1 (2026-06-22):** simplificación. Oráculo = materializador + committer (no fixer agent). Audit trabaja en main. Skill `self-grill-proposals` pendiente de crear como copia de `grill-with-docs` sin usuario. docs/ gitignored excepto `docs/adr/*.md`.
- **Skill `self-grill-proposals` creado (2026-06-22):** `~/.agents/skills/self-grill-proposals/SKILL.md` (899 palabras). Non-interactive adaptation of grill-with-docs con 4 questioning techniques, output format, iron rules, common mistakes. Reemplaza el fallback manual del pilot.
- **v4.2 reconciliation (2026-08-04, e_opus ronda-2, ses `ses_03295f16dffeROb3LojKe1jmiK`):** owner redescubrió necesidad de comparar tests de Apache Camel para minar edge cases; e_opus detectó que ADR-0046 (Accepted 2026-07-17, diseñado por el propio e_opus en `ses_08fc0fd19ffei7uuZcFoOrbnyq`) ya codificaba el método pero faltaba el puente con AUDIT.md. v4.2 = ese puente: **L7 = gate que invoca ADR-0046** (no método propio, anti-duplicación), regla de dos verbos (verificar pin por defecto / escalar solo en hueco genuino), finding class `FC-BEHAVIORAL-PARITY-GAP`, cierre blind spot #10, pausa levantada (rc-iq7 cerró `00c5f6ef`), camel-core `covered-by-rc-d0pu`. Spike de evidence: `rc-spt-camel-splitter-spike` (`8d31e74a`), spike doc `docs/spikes/camel-splitter-conformance-spike.md` (gitignored). Elección del owner: L7 inline en reporte (no columna tracking formal) — dosis mínima.
- **Crate count:** 55 + `bridges/` + `xtask/` (secondary).
- **Quality gates referencia:** ver `AGENTS.md` sección "QUALITY GATES".

## Convenciones

- **DATE:** por run (usar `date +%F`).
- **AUDITOR default:** `reviewers/r_glm`. Alternativas: `reviewers/r_gpt`.
- **VALIDATOR default:** `workers/w_fast`. Alternativas: `workers/w_balanced`, `workers/w_heavy`.
- **ORACLE default:** `experts/e_gpt`. Alternativas: `experts/e_glm`, `experts/e_opus`.
- **Skill `self-grill-proposals`:** disponible en `~/.agents/skills/self-grill-proposals/`. Creada 2026-06-22 (copia no-interactiva de `grill-with-docs`).
- **Target dir dedicado para validator:** `CARGO_TARGET_DIR=/home/shared/rust-camel-target-audit/<crate>` (por crate).
- **No borrar** reportes previos de `docs/audits/modules/` aunque sean baseline v0.9.0; preservar para comparación.
- **`docs/adr/*.md` tracked** (commiteable). **`docs/audits/` untracked** (backlog local, no commiteable).
