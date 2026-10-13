package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.BufferedReader;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Cross-language parity test for the outbound JMS property denylist (jms-bridge-080 task 1.4). It
 * reads the one shared corpus resource {@code /header-policy-corpus.tsv} — the same file the Rust
 * {@code rust_header_policy_matches_shared_corpus} test embeds with {@code include_str!} — and
 * asserts that {@link HeaderPolicy#isDenied(String)} and {@link HeaderPolicy#isAllowed(List,
 * String)} agree with every row. The corpus is never copied: both sides read the identical file, so
 * a change to either classifier that is not reflected in the corpus fails one side.
 */
@DisplayName("HeaderPolicy cross-language parity corpus")
class HeaderPolicyParityTest {

  private record Row(String name, boolean denied) {}

  private static List<Row> loadCorpus() throws Exception {
    InputStream in = HeaderPolicyParityTest.class.getResourceAsStream("/header-policy-corpus.tsv");
    assertNotNull(in, "shared header-policy corpus must be on the test classpath");
    List<Row> rows = new ArrayList<>();
    try (BufferedReader reader =
        new BufferedReader(new InputStreamReader(in, StandardCharsets.UTF_8))) {
      String line;
      while ((line = reader.readLine()) != null) {
        if (line.isBlank()) {
          continue;
        }
        String row = line;
        String[] parts = row.split("\t", -1);
        assertEquals(2, parts.length, () -> "corpus row is not tab-separated: " + row);
        boolean denied =
            switch (parts[1]) {
              case "denied" -> true;
              case "allowed" -> false;
              default ->
                  throw new AssertionError("corpus decision must be denied|allowed: " + parts[1]);
            };
        rows.add(new Row(parts[0], denied));
      }
    }
    return rows;
  }

  @Test
  void header_policy_matches_shared_corpus() throws Exception {
    List<Row> rows = loadCorpus();
    assertFalse(rows.isEmpty(), "shared corpus must not be empty");

    List<String> allowlist = rows.stream().filter(row -> !row.denied()).map(Row::name).toList();

    for (Row row : rows) {
      assertEquals(
          row.denied(),
          HeaderPolicy.isDenied(row.name()),
          () -> "denylist mismatch for: " + row.name());
      assertEquals(
          !row.denied(),
          HeaderPolicy.isAllowed(allowlist, row.name()),
          () -> "allowlist mismatch for: " + row.name());
      assertFalse(
          HeaderPolicy.isAllowed(List.of(), row.name()),
          () -> "an empty allowlist must allow nothing: " + row.name());
    }

    // A listed business name passes; an unlisted but non-denied business name does not.
    assertTrue(HeaderPolicy.isAllowed(List.of("customerId"), "customerId"));
    assertFalse(HeaderPolicy.isAllowed(List.of("customerId"), "region"));
  }
}
