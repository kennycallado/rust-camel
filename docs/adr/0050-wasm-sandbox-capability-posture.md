# ADR-0050: Postura de capacidades del sandbox WASM

**Fecha:** 2026-08-06
**Estado:** Aceptado; implementación pendiente
**Decisión:** Opción B, registro selectivo de WASI por mundo
**Referencias:** ADR-0011, ADR-0014, ADR-0031, ADR-0032, ADR-0033
**Origen:** auditoría de `camel-component-wasm`, hallazgos
`F-camel-component-wasm-I1` y `F-camel-component-wasm-I2`

## Contexto

El host WASM expone dos superficies de capacidades. La primera contiene las
funciones Camel de `wit/camel-plugin.wit`. La segunda contiene las interfaces
WASI 0.2 que registra Wasmtime.

`WasmCapabilities` controla la primera superficie. Las llamadas
`camel_call` y `camel_poll` usan una lista de esquemas permitidos. Una lista
vacía deniega todos los esquemas. Los mundos de política usan
`WasmCapabilities::denied()`. Los mundos de processor y bean permiten el
almacén del host de forma explícita.

La segunda superficie no sigue esa postura. El código actual llama a
`wasmtime_wasi::p2::add_to_linker_async` en los cuatro mundos. El contexto WASI
no concede preopens, variables de entorno, puertos de socket ni resolución de
nombres. Sin embargo, el linker anuncia la superficie WASI completa. Además,
los mundos de processor, bean y política heredan stderr. El mundo source no lo
hereda. Esta diferencia no responde a una política de seguridad.

El modelo de confianza acepta plugins instalados por el operador. El sandbox
limita defectos del huésped. No obstante, una capacidad que el host no necesita
no debe aparecer en el linker. Una actualización de Wasmtime o un cambio en
`WasiCtxBuilder` no debe ampliar capacidades por accidente.

## Decisión

Adoptamos la **Opción B: registro selectivo de WASI por mundo**.

El host aplicará estas reglas:

1. Cada mundo tendrá una lista explícita de interfaces WASI.
2. Los cuatro mundos podrán registrar `wasi:clocks` y `wasi:random` cuando sus
   componentes las importen.
3. Ningún mundo registrará filesystem, sockets, CLI, environment ni stdio por
   defecto.
4. El mundo source conservará su interfaz `http-listener`. Esta interfaz no
   concede acceso general a sockets.
5. Los mundos que tienen funciones Camel usarán `camel_call` para logging.
   El host no llamará a `inherit_stderr()`.
6. Una nueva interfaz WASI requiere una concesión por mundo, una prueba
   negativa para los demás mundos y documentación en el contexto del crate.

La postura de funciones Camel sigue el mismo principio. Los esquemas de
`camel_call` y `camel_poll` usan una lista permitida. Los mundos de política no
reciben operaciones de llamada ni almacén. Las concesiones de processor y bean
permanecen explícitas en `WasmCapabilities`.

Esta decisión describe el estado objetivo. El código actual todavía registra
WASI completo y mantiene la herencia desigual de stderr. Los hallazgos de la
auditoría cubren esa migración en el flujo de código.

## Consecuencias

### Positivas

- El linker y el contexto expresan la misma política de capacidades.
- Filesystem, sockets y variables de entorno no dependen de valores por defecto
  de Wasmtime para quedar denegados.
- Cada ampliación futura deja una concesión revisable por mundo.
- Los mundos de política conservan una superficie menor que los mundos de
  processor y bean.

### Negativas

- El registro selectivo acopla el host a APIs de submódulos de
  `wasmtime-wasi`.
- Las actualizaciones de Wasmtime pueden exigir cambios en varios registradores.
- Los huéspedes que usan `eprintln!` dejan de funcionar hasta que migren al
  canal de logging Camel. Los huéspedes source no tendrán salida stderr.

### Neutrales

- Los límites de memoria, instancias, tablas y epoch de ADR-0014 no cambian.
- La interfaz `http-listener` del mundo source sigue bajo ADR-0031.
- La configuración del operador sigue siendo confiable. Los datos del Exchange
  siguen siendo no confiables según ADR-0032.

## Opciones consideradas

### Opción A: WASI completo con denegación en el contexto

Rechazada. Tiene menor coste inmediato, pero el linker anuncia capacidades que
el host no pretende conceder. La seguridad depende de valores por defecto y de
que ningún cambio futuro amplíe el contexto.

### Opción B: registro selectivo por mundo

Elegida. Mantiene compatibilidad con clocks y random, y elimina interfaces que
los huéspedes no necesitan. El coste de integración con Wasmtime es aceptable
para obtener una superficie verificable.

### Opción C: eliminar WASI

Rechazada. Es la superficie mínima, pero rompe huéspedes compilados con imports
comunes de clocks o random. La opción B obtiene la mayor parte del beneficio sin
imponer esa incompatibilidad general.

## Relación con otras decisiones

ADR-0014 unifica configuración y límites de recursos del runtime WASM. No
define qué interfaces puede importar un huésped. Esta ADR decide una clase
distinta: la superficie de capacidades del sandbox. Por eso no modifica
ADR-0014.

ADR-0031 define el ciclo de vida del mundo source y su recurso
`http-listener`. ADR-0032 define la dirección de confianza de los datos del
Exchange. ADR-0033 exige valores seguros y concesiones específicas. Esta
decisión aplica esas reglas al linker WASI.

## Registro de self-grill

1. **Glosario:** "postura de capacidades del sandbox WASM" no reemplaza
   Component, Endpoint ni SecurityPolicy. Nombra la unión de dos superficies:
   funciones Camel y WASI. `CONTEXT-MAP.md` registra el término transversal.
2. **Precisión:** la decisión no afirma que todas las funciones Camel estén
   denegadas por defecto. `from_scheme_list()` habilita el almacén para
   processor y bean. La lista vacía solo deniega esquemas de llamada.
3. **Escenario:** un huésped que importa filesystem no podrá instanciarse. Ese
   fallo es intencional. Un huésped que solo importa clocks y random conserva
   compatibilidad.
4. **Código:** `runtime.rs`, `wasm_plugin_context.rs` y `source_host.rs` aún
   llaman a `add_to_linker_async`. `runtime.rs` aún usa `inherit_stderr()`.
   Por tanto, la ADR declara estado objetivo y no describe el código actual
   como ya conforme.

**Resultado:** aprobar la Opción B como decisión workspace-wide. La decisión es
costosa de revertir, sorprendente sin contexto y resuelve un trade-off real.
**Modo:** `self-grill-proposals`.
