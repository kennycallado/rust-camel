package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.Map;
import org.junit.jupiter.api.Test;

/**
 * Permanent startup-failure signalling (task 4.1). A set-but-invalid journal value — directory,
 * broker key, or one of the four numeric bounds — must throw before readiness, and the protocol
 * line must escape its reason safely.
 *
 * <p>Pure seams only: the existing {@code Map}-backed {@link PortAnnouncer#validateJournalStartup}
 * and the static line builder. No {@code System.setenv} and no {@code System.exit} in the test JVM.
 */
public class PortAnnouncerPermanentFailureTest {

  @Test
  void out_of_range_bound_is_startup_permanent_before_readiness() {
    JournalPermanentException thrown =
        assertThrows(
            JournalPermanentException.class,
            () ->
                PortAnnouncer.validateJournalStartup(
                    Map.of(BridgeConfig.JOURNAL_MAX_BYTES_ENV, "999999999999")),
            "an out-of-range byte ceiling must fail before readiness");
    assertTrue(
        thrown.getMessage().contains(BridgeConfig.JOURNAL_MAX_BYTES_ENV), thrown.getMessage());
  }

  @Test
  void non_numeric_bound_is_startup_permanent() {
    assertThrows(
        JournalPermanentException.class,
        () ->
            PortAnnouncer.validateJournalStartup(
                Map.of(BridgeConfig.MAX_DELIVERY_ATTEMPTS_ENV, "many")),
        "a non-numeric bound must fail before readiness");
  }

  @Test
  void permanent_failure_line_escapes_reason() {
    String line = PortAnnouncer.permanentFailureLine("bad \"dir\" \\ path\nnext");
    assertTrue(line.startsWith("{\"status\":\"permanent-failure\",\"reason\":\""), line);
    assertTrue(line.endsWith("\"}"), line);
    assertTrue(
        line.contains("bad \\\"dir\\\" \\\\ path\\nnext"),
        "the reason must be JSON-escaped: " + line);
    assertFalse(line.contains("\n"), "the signal must stay one physical line: " + line);
  }
}
