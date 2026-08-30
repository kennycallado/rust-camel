# Bendición: OpenSpec change `bench-era-2` (implementado, pre-merge)

**Autor:** e_opus (papal tier) · **Fecha:** 2026-08-30 · **HEAD verificado:** 0c96e294 · **base:** ab1c8424 · **48 commits, árbol limpio.**
**Método:** verificar, no confiar. Cada punto del grill se cotejó contra el árbol y, donde el grill lo invitó, se EJECUTÓ (tests read-only). No docker, no push, no cargo en el checkout principal.

---

## Grill point por punto (evidencia ejecutada, no aserción)

### (1) Zone contract + terminology diet — END STATE

**PASA.** La prueba del humano es `ls benchmarks/` (abrir el directorio). Verificado literal:

```
$ ls benchmarks/
attic  bench  harness  README.md  records  runner  scenarios      → 7 entradas exactas
```

Los "9" de `ls -A` incluyen `.cache` (scratch local, `git ls-files benchmarks/.cache` = **0 tracked**) y `.gitignore` (que solo ignora `__pycache__/`). Ni uno ni otro aparece cuando el humano abre el directorio ni existe en git. **Cero canas.** El README abre con: *"This README uses the public vocabulary only: run (corrida), scenario (escenario), contender (contendiente), date (fecha), record (registro). Technical depth lives in harness/CONTEXT.md."* — cinco palabras, cero M/T. `attic/README.md` presente (nota 5 de mi direction-check, incorporada).

### (2) SCHEMA.md — el contrato eterno

**PASA, y es el contrato que exigí.** Verificado en `benchmarks/records/SCHEMA.md`:

- **Forward-compat** (mi nota 2): *"Consumers MUST ignore unknown fields. Producers MUST bump `schema_version` on any breaking change. Additive fields are minor and do not bump the version within v1."* — regla presente, la que faltaba en el direction-check.
- **Índice versionado como OBJETO** (mi nota 3): `{"index_schema_version": 1, "runs": [...]}`, no un array pelado. La forma del índice puede evolucionar sin romper consumidores. Presente.
- **`era` STRING** (mi nota 6): *"A string, not an integer, so the vocabulary can grow without a type change."* — exacto.
- **`run_id` sortable date-first**: `<YYYYMMDD>-v<N>`, *"Date-first so records sort lexicographically by date"*, primer registro era-2 = `20260905-v5` (secuencia global continuando era-1). Correcto.
- **Ruta pública canónica** (mi nota 7): sección "Canonical public path" presente.
- **Determinismo explícito**: *"`repr(float(x))`, `sort_keys=True`, `indent=2`, LF"* + **orden de arrays parte del contrato** (`cells` sorted by `(scenario, contender, variant, payload_class, metric)`; `ratios` by `(numerator, denominator, metric)`). Esto es lo que hace la byte-identidad reproducible.

**¿Falta algún formato que cueste 10× post-v1?** Revisé los cuatro contratos-eternos de mi direction-check (§f): forward-compat ✓, índice versionado ✓, era/run_id ✓, ruta pública ✓. **Ninguno falta.** No encuentro decisión de formato ausente.

### (3) El records loop — EJECUTADO end-to-end

**PASA con evidencia ejecutada.** Corrí los tests Python read-only:

```
test_summarize: Ran 30 tests ... OK
test_publish:   Ran 9 tests  ... OK      → 39/39 verde
```

Los guards que exigí, pinneados como tests:
- `test_zero_cells_is_loud` + `test_zero_cells_cli_exits_nonzero` → **el bug del inter-phase REJECT (record vacío silencioso) es ahora regresión pinneada**: *"resolved 0 measurement cells … refusing to emit an empty record"*, exit nonzero. El modo de fallo que habría publicado un expediente eterno vacío está guardado ruidosamente.
- `test_check_detects_hand_edit` → la regla de oro (ningún número tecleado a mano) probada mecánicamente.
- `test_publish_refuses_duplicate`, `test_index_dir_crosscheck`, `test_check_index_entry_field_mismatch` → integridad del índice.
- `test_m1_flat_dir_no_prefix_match_is_loud`, `test_split_flat_dir` → **la identidad de celda viene del campo `cell` del summary, no del parseo de nombres de dir** (el corazón del fix w_heavy). El "layout fantasy" que leía nested no puede recurrir: el reader lee el layout FLAT real de run.sh (`cell_safe="${cell//\//_}"`).
- `test_ratio_mirrored_when_binary_flips_direction` + `ci_lo == 1.0/3.0`, `ci_hi == 1.0/1.5` → inversión de ratio pinneada.

### (4) Era-1 freeze — moves + banner, evidencia preservada

**PASA.** `git diff ab1c8424..HEAD -M -- docs/benchmarks/`: los 8 reportes migran con **una sola línea añadida** al head de cada uno: `> Era-1 report. Frozen at git tag bench/era-1-final. Live data: benchmarks/records/.` El cuerpo es byte-idéntico — verificado que los números que declaré load-bearing en mi orientación **siguen intactos**: v4 conserva `78,628 req/s`, `1.18×`, `2.08×`. La evidencia comparativa que rechacé destruir (P2) está preservada, congelada, con banner honesto. El `benchmark-runner:latest` que aparece es una CITA de provenance histórica dentro del reporte congelado — no un claim nuevo, es historia. Correcto no tocarla.

### (5) RUNBOOK — ¿podría YO ejecutar la v1 cold?

**PASA.** Leí `benchmarks/runner/RUNBOOK.md` completo. Podría ejecutarla en frío: (1) verificar quiet-host gates → (2) `bash runner/pin.sh` → `cat runner/DIGEST` → (3) docker socket para native-image → (4) la corrida en un comando → (5) summarize/publish/--check → (6) gauges ON confirmado → (7) payload axis marcado explícitamente **NO WIRED** (honestidad exigida en mi nota 4) → (8) checklist de validación post-corrida human-gated. Encabezado inequívoco: *"the run itself is human-invoked — the hours-long quiet-host predicate cannot be guaranteed by an agent"* — mi corrección de spec (nota 1) aterrizó como doctrina. **Sin agujeros que bloqueen.** Único matiz: §3 asume acceso al docker socket disponible en el host del humano — es correcto para un procedimiento humano, no es hueco.

### (6) Overclaim scan — ¿alguien dice que el v1 record EXISTE?

**PASA, limpio.** `records/index.json` = `{"index_schema_version": 1, "runs": []}` — **vacío, cero registros reclamados.** No hay dir de corrida, no hay run.json publicado. El grep por claims de "record exists / site serves records" no devuelve ninguna afirmación presente-tenso: las menciones de `v5`/`20260905` son de FORMATO ("el primer registro era-2 SERÁ `20260905-v5` style"), no de existencia. COVERAGE T2j/T2s siguen `open-if: awaiting first container run` — vocabulario respetado, sin promoción prematura a "measured". El static-fetch GIVEN queda abierto (docs-site wiring diferido) y está declarado como tal. **Nadie miente sobre lo que aún no corrió.**

### (7) Proporcionalidad + escape check

**Sano, sin bypass.** 48 commits, `grep -iE 'wip|fixup|squash!|revert|hack|bypass|skip'` → **ninguno**. Historia legible (`fix(bench): struct-serialize ratio json`, `chore(bench): freeze era-1 reports into history`, etc.). Dos task-REJECTs y un inter-phase REJECT resueltos con fix + verificación live (el inter-phase se cross-validó contra el dir era-1 real `20260723T161422Z`: 12 celdas, 5 ratios, lib/standalone 1.1769 [1.1665,1.1932] = exactamente los números v4 publicados — validación cruzada contra mi propia evidencia de orientación). **Nada que yo rechazaría ahora que gates más baratos hayan dejado pasar.** El único hallazgo que un gate barato podría haber perdido — el `serde_json` feature-unification — fue cazado por `cargo test --workspace` y arreglado estructuralmente (ver 8). Eso es rigor, no ceremonia.

### (8) El fix serde_json (72a9c78a) — determinismo incondicional

**PASA, y es el fix correcto.** Verificado en `benchmarks/harness/loadgen/src/ratios.rs:165-171`:

```rust
#[derive(serde::Serialize)]
struct RatioJsonLine<'a> {
    ci_hi: f64,        // ← orden de declaración = alfabético
    ci_lo: f64,
    denominator: &'a str,
    method: &'a str,
    metric: &'a str,
    ...
}
```

Serde serializa campos de struct en **orden de declaración**, fijado en compile-time, **independiente del tipo `Map`**. Por eso el fix esquiva por completo el hazard de `serde_json::Map` ordering que la unificación de features (`preserve_order`/IndexMap de otro miembro del workspace) introdujo: un struct no consulta el `Map`. Es **incondicionalmente determinista** — no depende del build mode ni de qué features unifique cargo. El golden test lo pinnea: `ratios.rs:545` `assert_eq!(line1, line2)` (byte-identidad en dos invocaciones) + comentario línea 164 *"line is sorted-key regardless"*. La lección grabada ("never trust serde_json::Map ordering in this workspace") es correcta y el fix la honra estructuralmente en vez de por convención.

---

## Balance

Mis 7 notas del direction-check: **7/7 incorporadas** y verificadas en el árbol. El giro honesto en la nota 4 (el sweep de payload-axis NO está wired, RUNBOOK §7 lo dice explícitamente en vez de fingir un comando que era no-op) es *más* fiel a mi doctrina "decouplable-not-gate" que una implementación completa apresurada — el reviewer cazó que el comando copy-paste era no-op y el equipo eligió honestidad sobre teatro. Eso es exactamente el estándar papal.

El expediente eterno que se publica hoy: (a) formato bloqueado con forward-compat + índice versionado, (b) records/ vacío sin overclaim, (c) evidencia era-1 congelada con banner sin mutación, (d) el bug que habría publicado un record vacío ahora es una regresión ruidosa pinneada, (e) determinismo incondicional en la única superficie donde el workspace tenía un hazard de ordering. Nada de esto es ceremonia; todo es contrato-eterno que el pre-1.0 abarató y que este change pagó.

No encuentro razón para retener la bendición. Los tres condiciones de divulgación abajo son para el cuerpo del merge — no son bloqueos, son la verdad que el registro debe llevar.

---

## VEREDICTO

**BLESSING GRANTED**

Disclosure conditions para el cuerpo del squash-merge (no bloqueos — transparencia del expediente):

1. **El v1 record NO existe todavía.** Este merge publica el *aparato* (schema, loop, guards, RUNBOOK, freeze) con `records/index.json` vacío. La gran corrida es human-invoked, tracked en bd rc-f4po; la bendición del aparato no es la bendición de números que aún no se midieron.
2. **El tag `bench/era-1-final` se crea POST-merge** sobre `<merge-sha>^1` (primer padre = último commit de main con los reportes en `docs/benchmarks/` sin mover, porque el move es branch-side). La lógica `^1` es correcta: verificada contra el diff — los reportes solo se movieron en la rama, así que merge^1 apunta al último estado main pre-freeze. El push del tag es acción del humano.
3. **El payload-axis sweep no está wired** (RUNBOOK §7, honesto); las celdas de payload son companion opcional, no gate del v1 record. Wiring diferido a rc-f4po.

---

### Self-grill record

**Questions generated:**
1. [glossary] ¿"7 entradas" es el número honesto, o estoy escondiendo dotfiles que recrearían canas bajo otra forma?
2. [sharpen] "BLESSING GRANTED" — ¿bendigo el aparato o bendigo la evidencia? ¿son la misma cosa o dos, y el merge body lo distingue?
3. [scenario] Si el humano corre la v1 mañana y summarize resuelve 0 celdas (era-1 dir vacío, o run.sh cambió su layout), ¿el expediente eterno sale vacío en silencio?
4. [cross-ref] ¿El fix serde (struct alfabético) es REALMENTE incondicional, o depende de una feature que otro build mode podría desactivar y re-romper el ordering?

**Answers (with citations):**
1. [glossary] Honesto. `ls benchmarks/` (lo que el humano hace) = 7 entradas exactas; los dotfiles `.cache`/`.gitignore` no aparecen en `ls` sin `-A` y `.cache` no está en git (`git ls-files benchmarks/.cache` = 0). No hay canas escondidas bajo dotfiles — `.cache` es scratch invisible, `.gitignore` es infraestructura de una línea (`__pycache__/`). El zone contract de 7 se cumple en la superficie que el humano ve. Outcome: confirm.
2. [sharpen] **Dos cosas, y las distingo en el veredicto.** Bendigo el APARATO (schema/loop/guards/freeze) — está completo, testeado 39/39, formato eterno bloqueado. NO bendigo NÚMEROS: el v1 record no existe (`index.json` runs=[]). La disclosure condition 1 hace la distinción explícita en el merge body. Confundirlas sería el error — un lector futuro no debe creer que este merge midió algo. Split aplicado. Outcome: confirm con disclosure.
3. [scenario] Input construido: run dir sin celdas usables. Ejecuté `test_zero_cells_cli_exits_nonzero` → produce *"resolved 0 measurement cells … refusing to emit an empty record"* y exit nonzero (verificado en la corrida real de unittest arriba). El summarize.py docstring línea 18-19: *"A run dir that resolves to 0 cells is a LOUD error — never a silent empty record."* El modo de fallo exacto del inter-phase REJECT está guardado y pinneado. NO sale vacío en silencio. Outcome: confirm — el escenario que más temía está cubierto por test ejecutado.
4. [cross-ref] Incondicional. `ratios.rs:165` `#[derive(serde::Serialize)] struct RatioJsonLine` con campos en orden alfabético de declaración. Serde emite campos de struct en orden de declaración (compile-time), sin consultar `serde_json::Map` — luego la unificación de features `preserve_order`/IndexMap (que rompía `json!` maps) no puede afectar un struct. El golden `ratios.rs:545 assert_eq!(line1,line2)` pinnea byte-identidad. No hay feature que reordene campos de struct: es propiedad del lenguaje, no de una crate feature. Outcome: confirm — el fix es estructural, no una mitigación condicional.

**Outcome:** confirm — la bendición se sostiene tras el auto-grilling. Los cuatro riesgos que interrogué (canas-por-dotfile, aparato-vs-evidencia, record-vacío-silencioso, serde-condicional) están resueltos con evidencia ejecutada o citada, no con aserción. El expediente eterno que se publica hoy es honesto sobre lo que es (aparato + freeze) y sobre lo que no es todavía (la gran corrida). BLESSING GRANTED con 3 disclosure conditions.
**Self-grill mode:** self-grill-proposals skill
