package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import jakarta.json.Json;
import jakarta.json.JsonObject;
import java.io.InputStream;
import java.util.List;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Guards the native-image reflection registration for the ActiveMQ Classic connection factory
 * (jms-bridge-080 task 6.1). ActiveMQ Classic resolves {@code
 * org.apache.activemq.ActiveMQConnectionFactory} and applies the broker-URL {@code jms.*} options
 * (including {@code jms.connectionIDPrefix}) through public-method invocation, so the entry must be
 * invocation-capable ({@code allPublicMethods}) and class-level. A query-only or package-level
 * registration would break the {@code jms.*} option proof in native images, so this test fails if
 * either is introduced.
 *
 * <p>The entry must live under the {@code <groupId>/<artifactId>} namespace ({@code
 * org.rustcamel/jms-bridge/}) because Quarkus 3.39.1 generates its own flat {@code
 * META-INF/native-image/reflect-config.json} at packaging, which shadows any hand-written file at
 * the same flat path (see {@code 356-rc-odr5c-diagnosis.md}). This test therefore also asserts the
 * factory entry is absent from the flat path.
 */
@DisplayName("Native reflection config registers ActiveMQConnectionFactory")
class NativeReflectionConfigTest {

  private static final String REFLECT_CONFIG =
      "/META-INF/native-image/org.rustcamel/jms-bridge/reflect-config.json";
  private static final String FLAT_REFLECT_CONFIG = "/META-INF/native-image/reflect-config.json";
  private static final String FACTORY_CLASS = "org.apache.activemq.ActiveMQConnectionFactory";
  private static final String ACTIVEMQ_PACKAGE = "org.apache.activemq";

  @Test
  void native_reflection_registers_activemq_factory() throws Exception {
    List<JsonObject> entries = loadReflectConfig(REFLECT_CONFIG);

    List<JsonObject> factoryEntries =
        entries.stream()
            .filter(entry -> FACTORY_CLASS.equals(entry.getString("name", "")))
            .toList();
    assertEquals(
        1,
        factoryEntries.size(),
        "exactly one class-level " + FACTORY_CLASS + " entry must exist (no silent duplicate)");

    JsonObject factory = factoryEntries.get(0);
    assertTrue(
        factory.getBoolean("allPublicMethods", false),
        FACTORY_CLASS + " must be invocation-capable: allPublicMethods must be true");

    List<String> packageLevelNames =
        entries.stream()
            .map(entry -> entry.getString("name", ""))
            .filter(NativeReflectionConfigTest::isActiveMqPackageLevel)
            .toList();
    assertTrue(
        packageLevelNames.isEmpty(),
        () -> "no package-level ActiveMQ entry may be added, found: " + packageLevelNames);

    List<JsonObject> flatFactoryEntries =
        loadOptionalReflectConfig(FLAT_REFLECT_CONFIG).stream()
            .filter(entry -> FACTORY_CLASS.equals(entry.getString("name", "")))
            .toList();
    assertTrue(
        flatFactoryEntries.isEmpty(),
        () ->
            "the "
                + FACTORY_CLASS
                + " entry must not remain at the flat "
                + FLAT_REFLECT_CONFIG
                + " path, which Quarkus shadows at packaging, found: "
                + flatFactoryEntries);
  }

  private static List<JsonObject> loadReflectConfig(String resource) throws Exception {
    try (InputStream in = NativeReflectionConfigTest.class.getResourceAsStream(resource)) {
      assertNotNull(in, resource + " must be on the test classpath");
      return Json.createReader(in).readArray().getValuesAs(JsonObject.class);
    }
  }

  /** Reads an optional config resource; returns an empty list when the resource is absent. */
  private static List<JsonObject> loadOptionalReflectConfig(String resource) throws Exception {
    try (InputStream in = NativeReflectionConfigTest.class.getResourceAsStream(resource)) {
      if (in == null) {
        return List.of();
      }
      return Json.createReader(in).readArray().getValuesAs(JsonObject.class);
    }
  }

  /**
   * A package-level ActiveMQ entry names the package (or a wildcard over it) instead of a concrete
   * class; such an entry would register every Classic class and is forbidden (task 6.1 step 2).
   */
  private static boolean isActiveMqPackageLevel(String name) {
    return name.equals(ACTIVEMQ_PACKAGE)
        || name.equals(ACTIVEMQ_PACKAGE + ".*")
        || name.equals(ACTIVEMQ_PACKAGE + ".*.*");
  }
}
