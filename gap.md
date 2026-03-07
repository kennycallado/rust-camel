# Análisis de Gaps - rust-camel (Confirmado)

Este documento detalla los gaps arquitectónicos confirmados tras la revisión técnica del código fuente (versión `0.2.1`).

## 1. Sistema de "Type Converters" (CONFIRMADO)
*   **Evidencia en código:** 
    *   En `camel-http/src/lib.rs`, el `HttpProducer` implementa su propio `body_to_bytes` (líneas 758-763).
    *   En `camel-http/src/lib.rs`, el `HttpConsumer` realiza una conversión ad-hoc de `Bytes` a `Text` intentando un `str::from_utf8` (líneas 430-435).
*   **Gap:** Cada componente reinventa la lógica de conversión. Si un componente nuevo espera `Json` pero recibe `Text` (que es un JSON válido), fallará porque no hay un motor de conversión centralizado que medie entre los pasos del pipeline.
*   **Impacto:** Redundancia de código y fragilidad en la interoperabilidad entre componentes de terceros.

## 2. Integración con "Beans" / Registry limitado (CONFIRMADO)
*   **Evidencia en código:** 
    *   `camel-core/src/registry.rs` define `Registry` estrictamente como un `HashMap<String, Box<dyn Component>>`.
*   **Gap:** No existe un mecanismo para registrar objetos arbitrarios (`Send + Sync + Any`) que puedan ser recuperados por nombre.
*   **Impacto:** Imposibilidad de inyectar lógica de negocio o configuraciones complejas desde el DSL de YAML. Las rutas están limitadas a lo que el framework proporciona "out-of-the-box" o requiere closures pesados en el Builder de Rust.

## 3. Streaming de Datos - Body as Stream (RESUELTO)
*   **Estado:** Implementado en v0.2.1.
*   **Solución:** 
    *   `Body::Stream` introducido en `camel-api` usando `BoxStream`.
    *   `camel-file` emite streams perezosos (lazy reading).
    *   `camel-http` optimizado para piping de streams a `reqwest`.
    *   Materialización segura con límites de memoria (100MB) para evitar OOM.
*   **Impacto:** Riesgo de **Out of Memory (OOM)** eliminado para flujos lineales de archivos grandes y tráfico de red. Soporte para archivos de varios GB.

## 4. Granularidad en el Manejo de Errores (MATIZADO)
*   **Evidencia en código:** 
    *   `camel-api/src/error_handler.rs` usa `Arc<dyn Fn(&CamelError) -> bool>` para el matching de excepciones.
*   **Gap:** La infraestructura en Rust es excelente y flexible. Sin embargo, existe un gap de **expresividad en el DSL**. Representar lógica de matching compleja (ej. "reintentar solo si el status code de HTTP es 5xx") en YAML es actualmente imposible sin pre-definir predicados.
*   **Impacto:** El DSL de YAML es menos potente que la API de Rust para gestionar errores complejos.

## 5. Introspección y Parsing de Endpoints (CONFIRMADO)
*   **Evidencia en código:** 
    *   `camel-endpoint/src/uri.rs` solo provee un `HashMap<String, String>` plano.
    *   `HttpConfig::from_uri` y `LogConfig::from_uri` contienen bloques extensos de parsing manual (`.get().and_then(|v| v.parse().ok())`).
*   **Gap:** Falta un sistema declarativo (basado en `serde`) para mapear URIs a estructuras de configuración.
*   **Impacto:** Alto *boilerplate* al crear componentes y mayor probabilidad de inconsistencias o errores de parsing de parámetros (ej. timeouts, booleanos).

## 6. Unidad de Trabajo (Unit of Work) (CONFIRMADO)
*   **Evidencia en código:** 
    *   El `Exchange` fluye por el pipeline de Tower sin un gestor de contexto superior que rastree su ciclo de vida completo.
*   **Gap:** No hay hooks para `on_complete` o `on_failure` a nivel de intercambio global.
*   **Impacto:** Dificulta la implementación de transacciones (Sagas) o la limpieza de recursos temporales creados durante el flujo.

## 7. Gestión de In-flight Exchanges en Hot-Reload (CONFIRMADO)
*   **Evidencia en código:** 
    *   `camel-core/src/reload.rs` usa `ArcSwap` para cambiar el pipeline instantáneamente.
*   **Gap:** El swap es "atómico" pero ciego. No espera a que los mensajes que están *dentro* del pipeline viejo terminen antes de descartarlo.
*   **Impacto:** Posible pérdida de datos o estados inconsistentes durante la recarga de rutas en caliente bajo carga alta.
