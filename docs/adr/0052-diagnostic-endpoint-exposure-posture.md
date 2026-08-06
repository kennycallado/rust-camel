# ADR-0052: Postura de exposición de endpoints de diagnóstico

**Fecha:** 2026-08-06
**Estado:** Aceptado
**Amends:** none
**Referencias:** ADR-0009 (co-hosting HTTP de rutas API y estáticas — plano de datos),
ADR-0032 (frontera de confianza de datos de exchange), ADR-0033 (defaults seguros y
validación fail-closed en arranque), ADR-0051 (redacción de credenciales en fronteras de
diagnóstico)
**Origen:** auditoría de `camel-prometheus`, hallazgo `F-camel-prometheus-I1`
(`FC-METRICS-EXPOSURE`, bd `rc-asm9`); superficie compartida con `camel-health`.

## Decisión

Los **endpoints de diagnóstico** —`/metrics` de `camel-prometheus` y
`/healthz`, `/readyz`, `/startupz`, `/health` de `camel-health`— siguen el modelo de
scrape de Prometheus: **no autenticados por defecto**, con TLS y autenticación como
**hooks opcionales**, y con **bind a loopback preferido** por defecto. El aislamiento de
red (NetworkPolicy, firewall) es responsabilidad del operador.

Un endpoint de diagnóstico es un endpoint HTTP que expone metadata operacional
(nombres de ruta, tipos de error, volúmenes de tráfico, profundidad de cola, estado de
circuit breaker, señales de liveness/readiness) para su consumo por sistemas de
observabilidad. **No es** plano de datos: no procesa mensajes de negocio ni cruza la
frontera de confianza de ADR-0032.

### Reglas

1. **No autenticado por convención.** El endpoint no monta capa de autenticación por
   defecto. Esto sigue el modelo de scrape de Prometheus, donde la protección canónica
   es la política de red, no la autorización a nivel de aplicación. No es una brecha de
   authz de negocio: la metadata operacional no es la superficie que ADR-0010 protege
   (autorización pre-pipeline de rutas).

2. **TLS y autenticación son hooks opt-in.** El crate expone un punto de extensión para
   envolver el router con TLS (patrón `axum_server::tls_rustls`, como en
   `camel-http`/`camel-grpc`/`camel-ws`) y/o un middleware de bearer-token
   (`axum::middleware::from_fn`). Ninguno se activa por defecto. Un operador que corre en
   una red no confiable los habilita explícitamente.

3. **Bind a loopback preferido.** El default de bind debe favorecer `127.0.0.1`. Un bind
   a una interfaz no-loopback (`0.0.0.0`) es una decisión explícita del operador y DEBE
   emitir un `warn!` en el arranque señalando que el endpoint queda alcanzable desde
   todas las interfaces sin capa de aplicación que lo restrinja.

4. **La metadata de diagnóstico no lleva bytes de credencial.** Por ADR-0051, cuerpos y
   labels de métricas y de health nunca filtran secretos. Este ADR no relaja esa regla:
   la exposición no autenticada del endpoint es aceptable **precisamente porque** su
   contenido es metadata operacional, no material de credencial.

### Alcance

Esta postura vincula a los crates de servicio que exponen endpoints de diagnóstico
(`camel-prometheus`, `camel-health`). **No** aplica a componentes de plano de datos
(`camel-http`, `camel-grpc`, `camel-ws`), que son inbound de negocio y SÍ montan TLS con
hot-reload de certificado (ver CONTEXT-MAP "TLS cert hot-reload"). La distinción es
deliberada: el plano de datos porta payload de negocio y cruza la frontera de confianza;
el plano de diagnóstico porta metadata operacional y no la cruza.

## Contexto

`camel-prometheus` construye su router axum sin ninguna capa de auth ni TLS, con
default de host `0.0.0.0:9090` (`crates/camel-config/src/config.rs`
`default_prometheus_host`). `camel-health` comparte esa superficie: su `health_router`
monta `/healthz`, `/readyz`, `/startupz`, `/health` sin auth, sin TLS, sin middleware.
La ruta config-driven exige `enabled = true` (default `false`), pero la ruta
programática (`PrometheusService::new`, como muestra el README Quick Start) no hereda ese
guard.

Ningún ADR previo gobierna **cómo** se exponen los endpoints de diagnóstico. Antes de
congelar v1.0 necesitamos una decisión registrada para no shippear una superficie de
información sin postura declarada. La convención Prometheus (no autenticado,
network-isolation propiedad del operador) es legítima y ampliamente adoptada, pero
legítima no es lo mismo que documentada: sin este ADR, un revisor no puede distinguir
"exposición no autenticada por diseño" de "olvido de autenticar".

## Opciones consideradas

### Autenticación a nivel de aplicación por defecto

Rechazada. Rompe el modelo de scrape de Prometheus: los scrapers estándar
(Prometheus server, agentes) esperan `/metrics` no autenticado o con un esquema de auth
configurado del lado del scraper, no impuesto por el target. Forzar auth por defecto
crea fricción operacional sin beneficio de seguridad real cuando el aislamiento de red ya
está presente.

### TLS obligatorio en los endpoints de diagnóstico

Rechazada. Impone overhead de terminación TLS y gestión de certificado a despliegues
single-node y de desarrollo, donde el endpoint está detrás de loopback o de una malla de
servicio que ya termina TLS. El caso de red no confiable se cubre con el hook opt-in
(regla 2), no con un mandato global.

### Postura documentada con hooks opt-in (elegida)

Elegida. Registra la decisión (no autenticado por convención de scrape), provee los
puntos de extensión para los despliegues que sí necesitan TLS/auth, y prefiere el bind a
loopback con warning en el opt-out. Hace la postura legible en revisión y deja al operador
la elección por despliegue, sin re-arquitectura.

## Consecuencias

- Los endpoints de diagnóstico de `camel-prometheus` y `camel-health` documentan su
  postura no autenticada como decisión registrada, no como omisión.
- El default de bind debe moverse hacia `127.0.0.1`; el bind a no-loopback requiere
  opt-in explícito y emite `warn!` de arranque (trabajo de código, stream de corrección,
  bd `rc-asm9`).
- Los crates que expongan endpoints de diagnóstico en el futuro heredan esta postura por
  defecto y declaran cualquier hook de TLS/auth que provean.
- La distinción diagnóstico-vs-plano-de-datos queda fijada: el plano de datos monta TLS
  con hot-reload (componentes inbound); el plano de diagnóstico no autentica por
  convención y ofrece TLS opcional.
- La regla de redacción de ADR-0051 sigue en pie: la exposición no autenticada es
  aceptable solo mientras el contenido sea metadata operacional sin bytes de credencial.

## Registro de self-grill

**Preguntas generadas:**

1. [glossary] ¿"endpoint de diagnóstico" colisiona con el "co-hosting HTTP" de ADR-0009 o
   con la frontera de confianza de ADR-0032?
2. [sharpen] ¿"no autenticado por defecto" contradice ADR-0033 (defaults fail-closed)?
3. [scenario] Si un operador hace bind a `0.0.0.0` en una red no confiable, ¿qué lo
   protege bajo esta postura?
4. [cross-ref] ¿Algún ADR existente ya cubre la exposición de endpoints de diagnóstico,
   de modo que esto debería ser amendment y no ADR nuevo?

**Respuestas:**

1. [glossary] No colisiona. ADR-0009 gobierna el plano de datos (rutas API `http:` +
   mounts estáticos `http-static:` que portan payload de negocio y precedencia de
   dispatch). ADR-0032 gobierna datos de exchange no confiables cruzando a decisiones de
   control/recurso. Un endpoint de diagnóstico no procesa payload de negocio ni datos de
   exchange: expone metadata operacional read-only. Es una tercera categoría distinta.
2. [sharpen] No contradice. ADR-0033 hace fail-closed en las elecciones de seguridad que
   el operador DEBE declarar explícitamente (query dinámica SQL, capacidad WASM por
   mundo, TLS gRPC). La exposición no autenticada de metadata operacional no es una de
   esas elecciones: la protección canónica del modelo de scrape es la red, no la auth de
   aplicación. Lo que este ADR sí adopta del espíritu de ADR-0033 es el bind-loopback
   preferido con warning explícito en el opt-out a no-loopback: el operador elige exponer
   más ampliamente de forma visible.
3. [scenario] Bajo esta postura lo protege: (a) el default preferido de bind a loopback,
   que exige opt-in explícito para `0.0.0.0`; (b) el `warn!` de arranque que señala la
   exposición ampliada; (c) el hook opt-in de TLS/bearer-token que el operador habilita
   para ese caso. La postura no autentica por defecto, pero da los mecanismos y la
   señal para el despliegue en red no confiable. La red (NetworkPolicy/firewall) sigue
   siendo la defensa primaria por convención de scrape.
4. [cross-ref] Ninguno cubre esto. ADR-0009 es plano de datos (rutas API + estáticos).
   ADR-0033 es validación de arranque de opt-ins de config, no exposición de superficie de
   diagnóstico. ADR-0051 es redacción de credenciales en representación, y afirma
   explícitamente que las métricas no portan credenciales. La decisión es genuinamente
   nueva: irreversible (v1.0 shippea la superficie), sorprendente (un endpoint no
   autenticado en un framework de seguridad merece registro) y con trade-off real
   (modelo de scrape vs auth de aplicación). Es ADR nuevo, no amendment.

**Outcome:** approve como ADR nuevo (0052). Postura no autenticada por convención de
scrape, TLS/auth como hooks opt-in, bind-loopback preferido con warning en opt-out.
Ejecución de código (bind default, warning, hooks) delegada al stream de corrección
(bd `rc-asm9`).
**Self-grill mode:** manual (4 principios L6: consistencia con CONTEXT-MAP, conflicto con
ADRs existentes, redundancia con ADRs implícitos, numeración correcta — 0052 siguiente
libre tras 0051).
