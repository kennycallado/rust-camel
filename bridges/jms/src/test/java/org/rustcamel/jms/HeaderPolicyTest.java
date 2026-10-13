package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Unit tests for {@link HeaderPolicy}, the Java backstop for the outbound JMS property denylist.
 * The table covers reserved JMS names, framework namespaces, credential variants, and business
 * names. A cross-language parity corpus is added in task 1.4.
 */
@DisplayName("HeaderPolicy outbound header denylist")
class HeaderPolicyTest {

  @Test
  void header_policy_denies_reserved_framework_and_credentials() {
    List<String> denied =
        List.of(
            // Reserved JMS provider-control namespace (JMS, JMSX, JMS_ all normalize to "jms").
            "JMSMessageID",
            "JMSXDeliveryCount",
            "JMS_Destination",
            "jms-correlation-id",
            // Framework namespaces camel and xcamel.
            "CamelStop",
            "camelContextId",
            "x-camel-leader-epoch",
            "xcamelId",
            // Credential substrings, including case and separator variants.
            "password",
            "Password",
            "passwd",
            "secret",
            "credential",
            "token",
            "username",
            "user",
            "api-key",
            "API_KEY",
            "api_key",
            "Authorization",
            "bearerToken",
            "principal");
    for (String name : denied) {
      assertTrue(
          HeaderPolicy.isDenied(name), () -> "must deny reserved/framework/credential: " + name);
    }
  }

  @Test
  void header_policy_allows_only_listed_business_names() {
    List<String> allowlist = List.of("customerId");
    assertTrue(HeaderPolicy.isAllowed(allowlist, "customerId"), "listed business name must pass");
    assertTrue(
        HeaderPolicy.isAllowed(allowlist, "CUSTOMERID"), "case variant of a listed name must pass");
    assertFalse(
        HeaderPolicy.isAllowed(allowlist, "region"), "unlisted business name must not pass");
    assertFalse(
        HeaderPolicy.isAllowed(List.of("JMSMessageID"), "JMSMessageID"),
        "a denied name must not pass even when explicitly listed");
  }
}
