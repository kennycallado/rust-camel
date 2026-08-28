package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.protobuf.ByteString;
import io.quarkus.grpc.GrpcClient;
import io.quarkus.test.junit.QuarkusTest;
import java.nio.charset.StandardCharsets;
import org.junit.jupiter.api.Test;
import xml_bridge.BridgeError;
import xml_bridge.MutinyXsdValidatorGrpc;
import xml_bridge.ValidateRequest;

@QuarkusTest
class XsdValidationIntegrationTest {

  @GrpcClient("xsd-validator")
  MutinyXsdValidatorGrpc.MutinyXsdValidatorStub xsd;

  @Test
  void validXmlWithValidXsdReturnsSuccess() {
    var schema =
        """
        <xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">
          <xs:element name=\"note\">
            <xs:complexType>
              <xs:sequence>
                <xs:element name=\"to\" type=\"xs:string\"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>
        """;
    var xml = "<note><to>Alice</to></note>";

    var resp =
        xsd.validate(
                ValidateRequest.newBuilder()
                    .setSchema(bytes(schema))
                    .setDocument(bytes(xml))
                    .build())
            .await()
            .indefinitely();

    assertTrue(resp.getValid());
    assertFalse(resp.hasError());
    assertEquals(0, resp.getErrorsCount());
  }

  @Test
  void invalidXmlWithValidXsdReturnsValidationError() {
    var schema =
        """
        <xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">
          <xs:element name=\"note\">
            <xs:complexType>
              <xs:sequence>
                <xs:element name=\"to\" type=\"xs:string\"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>
        """;
    var xml = "<note><from>Bob</from></note>";

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
    assertEquals(BridgeError.Kind.VALIDATION_FAILED, resp.getError().getKind());
    assertTrue(resp.getError().getMessage().contains("VALIDATION_ERROR"));
  }

  @Test
  void validXmlWithMalformedXsdReturnsSchemaParseError() {
    var malformedSchema =
        """
        <xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">
          <xs:element name=\"note\" type=\"xs:string\">
        </xs:schema>
        """;
    var xml = "<note>ok</note>";

    var resp =
        xsd.validate(
                ValidateRequest.newBuilder()
                    .setSchema(bytes(malformedSchema))
                    .setDocument(bytes(xml))
                    .build())
            .await()
            .indefinitely();

    assertFalse(resp.getValid());
    assertTrue(resp.hasError());
    assertEquals(BridgeError.Kind.COMPILATION_FAILED, resp.getError().getKind());
    assertTrue(resp.getError().getMessage().contains("SCHEMA_PARSE_ERROR"));
  }

  @Test
  void externalSchemaImportDenied() {
    var schemaWithExternalImport =
        """
        <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                   xmlns:ext="http://127.0.0.1:9/external.xsd">
          <xs:import namespace="http://127.0.0.1:9/external.xsd"
                     schemaLocation="http://127.0.0.1:9/external.xsd"/>
          <xs:element name="note" type="ext:SomeType"/>
        </xs:schema>
        """;
    var xml = "<note>ok</note>";

    var resp =
        xsd.validate(
                ValidateRequest.newBuilder()
                    .setSchema(bytes(schemaWithExternalImport))
                    .setDocument(bytes(xml))
                    .build())
            .await()
            .indefinitely();

    assertFalse(resp.getValid());
    assertTrue(resp.hasError());
    assertEquals(BridgeError.Kind.COMPILATION_FAILED, resp.getError().getKind());
    assertTrue(resp.getError().getMessage().contains("SCHEMA_PARSE_ERROR"));
    // Deny-all ACCESS_EXTERNAL_SCHEMA prevents the fetch entirely: the external
    // namespace is never loaded, so the reference fails to resolve. If a fetch
    // were attempted (deny-all absent), Xerces would instead report
    // "schema_reference.4: Failed to read schema document ...".
    assertTrue(resp.getError().getMessage().contains("ext:SomeType"));
  }

  private static ByteString bytes(String value) {
    return ByteString.copyFrom(value, StandardCharsets.UTF_8);
  }
}
