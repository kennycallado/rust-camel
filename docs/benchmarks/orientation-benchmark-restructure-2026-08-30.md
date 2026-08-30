# Orientación: reestructurar el área de benchmarks (pre-1.0)

**Autor:** e_opus (papal tier) · **Fecha:** 2026-08-30 · **Tipo:** ORIENTACIÓN, no implementación.
**Ámbito:** solo lectura del árbol. No se modifica código, no se hace push, no se corre cargo en el checkout principal.

> Consulta del HUMANO: reestructurar todo el área de benchmarks reusando el código,
> "borrar el pasado" y hacer "la gran corrida inicial" (la que queda para siempre),
> con salida en (a) JSON fetcheable desde web y (b) resumen visual por corrida.
> Abrir `benchmarks/` le da canas y eso es raro.

---

## 0. Diagnóstico — por qué da canas (esto no es subjetivo, tiene anatomía)

Verificado en el árbol esta sesión. `benchmarks/` (3.7 MB) mezcla **cinco clases de cosa** en un solo directorio, y el ojo no puede separarlas:

1. **Código vivo del harness** — `harness/loadgen/` (Rust, fresco, byte-reproducible), `harness/run.sh` (1150+ líneas), `harness/run-all.sh`, `harness/run-local-m3-m4.sh`.
2. **Fixtures** — `scenarios/` con **8 familias** (no 5): `http-server`, `t2-json`, `split-aggregate`, `startup-minimal`, `t2-realistic-eip`, `xsd-validation-bridge`, `xslt-bridge`, `multi-step`. Cada una con 4–6 subdirs de artefactos + `smoke/`.
3. **Resultados curados a mano y COMITEADOS** — `results-published/20260723T161422Z/` (14 JSON tracked en git: `m3-summary.json`, `m4-summary.json`, `provenance.json`, `measurement_order.json`). Ojo: los resultados *crudos* (`benchmarks/results/`) ya están gitignorados (`.gitignore:25`); lo comiteado es una copia curada. **Esa distinción curado-vs-crudo es la que el humano nunca vio con claridad.**
4. **Spikes muertos** — `spikes/` con 5 subárboles exploratorios (`spike-bridge-pinning`, `spike-http-server`, `spike-loadgen`, `spike-native-build`, `spike-yaml-predicate`). Ya excluidos de la medición por glob `spike-*`, pero **visualmente presentes** y ruidosos.
5. **Prosa suelta en la raíz** — 5 `.md` de peso muy distinto: `CONTEXT.md` (49 KB, glosario canónico), `COVERAGE.md` (matriz), `README.md` (10 KB), `spike-results.md` (20 KB, histórico), `quarkus-native-throughput-diagnosis.md`. Más `builder/` (1 script) y `runner/` (solo un `Dockerfile`).

**La causa raíz de las canas no es "el pasado", es la falta de un contrato de directorios.** El humano cree que el problema es histórico (v2/v3/v4); en realidad el 60% del ruido es estructural (spikes visibles, results curados comiteados, prosa de 4 pesos distintos en la raíz) y persistiría aunque borres todos los reportes. **Borrar el pasado sin re-contratar los directorios deja el problema intacto.** Este es el primer riesgo que no está viendo (ver §Riesgos R1).

Segundo hallazgo estructural clave: **hoy NO existe un JSON a nivel-de-corrida.** La salida es por-celda (`<cell>/m3-summary.json`). El modelo mental del humano ("tablas por fecha" = un array de corridas fechadas) **requiere una capa de agregación que no existe todavía.** Esto reordena la secuencia: el esquema JSON debe definirse ANTES de la gran corrida, porque el formato de salida de esa corrida se vuelve el formato eterno (§P4, §Secuencia).

Tercer hallazgo: `run.sh` **ya emite `provenance.json`** con `git_commit`, `git_dirty`, host CPU/cores/RAM, `docker_image`, kernel, `tool_versions`, topología cpuset y `measurement_order.json` con seed. **El sustrato del "expediente forever" ya está construido.** No hay que inventarlo, hay que envolverlo.

Cuarto hallazgo (cambia P4): ya existe `origin/copilot/build-publish-mdbook-docs` (rama de publicación mdbook en vuelo) **y** `crates/components/camel-component-mcp`. Hay pista de aterrizaje para publicación web y la historia de dogfooding-MCP es real, no hipotética.

---

## (a) Veredicto por pilar

### P1 — Reestructurar `benchmarks/` reusando el harness · **BENDECIDO CON CONDICIONES**

El instinto es correcto y el momento (pre-1.0) es el último barato. Pero el valor no está en "reorganizar", está en **imponer un contrato de tres zonas por función**:

- **Condición 1 — separar `código` / `fixtures` / `resultados` / `historia` en zonas nombradas.** Ver árbol objetivo en (b).
- **Condición 2 — los resultados NO viven en `benchmarks/`.** Ni crudos ni curados. `benchmarks/` = harness + fixtures + doc de entrada, y nada más. Los resultados publicados van a una zona de datos (`benchmarks/records/` o rama de datos, ver P4), regenerable, nunca hand-written.
- **Condición 3 — `spikes/` sale de la vista.** Muévelo a `benchmarks/attic/spikes/` o bórralo (git lo preserva). Su presencia en el nivel-1 es ~30% de las canas.
- **Condición 4 — un solo entrypoint conceptual.** Hoy hay `run.sh` + `run-all.sh` + `run-local-m3-m4.sh` + `builder/build-all.sh`. El humano no debería ver 4 scripts. Fachada única `bench` (wrapper delgado sobre el `run.sh` existente; NO reescribir `run.sh`).

**Rechazo explícito de una tentación:** NO reescribir el harness. Está fresco (mergeado ayer, CIs byte-reproducibles). Reestructurar = mover archivos + añadir una fachada + un agregador JSON. Cero cambios a `loadgen`, `bca.rs`, goldens, digests.

### P2 — "Borrar el pasado" (v2/v3/v4/addendum + results dirs) · **RECHAZADO Y REEMPLAZADO** por *tag-and-freeze + archivar*

Aquí el papa disiente del humano, con razón dura:

1. **Los ratios publicados son la ÚNICA evidencia comparativa del proyecto.** rust-camel 1.18× vs Camel-standalone y 2.08× vs Quarkus-native, con CIs. Borrar eso pre-1.0 no "resetea convenciones", **destruye el único activo de marketing técnico que existe** y no hay una gran-corrida-de-reemplazo todavía. Borras antes de tener con qué reemplazar.
2. **El A/B de coste-de-gauges es load-bearing para una decisión viva.** Verificado en `2026-08-29-benchmark-v4-addendum.md:95`: `RATIO ... point=0.9890 lo=0.9785 hi=1.0126`, el intervalo incluye 1.0 → "coste no resuelto desde cero". Esa cifra es la que justifica **mantener los gauges encendidos** (ADR-0066). Si borras el addendum, borras la evidencia que sostiene una decisión de arquitectura activa. Eso no es limpiar historia, es amputar una premisa.
3. **"No sale ni con lejía" se cumple con un TAG, no con un rm.** Git ya preserva todo — nada muere de verdad. Un tag `bench/era-1-final` sobre el commit de los reportes antiguos es *más* permanente que dejarlos sueltos en `docs/`: queda inmutable, fechado, firmado por el árbol. Ese ES el expediente que no sale ni con lejía.
4. **Los goldens/digests/protocolo son activos reusables INDEPENDIENTES de los reportes viejos.** Verificado: los digests canónicos y `aggregate-ratios` viven en `harness/loadgen`, no en los `.md`. Borrar reportes no toca reproducibilidad. Pero borrar reportes SÍ rompe los links: `COVERAGE.md` cita `docs/benchmarks/2026-07-21-benchmark-v3.md` etc. en celdas load-bearing.

**Disposición exacta (ver (d)):** archivar, no borrar. La sensación de "empezar limpio" se logra con la *nueva era v1* (P3) siendo el reporte visible y default, y los viejos moviéndose a `docs/benchmarks/history/` + un tag. El humano obtiene el "gran inicio limpio" sin pagar la destrucción de evidencia.

### P3 — "La gran corrida inicial" (era nueva v1, permanente) · **BENDECIDO CON CONDICIONES**

El concepto es correcto y es el corazón emocional de la petición. Condiciones para que sea real y no una promesa vacía:

- **Debe ser containerizada.** Lección grabada esta sesión: los números de host son solo-chat (A/B contaminado). Canónico = contenedor. `runner/Dockerfile` ya existe.
- **Debe fijar el commit con TAG + pinear la imagen del contenedor** (digest, no `:latest` — hoy `provenance.json` dice `benchmark-runner:latest`, eso es una condición a corregir: pinear por digest sha256).
- **Alcance realista, NO todo el matrix de golpe.** Con la matemática de wall-clocks de esta sesión (~15 s/celda natives warm, ~13.6 min/celda M2, ~5 min/celda M3, ~25 min builds), el matrix completo × payload-axis × M1–M4 es multi-hora y frágil. **La v1 de la nueva era debe ser el subconjunto ya validado**: las 8 familias de escenarios que tienen fixtures + smoke pasado, en las métricas donde ya hay protocolo (M1/M3/M4 donde aplica). El payload-axis queda como *primer eje declarado pero no barrido matrix-wide* (ya es su estado hoy).
- **NO prometer "forever" en el sentido de "final".** El matrix crece (T4c/T5/T6/T7 son `open-if`). La promesa correcta es: **"v1 baseline de la nueva era; corridas futuras comparables in-protocolo"**. "Forever" aplica al *registro* (tag inmutable), no a la *completitud*.
- Esto **supersede/reencuadra `rc-f4po`** ("container run + first published numbers") dentro de un épico mayor (ver (e)).

### P4 — JSON web-fetcheable + resumen visual por corrida · **BENDECIDO CON CONDICIONES (versión perezosa)**

Correcto y alineado con infra ya en vuelo (`copilot/build-publish-mdbook-docs`). Condiciones de pereza-pero-bien-hecho:

- **Falta la capa de agregación run-level.** Hoy solo hay per-cell JSON. Hay que definir **un** esquema `run.json` (o `record.json`): un objeto por corrida con `{ schema_version, run_id, date, git_commit, container_digest, provenance, cells: [...] }`. El array de estos objetos ES el "tablas por fecha" del humano.
- **Publicación mínima viable:** JSON estático servido por la infra mdbook/gh-pages que YA se está montando. NO construir un proyecto-website. Un `records/index.json` (array de run-records) + un `records/<run_id>.json` por corrida. Fetch público = URL cruda del sitio.
- **Resumen visual: generado, nunca hand-written.** Un generador (puede ser un subcomando del propio `bench` o un script) que toma `run.json` → emite `<run_id>.md` (tabla + ratios + CIs) y opcionalmente un HTML mínimo con una tabla y, si acaso, un chart embebido estático. Regla de oro: **si un humano teclea un número en un reporte, está mal.**
- **Dogfooding-MCP (opcional, no bloqueante):** `camel-component-mcp` existe. Servir `records/index.json` como recurso MCP es una demo elegante de dogfooding, pero es *nice-to-have* post-v1, no parte del camino crítico. Anótalo como `open-if: cuando la historia MCP necesite un demo público`.

### P5 — Dieta terminológica · **BENDECIDO CON CONDICIONES**

El humano tiene razón: M1–M4 / T1–T7 / vN / cells / paired-ratios / payload-axis es demasiado vocabulario para el dueño del proyecto. Pero **no se puede borrar todo** — parte es load-bearing para reproducibilidad. La cura es de **dos capas** (ver (f)): un vocabulario mínimo público (el que el humano usa a diario) y el vocabulario técnico confinado a UN solo lugar (`CONTEXT.md` del harness, que ya es el glosario canónico). El error a evitar: diluir el glosario técnico (rompe reproducibilidad). El objetivo: que el humano **nunca tenga que abrir el glosario técnico** para operar.

---

## (b) Forma objetivo de `benchmarks/` tras reestructurar (sketch de árbol)

```
benchmarks/
├── README.md                 # ← ÚNICO punto de entrada humano. Generado/curado corto.
│                             #   "qué es, cómo corro `bench run`, dónde están los datos".
├── bench                     # ← fachada única (wrapper delgado). Subcomandos:
│                             #   run / summarize / publish. NO reescribe run.sh.
├── harness/                  # CÓDIGO VIVO (intacto, fresco, no se toca)
│   ├── run.sh                #   orquestador existente
│   ├── loadgen/              #   Rust: loadgen, bca, digests, aggregate-ratios, goldens
│   └── CONTEXT.md            #   ← glosario técnico canónico (M1-M4, T-families, pairing…)
│                             #     confinado AQUÍ. El humano no lo abre para operar.
├── scenarios/                # FIXTURES (8 familias). Sin cambios de contenido.
│   ├── startup-minimal/  http-server/  t2-json/  split-aggregate/
│   ├── t2-realistic-eip/  xsd-validation-bridge/  xslt-bridge/  multi-step/
│   └── COVERAGE.md           #   matriz de cobertura (índice, no reporte)
├── runner/                   # Dockerfile (pineado por digest, no :latest)
├── records/                  # DATOS PUBLICADOS (regenerables, nunca hand-written)
│   ├── index.json            #   array de run-records → "tablas por fecha"
│   └── <run_id>/             #   run.json + summary.md + (opcional) summary.html
└── attic/                    # HISTORIA / muertos (fuera de la vista de nivel-1)
    ├── spikes/               #   los 5 spike-* movidos aquí
    └── (spike-results.md, quarkus-native-throughput-diagnosis.md)
```

Notas:
- `benchmarks/results/` (crudos, gitignored) **desaparece de la vista**; su contenido se resume a `records/` vía `bench summarize`. Los crudos siguen viviendo solo en el host de medición.
- `results-published/20260723T161422Z/` (los 14 JSON comiteados hoy) → se re-emite como el **primer record de la era vieja** en `attic/` o se congela con el tag; NO se mezcla con la nueva era.
- Todo `.md` de prosa larga que no sea README/COVERAGE/CONTEXT(harness) → `attic/`.

---

## (c) Recomendación de publicación JSON (perezosa-pero-correcta)

**Esquema mínimo — `records/<run_id>/run.json`:**

```jsonc
{
  "schema_version": 1,
  "run_id": "20260830T120000Z",           // = timestamp UTC (ya lo genera run.sh)
  "date": "2026-08-30T12:00:00Z",
  "era": "v1",                             // nueva era; los viejos quedan en attic
  "provenance": { /* ← el provenance.json que run.sh YA emite, embebido */ },
  "cells": [
    {
      "scenario": "http-server",
      "artifact": "rust-camel-lib",
      "metric": "m3",
      "median_mean_msgs_per_sec": 78628.0,
      "per_round_means": [ /* … */ ],
      "rounds": 5
    }
    // …
  ],
  "ratios": [                              // salida de aggregate-ratios, embebida
    { "a": "rust-camel-lib", "b": "camel-quarkus-yaml-native",
      "point": 2.08, "lo": 1.9, "hi": 2.2 }
  ]
}
```

**`records/index.json`** = `[ { run_id, date, era, git_commit, summary_url } ]` en orden cronológico. Esto ES el modelo "tablas por fecha".

**Publicación:** reusar la rama/infra `copilot/build-publish-mdbook-docs` que ya existe. Copiar `records/**` al sitio publicado → fetch público por URL cruda. **Cero proyecto-website nuevo.**

**Visual:** `bench summarize <run_id>` genera `summary.md` (tabla + ratios + CIs, humano-legible) desde `run.json`. HTML/chart estático = opcional, misma fuente, generado. Nunca a mano.

---

## (d) Qué pasa con los reportes viejos — disposición EXACTA

| Artefacto | Acción | Razón |
|---|---|---|
| `docs/benchmarks/2026-07-18-*.md`, `v3`, `v4` | Mover a `docs/benchmarks/history/` | Evidencia comparativa; links en COVERAGE.md |
| `docs/benchmarks/2026-08-29-benchmark-v4-addendum.md` | Mover a `history/` **pero citar la cifra gauge (0.9890, CI incl. 1.0) en el ADR-0066** si no está ya | Load-bearing para "gauges ON" |
| `docs/benchmarks/consultation-*.md`, `e_opus-*analysis.md` | Mover a `history/` | Contexto histórico, no operativo |
| `benchmarks/results-published/20260723T161422Z/` | Congelar bajo tag `bench/era-1-final`; NO mezclar con nueva era | El "no sale ni con lejía" real |
| `COVERAGE.md` | **Reescribir los links** a `history/…` para no romperlos | Celdas load-bearing citan reportes |
| `benchmarks/spikes/`, `spike-results.md`, `quarkus-*.md` | Mover a `attic/` | Ruido visual, no borrar (git preserva) |

**Regla papal:** un solo `git tag bench/era-1-final <commit>` + mover a `history/attic/` sustituye por completo al `rm`. El humano obtiene "empezar limpio" (la nueva era v1 es lo visible y default) sin destruir la única evidencia comparativa ni la premisa del gauge.

---

## (e) Secuencia (y cambios en bd)

**Orden obligatorio** (una dependencia dura y una lógica):

1. **P5 (dieta terminológica) + P1 (contrato de directorios)** primero — barato, sin correr nada, desbloquea la claridad. Mover spikes→attic, prosa→attic/history, definir zonas.
2. **P4-esquema (definir `run.json` + `index.json`)** ANTES de P3. **Dependencia dura:** el formato de salida de la gran corrida se vuelve el formato eterno; definirlo después obliga a re-correr. El agregador `bench summarize` (per-cell → run.json) es el trabajo nuevo de código real.
3. **P3 (la gran corrida)** — con contenedor pineado por digest, tag del commit, emitiendo directo al esquema de (2).
4. **P4-publicación + visual** — enganchar `records/**` a la infra mdbook ya en vuelo; generar `summary.md`.
5. **P2 (archivar/tag)** — se ejecuta *junto con* P3 (cuando la nueva era v1 existe, los viejos se congelan). No antes: no borres evidencia hasta tener reemplazo.

**Cambios bd propuestos:**
- **Nuevo épico** `bench: nueva era v1 (reestructura + gran corrida + publicación)` — el paraguas de P1–P5.
- **Reencuadrar `rc-f4po`** ("container run + first published numbers") como *hijo* del épico, específicamente la tarea P3. Ya no es standalone; su alcance queda absorbido y precisado.
- Hijos del épico: (i) contrato de directorios + attic [P1/P5], (ii) esquema `run.json`+`index.json`+`bench summarize` [P4-schema, código nuevo], (iii) gran corrida containerizada + tag + digest-pin [P3], (iv) publicación mdbook + summary generado [P4-pub], (v) archivar viejos + tag `bench/era-1-final` + fix links COVERAGE [P2].
- `rc-90ez` (P2 splitter bug) y `rc-p2vm` (CI watch): **independientes, no bloquean.** El splitter bug (rc-90ez) sí conviene resolverlo *antes* de la gran corrida si `split-aggregate` entra en el matrix v1 (un fixture con bug contamina esa familia). Marcarlo como dep blanda de (iii).

---

## (f) Dieta terminológica — vocabulario final

**Capa pública (la que el humano usa a diario — 5 palabras):**

| Palabra | Significado | Reemplaza a |
|---|---|---|
| **corrida** (run) | una ejecución fechada del suite | "cell family run", vN |
| **escenario** (scenario) | un workload nombrado (http-server, t2-json…) | T1–T7, "tier" |
| **contendiente** (artifact/contender) | un binario bajo prueba (rust-camel-lib, camel-quarkus-native…) | "artifact vs contender" |
| **fecha** | cuándo corrió (= run_id) | vN versioning |
| **registro** (record) | el JSON publicado de una corrida | results-published, m3/m4-summary |

Con esas 5, el humano opera: "la corrida del 30 de agosto tiene estos escenarios y contendientes; el registro está publicado".

**Capa técnica (confinada a `harness/CONTEXT.md`, el humano NO la abre para operar):**
M1–M4, T-families, pairing/Pair A-B, payload-axis, paired-ratios, BCa/CI, goldens, digests, measurement-order, marker contract. **No se diluye** (reproducibilidad depende de ella). Solo se *encierra*.

**Dónde vive el vocabulario:** el README de `benchmarks/` usa SOLO la capa pública. `COVERAGE.md` es el puente (usa ambas, con la técnica entre paréntesis). `harness/CONTEXT.md` es el único lugar con el glosario completo. Un humano que abre `benchmarks/README.md` no ve ni una M ni una T.

---

## (g) Riesgos que el humano NO está viendo

- **R1 — Borrar el pasado no cura las canas.** ~60% del ruido es estructural (spikes visibles, results curados comiteados, 5 `.md` de pesos distintos en la raíz), no histórico. Si borras reportes pero no re-contratas directorios, abres `benchmarks/` en un mes y tienes las mismas canas con menos evidencia. **El orden correcto es re-contratar primero, archivar después.**
- **R2 — El esquema JSON es la decisión irreversible, no la corrida.** Puedes re-correr un benchmark; no puedes cambiar barato el formato de un JSON que ya publicaste como "eterno" y que consumidores externos fetchean. **Define `run.json` con `schema_version` desde el día 1** o pagas migración pública después.
- **R3 — `docker_image: latest` rompe "forever".** El `provenance.json` actual pinea la imagen por tag `:latest`, que es mutable. Una corrida "que no sale ni con lejía" fijada a `:latest` NO es reproducible: `:latest` cambia. **Pinear por digest sha256** es condición para que la gran corrida merezca su tag.
- **R4 — Borrar el addendum del gauge amputa una premisa viva.** La cifra 0.9890 / CI incl. 1.0 es la que sostiene "gauges ON" (ADR-0066). Antes de mover el addendum a history, **la conclusión debe estar citada en el ADR** o pierdes la trazabilidad de por qué los gauges siguen encendidos.
- **R5 — Romper links de COVERAGE.md silenciosamente.** Las celdas load-bearing citan `docs/benchmarks/2026-07-2*-*.md`. Mover sin reescribir links deja la matriz apuntando al vacío. Hay un lint `lint-context-citations` en las quality gates — **puede fallar el CI** si mueves reportes sin arreglar citas.
- **R6 — El `split-aggregate` splitter bug (rc-90ez) contamina la era v1 si entra sin arreglar.** Si esa familia se incluye en la gran corrida con el bug vivo, el registro eterno queda con una celda sucia. Resolver rc-90ez ANTES de incluir split-aggregate en v1.
- **R7 — Sobre-alcance de la gran corrida = nunca sale.** El matrix completo × payload × M1–M4 es multi-hora y frágil (una celda que falla puede tumbar la corrida). Si la v1 intenta ser exhaustiva, el "gran inicio" se pospone indefinidamente. **v1 = subconjunto ya validado con smoke pasado; el resto entra como corridas incrementales in-protocolo.**
- **R8 — Dos esfuerzos de docs en paralelo.** Ya existe `origin/copilot/build-publish-mdbook-docs`. Si P4 monta su propia publicación sin coordinar, tienes dos pipelines de docs compitiendo. **Enganchar P4 a esa rama, no crear una tercera.**

---

## Resumen de veredictos

| Pilar | Veredicto |
|---|---|
| P1 Reestructurar reusando harness | **Bendecido con condiciones** (contrato de 3 zonas, no reescribir harness) |
| P2 Borrar el pasado | **Rechazado y reemplazado** por tag-and-freeze + archivar |
| P3 La gran corrida inicial | **Bendecido con condiciones** (contenedor+digest+tag, subconjunto validado, "baseline" no "final") |
| P4 JSON web + visual | **Bendecido con condiciones** (definir esquema ANTES; reusar infra mdbook; todo generado) |
| P5 Dieta terminológica | **Bendecido con condiciones** (2 capas; técnica confinada, no diluida) |

**Una frase para el humano:** el pasado no se borra, se *congela con un tag* (más permanente que dejarlo suelto); las canas no vienen del pasado sino de mezclar código+fixtures+resultados+spikes+prosa en un solo directorio — arregla el **contrato de directorios** primero, define el **JSON eterno** segundo, y solo entonces haz la **gran corrida** que se convierte en la era v1 visible-y-default.

---

### Self-grill record

**Questions generated:**
1. [glossary] ¿"corrida/escenario/contendiente/registro" choca con el glosario canónico de `harness/CONTEXT.md`, o es una capa pública compatible?
2. [sharpen] "Borrar el pasado" — ¿son una o dos decisiones distintas escondidas en una frase?
3. [scenario] Si el humano hace la gran corrida y una celda falla a mitad, ¿el registro eterno queda corrupto? ¿el esquema lo tolera?
4. [cross-ref] ¿El código realmente emite hoy un JSON run-level, o solo per-cell? ¿La afirmación "falta capa de agregación" es cierta contra el árbol?

**Answers (with citations):**
1. [glossary] No choca: es una **capa pública** deliberadamente por encima del glosario técnico. `harness/CONTEXT.md` §1 ya distingue Suite/Scenario/Contender/Artifact/Metric formalmente; la capa pública mapea 1:N a esos términos (contendiente≈artifact+contender colapsados) y los confina. Verificado que el glosario canónico existe y es load-bearing (`benchmarks/CONTEXT.md` §1 Domain Language, 8.2 KB de tabla). El riesgo sería *diluir* el técnico; la propuesta lo *encierra*, no lo toca. Outcome: compatible.
2. [sharpen] **Dos decisiones.** (i) "dejar de mostrar el pasado como default/visible" (UX, deseable) y (ii) "destruir la evidencia" (irreversible-en-intención, indeseable). La frase del humano las funde. Se separan: (i) se logra con la nueva era v1 default + mover a `history/attic/`; (ii) se rechaza. `git tag` + git-history cumplen "no sale ni con lejía" sin (ii). Verificado que results crudos ya están gitignored (`.gitignore:25`) pero `results-published/` está tracked (14 files, `git ls-files`), confirmando que hay una copia curada que el humano no distinguía. Outcome: split aplicado en el veredicto P2.
3. [scenario] Input construido: gran corrida de N celdas, la celda k falla (marker ausente → `run.sh` la marca FATAL para esa celda). Hoy el harness ya tiene `status:"ok"` por celda (verificado en `m3-summary.json` sample: `"status": "ok"`), luego el esquema `run.json` debe llevar `status` por celda y la corrida es válida con celdas parciales marcadas. Riesgo real: si `run.sh` aborta toda la corrida ante una celda FATAL (usa `set -euo pipefail`, verificado línea 39), la gran corrida se cae entera → R7 confirmado. Por eso la condición "subconjunto validado con smoke pasado" en P3 y el riesgo R7. Outcome: esquema debe tolerar celdas con `status`, y v1 debe limitarse a lo smoke-validado.
4. [cross-ref] **Confirmado contra el árbol:** solo existe per-cell JSON (`m3-summary.json`, `m4-summary.json`) + `provenance.json` + `measurement_order.json` a nivel corrida, pero NO un `run.json` agregado con las celdas dentro (`find benchmarks/results-published -name '*.json' -exec basename` → 4 nombres, ninguno run-level agregado). La afirmación "falta capa de agregación run-level" es **cierta**. Además `run.sh:628 write_provenance()` confirma que el provenance ya se emite. Outcome: cross-ref sostiene la secuencia (esquema antes de gran corrida) y el hallazgo #2 del diagnóstico.

**Outcome:** confirm — la orientación se sostiene tras el auto-grilling. Ninguna afirmación quedó sin citar; los dos hallazgos que reordenan la secuencia (falta de JSON run-level; results-published tracked vs raw gitignored) están verificados contra el árbol.
**Self-grill mode:** self-grill-proposals skill
