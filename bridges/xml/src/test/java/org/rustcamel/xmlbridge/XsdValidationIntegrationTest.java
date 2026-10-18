package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.protobuf.ByteString;
import io.grpc.Channel;
import io.quarkus.grpc.GrpcClient;
import io.quarkus.grpc.GrpcService;
import io.quarkus.test.junit.QuarkusTest;
import jakarta.inject.Inject;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.lang.management.ManagementFactory;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.time.Duration;
import java.util.HexFormat;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import java.util.concurrent.locks.LockSupport;
import javax.xml.transform.stream.StreamSource;
import javax.xml.validation.Schema;
import org.junit.jupiter.api.Test;
import org.xml.sax.SAXException;
import xml_bridge.BridgeError;
import xml_bridge.HealthCheckRequest;
import xml_bridge.MutinyHealthGrpc;
import xml_bridge.MutinyXsdValidatorGrpc;
import xml_bridge.RegisterSchemaRequest;
import xml_bridge.RegisterSchemaResponse;
import xml_bridge.UnregisterSchemaRequest;
import xml_bridge.UnregisterSchemaResponse;
import xml_bridge.ValidateRequest;
import xml_bridge.ValidateResponse;
import xml_bridge.ValidateWithRequest;

@QuarkusTest
class XsdValidationIntegrationTest {

  private static final String XS = "http://www.w3.org/2001/XMLSchema";

  private static final String NOTE_TO =
      "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:to>Alice</c:to></note>";

  private static final String NOTE_FROM =
      "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:from>Alice</c:from></note>";

  @GrpcClient("xsd-validator")
  MutinyXsdValidatorGrpc.MutinyXsdValidatorStub xsd;

  /** The same channel as {@link #xsd}; both stubs must share it so event-loop affinity holds. */
  @GrpcClient("xsd-validator")
  Channel channel;

  /** The actual Quarkus server bean, so the test holds the exact monitor the RPC handlers use. */
  @Inject @GrpcService XsdValidatorService actualRoot;

  /** The same cache bean the service uses, so size/eviction assertions observe real state. */
  @Inject SchemaCache schemaCache;

  // ── Legacy single-blob (validate) cases ────────────────────────────────────

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

  // ── Decoded caps ───────────────────────────────────────────────────────────

  @Test
  void registerAtExactByteBudgetSucceeds() {
    var root = paddedRoot(8 * 1024 * 1024);
    var request =
        RegisterSchemaRequest.newBuilder()
            .setSchemaId("exact-byte-budget")
            .setSchema(root)
            .setRootSystemId("root.xsd")
            .putResources("root.xsd", root)
            .build();
    assertEquals(8 * 1024 * 1024, root.size());
    assertEquals(root, request.getSchema());
    assertTrue(request.getSerializedSize() > 16 * 1024 * 1024);
    var response = xsd.registerSchema(request).await().atMost(Duration.ofSeconds(10));
    assertFalse(response.hasError(), response.getError().getMessage());
    assertEquals("exact-byte-budget", response.getSchemaId());
    assertTrue(validateWith(response.getSchemaId(), "<note>ok</note>").getValid());
  }

  @Test
  void registerOneByteOverBudgetIsDomainRejected() {
    var root = paddedRoot(8 * 1024 * 1024 + 1);
    var request =
        RegisterSchemaRequest.newBuilder()
            .setSchemaId("one-byte-over-budget")
            .setSchema(root)
            .setRootSystemId("root.xsd")
            .putResources("root.xsd", root)
            .build();
    assertEquals(8 * 1024 * 1024 + 1, root.size());
    var response = xsd.registerSchema(request).await().atMost(Duration.ofSeconds(10));
    assertTrue(response.hasError());
    assertEquals(BridgeError.Kind.INVALID_INPUT, response.getError().getKind());
    assertTrue(response.getError().getMessage().startsWith("SCHEMA_SET_BYTE_CAP:"));
  }

  @Test
  void legacySingleBlobStillKeyedBySha256() throws Exception {
    var root = paddedRoot(256);
    var digest =
        HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(root.toByteArray()));
    var schemaId = "xsd-" + digest;
    var request = RegisterSchemaRequest.newBuilder().setSchemaId(schemaId).setSchema(root).build();
    assertEquals(0, request.getResourcesCount());
    assertEquals(0, request.getAliasesCount());
    var response = xsd.registerSchema(request).await().atMost(Duration.ofSeconds(10));
    assertFalse(response.hasError(), response.getError().getMessage());
    assertEquals(schemaId, response.getSchemaId());
    var snapshot = actualRoot.snapshotFor(schemaId);
    assertNotNull(snapshot);
    assertEquals(digest, snapshot.digest());
    assertTrue(validateWith(schemaId, "<note>ok</note>").getValid());
  }

  private static ByteString paddedRoot(int size) {
    var prefix =
        "<xs:schema xmlns:xs=\"" + XS + "\"><xs:element name=\"note\" type=\"xs:string\"/><!--";
    var suffix = "--></xs:schema>";
    return bytes(prefix + " ".repeat(size - prefix.length() - suffix.length()) + suffix);
  }

  @Test
  void moreThan256DecodedEntriesRejected() {
    var builder =
        RegisterSchemaRequest.newBuilder().setSchemaId("cap-count").setRootSystemId("root.xsd");
    for (int i = 0; i < 257; i++) {
      builder.putResources("r/" + i + ".xsd", ByteString.copyFrom(new byte[] {1}));
    }

    var resp = xsd.registerSchema(builder.build()).await().indefinitely();

    assertTrue(resp.hasError());
    assertTrue(resp.getError().getMessage().contains("256"));
  }

  @Test
  void moreThan8MiBDecodedRejected() {
    var big = ByteString.copyFrom(new byte[9 * 1024 * 1024]);
    var resp =
        xsd.registerSchema(
                RegisterSchemaRequest.newBuilder()
                    .setSchemaId("cap-bytes")
                    .setRootSystemId("root.xsd")
                    .putResources("root.xsd", big)
                    .build())
            .await()
            .indefinitely();

    assertTrue(resp.hasError());
    assertTrue(resp.getError().getMessage().contains("8 MiB"));
  }

  @Test
  void resourceAliasUrisOver128Rejected() {
    var builder =
        RegisterSchemaRequest.newBuilder()
            .setSchemaId("cap-resource-alias")
            .setRootSystemId("http://example.com/r0.xsd");
    for (int i = 0; i < 129; i++) {
      builder.putResources("http://example.com/r" + i + ".xsd", bytes("<xs:schema/>"));
    }

    var resp = xsd.registerSchema(builder.build()).await().indefinitely();

    assertTrue(resp.hasError());
    assertTrue(resp.getError().getMessage().contains("128"));
    assertTrue(resp.getError().getMessage().contains("resource-alias"));
  }

  @Test
  void aliasMapOver128Rejected() {
    var builder = RegisterSchemaRequest.newBuilder().setSchemaId("cap-alias-map");
    for (int i = 0; i < 129; i++) {
      builder.putAliases("http://example.com/a" + i + ".xsd", "a/x.xsd");
    }

    var resp = xsd.registerSchema(builder.build()).await().indefinitely();

    assertTrue(resp.hasError());
    assertTrue(resp.getError().getMessage().contains("alias-map"), resp.getError().getMessage());
  }

  @Test
  void aliasAndRelativeSameFileCountSeparately() {
    // The alias-URI entry and the relative entry denote the same file but are
    // two decoded entries: 255 other entries + those two exceed the 256 cap. A
    // deduplicating decoder would see 256 and proceed.
    var builder =
        RegisterSchemaRequest.newBuilder().setSchemaId("cap-separate").setRootSystemId("root.xsd");
    builder.putResources("root.xsd", bytes("<xs:schema/>"));
    for (int i = 0; i < 254; i++) {
      builder.putResources("d/" + i + ".xsd", ByteString.copyFrom(new byte[] {1}));
    }
    builder.putResources("a/child.xsd", bytes(schemaType("urn:child", "childType", "to")));
    builder.putResources(
        "http://example.com/child.xsd", bytes(schemaType("urn:child", "childType", "to")));

    var resp = xsd.registerSchema(builder.build()).await().indefinitely();

    assertTrue(resp.hasError());
    assertTrue(resp.getError().getMessage().contains("256"), resp.getError().getMessage());
  }

  @Test
  void countCapAndByteCapAreIndependent() {
    var manyBuilder =
        RegisterSchemaRequest.newBuilder().setSchemaId("ind-count").setRootSystemId("root.xsd");
    for (int i = 0; i < 257; i++) {
      manyBuilder.putResources("r/" + i + ".xsd", ByteString.copyFrom(new byte[] {1}));
    }
    var countResp = xsd.registerSchema(manyBuilder.build()).await().indefinitely();
    assertTrue(countResp.hasError());
    assertTrue(countResp.getError().getMessage().contains("256"));

    var bigBuilder =
        RegisterSchemaRequest.newBuilder().setSchemaId("ind-bytes").setRootSystemId("root.xsd");
    bigBuilder.putResources("root.xsd", ByteString.copyFrom(new byte[9 * 1024 * 1024]));
    var byteResp = xsd.registerSchema(bigBuilder.build()).await().indefinitely();
    assertTrue(byteResp.hasError());
    assertTrue(byteResp.getError().getMessage().contains("8 MiB"));
  }

  @Test
  void duplicateMapKeyIsLastWinsNoError() {
    // Proto map semantics overwrite the duplicate before decode; no duplicate
    // key can reach the bridge and no duplicate-key error is possible.
    var builder =
        RegisterSchemaRequest.newBuilder().setSchemaId("dup-last").setRootSystemId("root.xsd");
    builder.putResources("root.xsd", bytes(importSchema("urn:child", "child.xsd", "noteType")));
    builder.putResources("child.xsd", bytes("<xs:schema/>"));
    builder.putResources("child.xsd", bytes(schemaType("urn:child", "noteType", "to")));

    var resp = xsd.registerSchema(builder.build()).await().indefinitely();
    assertFalse(resp.hasError());

    assertTrue(
        validateWith(
                "dup-last",
                "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:to>Alice</c:to></note>")
            .getValid());
  }

  // ── Alias validation ───────────────────────────────────────────────────────

  @Test
  void aliasKeyNotAbsoluteUriRejected() {
    assertValidAliasControl("alias-key-control");
    var resp =
        register(
            "alias-key",
            "root.xsd",
            aliasGuardResources(),
            Map.of("relative/child.xsd", "child.xsd"));
    assertAliasRejected(resp, "alias-key", "SCHEMA_SET_ALIAS_KEY:");
  }

  @Test
  void aliasValueAbsoluteRejected() {
    assertValidAliasControl("alias-abs-control");
    var resp =
        register(
            "alias-abs",
            "root.xsd",
            aliasGuardResources(),
            Map.of("http://example.com/child.xsd", "http://example.com/other.xsd"));
    assertAliasRejected(resp, "alias-abs", "SCHEMA_SET_ALIAS_VALUE_ABSOLUTE:");
  }

  @Test
  void aliasValueMissingResourceRejected() {
    assertValidAliasControl("alias-missing-control");
    var resp =
        register(
            "alias-missing",
            "root.xsd",
            aliasGuardResources(),
            Map.of("http://example.com/child.xsd", "missing.xsd"));
    assertAliasRejected(resp, "alias-missing", "SCHEMA_SET_ALIAS_VALUE_MISSING:");
  }

  @Test
  void aliasValueTraversalRejected() {
    assertValidAliasControl("alias-traversal-control");
    var resp =
        register(
            "alias-traversal",
            "root.xsd",
            aliasGuardResources(),
            Map.of("http://example.com/child.xsd", "../escape.xsd"));
    assertAliasRejected(resp, "alias-traversal", "SCHEMA_SET_ALIAS_VALUE_ESCAPES:");
  }

  @Test
  void aliasValueInRootTraversalResolves() {
    var resources =
        res(
            "root.xsd",
            importSchema("urn:child", "http://example.com/child.xsd", "noteType"),
            "a/child.xsd",
            schemaType("urn:child", "noteType", "to"));
    var resp =
        register(
            "alias-inroot",
            "root.xsd",
            resources,
            Map.of("http://example.com/child.xsd", "a/../a/child.xsd"));

    assertFalse(resp.hasError());
    assertTrue(
        validateWith(
                "alias-inroot",
                "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:to>Alice</c:to></note>")
            .getValid());
  }

  // ── Resolver precedence and supplied-keys-only resolution ──────────────────

  @Test
  void relativeImportResolvesFromSuppliedSet() {
    var resources =
        res(
            "root.xsd",
            importSchema("urn:child", "child.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    var resp = register("relative-import", "root.xsd", resources, Map.of());

    assertFalse(resp.hasError());
    assertTrue(validateWith("relative-import", NOTE_TO).getValid());
    assertFalse(validateWith("relative-import", NOTE_FROM).getValid());

    // Filesystem-like names must round-trip: literal spaces, literal '%', and a
    // literal '+' (never decoded to a space).
    assertNamingResolves(
        "relative-import-space", "root.xsd", "child%20space.xsd", "child space.xsd");
    assertNamingResolves("relative-import-pct", "root.xsd", "child%25name.xsd", "child%name.xsd");
    assertNamingResolves("relative-import-plus", "root.xsd", "child+name.xsd", "child+name.xsd");
    assertNamingResolves(
        "relative-import-literal-space", "root.xsd", "child space.xsd", "child space.xsd");
    assertNamingResolves(
        "relative-import-nested-space",
        "my schema.xsd",
        "nested%20dir/child%20space.xsd",
        "nested dir/child space.xsd");
  }

  @Test
  void encodedFragmentFilenameRegisters() {
    assertEncodedFilenameRegisters("encoded-fragment", "child%23v1.xsd", "child#v1.xsd");
  }

  @Test
  void encodedQueryFilenameRegisters() {
    assertEncodedFilenameRegisters("encoded-query", "child%3Fv1.xsd", "child?v1.xsd");
  }

  @Test
  void rawRelativeUriDelimitersAreRefused() {
    for (var reference : new String[] {"child.xsd#v1", "child.xsd?v1"}) {
      var schemaId = "raw-delimiter-" + reference;
      // Supply both interpretations: neither literal lookup nor truncation is permitted.
      var resources =
          res(
              "root.xsd",
              importSchema("urn:child", reference, "noteType"),
              reference,
              schemaType("urn:child", "noteType", "to"),
              "child.xsd",
              schemaType("urn:child", "noteType", "to"));
      var response = register(schemaId, "root.xsd", resources, Map.of());

      assertTrue(response.hasError(), reference);
      assertEquals(BridgeError.Kind.COMPILATION_FAILED, response.getError().getKind());
      assertTrue(
          response.getError().getMessage().startsWith("SCHEMA_PARSE_ERROR:"),
          response.getError().getMessage());
      assertNull(actualRoot.snapshotFor(schemaId));
      assertEquals(
          BridgeError.Kind.RESOURCE_NOT_FOUND,
          validateWith(schemaId, NOTE_TO).getError().getKind());
    }
  }

  @Test
  void baseRelativeBeatsExactRootRawKey() {
    var resources =
        res(
            "a/parent.xsd",
            importSchema("urn:child", "child.xsd", "noteType"),
            "a/child.xsd",
            schemaType("urn:child", "noteType", "to"),
            "child.xsd",
            schemaType("urn:child", "noteType", "from"));
    var resp = register("base-relative", "a/parent.xsd", resources, Map.of());

    assertFalse(resp.hasError(), resp.hasError() ? resp.getError().getMessage() : "ok");
    // a/child.xsd is used (requires c:to); the root decoy requires c:from.
    assertTrue(validateWith("base-relative", NOTE_TO).getValid());
    assertFalse(validateWith("base-relative", NOTE_FROM).getValid());

    // A nested importer directory containing a literal space resolves its own
    // child, not a root-level decoy equal to the raw systemId.
    assertNestedBaseResolves(
        "base-relative-space", "nested dir/parent.xsd", "child.xsd", "nested dir/child.xsd");
    // A nested importer directory containing a literal '%'.
    assertNestedBaseResolves(
        "base-relative-pct", "nested%dir/parent.xsd", "child.xsd", "nested%dir/child.xsd");
  }

  @Test
  void aliasResolvesWithVirtualBase() {
    var resources =
        res(
            "root.xsd",
            importSchema("urn:child", "http://example.com/parent.xsd", "childType"),
            "a/parent.xsd",
            includeSchema("urn:child", "child.xsd"),
            "a/child.xsd",
            schemaType("urn:child", "childType", "to"),
            "child.xsd",
            schemaType("urn:child", "childType", "from"));
    var resp =
        register(
            "alias-virtual-base",
            "root.xsd",
            resources,
            Map.of("http://example.com/parent.xsd", "a/parent.xsd"));

    assertFalse(resp.hasError());
    assertTrue(validateWith("alias-virtual-base", NOTE_TO).getValid());
    assertFalse(validateWith("alias-virtual-base", NOTE_FROM).getValid());

    // An alias mapped to a virtual base containing a literal space: its own
    // relative include resolves against that base, not the root decoy.
    var spaced =
        res(
            "root.xsd",
            importSchema("urn:child", "http://example.com/space-parent.xsd", "childType"),
            "a dir/parent.xsd",
            includeSchema("urn:child", "child.xsd"),
            "a dir/child.xsd",
            schemaType("urn:child", "childType", "to"),
            "child.xsd",
            schemaType("urn:child", "childType", "from"));
    var spacedResp =
        register(
            "alias-virtual-space",
            "root.xsd",
            spaced,
            Map.of("http://example.com/space-parent.xsd", "a dir/parent.xsd"));
    assertFalse(
        spacedResp.hasError(), spacedResp.hasError() ? spacedResp.getError().getMessage() : "ok");
    assertTrue(validateWith("alias-virtual-space", NOTE_TO).getValid());
    assertFalse(validateWith("alias-virtual-space", NOTE_FROM).getValid());

    // An alias mapped to a virtual base containing a literal '%'.
    var pct =
        res(
            "root.xsd",
            importSchema("urn:child", "http://example.com/pct-parent.xsd", "childType"),
            "a/parent%name.xsd",
            includeSchema("urn:child", "child.xsd"),
            "a/child.xsd",
            schemaType("urn:child", "childType", "to"),
            "child.xsd",
            schemaType("urn:child", "childType", "from"));
    var pctResp =
        register(
            "alias-virtual-pct",
            "root.xsd",
            pct,
            Map.of("http://example.com/pct-parent.xsd", "a/parent%name.xsd"));
    assertFalse(pctResp.hasError(), pctResp.hasError() ? pctResp.getError().getMessage() : "ok");
    assertTrue(validateWith("alias-virtual-pct", NOTE_TO).getValid());
    assertFalse(validateWith("alias-virtual-pct", NOTE_FROM).getValid());
  }

  @Test
  void nestedImporterAndAliasDoNotDoubleBase() {
    var resources =
        res(
            "root.xsd",
            """
            <xs:schema xmlns:xs="%s" xmlns:c="urn:child" targetNamespace="urn:root"
                       elementFormDefault="qualified">
              <xs:import namespace="urn:nested" schemaLocation="nested/child.xsd"/>
              <xs:import namespace="urn:child"
                         schemaLocation="http://example.com/parent.xsd"/>
              <xs:element name="note" type="c:childType"/>
            </xs:schema>
            """
                .formatted(XS),
            "nested/child.xsd",
            schemaType("urn:nested", "nestedType", "to"),
            "a/parent.xsd",
            includeSchema("urn:child", "child.xsd"),
            "a/child.xsd",
            schemaType("urn:child", "childType", "to"),
            "child.xsd",
            schemaType("urn:child", "childType", "from"));
    var resp =
        register(
            "no-double-base",
            "root.xsd",
            resources,
            Map.of("http://example.com/parent.xsd", "a/parent.xsd"));

    assertFalse(resp.hasError());
    assertTrue(
        validateWith(
                "no-double-base",
                "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:to>Alice</c:to></note>")
            .getValid());
  }

  @Test
  void suppliedAliasResolvesWithoutFetch() {
    var resources =
        res(
            "root.xsd",
            importSchema("urn:child", "http://example.com/child.xsd", "noteType"),
            "a/child.xsd",
            schemaType("urn:child", "noteType", "to"));
    var resp =
        register(
            "supplied-alias",
            "root.xsd",
            resources,
            Map.of("http://example.com/child.xsd", "a/child.xsd"));

    assertFalse(resp.hasError());
    assertTrue(
        validateWith(
                "supplied-alias",
                "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:to>Alice</c:to></note>")
            .getValid());
  }

  @Test
  void unsuppliedUrlRefused() {
    var resources =
        res(
            "root.xsd",
            importSchema("urn:child", "http://127.0.0.1:9/external.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    var resp = register("unsupplied-url", "root.xsd", resources, Map.of());

    assertTrue(resp.hasError());
    assertNotNull(resp.getError().getMessage());
  }

  @Test
  void escapingTraversalAndDtdRefused() {
    // root-relative '../' that escapes the anchor.
    var escaping =
        res(
            "root.xsd",
            importSchema("urn:child", "../escape.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    assertTrue(register("escaping-traversal", "root.xsd", escaping, Map.of()).hasError());

    var withDtd =
        res(
            "root.xsd",
            importSchema("urn:child", "child.xsd", "noteType"),
            "child.xsd",
            "<!DOCTYPE xs:schema SYSTEM \"http://127.0.0.1:9/evil.dtd\">\n"
                + schemaType("urn:child", "noteType", "to"));
    assertTrue(register("dtd-refused", "root.xsd", withDtd, Map.of()).hasError());

    // A nested importer with an absolute path is refused before any join, even
    // though a valid a/child.xsd decoy is supplied.
    var nestedAbs =
        res(
            "a/parent.xsd",
            importSchema("urn:child", "/child.xsd", "noteType"),
            "a/child.xsd",
            schemaType("urn:child", "noteType", "to"));
    assertTrue(register("nested-abs-path", "a/parent.xsd", nestedAbs, Map.of()).hasError());

    // A percent-encoded leading slash must not become a relative join.
    var nestedEncAbs =
        res(
            "a/parent.xsd",
            importSchema("urn:child", "%2Fchild.xsd", "noteType"),
            "a/child.xsd",
            schemaType("urn:child", "noteType", "to"));
    assertTrue(register("nested-enc-abs-path", "a/parent.xsd", nestedEncAbs, Map.of()).hasError());

    // Malformed percent escape and invalid UTF-8 are refused, not fetched.
    var malformed =
        res(
            "root.xsd",
            importSchema("urn:child", "child%zz.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    assertTrue(register("malformed-percent", "root.xsd", malformed, Map.of()).hasError());

    var badUtf8 =
        res(
            "root.xsd",
            importSchema("urn:child", "child%FF.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    assertTrue(register("invalid-utf8", "root.xsd", badUtf8, Map.of()).hasError());

    // An unsupplied out-of-anchor file: URL is refused.
    var fileUrl =
        res(
            "root.xsd",
            importSchema("urn:child", "file:///etc/passwd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    assertTrue(register("file-out-anchor", "root.xsd", fileUrl, Map.of()).hasError());

    // The root systemId is never a URI or absolute path, even when a resource
    // key matches it exactly.
    assertTrue(
        register(
                "root-abs-uri",
                "http://example.com/root.xsd",
                res(
                    "http://example.com/root.xsd",
                    importSchema("urn:child", "child.xsd", "noteType"),
                    "child.xsd",
                    schemaType("urn:child", "noteType", "to")),
                Map.of())
            .hasError());
    assertTrue(
        register("root-abs-path", "/root.xsd", res("/root.xsd", "<xs:schema/>"), Map.of())
            .hasError());
  }

  @Test
  void inRootTraversalNormalizesResolves() {
    var resources =
        res(
            "root.xsd",
            importSchema("urn:child", "a/../child.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    var resp = register("in-root-traversal", "root.xsd", resources, Map.of());

    assertFalse(resp.hasError());
    assertTrue(
        validateWith(
                "in-root-traversal",
                "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:to>Alice</c:to></note>")
            .getValid());
  }

  // ── Cache rotation and shared digests ──────────────────────────────────────

  @Test
  void changedChildRotatesCacheEntry() {
    var first =
        res(
            "root.xsd",
            importSchema("urn:child", "child.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    assertFalse(register("rotating", "root.xsd", first, Map.of()).hasError());
    assertTrue(
        validateWith(
                "rotating",
                "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:to>Alice</c:to></note>")
            .getValid());

    var second =
        res(
            "root.xsd",
            importSchema("urn:child", "child.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "from"));
    assertFalse(register("rotating", "root.xsd", second, Map.of()).hasError());
    // The changed child rotated the digest: the new set requires c:from.
    assertTrue(
        validateWith(
                    "rotating",
                    "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:to>Alice</c:to></note>")
                .getValid()
            == false);
    assertTrue(
        validateWith(
                "rotating",
                "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:from>Alice</c:from></note>")
            .getValid());
  }

  @Test
  void sharedDigestSurvivesUnregister() {
    var resources =
        res(
            "root.xsd",
            importSchema("urn:child", "child.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    assertFalse(register("shared-a", "root.xsd", resources, Map.of()).hasError());
    assertFalse(register("shared-b", "root.xsd", resources, Map.of()).hasError());

    var released =
        xsd.unregisterSchema(UnregisterSchemaRequest.newBuilder().setSchemaId("shared-a").build())
            .await()
            .indefinitely();
    assertTrue(released.getReleased());

    // B still references the shared compiled digest.
    assertTrue(
        validateWith(
                "shared-b",
                "<note xmlns=\"urn:root\" xmlns:c=\"urn:child\"><c:to>Alice</c:to></note>")
            .getValid());
    // A is gone.
    var afterUnregister = validateWith("shared-a", "<note/>");
    assertFalse(afterUnregister.getValid());
    assertEquals(BridgeError.Kind.RESOURCE_NOT_FOUND, afterUnregister.getError().getKind());
  }

  // ── Bounded cache under inline pressure ────────────────────────────────────

  @Test
  void inlineValidateBeyondBoundKeepsCacheBounded() {
    // The cache bean and schemaHashById are class-wide singletons: earlier tests
    // may have registered (and pinned) digests, so measure delta from a baseline.
    int sizeBase = schemaCache.size();
    long evBase = schemaCache.evictions();
    var tag = uniqueTag();

    for (int i = 0; i < 10; i++) {
      var resp =
          xsd.validate(
                  ValidateRequest.newBuilder()
                      .setSchema(bytes(inlineSchema(tag, i)))
                      .setDocument(bytes(inlineDocument(tag, i)))
                      .build())
              .await()
              .atMost(Duration.ofSeconds(10));
      assertTrue(resp.getValid(), "inline validation " + i + " must stay valid");
    }

    assertTrue(
        schemaCache.size() <= sizeBase + 8,
        "cache size " + schemaCache.size() + " must stay within baseline+8 " + (sizeBase + 8));
    assertTrue(
        schemaCache.evictions() >= evBase + 2,
        "inline pressure must have evicted: evictions=" + schemaCache.evictions());
  }

  @Test
  void registeredSchemaSetNotEvictedByInlinePressure() {
    int sizeBase = schemaCache.size();
    var tag = uniqueTag();
    var schemaId = "pinned-set-" + tag;

    var registered =
        register(
            schemaId,
            "root.xsd",
            res(
                "root.xsd",
                importSchema("urn:child", "child.xsd", "noteType"),
                "child.xsd",
                schemaType("urn:child", "noteType", "to")),
            Map.of());
    assertFalse(
        registered.hasError(), registered.hasError() ? registered.getError().getMessage() : "ok");

    for (int i = 0; i < 10; i++) {
      var resp =
          xsd.validate(
                  ValidateRequest.newBuilder()
                      .setSchema(bytes(inlineSchema(tag, i)))
                      .setDocument(bytes(inlineDocument(tag, i)))
                      .build())
              .await()
              .atMost(Duration.ofSeconds(10));
      assertTrue(resp.getValid(), "inline validation " + i + " must stay valid");
    }

    assertTrue(
        validateWith(schemaId, NOTE_TO).getValid(),
        "a registered schema set must survive inline eviction pressure");
    assertTrue(
        schemaCache.size() <= sizeBase + 1 + 8,
        "cache size "
            + schemaCache.size()
            + " must stay within baseline + pinned set + 8 inline");
  }

  @Test
  void unregisterAllowsBoundRecovery() {
    int sizeBase = schemaCache.size();
    var tag = uniqueTag();
    var schemaId = "temp-set-" + tag;

    var registered =
        register(
            schemaId,
            "root.xsd",
            res(
                "root.xsd",
                importSchema("urn:child", "child.xsd", "noteType"),
                "child.xsd",
                schemaType("urn:child", "noteType", "to")),
            Map.of());
    assertFalse(
        registered.hasError(), registered.hasError() ? registered.getError().getMessage() : "ok");

    var released =
        xsd.unregisterSchema(UnregisterSchemaRequest.newBuilder().setSchemaId(schemaId).build())
            .await()
            .atMost(Duration.ofSeconds(10));
    assertTrue(released.getReleased());

    for (int i = 0; i < 10; i++) {
      var resp =
          xsd.validate(
                  ValidateRequest.newBuilder()
                      .setSchema(bytes(inlineSchema(tag, i)))
                      .setDocument(bytes(inlineDocument(tag, i)))
                      .build())
              .await()
              .atMost(Duration.ofSeconds(10));
      assertTrue(resp.getValid(), "inline validation " + i + " must stay valid");
    }

    assertTrue(
        schemaCache.size() <= sizeBase + 8,
        "cache size " + schemaCache.size() + " must stay within baseline+8 " + (sizeBase + 8));
    assertEquals(
        BridgeError.Kind.RESOURCE_NOT_FOUND, validateWith(schemaId, NOTE_TO).getError().getKind());
  }

  private static String uniqueTag() {
    return java.util.UUID.randomUUID().toString().replace("-", "");
  }

  /** A minimal inline XSD whose element name is unique per {@code (tag, index)}. */
  private static String inlineSchema(String tag, int index) {
    return """
        <xs:schema xmlns:xs="%s">
          <xs:element name="e%s%d">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="v" type="xs:string"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>
        """
        .formatted(XS, tag, index);
  }

  private static String inlineDocument(String tag, int index) {
    return "<e" + tag + index + "><v>x</v></e" + tag + index + ">";
  }

  // ── Atomic registry snapshot ────────────────────────────────────────────────

  @Test
  void validationSnapshotSurvivesConcurrentReplacement() throws Exception {
    // A directly constructed service is a real instance, never a CDI client
    // proxy, so registryLock() returns the actual monitor the RPCs use.
    var service = new XsdValidatorService();
    service.schemaCache = new SchemaCache(256);

    var setA =
        res(
            "root.xsd",
            importSchema("urn:child", "child.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "to"));
    var setB =
        res(
            "root.xsd",
            importSchema("urn:child", "child.xsd", "noteType"),
            "child.xsd",
            schemaType("urn:child", "noteType", "from"));

    assertFalse(registerOn(service, "rot", "root.xsd", setA, Map.of()).hasError());
    var snapshotA = service.snapshotFor("rot");
    assertNotNull(snapshotA);
    assertNotNull(snapshotA.schema());

    // Hold the real monitor, start a validateWith worker, prove it blocks on the
    // monitor, replace the set reentrantly, then release. A lock-free reader
    // would not block and this bounded assertion fails.
    var workerStarted = new CountDownLatch(1);
    var workerDone = new CountDownLatch(1);
    var workerResult = new AtomicReference<ValidateResponse>();
    var worker =
        new Thread(
            () -> {
              workerStarted.countDown();
              workerResult.set(validateWithOn(service, "rot", NOTE_FROM));
              workerDone.countDown();
            });
    synchronized (service.registryLock()) {
      worker.start();
      assertTrue(workerStarted.await(2, TimeUnit.SECONDS));
      // Causally prove the worker is BLOCKED entering this thread's registry
      // monitor before replacing reentrantly. A lock-free reader never blocks on
      // it, so this bounded probe fails rather than passing on a scheduler gap.
      awaitBlockedOn(
          service.registryLock(),
          worker,
          Thread.currentThread(),
          System.nanoTime() + TimeUnit.SECONDS.toNanos(5));
      assertFalse(registerOn(service, "rot", "root.xsd", setB, Map.of()).hasError());
    }
    assertTrue(workerDone.await(5, TimeUnit.SECONDS));

    // After the replacement the worker follows B and is never RESOURCE_NOT_FOUND.
    var workerResponse = workerResult.get();
    assertNotNull(workerResponse);
    assertTrue(workerResponse.getValid(), "worker must follow replacement set B");
    assertFalse(workerResponse.hasError());

    // Public validateWith follows B: c:from valid, c:to invalid.
    assertTrue(validateWithOn(service, "rot", NOTE_FROM).getValid());
    assertFalse(validateWithOn(service, "rot", NOTE_TO).getValid());

    // The captured snapshot still applies A after B evicted digest A.
    assertTrue(validAgainst(snapshotA.schema(), NOTE_TO));
    assertFalse(validAgainst(snapshotA.schema(), NOTE_FROM));
  }

  // ── Transport: blocking unregister keeps the event loop responsive ──────────

  @Test
  void unregisterWaitDoesNotBlockGrpcEventLoop() throws Exception {
    var xsdStub = MutinyXsdValidatorGrpc.newMutinyStub(channel);
    var healthStub = MutinyHealthGrpc.newMutinyStub(channel);
    var schemaId = "evt-unregister";
    var healthRequest = HealthCheckRequest.newBuilder().build();

    // Baseline: a registered schema and a responsive health check on the same channel.
    var registered =
        xsdStub
            .registerSchema(
                RegisterSchemaRequest.newBuilder()
                    .setSchemaId(schemaId)
                    .setRootSystemId("root.xsd")
                    .putResources("root.xsd", bytes(schemaType("urn:child", "noteType", "to")))
                    .build())
            .await()
            .indefinitely();
    assertFalse(registered.hasError());
    assertEquals("SERVING", healthStub.check(healthRequest).await().indefinitely().getStatus());

    var unregisterDone = new CountDownLatch(1);
    var unregisterResponse = new AtomicReference<UnregisterSchemaResponse>();
    // synchronized releases the monitor on every exit path, including a RED timeout.
    synchronized (actualRoot.registryLock()) {
      // Fire the unregister through the real gRPC client, asynchronously.
      xsdStub
          .unregisterSchema(UnregisterSchemaRequest.newBuilder().setSchemaId(schemaId).build())
          .subscribe()
          .with(
              response -> {
                unregisterResponse.set(response);
                unregisterDone.countDown();
              },
              failure -> unregisterDone.countDown());

      // Causally prove a server thread (worker with @Blocking, event loop without it)
      // is BLOCKED on the monitor this test thread owns before the health request.
      awaitAnyThreadBlockedOn(
          actualRoot.registryLock(),
          Thread.currentThread(),
          System.nanoTime() + TimeUnit.SECONDS.toNanos(5));

      // The event loop must still answer a health RPC on the SAME channel promptly.
      assertEquals(
          "SERVING",
          healthStub.check(healthRequest).await().atMost(Duration.ofSeconds(3)).getStatus());

      // The unregister is still pending while the monitor is held.
      assertNull(unregisterResponse.get());
    }

    assertTrue(unregisterDone.await(5, TimeUnit.SECONDS));
    assertNotNull(unregisterResponse.get());
    assertTrue(unregisterResponse.get().getReleased());
  }

  // ── Bounded registration diagnostics ───────────────────────────────────────

  @Test
  void registrationDiagnosticsAreBounded() {
    final int bound = 512;

    // A 10000-char root key with malformed XML fails during pre-parse; neither
    // the message nor the detail may echo the whole untrusted key.
    var longKey = "k".repeat(10000) + ".xsd";
    var preResp = register("diag-preparse", longKey, res(longKey, "<xs:schema><oops"), Map.of());
    assertTrue(preResp.hasError());
    assertBounded(preResp.getError(), bound);
    assertFalse(preResp.getError().getMessage().contains(longKey));
    assertFalse(preResp.getError().getDetail().contains(longKey));

    // Valid XML whose schema references a 5000-char unresolvable type name: the
    // full Xerces message is not passed through.
    var badType = "N".repeat(5000);
    var badSchema =
        """
        <xs:schema xmlns:xs="%s" xmlns:t="urn:other" targetNamespace="urn:root"
                   elementFormDefault="qualified">
          <xs:element name="note" type="t:%s"/>
        </xs:schema>
        """
            .formatted(XS, badType);
    var compileResp = register("diag-compile", "root.xsd", res("root.xsd", badSchema), Map.of());
    assertTrue(compileResp.hasError());
    assertBounded(compileResp.getError(), bound);
    assertFalse(compileResp.getError().getMessage().contains(badType));
    assertFalse(compileResp.getError().getDetail().contains(badType));

    // Cap errors remain meaningful and independently bounded.
    var capBuilder =
        RegisterSchemaRequest.newBuilder().setSchemaId("diag-cap").setRootSystemId("root.xsd");
    for (int i = 0; i < 257; i++) {
      capBuilder.putResources("r/" + i + ".xsd", ByteString.copyFrom(new byte[] {1}));
    }
    var capResp = xsd.registerSchema(capBuilder.build()).await().indefinitely();
    assertTrue(capResp.hasError());
    assertTrue(capResp.getError().getMessage().contains("256"));
    assertBounded(capResp.getError(), bound);
  }

  private static void assertBounded(BridgeError error, int bound) {
    assertFalse(error.getMessage().isEmpty());
    assertTrue(
        error.getMessage().length() <= bound, "message length " + error.getMessage().length());
    assertTrue(error.getDetail().length() <= bound, "detail length " + error.getDetail().length());
  }

  // ── Fixtures ───────────────────────────────────────────────────────────────

  private void assertEncodedFilenameRegisters(String schemaId, String reference, String childKey) {
    // No files exist under the private anchor: every level must use supplied bytes.
    var resources =
        res(
            "root.xsd",
            importSchema("urn:child", "nested/" + reference, "noteType"),
            "nested/" + childKey,
            includeSchema("urn:child", "grandchild.xsd"),
            "nested/grandchild.xsd",
            schemaType("urn:child", "noteType", "to"),
            "grandchild.xsd",
            schemaType("urn:child", "noteType", "from"));
    var response = register(schemaId, "root.xsd", resources, Map.of());
    assertFalse(response.hasError(), response.getError().getMessage());
    assertTrue(validateWith(schemaId, NOTE_TO).getValid());
    assertFalse(validateWith(schemaId, NOTE_FROM).getValid());

    // Removing a required supplied key must refuse, never fall back to a fetch.
    resources.remove("nested/grandchild.xsd");
    var missingId = schemaId + "-missing";
    var missing = register(missingId, "root.xsd", resources, Map.of());
    assertTrue(missing.hasError());
    assertEquals(BridgeError.Kind.COMPILATION_FAILED, missing.getError().getKind());
    assertTrue(missing.getError().getMessage().contains("no supplied schema"));
    assertNull(actualRoot.snapshotFor(missingId));
  }

  private void assertNamingResolves(String schemaId, String rootKey, String ref, String childKey) {
    var resources =
        res(
            rootKey,
            importSchema("urn:child", ref, "noteType"),
            childKey,
            schemaType("urn:child", "noteType", "to"));
    var resp = register(schemaId, rootKey, resources, Map.of());
    assertFalse(resp.hasError(), resp.hasError() ? resp.getError().getMessage() : "ok");
    assertTrue(validateWith(schemaId, NOTE_TO).getValid(), schemaId + " should accept c:to");
    assertFalse(validateWith(schemaId, NOTE_FROM).getValid(), schemaId + " should reject c:from");
  }

  private void assertNestedBaseResolves(
      String schemaId, String rootKey, String ref, String childKey) {
    var resources =
        res(
            rootKey,
            importSchema("urn:child", ref, "noteType"),
            childKey,
            schemaType("urn:child", "noteType", "to"),
            "child.xsd",
            schemaType("urn:child", "noteType", "from"));
    var resp = register(schemaId, rootKey, resources, Map.of());
    assertFalse(resp.hasError(), resp.hasError() ? resp.getError().getMessage() : "ok");
    assertTrue(
        validateWith(schemaId, NOTE_TO).getValid(), schemaId + " should use the nested child");
    assertFalse(
        validateWith(schemaId, NOTE_FROM).getValid(), schemaId + " should not use the root decoy");
  }

  /** A schema that imports {@code location} for {@code typeName}. */
  private static String importSchema(String childNs, String location, String typeName) {
    return """
        <xs:schema xmlns:xs="%s" xmlns:c="%s" targetNamespace="urn:root"
                   elementFormDefault="qualified">
          <xs:import namespace="%s" schemaLocation="%s"/>
          <xs:element name="note" type="c:%s"/>
        </xs:schema>
        """
        .formatted(XS, childNs, childNs, location, typeName);
  }

  /** A same-namespace schema that includes {@code location}. */
  private static String includeSchema(String ns, String location) {
    return """
        <xs:schema xmlns:xs="%s" targetNamespace="%s" elementFormDefault="qualified">
          <xs:include schemaLocation="%s"/>
        </xs:schema>
        """
        .formatted(XS, ns, location);
  }

  /** A schema defining {@code typeName} requiring one child element. */
  private static String schemaType(String ns, String typeName, String requiredElement) {
    return """
        <xs:schema xmlns:xs="%s" targetNamespace="%s" elementFormDefault="qualified">
          <xs:complexType name="%s">
            <xs:sequence>
              <xs:element name="%s" type="xs:string"/>
            </xs:sequence>
          </xs:complexType>
        </xs:schema>
        """
        .formatted(XS, ns, typeName, requiredElement);
  }

  private RegisterSchemaResponse register(
      String schemaId,
      String rootSystemId,
      Map<String, String> resources,
      Map<String, String> aliases) {
    var builder =
        RegisterSchemaRequest.newBuilder().setSchemaId(schemaId).setRootSystemId(rootSystemId);
    resources.forEach((key, value) -> builder.putResources(key, bytes(value)));
    aliases.forEach(builder::putAliases);
    return xsd.registerSchema(builder.build()).await().indefinitely();
  }

  private ValidateResponse validateWith(String schemaId, String document) {
    return xsd.validateWith(
            ValidateWithRequest.newBuilder()
                .setSchemaId(schemaId)
                .setDocument(bytes(document))
                .build())
        .await()
        .indefinitely();
  }

  private static Map<String, String> res(String... pairs) {
    var map = new LinkedHashMap<String, String>();
    for (int i = 0; i < pairs.length; i += 2) {
      map.put(pairs[i], pairs[i + 1]);
    }
    return map;
  }

  /** Resources whose root imports the alias URI and uses the imported type. */
  private Map<String, String> aliasGuardResources() {
    return res(
        "root.xsd",
        importSchema("urn:child", "http://example.com/child.xsd", "noteType"),
        "child.xsd",
        schemaType("urn:child", "noteType", "to"));
  }

  private void assertValidAliasControl(String schemaId) {
    var resp =
        register(
            schemaId,
            "root.xsd",
            aliasGuardResources(),
            Map.of("http://example.com/child.xsd", "child.xsd"));
    assertFalse(resp.hasError(), resp.hasError() ? resp.getError().getMessage() : "ok");
    assertTrue(validateWith(schemaId, NOTE_TO).getValid());
    assertFalse(validateWith(schemaId, NOTE_FROM).getValid());
  }

  private void assertAliasRejected(RegisterSchemaResponse resp, String schemaId, String category) {
    assertTrue(resp.hasError(), "expected a bounded alias rejection");
    assertEquals(BridgeError.Kind.INVALID_INPUT, resp.getError().getKind());
    assertTrue(resp.getError().getMessage().startsWith(category), resp.getError().getMessage());
    assertBounded(resp.getError(), 512);
    // A rejected alias must leave no registration side effect.
    var after = validateWith(schemaId, "<note/>");
    assertFalse(after.getValid());
    assertEquals(BridgeError.Kind.RESOURCE_NOT_FOUND, after.getError().getKind());
  }

  private static RegisterSchemaResponse registerOn(
      XsdValidatorService service,
      String schemaId,
      String rootSystemId,
      Map<String, String> resources,
      Map<String, String> aliases) {
    var builder =
        RegisterSchemaRequest.newBuilder().setSchemaId(schemaId).setRootSystemId(rootSystemId);
    resources.forEach((key, value) -> builder.putResources(key, bytes(value)));
    aliases.forEach(builder::putAliases);
    return service.registerSchema(builder.build()).await().indefinitely();
  }

  private static ValidateResponse validateWithOn(
      XsdValidatorService service, String schemaId, String document) {
    return service
        .validateWith(
            ValidateWithRequest.newBuilder()
                .setSchemaId(schemaId)
                .setDocument(bytes(document))
                .build())
        .await()
        .indefinitely();
  }

  /**
   * Boundedly proves {@code worker} is BLOCKED entering the {@code monitor} owned by {@code owner},
   * without sleeping or an unbounded spin. Throws if the worker finishes first (a lock-free reader)
   * or does not block before the deadline.
   */
  private static void awaitBlockedOn(
      Object monitor, Thread worker, Thread owner, long deadlineNanos) {
    var bean = ManagementFactory.getThreadMXBean();
    var monitorIdentity = System.identityHashCode(monitor);
    while (System.nanoTime() < deadlineNanos) {
      var info = bean.getThreadInfo(worker.threadId());
      if (info != null
          && info.getThreadState() == Thread.State.BLOCKED
          && info.getLockOwnerId() == owner.threadId()) {
        var lock = info.getLockInfo();
        if (lock != null && lock.getIdentityHashCode() == monitorIdentity) {
          return;
        }
      }
      if (!worker.isAlive()) {
        throw new AssertionError("worker finished without blocking on registryLock");
      }
      LockSupport.parkNanos(1_000_000L);
    }
    throw new AssertionError("worker did not reach BLOCKED on registryLock before the deadline");
  }

  /**
   * Boundedly proves ANY JVM thread is BLOCKED entering {@code monitor}, owned by {@code owner}.
   * Used for the transport case where the blocking server thread is not known up front. No sleep,
   * no unbounded spin.
   */
  private static void awaitAnyThreadBlockedOn(Object monitor, Thread owner, long deadlineNanos) {
    var bean = ManagementFactory.getThreadMXBean();
    var monitorIdentity = System.identityHashCode(monitor);
    while (System.nanoTime() < deadlineNanos) {
      for (var info : bean.getThreadInfo(bean.getAllThreadIds())) {
        if (info == null
            || info.getThreadState() != Thread.State.BLOCKED
            || info.getLockOwnerId() != owner.threadId()) {
          continue;
        }
        var lock = info.getLockInfo();
        if (lock != null && lock.getIdentityHashCode() == monitorIdentity) {
          return;
        }
      }
      LockSupport.parkNanos(1_000_000L);
    }
    throw new AssertionError(
        "no server thread reached BLOCKED on registryLock before the deadline");
  }

  private static boolean validAgainst(Schema schema, String document) {
    try {
      schema
          .newValidator()
          .validate(
              new StreamSource(
                  new ByteArrayInputStream(document.getBytes(StandardCharsets.UTF_8))));
      return true;
    } catch (SAXException | IOException e) {
      return false;
    }
  }

  private static ByteString bytes(String value) {
    return ByteString.copyFrom(value, StandardCharsets.UTF_8);
  }
}
