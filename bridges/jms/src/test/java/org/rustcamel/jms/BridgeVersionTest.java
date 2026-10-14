package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Guards the Gradle default version fallback (jms-bridge-080 task 6.3). The Rust crate pins the
 * same wire version in {@code camel-jms::BRIDGE_VERSION}; a bump must change both together. This
 * test reads the fallback from the build source so a stale default cannot ship silently.
 *
 * <p>The default is the value used when the {@code version} Gradle property is absent, i.e. the
 * {@code ?:} branch of the {@code findProperty("version")} expression. A focused test is used
 * because no runtime class exposes the fallback.
 */
@DisplayName("Gradle default bridge version is 0.8.1")
class BridgeVersionTest {

  private static final Pattern FALLBACK = Pattern.compile("\\?:\\s*\"([^\"]+)\"");

  @Test
  void gradle_default_is_0_8_1() throws Exception {
    Path buildGradle = locateBuildGradle();
    String line = findVersionFallbackLine(buildGradle);

    Matcher matcher = FALLBACK.matcher(line);
    assertTrue(matcher.find(), () -> "version fallback string missing in: " + line.trim());

    assertEquals(
        "0.8.1",
        matcher.group(1),
        () ->
            "Gradle default fallback must be 0.8.1 to match crate BRIDGE_VERSION; got "
                + matcher.group(1));
  }

  private static Path locateBuildGradle() {
    Path dir = Paths.get(System.getProperty("user.dir")).toAbsolutePath();
    for (int i = 0; i < 4 && dir != null; i++) {
      Path candidate = dir.resolve("build.gradle.kts");
      if (Files.isRegularFile(candidate)) {
        return candidate;
      }
      dir = dir.getParent();
    }
    throw new IllegalStateException(
        "build.gradle.kts not found from " + System.getProperty("user.dir"));
  }

  private static String findVersionFallbackLine(Path buildGradle) throws Exception {
    return Files.readAllLines(buildGradle).stream()
        .filter(line -> line.contains("findProperty(\"version\")"))
        .findFirst()
        .orElseThrow(() -> new AssertionError("no version fallback line in " + buildGradle));
  }
}
