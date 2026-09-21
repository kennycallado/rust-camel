package org.rustcamel.xmlbridge;

import io.quarkus.runtime.StartupEvent;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import java.util.logging.Logger;
import org.eclipse.microprofile.config.Config;
import org.eclipse.microprofile.config.spi.ConfigSource;

/**
 * Fail-closed guard for the bridge's OTLP egress surface.
 *
 * <p>The OpenTelemetry extension ships its OTLP exporter enabled at build time, and when the SDK is
 * on and no traces endpoint is configured, the config mapping materializes a built-in {@code
 * http://localhost:4317} default — a value no {@link ConfigSource} provides, so it is
 * indistinguishable from operator config through the property API alone. The repo's egress doctrine
 * (fail-closed allowlist; see the Rust-side {@code FunctionConfig.egress_allowlist} precedent,
 * 69a7f143) has no room for dial-out defaults: whether a collector is trusted is operator config,
 * not code.
 *
 * <p>Invariant enforced at startup: SDK enabled implies the traces endpoint is present in an actual
 * config source (env var, system property, or a deployment-provided file). Operators opt in with
 * BOTH runtime properties: {@code QUARKUS_OTEL_SDK_DISABLED=false} and {@code
 * QUARKUS_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://<collector>:4317}. Enabling the SDK without an
 * endpoint aborts startup instead of dialing the built-in fallback.
 *
 * <p>Twin copies of this guard live in bridges/jms, bridges/xml, and bridges/cxf (independent
 * gradle builds, repo convention): keep the invariant in lockstep when any copy changes.
 */
@ApplicationScoped
public class OtlpEgressGuard {

  static final String SDK_DISABLED_KEY = "quarkus.otel.sdk.disabled";
  static final String TRACES_ENDPOINT_KEY = "quarkus.otel.exporter.otlp.traces.endpoint";
  static final String TRACES_ENDPOINT_ENV = "QUARKUS_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT";

  /** Annotation-level defaults are code, not operator config: never count as explicit. */
  static final String DEFAULT_VALUES_SOURCE = "DefaultValuesConfigSource";

  private static final Logger LOG = Logger.getLogger(OtlpEgressGuard.class.getName());

  private final Config config;

  OtlpEgressGuard(Config config) {
    this.config = config;
  }

  void enforceOnStartup(@Observes StartupEvent event) {
    boolean sdkDisabled = config.getOptionalValue(SDK_DISABLED_KEY, Boolean.class).orElse(false);
    if (sdkDisabled) {
      LOG.info("otel SDK disabled: OTLP egress surface closed");
      return;
    }
    if (!explicitlyConfigured(config, TRACES_ENDPOINT_KEY)) {
      throw new IllegalStateException(
          "fail-closed OTLP egress: quarkus.otel.sdk.disabled resolved to false but "
              + TRACES_ENDPOINT_KEY
              + " is not set in any config source. The OTLP exporter must not fall back to its"
              + " built-in http://localhost:4317 default. Set "
              + TRACES_ENDPOINT_ENV
              + " to the trusted collector, or leave the SDK disabled.");
    }
    LOG.info("otel SDK enabled with explicit OTLP traces endpoint: egress surface open");
  }

  /**
   * True when an actual {@link ConfigSource} (env var, system property, or config file) provides
   * the key. The extension's built-in endpoint default never passes this check: it is injected by
   * the config mapping, visible to {@code getOptionalValue} while no source owns it.
   *
   * <p>Names are read through {@link ConfigSource#getPropertyNames()}, not {@code getProperties()}:
   * env sources key their property map by raw env-var names ({@code QUARKUS_...}) and expose the
   * dotted aliases only through {@code getPropertyNames()} plus {@link
   * ConfigSource#getValue(String)}.
   *
   * <p>Supported opt-in spellings: the plain unqualified key in any config file, a system property,
   * or the {@code QUARKUS_OTEL_EXPORTER_OTLP_TRACES_ENDPOINT} env var. Profile-qualified keys
   * ({@code %prod.…}) and relocated legacy spellings resolve only through property lookup, not
   * through {@code getPropertyNames()}: the guard does not recognize them and fails closed. An
   * explicitly configured empty value passes this check and is left to the extension's own endpoint
   * validation. The guard's tests live in all three bridges (twin-copy convention).
   */
  static boolean explicitlyConfigured(Config config, String key) {
    for (ConfigSource source : config.getConfigSources()) {
      if (DEFAULT_VALUES_SOURCE.equals(source.getName())) {
        continue;
      }
      if (source.getPropertyNames().contains(key) && source.getValue(key) != null) {
        return true;
      }
    }
    return false;
  }
}
