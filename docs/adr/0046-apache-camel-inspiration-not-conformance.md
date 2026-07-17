# ADR-0046: Apache Camel como corpus de inspiración, no autoridad de conformance

**Date:** 2026-07-17
**Status:** Accepted
**Amends:** none
**Cross-refs:** epic rc-ca8z (positioning decision que este ADR codifica a nivel arquitectural), ADR-0019 (ExceptionDisposition — base de divergencias tipo D2), ADR-0024 (PipelineOutcome — reemplaza CamelError::Stopped), ADR-0025 (outcome-aware structural EIPs), ADR-0032 (Exchange-data trust boundary), ADR-0033 (security defaults — policy-ADR precedent)

## Decision

Apache Camel es **corpus de inspiración de diseño**, NO autoridad de conformance. Las decisiones sobre qué debe hacer un EIP se diseñan contra los ADRs del proyecto, no contra el comportamiento observado en Camel. Camel sigue siendo valioso porque codifica 20 años de edge cases reales de producción — pero como **input al diseño**, no como spec de aceptación.

### Protocolo de consulta (obligatorio para EIPs nuevos o rediseños mayores)

Cuando se diseñe/implemente un EIP nuevo o se rediseñe uno existente sustantivamente:

1. **Disparo por densidad de divergencia.** Aplicar el protocolo completo **solo si** el EIP toca ≥1 ADR que rompe conformance con Camel. Marcadores operativos:
   - EIP stateful (agregación, correlación, repositorios)
   - Temporización con completion/timeout (aggregate completion, resequencer)
   - Control-flow divergente (ADR-0019 ExceptionDisposition, ADR-0024/0025 PipelineOutcome, Stop EIP)
   - Trust-boundary impact (ADR-0032 — datos no confiables en sinks/numeric decisions)
   - Backpressure/admission (ADR-0044 — Camel no tiene `poll_ready`)

   Para EIPs stateless casi idénticos a Camel (Filter, Content-Based Router, Throttle, SetBody/SetHeader), basta un **audit de coverage puro** — no se requiere leer Camel.

2. **Dosis: 3 tests, no 5.** Leer 3 tests representativos de `apache/camel/<comp>/src/test/java/...`. Parar cuando 2 consecutivos no aporten escenario nuevo. La tabulación completa (5+ tests) tiene valor marginal plano.

3. **Clasifica mientras lees.** Sin fase separada de tabulación. Por cada test: extrae (a) escenario, (b) invariante del EIP ejercitada, (c) decisión: igual/diverge. Las divergencias se documentan inline en el ADR del EIP (si existe) o en el `CONTEXT.md` del crate (si es cross-EIP).

4. **Tests nativos, no traducciones.** Escribir tests con el harness del proyecto (`CamelTestContext`, `MockEndpoint`, etc.) aseverando **nuestra** semántica. Nunca traducir asserts literalmente — la traducción produce verde inválido o rojo espurio.

5. **KPI: divergencias-documentadas/EIP**, no bugs/hora. Los bugs encontrados por este protocolo son bugs que un audit de coverage habría encontrado igual. El valor irreemplazable son las **divergencias forzadas a documentarse** — esas solo se revelan al leer el espacio de features de Camel que deliberadamente no implementamos.

## Context

El epic `rc-ca8z` fija el posicionamiento: "cloud-native runtime distinto con EIP vocab compat ONLY, no drop-in replacement". La consecuencia operacional — "qué hace un dev cuando se pregunta si un EIP debe comportarse como en Camel" — no estaba codificada. Sin codificación, dos riesgos:

1. **Deriva por inercia:** dev porta tests por costumbre, generando verde inválido (test pasa por la razón equivocada) o rojo espurio (test falla sobre comportamiento que nuestros ADRs declaran correcto).
2. **Pérdida de memoria:** las decisiones de divergencia se toman implícitamente y se pierden en compactación de contexto, dejando deuda cognitiva al próximo contribuidor.

El spike `rc-spt-camel-splitter-spike` (rama `spike/rc-spt-camel-splitter-spike`, commit `8d31e74a`) produjo evidence concreta:

- **2 divergencias forzadas a documentarse** (D1 `parallelAggregate()` no aplica arquitectónicamente — `join_all` secuencial; D2 agregación recibe `Err(e)` en `Vec`, no Exchange con excepción adjunta — ADR-0019). Ambas son decisiones que **solo leer Camel revela**.
- **1 bug real** (G3 `CAMEL_SPLIT_SIZE` nunca seteado en último fragment streaming). Este es un hueco de coverage que un audit habría encontrado.
- **3 pineos de invariantes preexistentes** (G1 IDs únicos por fragmento, G2 split JSON Array, semántica streaming). Coverage pero no bugs.

El spike también confirma que un `cargo xtask port-camel-test` automático habría: producido verde inválido en `parallelAggregate()` (semántica inexistente en rust-camel); producido rojo espurio en `testSplitterWithException` (Camel pasa exchange fallido a la estrategia, nosotros `Err` en Vec); perdido G1 y G3 (asserts sin traducción directa). **El porteo automático institucionaliza el error de confundir "Camel hace X" con "X es correcto".**

## Consequences

### Positivas

- Las decisiones de divergencia sobreviven a compactación de contexto al aterrizar en ADRs/CONTEXT.md tracked.
- Un nuevo dev no pregunta "¿por qué rust-camel no tiene `parallelAggregate()`?" — la respuesta vive en el ADR/CONTEXT del EIP.
- Costo de diseño predecible: 3 tests por EIP divergente, no más.
- KPI medible y honesto: divergencias-documentadas, no falsos positivos de coverage.

### Negativas

- Inversión de tiempo de diseño por EIP divergente (lectura + clasificación + documentación). Asumido como costo de ser una reinvención, no un port.
- Riesgo de sobre-aplicar el protocolo a EIPs stateless donde un audit bastaba. El disparo por densidad mitiga, pero requiere juicio.

## Scope (no retroactivo)

El protocolo aplica a **EIPs nuevos o rediseños mayores posteriores a este ADR**. No obliga a aplicar retroactivamente a ADRs estables (p.ej. ADR-0006 Script EIP, ADR-0019 error handling). Para EIPs existentes, la consulta de Camel es opcional y solo cuando emerge una pregunta de diseño concreta.

## Anti-patrones

1. **"Camel hace X ⟹ X es correcto".** Nuestros ADRs prueban lo contrario: ADR-0024 llama al modelo `CamelError::Stopped`/HTTP-204 un **bug** que Camel-el-diseño induce; ADR-0032 llama al trust model de Camel un riesgo de seguridad rechazado. Camel es punto de partida, no oráculo.
2. **Traducir asserts literalmente.** `expectedBodiesReceived(...)` de un test de error handling asume el modelo Processor-chain. El assert equivalente en rust-camel depende del `ExceptionDisposition` aplicado — no es traducción, es re-derivación.
3. **Tratar verde-portado como coverage.** Un test porteado que pasa por acoplamiento accidental a semántica que no compartimos NO valida la invariante correcta.
4. **Automatizar el porteo "para escalar".** Escalar el error no lo corrige. La disciplina de decidir divergencia-por-EIP no es paralelizable ni automatizable; es el trabajo de diseño que hace de rust-camel una reinvención y no un port.
5. **Documentar divergencias en docs efímeros.** Los spike docs viven bajo `docs/*` (gitignored por policy). Las divergencias documentadas deben aterrizar en **tracked docs** (ADRs, `CONTEXT.md`, o como `notes` en bd referenciando el ADR/CONTEXT), no en gitignored artifacts.

## Rejected alternatives

- **`cargo xtask port-camel-test`**: descartado por evidence del spike Splitter (ver Context). Habría producido verde inválido + rojo espurio + pérdida de invariantes no traducibles.
- **Conformance TCK unificado**: no existe para Apache Camel y rompería el positioning decision del epic rc-ca8z.
- **Prohibir leer Camel**: excesivo. Pierde el valor real (edge cases de 20 años de producción). El protocolo captura ese valor sin convertirse en spec de aceptación.
- **Política ad-hoc sin ADR**: cada dev decide por sí mismo. Reproduce los dos riesgos del Context (deriva + pérdida de memoria).

## Measurement

El KPI `divergencias-documentadas/EIP` se aplica:

- Por cada EIP sujeto al protocolo, registrar en bd `discovered-from: <EIP-issue>` los hallazgos con label `divergence` o `gap-coverage` o `pin-invariant`.
- Cerrar el bd al aterrizar la divergencia en tracked doc (ADRs nuevos/amendments, `CONTEXT.md` updates).
- Métrica de salud del protocolo: ratio `divergences / (divergences + gaps)` por EIP. Si tiende a 0, el EIP no divergía y el protocolo sobre-invirtió → próxima vez aplicar audit de coverage.

## Evidence

- **Spike Splitter**: rama `spike/rc-spt-camel-splitter-spike`, commit `8d31e74a`. Spike doc (gitignored): `docs/spikes/camel-splitter-conformance-spike.md`.
- **Consulta al oráculo (e_opus)**: 2 pasadas, sesión `ses_08fc0fd19ffei7uuZcFoOrbnyq`. Veredicto: protocolo validado por divergencias (D1/D2), no por bugs (G3 — coverage).
- **bd follow-ups**: `rc-0dgq` (D2 doc en `crates/camel-processor/CONTEXT.md`).

## Self-grill record

**Questions generated:**

1. [glossary] ¿"Camel corpus de inspiración" usa términos que chocan con entradas de CONTEXT-MAP?
2. [sharpen] ¿Cómo operacionalizar "densidad de divergencia" para que no sea subjetiva?
3. [scenario] ¿El protocolo aplica retroactivamente a ADRs estables (p.ej. Script EIP ADR-0006)?
4. [cross-ref] ¿La evidence del spike es rastreable o efímera?

**Answers (with citations):**

1. [glossary] No hay entrada en CONTEXT-MAP sobre Camel como autoridad. La "Documentation Authority & Refresh" (`CONTEXT-MAP.md:127-152`) lista source code → ARCHITECT.md → CONTEXT-MAP → README — Camel fuera de la lista. El ADR es consistente: codifica que Camel queda fuera del orden de autoridad.
2. [sharpen] Operacionalizado con 5 marcadores: stateful, temporización/completion, control-flow divergente (ADR-0019/0024/0025), trust-boundary (ADR-0032), backpressure (ADR-0044). ≥1 marcador → protocolo completo; 0 → audit puro. Reflejado en sección Decision §1.
3. [scenario] No retroactivo — aclarado en sección "Scope". ADR-0006 no requiere re-aplicar el protocolo. Aplica a EIPs nuevos o rediseños mayores posteriores a este ADR.
4. [cross-ref] Spike doc está gitignored (`docs/*` policy, verificado en `.gitignore:3`). Drift detectado: las divergencias documentadas en spike docs se perderían. Solución: sección "Anti-patrones §5" obliga a aterrizar divergencias en tracked docs. rc-0dgq ya abierto para D2.

**Outcome:** refine (aplicado)
**Self-grill mode:** self-grill-proposals skill
