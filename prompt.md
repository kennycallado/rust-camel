# Prompt para agente: `function:` DSL Step en rust-camel

Estamos trabajando en `kennycallado/rust-camel`.

Necesitamos empezar formalmente el diseño e implementación de una nueva capacidad crítica del proyecto: un step DSL `function:` inspirado en Knative Functions / serverless / edge functions.

Usamos Superpowers. Primero crea un SPEC completo y después un PLAN de implementación. No empieces a codificar hasta que el spec y el plan estén claros.

## Contexto ya investigado

- `camel-dsl` es extensible y añadir un nuevo step sigue el patrón:
  `model.rs → contract.rs → yaml_ast.rs → yaml.rs → compile.rs → route_definition.rs → step_resolution.rs`.
- `script:` ya existe y debe seguir siendo para ejecución in-process vía `camel-language-*` (`simple`, `rhai`, etc.).
- `function:` debe ser out-of-process, async, gestionado por runners/containers.
- Runtime inicial preferido para v1: **Deno ejecutando TypeScript**.
- `camel-container` ya cubre gran parte de las necesidades: port bindings, env vars, mounts, exec, log streaming y cleanup mediante `CONTAINER_TRACKER`.
- `CamelContext` ya tiene lifecycle services con `with_lifecycle()`.
- El patrón `as_metrics_collector()` en `Lifecycle` sirve como referencia para exponer capacidades adicionales desde servicios.
- La recomendación actual es un `FunctionRuntimeService` context-scoped que implemente `Lifecycle` y exponga un `FunctionInvoker`.
- `resolve_steps()` es síncrono: no debe arrancar containers. Solo debe construir `FunctionStep` con un invoker ya disponible.
- Hot-reload debe estar soportado desde v1, no como “fase 2”.

## Objetivo general

Implementar `function:` como una primitive seria de rust-camel para ejecutar funciones gestionadas fuera del proceso Rust, inicialmente mediante runners containerizados locales, manteniendo semántica Camel: Exchange, errores, lifecycle, hot-reload, observabilidad y cleanup correcto.

## Ejemplo objetivo de DSL v1

```yaml
routes:
  - id: enrich-users
    from: "direct:start"
    steps:
      - function:
          runtime: deno
          timeout_ms: 5000
          source: |
            const body = camel.body()
            camel.setBody(`${body} enriched`)
      - to: "log:info"
```

## Requisitos de aceptación para v1

La v1 NO debe ser un MVP anémico. Para considerarse aceptable, debe incluir como mínimo:

### 1. DSL `function:` explícito

- Soportar forma larga:
  `function: { runtime, source, timeout_ms }`.
- Runtime inicial obligatorio: `deno`.
- El código fuente debe poder ser TypeScript ejecutado por Deno.
- No es obligatorio soportar shorthand tipo `function: { deno: | ... }` en v1.

### 2. Separación clara con `script:`

- `script:` permanece in-process.
- `function:` es out-of-process y async.
- Documentar esta diferencia.

### 3. Nuevo crate o módulo claro

Preferiblemente `camel-function`.

Debe contener:

- `FunctionRuntimeService`
- `FunctionInvoker`
- `FunctionStep`
- `FunctionDefinition`
- `FunctionId`
- `RunnerPool`
- `RunnerPoolKey`
- `ExchangePatch`
- provider container inicial
- cliente HTTP del runner

### 4. Integración con lifecycle

- `FunctionRuntimeService` debe registrarse con `CamelContext::builder().with_lifecycle(...)`.
- Debe arrancar antes que las rutas.
- Debe parar después que las rutas.
- Debe limpiar runners/containers sin fugas.
- Debe seguir el patrón existente de `as_metrics_collector()` si se añade algo como `as_function_invoker()`.

### 5. Registro pre-start y post-start

- Las `FunctionDefinition`s añadidas antes de `ctx.start()` deben quedar pendientes y arrancarse en `start()`.
- Las rutas añadidas después de `ctx.start()` deben poder registrar funciones y arrancar runners correctamente.
- No aceptar dejar el registro tardío para una fase futura.
- Evitar mantener locks mientras se hacen `.await`.
- Usar patrón snapshot → async work → commit.

### 6. Hot-reload funcional desde v1

- Detectar funciones añadidas, modificadas, eliminadas y no modificadas.
- Reusar funciones/runners no modificados cuando sea posible.
- Registrar nuevas funciones.
- Limpiar funciones eliminadas.
- No filtrar containers ni handles.
- Definir comportamiento si una nueva función falla durante reload:
  - preferiblemente reload transaccional o rollback claro;
  - si no es viable, documentar limitación y dejar el sistema en estado seguro.

### 7. Runner Deno/TypeScript funcional

- Runtime inicial obligatorio: Deno ejecutando TypeScript.
- Runner containerizado con endpoints mínimos:
  - `GET /health`
  - `POST /register`
  - `POST /invoke`
- El runner debe aceptar scripts TypeScript registrados por `FunctionId`/hash.
- El runner debe exponer una API `camel` idiomática para TypeScript:
  - `camel.body()`
  - `camel.setBody(value)`
  - `camel.header(name)`
  - `camel.setHeader(name, value)`
  - `camel.removeHeader(name)`
- Debe devolver `ExchangePatch` o error estructurado.

### 8. Protocolo `ExchangePatch`

- No devolver ni mutar el `Exchange` interno completo sin contrato.
- Definir JSON estable para:
  - set body
  - set headers
  - remove headers, si aplica
  - error estructurado
- Manejar body textual/JSON en v1.
- Binario/streams pueden quedar para después si se documentan.

### 9. Timeouts y errores

- `timeout_ms` obligatorio o default explícito.
- Timeout debe cancelar/fallar la invocación.
- Errores del runner deben mapearse a `CamelError` compatible con error handlers, retries y dead letter channels.
- Incluir runtime, function_id y mensaje/stack cuando exista.

### 10. Health polling

- El servicio debe esperar a que el runner esté healthy antes de arrancar rutas.
- Si el runner no está healthy, `ctx.start()` debe fallar limpiamente.
- No vale solo “crear container y asumir que está listo”.

### 11. Observabilidad mínima

- tracing con:
  - route_id
  - step_id
  - function_id
  - runtime
  - provider
  - timeout
  - duration
  - correlation_id si está disponible
- logs del runner accesibles o reenviados razonablemente.
- Métricas pueden ser básicas, pero el diseño debe dejar claro cómo se integran.

### 12. Seguridad mínima

- Env vars allowlist, no pasar todo el environment por defecto.
- Timeout obligatorio.
- Cleanup garantizado.
- Documentar riesgos de ejecutar código arbitrario.
- Definir defaults seguros para mounts/network en lo posible.

### 13. Tests

- Tests unitarios de parseo DSL.
- Tests de compile/resolution del step.
- Tests de `ExchangePatch`.
- Tests de lifecycle start/stop.
- Tests de registro pre-start y post-start.
- Tests o integración controlada para hot-reload add/change/remove.
- Tests de timeout/error.
- Si Docker no está disponible en CI, separar tests dockerizados bajo feature/ignore claro.

### 14. Documentación

- Añadir docs del step `function:`.
- Explicar `script:` vs `function:`.
- Añadir ejemplo funcional.
- Documentar limitaciones v1.

## Fuera de alcance para después de v1

Esto NO bloquea v1:

- Provider Knative.
- Provider AWS Lambda.
- Provider Cloudflare Workers.
- Provider WASM in-process.
- Python runner.
- PHP runner.
- Node runner separado de Deno.
- Shorthand syntax `function: { deno: | ... }`.
- Buildpacks.
- Registry push.
- Kubernetes manifests.
- Autoscaling avanzado.
- Scale-to-zero.
- Unix sockets/gRPC.
- Binary body completo.
- Streaming body.
- Dependency packaging avanzado.
- UI/CLI avanzada para scaffolding de funciones.
- Remote function projects tipo `source: ./functions/foo`.

## Decisiones arquitectónicas esperadas en el SPEC

El SPEC debe responder explícitamente:

### 1. ¿Dónde vive `FunctionRuntimeService`?

Recomendación esperada: context-scoped lifecycle service.

### 2. ¿Dónde vive el contrato `FunctionInvoker`?

- Evaluar si debe estar en `camel-api`, `camel-core` o `camel-function`.
- Preferir contrato mínimo estable y evitar que `camel-core` conozca detalles de Docker/runners.

### 3. ¿Cómo llegan las `FunctionDefinition`s al service?

Debe cubrir:

- antes de `ctx.start()`;
- después de `ctx.start()`;
- durante hot-reload.

### 4. ¿Cómo se evita async dentro de `resolve_steps()`?

- `resolve_steps()` solo construye processors y registra metadata/invoker.
- Nada de arrancar containers ahí.

### 5. ¿Cómo se manejan hot-reload y cleanup?

- Definir diff de funciones.
- Definir ownership por `route_id`/`function_id`.
- Definir cleanup de funciones removidas.
- Definir qué pasa si falla una función nueva.

### 6. ¿Cómo se comparte un runner?

- Definir `RunnerPoolKey`.
- Mínimo: runtime + image + provider + env/resources relevantes.
- Definir cuándo scripts comparten runner.

### 7. ¿Cómo se evitan locks sobre await?

Diseñar explícitamente snapshot → async work → commit.

### 8. ¿Cómo se integra con `camel-container`?

- Para v1, puede usar bollard directamente o extraer utilidades.
- No usar el endpoint `container:` como si fuera un step por exchange.
- Los runners son infraestructura del proceso.

## Entregables

Primero:

- Crear un SPEC usando Superpowers.
- Guardarlo en una ruta tipo:
  `docs/superpowers/specs/function-dsl-step.md`
  o similar según la convención existente.

Después:

- Crear un PLAN de implementación usando Superpowers.
- El plan debe dividirse en tareas concretas, revisables y testeables.
- El plan no debe posponer requisitos esenciales de v1 a “fase 2”.
- Puede marcar claramente “post-v1” para Knative, Lambda, WASM, Python, PHP, etc.

El PLAN debe incluir al menos estas áreas:

1. Contratos/API
2. DSL parsing
3. Route definition / compile
4. Step resolution
5. FunctionRuntimeService lifecycle
6. Function registration pre/post start
7. RunnerPool
8. Deno/TypeScript runner
9. HTTP protocol
10. ExchangePatch
11. Error/timeout handling
12. Health polling
13. Hot-reload integration
14. Cleanup/container tracking
15. Tests
16. Docs/examples

## Reglas importantes

- No hagas una implementación superficial.
- No dejes hot-reload, registro tardío, cleanup o health polling para después.
- Sí puedes dejar providers avanzados para después.
- Mantén cambios en `camel-core` mínimos y justificados.
- Si hay una decisión que requiere tocar arquitectura core, explícala antes de codificar.
- Si encuentras que algún requisito v1 es inviable sin un cambio grande, documenta la alternativa y el coste.

La idea central de v1 es:

> Puede soportar solo Deno/TypeScript con `provider: container`, pero lifecycle, hot-reload, cleanup, health, errores, timeouts y registro tardío tienen que estar bien diseñados e implementados desde el inicio.
