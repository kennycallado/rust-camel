package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class XsdValidatorServiceCauseTest {

  @Test
  void nullOuterMessageFallsBackToCauseMessage() {
    var wrapped = new RuntimeException(null, new IllegalStateException("real xerces message"));
    assertEquals("real xerces message", XsdValidatorService.withCauses(wrapped));
  }

  @Test
  void distinctCauseMessagesAreJoinedAndDuplicatesDropped() {
    var inner = new IllegalArgumentException("pattern '[x' is invalid");
    var outer = new IllegalStateException("compile failed: pattern '[x' is invalid", inner);
    assertEquals("compile failed: pattern '[x' is invalid", XsdValidatorService.withCauses(outer));

    var generic = new IllegalStateException("compile failed", inner);
    assertEquals(
        "compile failed | pattern '[x' is invalid", XsdValidatorService.withCauses(generic));
  }

  @Test
  void messagelessChainReportsExceptionType() {
    assertEquals("RuntimeException", XsdValidatorService.withCauses(new RuntimeException()));
  }
}
