# Direction-check: OpenSpec change `bench-era-2` (spec blessed, pre-planning)

**Autor:** e_opus (papal tier) · **Fecha:** 2026-08-30 · **Tipo:** revisión de DIRECCIÓN sobre el spec bendecido por e_glm (BLESS-WITH-FIXES, 9 fixes, sha256:f315165b…). No implemento, no edito; solo lectura del worktree.
**Baseline de fidelidad:** `docs/benchmarks/orientation-benchmark-restructure-2026-08-30.md` (mis 5 pilares).

---

## (a) FIDELIDAD — ¿el spec sigue implementando los 5 pilares? ¿los fixes de e_glm mejoraron o distorsionaron?

**Fidelidad global: alta.** Los 5 pilares están presentes y mapeados 1:1 (P1 zone contract, P2 tag-and-freeze, P3 canonical v1 run, P4 records layer, P5 terminology diet). Las 9 correcciones de e_glm **mejoraron** el spec; ninguna lo distorsionó. Evidencia por corrección:

- **Phantom loadgen-move borrado** → correcto: verifiqué que `loadgen` ya vive en `harness/loadgen` y el root `Cargo.toml` ya apunta ahí (design "Affected crates: None in crates/"). Mi orientación asumía el move; e_glm detectó que ya estaba hecho. Mejora.
- **CI bench-subset restaurado** → correcto y superior a mi orientación. Yo dejé el CI-subset vago; e_glm lo ancló a substancia real: los criterion benches `pipeline` + `body_coercion` en quick-mode **más** el `bench run --dry-run` facade smoke. Eso preserva la cobertura CI existente (que yo no había inventariado) y añade la validación del nuevo facade. Mejora clara.
- **ADR-0066 gauge-citation añadido** → mejora crítica. Verifiqué en el árbol: `grep 0.9890` sobre ADR-0066 **no devuelve nada** — el gap R4 que diagnostiqué es real y estaba sin cerrar en mi propia orientación (yo lo marqué como riesgo, e_glm lo convirtió en step + scenario "Gauge premise preserved"). Fidelidad reforzada.
- **Quiet-host criteria pre-run en `harness/CONTEXT.md`** → mejora. Mi orientación exigía contenedor canónico pero no formalizó el criterio de "host quieto"; el spec ahora lo exige escrito ANTES de la corrida y que el record demuestre conformidad. Cierra la lección del A/B contaminado.
- **`era` field en run.json + index** → mejora, y load-bearing (ver (f)).
- **Phase exit criteria explícitos + `builder/ → harness/builder/`** → mejora; hace las fases auditables.

### El punto que me pides juzgar: reports a `docs/benchmarks/history/` vs mi `attic/reports-era-1/`

**e_glm acertó. Su call es SUPERIOR a mi orientación original.** Razonamiento:

1. Mi `attic/` era para **ruido dentro de `benchmarks/`** (spikes, prosa muerta). Los reportes v2/v3/v4 **nunca vivieron en `benchmarks/`** — viven en `docs/benchmarks/`. Meterlos en `benchmarks/attic/` los habría *movido de árbol*, cruzando una frontera que no debía cruzarse (docs es docs, bench es bench).
2. **La prueba de "canas" es sobre `benchmarks/`, no sobre `docs/`.** El humano dijo "abro el directorio benchmarks y me da canas". `docs/benchmarks/history/` NO aparece cuando abres `benchmarks/`. Cero canas nuevas.
3. Verifiqué que **`docs/src/SUMMARY.md` no referencia ninguna página de benchmarks** → mover a `history/` tiene **cero impacto en el build del sitio mdbook**. El claim de e_glm es cierto.
4. `history/` mantiene los reportes **findables en su contexto natural** (un lector de docs que busca evidencia comparativa los encuentra donde espera), mientras el tag `bench/era-1-final` provee la inmutabilidad "no sale ni con lejía".

Además e_glm **separó bien las dos disposiciones**: reportes (`docs/`) → `history/`; results-trees crudos (`benchmarks/results-published/`) → `benchmarks/attic/results-era-1/`. Esa es exactamente la distinción curado-vs-crudo que yo diagnostiqué como invisible para el humano, ahora hecha explícita en dos destinos distintos. **No recrea el problema de canas; lo resuelve mejor que mi versión.**

**Veredicto (a): fidelidad preservada; los fixes mejoraron el spec. Sin distorsión.**

---

## (b) SECUENCIA — ¿el orden de fases es correcto? ¿la gran corrida como tarea de agente?

**Orden de fases: CORRECTO.** Phase 1 Zones → Phase 2 Records layer → Phase 3 Run+freeze respeta mi dependencia dura declarada (el esquema JSON debe existir ANTES de la gran corrida, porque el formato de salida se vuelve el formato eterno). El spec lo cumple: `summarize.py` + `run.json` schema + checksum guard aterrizan en Phase 2, la corrida los consume en Phase 3. Bien.

**Un ajuste de secuencia (NOTA, no bloqueo):** el **digest-pin del `runner/Dockerfile`** está en Phase 3. Pero el digest-pin es *independiente* de la corrida y **más barato de hacer temprano** — es una edición de Dockerfile + registro de digest, sin correr nada. Moverlo a Phase 1 (con el resto del contrato de zonas de `runner/`) elimina el riesgo de que Phase 3 arranque, construya la imagen, y descubra tarde que el pinning necesita iteración. **Recomendación STAGE 2:** tarea de digest-pin en Phase 1 o inicio de Phase 3, con exit-criterion propio, no acoplada a la corrida. No es corrección de spec — es orden de tasks.md.

### El punto grande: ¿quién EJECUTA la gran corrida?

**Esta es la única cuestión de dirección genuinamente sin resolver, y debe cerrarse antes de tasks.md.** Verifiqué: el spec es **SILENCIOSO** sobre el ejecutor. No dice "human-invoked" ni "conductor executes". El scenario "v1 record lands" describe el resultado (`GIVEN digest-pinned image AND host meeting quiet-host criteria WHEN the v1 subset run completes THEN records/...`) pero **no nombra al agente que teclea el comando**.

**Mi veredicto: la corrida DEBE ser human-invoked; el agente prepara y valida TODO menos el pulsador final.** Razones duras:

1. **Restricción del proyecto, verificada en AGENTS.md:** "Never run cargo build/test/clippy/check in the main checkout"; los agentes trabajan en worktrees con target frío. La gran corrida es multi-hora, en contenedor, con builds cargo+maven+gradle-native — exactamente la clase de carga que la política prohíbe a un agente lanzar sobre recursos compartidos. Un agente que arranca una corrida de horas dentro de una task viola el espíritu (y probablemente la letra) de esa restricción.
2. **Quiet-host es un predicado que un agente no puede garantizar.** El propio spec exige "host meeting the quiet-host criteria (devnull baseline within stability bound; host load below ceiling)". Un agente en una task no controla la carga del host ni puede certificar quietud sostenida durante horas. El humano sí. La lección del A/B contaminado de esta sesión (números de host = solo-chat) grita esto.
3. **El expediente eterno merece un pulsador humano.** "La corrida que se queda para siempre" es una decisión de registro; que el humano la invoque explícitamente (y firme el tag) es correcto ceremonialmente y operacionalmente.

**Lo que el agente SÍ hace (todo lo preparatorio, que es el 90%):** Dockerfile digest-pinned, quiet-host criteria escritos en CONTEXT.md, `summarize.py` + schema + checksum guard verdes, facade funcional, dry-run del subset validado exitoso, docs-site wiring listo. El agente entrega un botón de un solo uso; el humano lo pulsa en host quieto y el agente luego valida el `run.json` resultante + publica + taggea.

**Corrección requerida en spec:** el requirement "Canonical v1 baseline run" debe añadir una frase que fije al **operador humano** como ejecutor de la corrida en sí, con el agente responsable de todo lo preparatorio y la validación post-corrida. Hoy el silencio permite que tasks.md asigne la corrida a una agent-task de horas — que es el anti-patrón. **Nombra esto: editar el requirement en `specs/benchmark-suite/spec.md` (Canonical v1 baseline run) para explicitar el executor split.**

---

## (c) REQUIREMENTS FALTANTES — ¿algo load-bearing de mi orientación que el spec AÚN carezca?

Inventario contra mi orientación. **Casi todo sobrevivió.** Cotejo:

| Activo de la orientación | ¿Presente en spec? | Dónde |
|---|---|---|
| `schema_version` en run.json | ✅ | Run-level record schema |
| `era` field (run + index) | ✅ | e_glm fix; ambos deltas |
| digest-pin, `:latest` prohibido | ✅ | Digest-pinned runner + scenario "tag rejected" |
| summary generated-only (no hand-typed) | ✅ | Generated summaries only + checksum guard scenario |
| checksum-guard | ✅ | Phase 2 + "Schema rejects hand-typed drift" |
| static-fetch público | ✅ | Records index → scenario "Static fetch" |
| ADR-0066 gauge citation (R4) | ✅ | e_glm fix; scenario "Gauge premise preserved" |
| COVERAGE link rewrite (R5) | ✅ | Design Approach(2); `lint-context-citations` green |
| quiet-host criteria pre-run | ✅ | e_glm fix; en CONTEXT.md |
| gauges ON | ✅ | Scenario "Gauges stay on" |
| rc-90ez soft-dep (no bloquea) | ✅ | Design: split unmarshal antes de split, code path intacto |

**Dos huecos load-bearing que el spec AÚN no cubre (NOTAS accionables en STAGE 2):**

1. **`schema_version` está declarado pero no hay requirement de EVOLUCIÓN.** El spec fija `schema_version: 1` pero no dice qué pasa cuando llegue el field #2 (v2 del schema): ¿los consumidores externos que fetchean `index.json` deben poder leer records de schema mixto? Mi orientación (R2) marcó el esquema como "la decisión irreversible". El spec fija v1 pero **no fija la regla de compatibilidad hacia adelante** (p.ej. "consumers MUST ignore unknown fields; schema_version bump only on breaking change"). Sin esa regla, el primer cambio de schema rompe fetchers públicos. **NOTA STAGE 2:** añadir a `run.json` schema doc una cláusula de forward-compat (additive-only within a version; ignore-unknown-fields para consumidores). Barato ahora, caro post-v1 (ver (f)).

2. **El `index.json` no declara su propio `schema_version`.** El requirement "Records index" lista los campos del array pero el índice-como-documento no tiene versión propia. Si la *forma del índice* cambia (no la de un record), no hay señal. **NOTA STAGE 2:** el índice debe ser `{ index_schema_version, runs: [...] }`, no un array pelado. Trivial ahora; migración pública después.

Ninguno de los dos es una corrección de dirección — son endurecimientos del contrato eterno que el pre-1.0 window hace baratos.

---

## (d) SCOPE CREEP — ¿algo NO respaldado por mi orientación que deba cortarse antes de que tasks.md lo multiplique?

**Revisión: el spec está notablemente disciplinado. Encontré UN caso menor y cero creep grave.**

- **`payload axis on two reference contenders × 4 payload classes` dentro de la v1 run.** Mi orientación fue explícita: el payload-axis queda como "primer eje declarado pero NO barrido matrix-wide" (es su estado hoy). El spec mete un barrido de payload (2 contenders × 4 clases) en la corrida v1. **Esto es un pequeño creep respecto de mi orientación**, pero es *defendible*: 2 contenders × 4 clases = 8 celdas extra, acotado, y da al eje su primer dato real sin explotar el matrix. **No lo corto, pero lo marco:** STAGE 2 debe tratar el payload-axis como un bloque *opcional/desacoplable* de la v1 run — si la corrida se alarga o una celda de payload falla, debe poder caer sin invalidar la v1 baseline. Que sea aditivo, no un gate del expediente.
- **`bench publish` como tercer subcomando del facade.** Mi orientación mencionó `run` + `summarize`; `publish` (copia a `records/` + refresca index) es nuevo pero es exactamente la mecánica que mi P4 describía sin nombrar. No es creep — es el nombre del paso que yo dejé implícito. Se queda.
- **MCP dogfooding:** correctamente marcado `open-if, post-v1` en "Excluded". Cero creep. Bien.
- **Website project:** correctamente excluido ("static JSON + generated summary only"). Bien.

**Veredicto (d): sin creg grave. Único ajuste: payload-axis debe ser desacoplable de la v1 run (nota STAGE 2), no un gate.**

---

## (e) LA PRUEBA DEL HUMANO — abre `benchmarks/` tras las 3 fases. ¿Qué ve EXACTAMENTE? ¿Pasa "sin canas"?

Camino el `ls benchmarks/` post-Phase-3 según el zone contract:

```
benchmarks/
├── README.md      ← 5 palabras públicas (corrida/escenario/contendiente/fecha/registro). Cero M, cero T.
├── bench          ← un ejecutable. run / summarize / publish.
├── harness/       ← código vivo (run.sh, loadgen/, summarize.py, builder/, CONTEXT.md técnico)
├── scenarios/     ← 8 familias de fixtures + COVERAGE.md
├── runner/        ← Dockerfile (pineado por digest)
├── records/       ← index.json + v1-<date>/ (run.json + summary.md)
└── attic/         ← spikes/, prosa muerta, results-era-1/
```

**¿Pasa la prueba? SÍ, con un matiz.** Lo que ve:
- **7 entradas de nivel-1, cada una con función obvia.** El diagnóstico original era "5 clases de cosa mezcladas sin contrato"; ahora hay contrato. Esto es la cura real de las canas (yo dije que el 60% del ruido era estructural, no histórico — el spec ataca lo estructural).
- **Los spikes NO están a la vista** (en `attic/`). ~30% de las canas, eliminado.
- **Los reportes históricos NO están** (en `docs/benchmarks/history/`, otro árbol). No contaminan.
- **Los results crudos comiteados NO están** (en `attic/results-era-1/`). La copia curada-vs-cruda que confundía, resuelta.
- **Un solo README con vocabulario de 5 palabras.** Abre, entiende, cierra.

**El matiz (NOTA):** `attic/` es honesto pero sigue estando a nivel-1. Un humano meticuloso puede sentir un leve pinchazo al verlo ("¿qué hay ahí?"). No es canas — es un desván etiquetado, que es lo correcto (git preserva, pero visible-y-nombrado > borrado-y-olvidado). Lo dejo: la alternativa (borrar) fue rechazada en P2 por buenas razones. Un `attic/README.md` de una línea ("desván: material congelado de era-1, ver tag bench/era-1-final") remata la prueba. **NOTA STAGE 2:** añadir ese README de una línea al desván.

**Veredicto (e): pasa "sin canas". El único residuo (`attic/` visible) es intencional y se remata con un README de una línea.**

---

## (f) VENTANA PRE-1.0 — ¿qué debe fijarse AHORA porque cuesta 10× tras la corrida v1?

Todo lo que un consumidor externo empiece a fetchear se vuelve un contrato público el día que se publica el primer `records/index.json`. Fijar ahora (barato) vs después (migración pública, caro):

1. **Regla de forward-compat del schema** (hueco (c)-1). Debe estar en el schema doc ANTES de la v1 run. "Additive-only dentro de una versión; consumidores ignoran campos desconocidos; bump de `schema_version` solo en breaking change." Post-v1, cambiar esto rompe cada fetcher. **LOCK NOW.**
2. **Índice versionado** (hueco (c)-2). `{ index_schema_version, runs: [...] }` no un array pelado. Cambiar la forma del índice después = romper todo consumidor del índice. **LOCK NOW.**
3. **Nomenclatura de `era` y `run_id`.** `era: "v1"` y `run_id` = timestamp UTC. Una vez publicado, el naming es citado externamente (URLs, blog). Verificar que `era` es un string estable (no un int que luego quieras renombrar) y que `run_id` es sortable lexicográficamente (el timestamp UTC `YYYYMMDDTHHMMSSZ` lo es — bien). **CONFIRMAR el formato en el schema doc.**
4. **Ruta pública de `records/`** (la URL base que el sitio expone). Una vez que alguien la fetchea, moverla rompe enlaces. Fijar la ruta canónica (p.ej. `<site>/benchmarks/records/index.json`) en Phase 3 antes de anunciar. **LOCK en Phase 3, documentar en README.**

Los cuatro son de contrato-eterno y el pre-1.0 los hace triviales hoy. Ninguno requiere corrección de dirección — son endurecimientos de tasks.md + una línea en el schema doc.

---

## Resumen de acciones (todas accionables en STAGE 2 salvo una edición de spec)

| # | Acción | Tipo | Dónde |
|---|---|---|---|
| 1 | Fijar ejecutor de la v1 run = **humano**; agente prepara+valida | **EDICIÓN DE SPEC** | `specs/benchmark-suite/spec.md` → "Canonical v1 baseline run" |
| 2 | Regla forward-compat del schema (additive-only, ignore-unknown) | NOTA STAGE 2 | schema doc, antes de v1 run |
| 3 | Índice versionado `{ index_schema_version, runs }` | NOTA STAGE 2 | Records index requirement |
| 4 | Payload-axis desacoplable de v1 run (no gate) | NOTA STAGE 2 | tasks.md Phase 3 |
| 5 | Digest-pin del Dockerfile antes de la corrida (no acoplado) | NOTA STAGE 2 | tasks.md fase temprana |
| 6 | `attic/README.md` de una línea | NOTA STAGE 2 | tasks.md Phase 1 |
| 7 | Confirmar ruta pública canónica de `records/` | NOTA STAGE 2 | tasks.md Phase 3 |

Una sola es corrección de spec (#1, el ejecutor humano); las otras seis son endurecimientos de tasks.md que no cambian la dirección.

---

## VEREDICTO

**DIRECTION CONFIRMED WITH NOTES: 7 notes** (6 tasks.md hardenings + 1 spec edit naming the human as executor of the v1 run).

---

### Self-grill record

**Questions generated:**
1. [glossary] ¿"zone contract", "records layer", "facade" chocan con algún término de mi orientación o del glosario canónico, o son fieles?
2. [sharpen] "La gran corrida como tarea" — ¿esconde dos decisiones (quién prepara vs quién ejecuta)?
3. [scenario] Si el spec queda silencioso sobre el ejecutor y tasks.md asigna la corrida a un agente, ¿qué se rompe concretamente?
4. [cross-ref] ¿El claim de e_glm "docs no está en mdbook, mover a history/ es inocuo" es cierto contra el árbol? ¿Y el gap de ADR-0066?

**Answers (with citations):**
1. [glossary] Fieles, sin choque. "zone contract" = mi "contrato de directorios" (orientación §P1 Cond 1); "records layer" = mi `records/` (orientación (b) árbol objetivo); "facade" = mi "fachada única `bench`" (orientación §P1 Cond 4). El glosario técnico canónico (`benchmarks/CONTEXT.md` §1, ahora `harness/CONTEXT.md`) no define ninguno de estos — son términos de estructura, no de medición. Sin colisión. Outcome: confirm fidelidad terminológica.
2. [sharpen] **Dos decisiones, sí.** "preparar" (Dockerfile pin, schema, quiet-host criteria, dry-run verde — todo automatizable y seguro para un agente en worktree) y "ejecutar" (lanzar la corrida multi-hora en contenedor sobre host quieto — carga compartida, prohibida a agentes por AGENTS.md "Never run cargo build/test in main checkout", y no certificable por un agente). El spec funde ambas en un silencio. Separadas explícitamente en mi veredicto (b). Outcome: split → corrección de spec #1.
3. [scenario] Input construido: tasks.md STAGE 2 crea "Phase 3 task: execute v1 run" y la asigna a la agent-task del conductor (patrón por defecto, ya que el conductor "ejecuta" las fases). Rompe: (i) política AGENTS.md de no correr builds pesados en checkout compartido — verificado literal en AGENTS.md; (ii) el predicado quiet-host del propio scenario "v1 record lands" (`host meeting quiet-host criteria`) que un agente no puede garantizar durante horas; (iii) reproducibilidad — la lección del A/B contaminado de esta sesión (host numbers = chat-only). Los tres son daños concretos, no hipotéticos. Outcome: justifica que #1 sea corrección de spec, no nota.
4. [cross-ref] **Ambos claims verificados.** (a) `grep -i bench docs/src/SUMMARY.md` → "CONFIRMED: no benchmark page in mdbook SUMMARY" — mover reportes a `history/` es inocuo para el build del sitio, e_glm acertó. (b) `grep 0.9890|0.9785|gauge A/B docs/adr/*0066*` → sin resultados — el gap R4 es real; ADR-0066 hoy NO cita el verdict del gauge, luego el step añadido por e_glm es necesario y el scenario "Gauge premise preserved" cierra una premisa viva que estaba sin trazar. Outcome: confirm — los dos fixes de e_glm que más dependían de hechos del árbol son correctos.

**Outcome:** confirm — el direction-check se sostiene tras el auto-grilling. La única corrección de dirección (ejecutor humano de la v1 run) está verificada contra AGENTS.md y contra el propio scenario del spec; las 6 notas restantes son endurecimientos de contrato-eterno que el pre-1.0 abarata. Fidelidad a los 5 pilares: preservada; los 9 fixes de e_glm mejoraron el spec sin distorsión.
**Self-grill mode:** self-grill-proposals skill
