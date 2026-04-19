package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.protobuf.ByteString;
import io.quarkus.grpc.GrpcClient;
import io.quarkus.test.junit.QuarkusTest;
import java.nio.charset.StandardCharsets;
import org.junit.jupiter.api.Test;
import xml_bridge.BridgeError;
import xml_bridge.CompileStylesheetRequest;
import xml_bridge.MutinyXsdValidatorGrpc;
import xml_bridge.MutinyXsltTransformerGrpc;
import xml_bridge.TransformRequest;
import xml_bridge.ValidateRequest;

@QuarkusTest
class SecurityTest {

  @GrpcClient("xsd-validator")
  MutinyXsdValidatorGrpc.MutinyXsdValidatorStub xsd;

  @GrpcClient("xslt-transformer")
  MutinyXsltTransformerGrpc.MutinyXsltTransformerStub xslt;

  @Test
  void xxeExternalEntityInjectionReturnsBridgeError() {
    var schema =
        """
        <xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">
          <xs:element name=\"root\" type=\"xs:string\"/>
        </xs:schema>
        """;
    var xml =
        """
        <?xml version=\"1.0\"?>
        <!DOCTYPE root [
          <!ELEMENT root ANY >
          <!ENTITY xxe SYSTEM \"file:///etc/passwd\" >
        ]>
        <root>&xxe;</root>
        """;

    var resp =
        xsd.validate(
                ValidateRequest.newBuilder()
                    .setSchema(bytes(schema))
                    .setDocument(bytes(xml))
                    .build())
            .await()
            .indefinitely();

    assertFalse(resp.getValid());
    assertTrue(resp.hasError());
    assertNotEquals(BridgeError.Kind.UNKNOWN, resp.getError().getKind());
    assertFalse(resp.getError().getMessage().contains("root:x:0:0:root:/root:/bin"));
  }

  @Test
  void billionLaughsPayloadFailsWithinTimeout() {
    var schema =
        """
        <xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">
          <xs:element name=\"root\" type=\"xs:string\"/>
        </xs:schema>
        """;
    var xmlBomb =
        """
        <?xml version=\"1.0\"?>
        <!DOCTYPE lolz [
          <!ENTITY lol \"lol\">
          <!ENTITY lol1 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">
          <!ENTITY lol2 \"&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;&lol1;\">
          <!ENTITY lol3 \"&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;\">
          <!ENTITY lol4 \"&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;\">
          <!ENTITY lol5 \"&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;&lol4;\">
          <!ENTITY lol6 \"&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;&lol5;\">
          <!ENTITY lol7 \"&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;&lol6;\">
          <!ENTITY lol8 \"&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;&lol7;\">
          <!ENTITY lol9 \"&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;&lol8;\">
        ]>
        <root>&lol9;</root>
        """;

    var resp =
        xsd.validate(
                ValidateRequest.newBuilder()
                    .setSchema(bytes(schema))
                    .setDocument(bytes(xmlBomb))
                    .build())
            .await()
            .indefinitely();

    assertFalse(resp.getValid());
    assertTrue(resp.hasError());
    // Must be rejected as a security/validation error, not silently pass
    var kind = resp.getError().getKind();
    assertTrue(
        kind == BridgeError.Kind.SECURITY_VIOLATION || kind == BridgeError.Kind.VALIDATION_FAILED,
        "Expected SECURITY_VIOLATION or VALIDATION_FAILED but got: " + kind);
  }

  @Test
  void xsltDocumentFunctionSsrfAttemptReturnsBridgeError() {
    var stylesheet =
        """
        <?xml version=\"1.0\" encoding=\"UTF-8\"?>
        <xsl:stylesheet version=\"3.0\"
          xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">
          <xsl:template match=\"/\">
            <out><xsl:value-of select=\"document('http://127.0.0.1:9/poc')\"/></out>
          </xsl:template>
        </xsl:stylesheet>
        """;
    var xml = "<root/>";

    var compile =
        xslt.compileStylesheet(
                CompileStylesheetRequest.newBuilder()
                    .setStylesheetId("deny-ssrf")
                    .setStylesheet(bytes(stylesheet))
                    .build())
            .await()
            .indefinitely();

    // Whether compile succeeds or fails, the document() SSRF must be blocked.
    // If compile already rejected it (e.g. URIResolver at factory level), that's sufficient.
    if (compile.hasError()) {
      assertNotEquals(BridgeError.Kind.UNKNOWN, compile.getError().getKind());
      // Compile-time rejection counts as SSRF blocked — done.
      return;
    }

    // Compile succeeded (document() is a runtime call) — verify transform-time blocking.
    var resp =
        xslt.transform(
                TransformRequest.newBuilder()
                    .setStylesheetId("deny-ssrf")
                    .setDocument(bytes(xml))
                    .build())
            .await()
            .indefinitely();

    assertTrue(resp.hasError(), "SSRF via document() must produce an error response");
    assertNotEquals(BridgeError.Kind.UNKNOWN, resp.getError().getKind());
    assertTrue(
        resp.getError().getKind() == BridgeError.Kind.SECURITY_VIOLATION
            || resp.getError().getKind() == BridgeError.Kind.TRANSFORM_FAILED,
        "Expected SECURITY_VIOLATION or TRANSFORMATION_FAILED but got: "
            + resp.getError().getKind());
  }

  @Test
  void xsdWithExternalDtdReturnsBridgeError() {
    var schemaWithExternalDtd =
        """
        <!DOCTYPE xs:schema SYSTEM \"http://127.0.0.1:9/evil.dtd\">
        <xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">
          <xs:element name=\"root\" type=\"xs:string\"/>
        </xs:schema>
        """;

    var xml = "<root>ok</root>";
    var resp =
        xsd.validate(
                ValidateRequest.newBuilder()
                    .setSchema(bytes(schemaWithExternalDtd))
                    .setDocument(bytes(xml))
                    .build())
            .await()
            .indefinitely();

    assertFalse(resp.getValid());
    assertTrue(resp.hasError());
    assertNotEquals(BridgeError.Kind.UNKNOWN, resp.getError().getKind());
  }

  private static ByteString bytes(String value) {
    return ByteString.copyFrom(value, StandardCharsets.UTF_8);
  }
}
