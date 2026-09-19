package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.quarkus.logging.json.runtime.JsonFormatter;
import java.util.logging.Level;
import java.util.logging.LogRecord;
import org.junit.jupiter.api.Test;

/**
 * Plain JUnit check that the quarkus-logging-json formatter (wired in by
 * quarkus.log.console.json.enabled=true) emits a single line of JSON per record, carrying the
 * message text, the level, and the logger name.
 *
 * <p>Uses the formatter's public no-arg constructor so no Quarkus boot or config machinery is
 * required.
 */
class JsonConsoleLoggingTest {

  @Test
  void formatterEmitsSingleLineJson() {
    var formatter = new JsonFormatter();
    var record = new LogRecord(Level.INFO, "bridge ready for duty");
    record.setLoggerName("org.rustcamel.jms.PortAnnouncer");

    var line = formatter.format(record).strip();

    // Single line, brace-delimited JSON object.
    assertEquals(1, line.lines().count(), "expected exactly one line, got: " + line);
    assertTrue(line.startsWith("{"), "not a JSON object: " + line);
    assertTrue(line.endsWith("}"), "not a JSON object: " + line);

    // JSON-parse via key regex: message text and level present as key:value pairs.
    assertTrue(
        line.matches("(?s).*\"message\"\\s*:\\s*\"bridge ready for duty\".*"),
        "message text missing from JSON: " + line);
    assertTrue(
        line.matches("(?s).*\"level\"\\s*:\\s*\"INFO\".*"), "level missing from JSON: " + line);
    assertTrue(line.contains("org.rustcamel.jms.PortAnnouncer"), "logger name missing: " + line);
    assertTrue(line.contains("\"timestamp\""), "timestamp missing: " + line);
  }
}
