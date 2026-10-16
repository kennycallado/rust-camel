package org.rustcamel.xmlbridge;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.Reader;
import java.net.URI;
import java.net.URISyntaxException;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HexFormat;
import java.util.Map;
import java.util.TreeSet;
import javax.xml.XMLConstants;
import javax.xml.parsers.SAXParserFactory;
import javax.xml.transform.stream.StreamSource;
import javax.xml.validation.Schema;
import javax.xml.validation.SchemaFactory;
import org.w3c.dom.ls.LSInput;
import org.w3c.dom.ls.LSResourceResolver;
import org.xml.sax.ErrorHandler;
import org.xml.sax.InputSource;
import org.xml.sax.SAXException;
import org.xml.sax.SAXParseException;
import org.xml.sax.XMLReader;
import org.xml.sax.helpers.DefaultHandler;
import xml_bridge.BridgeError;
import xml_bridge.RegisterSchemaRequest;

/**
 * Stateless policy for the supplied schema set: decoded caps, alias and root validation, virtual
 * path normalization, the supplied-keys-only resolver, the per-registration URI anchor, the
 * hardened parser, and the compiled-set digest. It holds no state and no lock; the service owns the
 * registry lifecycle and the RPC diagnostic mapping.
 */
final class SchemaSetCompiler {
  private static final String W3C_XML_SCHEMA_NS = "http://www.w3.org/2001/XMLSchema";

  private static final String FEATURE_LOAD_EXTERNAL_DTD =
      "http://apache.org/xml/features/nonvalidating/load-external-dtd";
  private static final String FEATURE_EXTERNAL_GENERAL_ENTITIES =
      "http://xml.org/sax/features/external-general-entities";
  private static final String FEATURE_EXTERNAL_PARAMETER_ENTITIES =
      "http://xml.org/sax/features/external-parameter-entities";
  private static final String FEATURE_DISALLOW_DOCTYPE =
      "http://apache.org/xml/features/disallow-doctype-decl";
  private static final String PROPERTY_ENTITY_EXPANSION_LIMIT =
      "http://www.oracle.com/xml/jaxp/properties/entityExpansionLimit";

  /**
   * Fixed private virtual root shared by the root source and every returned child {@link LSInput}.
   * Xerces expands a relative {@code systemId} against whichever base it carries; pinning an
   * explicit anchor keeps that mapping deterministic and isolated per registration. The scheme is
   * {@code file:} but no file is ever opened: the resolver always supplies bytes first.
   */
  private static final String VIRTUAL_ANCHOR_URI = "file:///rust-camel-xsd/";

  private static final URI VIRTUAL_ANCHOR = URI.create(VIRTUAL_ANCHOR_URI);

  /** Domain separation for the schema-set digest, matching the Rust key. */
  private static final String SCHEMA_SET_DOMAIN = "rust-camel/schema-set/v1\0";

  private static final int MAX_RESOURCES = 256;
  private static final int MAX_ALIASES = 128;
  private static final int MAX_RESOURCE_ALIAS_URIS = 128;
  private static final long MAX_RESOURCE_BYTES = 8L * 1024L * 1024L;

  private SchemaSetCompiler() {}

  /**
   * Caps and alias validation for a decoded set, run before any cache lookup or compile. Proto map
   * semantics make duplicate keys unrepresentable on the wire, so no duplicate-key detection is
   * attempted.
   */
  static SuppliedSet decodeSuppliedSet(RegisterSchemaRequest req) {
    var protoResources = req.getResourcesMap();
    if (protoResources.size() > MAX_RESOURCES) {
      throw new SchemaSetException(
          BridgeError.Kind.INVALID_INPUT,
          "SCHEMA_SET_RESOURCE_CAP: decoded resources exceeds the 256-entry cap");
    }

    var resources = new HashMap<String, byte[]>();
    long totalBytes = 0;
    int aliasKeyed = 0;
    for (var entry : protoResources.entrySet()) {
      var key = entry.getKey();
      var value = entry.getValue().toByteArray();
      totalBytes += value.length;
      if (isAbsoluteUri(key)) {
        aliasKeyed++;
      }
      resources.put(key, value);
    }
    if (totalBytes > MAX_RESOURCE_BYTES) {
      throw new SchemaSetException(
          BridgeError.Kind.INVALID_INPUT,
          "SCHEMA_SET_BYTE_CAP: decoded resource bytes exceed the 8 MiB cap");
    }
    if (aliasKeyed > MAX_RESOURCE_ALIAS_URIS) {
      throw new SchemaSetException(
          BridgeError.Kind.INVALID_INPUT,
          "SCHEMA_SET_RESOURCE_ALIAS_CAP: decoded resources keyed by alias URIs exceed the"
              + " 128-entry resource-alias cap");
    }

    var protoAliases = req.getAliasesMap();
    if (protoAliases.size() > MAX_ALIASES) {
      throw new SchemaSetException(
          BridgeError.Kind.INVALID_INPUT,
          "SCHEMA_SET_ALIAS_CAP: decoded aliases exceed the 128-entry alias-map cap");
    }
    var aliases = new HashMap<String, String>();
    for (var entry : protoAliases.entrySet()) {
      var uri = entry.getKey();
      if (!isAbsoluteUri(uri)) {
        throw new SchemaSetException(
            BridgeError.Kind.INVALID_INPUT,
            "SCHEMA_SET_ALIAS_KEY: alias key is not an absolute URI");
      }
      var value = entry.getValue();
      if (isAbsoluteUri(value)) {
        throw new SchemaSetException(
            BridgeError.Kind.INVALID_INPUT,
            "SCHEMA_SET_ALIAS_VALUE_ABSOLUTE: alias value is an absolute URI");
      }
      String normalized;
      try {
        normalized = normalizeVirtual(value);
      } catch (ResourceRefusedException e) {
        throw new SchemaSetException(
            BridgeError.Kind.INVALID_INPUT,
            "SCHEMA_SET_ALIAS_VALUE_ESCAPES: alias value normalizes outside the virtual root");
      }
      if (!resources.containsKey(normalized)) {
        throw new SchemaSetException(
            BridgeError.Kind.INVALID_INPUT,
            "SCHEMA_SET_ALIAS_VALUE_MISSING: alias value is not present in resources");
      }
      aliases.put(uri, normalized);
    }

    var rootRaw = req.getRootSystemId();
    // The root systemId is a filesystem-like root-relative virtual path, never a
    // URI reference: an absolute URI or an absolute path is refused even if a
    // resource key happens to match it.
    if (isAbsoluteUri(rootRaw) || rootRaw.startsWith("/") || rootRaw.startsWith("\\")) {
      throw new SchemaSetException(
          BridgeError.Kind.INVALID_INPUT,
          "SCHEMA_SET_ROOT: root_system_id must be a relative virtual path",
          "absolute URI or path refused");
    }
    String normalizedRoot;
    try {
      normalizedRoot = normalizeVirtual(rootRaw);
    } catch (ResourceRefusedException e) {
      throw new SchemaSetException(
          BridgeError.Kind.INVALID_INPUT,
          "SCHEMA_SET_ROOT: root_system_id is not a normalized relative virtual path",
          e.getMessage());
    }
    if (!resources.containsKey(normalizedRoot)) {
      throw new SchemaSetException(
          BridgeError.Kind.INVALID_INPUT,
          "SCHEMA_SET_ROOT_MISSING: root_system_id is not present in resources",
          null);
    }

    return new SuppliedSet(
        normalizedRoot, resources.get(normalizedRoot), Map.copyOf(resources), Map.copyOf(aliases));
  }

  /**
   * Rejects any resource carrying a DOCTYPE. An internal DOCTYPE subset never reaches an {@link
   * LSResourceResolver}, so the resolver cannot be the only DTD guard; every transmitted resource
   * is parsed once with the shared hardened reader.
   */
  static void preparseForDoctype(Map<String, byte[]> resources) {
    for (var entry : resources.entrySet()) {
      try {
        var reader = newHardenedReader();
        reader.setContentHandler(new DefaultHandler());
        // A throwing handler keeps parser diagnostics off stderr and out of any
        // raw-data echo; the refusal is reported as a bounded category below.
        reader.setErrorHandler(new ThrowingErrorHandler());
        reader.parse(new InputSource(new ByteArrayInputStream(entry.getValue())));
      } catch (Exception e) {
        throw new SchemaSetException(
            BridgeError.Kind.INVALID_INPUT,
            "SCHEMA_SET_RESOURCE_INVALID: transmitted resource is not accepted",
            "resource '"
                + XsdValidatorService.snippet(entry.getKey())
                + "': DOCTYPE or malformed"
                + " XML");
      }
    }
  }

  static Schema compileSupplied(SuppliedSet set) throws SAXException {
    var resolver = new SuppliedKeyResolver(set.resources(), set.aliases());
    var factory = newSchemaFactory(resolver);
    var root = new StreamSource(new ByteArrayInputStream(set.rootBytes()));
    root.setSystemId(anchorUri(set.rootSystemId()).toString());
    return factory.newSchema(root);
  }

  static Schema compileLegacy(byte[] schemaBytes) throws SAXException {
    var factory = newSchemaFactory(null);
    return factory.newSchema(new StreamSource(new ByteArrayInputStream(schemaBytes)));
  }

  private static SchemaFactory newSchemaFactory(LSResourceResolver resolver) throws SAXException {
    // Construct the Xerces-J schema factory directly. JAXP discovery is
    // non-deterministic under native-image: GraalVM 25 falls back to the
    // JDK-internal Xerces, whose message bundles are absent from the image.
    // Direct construction always resolves to the classpath Xerces.
    var factory = new org.apache.xerces.jaxp.validation.XMLSchemaFactory();
    factory.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
    // Block external DTD/schema access (JAXP 1.5+)
    trySetProperty(factory, XMLConstants.ACCESS_EXTERNAL_DTD, "");
    trySetProperty(factory, XMLConstants.ACCESS_EXTERNAL_SCHEMA, "");
    if (resolver != null) {
      factory.setResourceResolver(resolver);
    }
    return factory;
  }

  /**
   * Builds an absolute anchor URI from a literal virtual path key. The key is a filesystem-like
   * string that may contain spaces, {@code %}, {@code +}, and other characters; the five-argument
   * {@link URI} constructor quotes the path component correctly instead of parsing the key as URI
   * syntax (which would misinterpret or reject those characters).
   */
  private static URI anchorUri(String virtualKey) {
    try {
      return new URI("file", "", VIRTUAL_ANCHOR.getPath() + virtualKey, null, null);
    } catch (URISyntaxException e) {
      throw new ResourceRefusedException("virtual key cannot be encoded as a URI");
    }
  }

  /**
   * Builds the hardened SAX reader shared by document validation and resource pre-parsing: no
   * external entities, no external DTD, and DOCTYPE refused.
   */
  static XMLReader newHardenedReader() throws Exception {
    // Construct the Xerces-J SAX parser factory directly. JAXP discovery is
    // non-deterministic under native-image: GraalVM 25 falls back to the
    // JDK-internal Xerces, whose message bundles are absent from the image.
    // Direct construction always resolves to the classpath Xerces.
    var spf = new org.apache.xerces.jaxp.SAXParserFactoryImpl();
    spf.setNamespaceAware(true);
    spf.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
    // Block external entity loading (standard JAXP)
    trySetFeature(spf, FEATURE_EXTERNAL_GENERAL_ENTITIES, false);
    trySetFeature(spf, FEATURE_EXTERNAL_PARAMETER_ENTITIES, false);
    trySetFeature(spf, FEATURE_LOAD_EXTERNAL_DTD, false);
    // Disallow DOCTYPE entirely (Xerces-specific but effective when available)
    trySetFeature(spf, FEATURE_DISALLOW_DOCTYPE, true);
    var reader = spf.newSAXParser().getXMLReader();
    // Hard entity-expansion limit (Xerces-specific, skip if unsupported)
    try {
      reader.setProperty(PROPERTY_ENTITY_EXPANSION_LIMIT, 100);
    } catch (SAXException ignored) {
    }
    reader.setEntityResolver(
        (publicId, systemId) -> new InputSource(new ByteArrayInputStream(new byte[0])));
    return reader;
  }

  /** Set a SchemaFactory property, ignoring unsupported-property exceptions. */
  private static void trySetProperty(SchemaFactory factory, String name, String value) {
    try {
      factory.setProperty(name, value);
    } catch (org.xml.sax.SAXNotRecognizedException | org.xml.sax.SAXNotSupportedException ignored) {
      // property not supported by this implementation — skip
    }
  }

  private static void trySetFeature(SAXParserFactory spf, String name, boolean value) {
    try {
      spf.setFeature(name, value);
    } catch (Exception ignored) {
    }
  }

  /**
   * Length-prefixed, domain-separated digest of the whole supplied schema set: root systemId,
   * sorted resource keys with per-entry content digests, and the sorted alias map. A changed child
   * or alias rotates the key.
   */
  static String schemaSetDigest(
      String rootSystemId, Map<String, byte[]> resources, Map<String, String> aliases) {
    try {
      var digest = MessageDigest.getInstance("SHA-256");
      digest.update(SCHEMA_SET_DOMAIN.getBytes(StandardCharsets.UTF_8));
      updateLengthPrefixed(digest, rootSystemId.getBytes(StandardCharsets.UTF_8));
      digest.update(longToBytes(resources.size()));
      for (var key : new TreeSet<>(resources.keySet())) {
        updateLengthPrefixed(digest, key.getBytes(StandardCharsets.UTF_8));
        digest.update(sha256Bytes(resources.get(key)));
      }
      digest.update(longToBytes(aliases.size()));
      for (var key : new TreeSet<>(aliases.keySet())) {
        updateLengthPrefixed(digest, key.getBytes(StandardCharsets.UTF_8));
        updateLengthPrefixed(digest, aliases.get(key).getBytes(StandardCharsets.UTF_8));
      }
      return "xsd-" + HexFormat.of().formatHex(digest.digest());
    } catch (NoSuchAlgorithmException e) {
      throw new IllegalStateException("SHA-256 unavailable", e);
    }
  }

  static String sha256Hex(byte[] bytes) {
    return HexFormat.of().formatHex(sha256Bytes(bytes));
  }

  private static void updateLengthPrefixed(MessageDigest digest, byte[] bytes) {
    digest.update(longToBytes(bytes.length));
    digest.update(bytes);
  }

  private static byte[] longToBytes(long value) {
    return ByteBuffer.allocate(Long.BYTES).putLong(value).array();
  }

  private static byte[] sha256Bytes(byte[] bytes) {
    try {
      return MessageDigest.getInstance("SHA-256").digest(bytes);
    } catch (NoSuchAlgorithmException e) {
      throw new IllegalStateException("SHA-256 unavailable", e);
    }
  }

  /**
   * Normalizes a literal root-relative virtual path: drops {@code .}/empty segments, applies {@code
   * ..}, and refuses anything that escapes the virtual root or names no file. Characters such as
   * {@code #} and {@code ?} are filename data here, not URI delimiters.
   */
  private static String normalizeVirtual(String raw) {
    if (raw == null || raw.isEmpty()) {
      throw new ResourceRefusedException("virtual path is empty");
    }
    rejectBackslash(raw);
    if (raw.startsWith("/")) {
      throw new ResourceRefusedException("virtual path is absolute");
    }
    var segments = new ArrayList<String>();
    for (var segment : raw.split("/", -1)) {
      switch (segment) {
        case "", "." -> {}
        case ".." -> {
          if (segments.isEmpty()) {
            throw new ResourceRefusedException("virtual path escapes the virtual root");
          }
          segments.remove(segments.size() - 1);
        }
        default -> segments.add(segment);
      }
    }
    if (segments.isEmpty()) {
      throw new ResourceRefusedException("virtual path does not name a file");
    }
    return String.join("/", segments);
  }

  private static void rejectBackslash(String raw) {
    if (raw.indexOf('\\') >= 0) {
      throw new ResourceRefusedException("reference contains a refused separator");
    }
  }

  /**
   * Percent-decodes a relative URI reference exactly once.
   *
   * <p>Unlike form decoding, a literal {@code +} is preserved (it is never a space). Non-ASCII
   * literals pass through as their UTF-8 bytes, so {@code %XX} escapes decode to UTF-8 exactly as
   * the Rust discovery path does. A malformed escape or an invalid UTF-8 sequence is refused; a
   * {@code %25} decodes to a literal {@code %} and is not decoded a second time.
   */
  private static String decodeUriReferenceOnce(String raw) {
    var out = new ByteArrayOutputStream(raw.length());
    for (int i = 0; i < raw.length(); ) {
      var c = raw.charAt(i);
      if (c == '%') {
        if (i + 2 >= raw.length()) {
          throw new ResourceRefusedException("reference has a truncated percent escape");
        }
        var hi = hexVal(raw.charAt(i + 1));
        var lo = hexVal(raw.charAt(i + 2));
        if (hi < 0 || lo < 0) {
          throw new ResourceRefusedException("reference has a malformed percent escape");
        }
        out.write((hi << 4) | lo);
        i += 3;
      } else {
        var codePoint = raw.codePointAt(i);
        out.writeBytes(new String(Character.toChars(codePoint)).getBytes(StandardCharsets.UTF_8));
        i += Character.charCount(codePoint);
      }
    }
    try {
      return StandardCharsets.UTF_8
          .newDecoder()
          .onMalformedInput(CodingErrorAction.REPORT)
          .onUnmappableCharacter(CodingErrorAction.REPORT)
          .decode(ByteBuffer.wrap(out.toByteArray()))
          .toString();
    } catch (CharacterCodingException e) {
      throw new ResourceRefusedException("reference is not valid UTF-8 after percent-decoding");
    }
  }

  private static int hexVal(char c) {
    if (c >= '0' && c <= '9') {
      return c - '0';
    }
    if (c >= 'a' && c <= 'f') {
      return c - 'a' + 10;
    }
    if (c >= 'A' && c <= 'F') {
      return c - 'A' + 10;
    }
    return -1;
  }

  private static String directoryOf(String key) {
    var idx = key.lastIndexOf('/');
    return idx < 0 ? "" : key.substring(0, idx);
  }

  /**
   * RFC 3986 absolute-URI test: {@code scheme ":" ...} with a well-formed scheme. Any scheme
   * counts; the alias map decides whether it is supplied.
   */
  private static boolean isAbsoluteUri(String value) {
    if (value == null) {
      return false;
    }
    var colon = value.indexOf(':');
    if (colon <= 0) {
      return false;
    }
    if (!isAsciiAlpha(value.charAt(0))) {
      return false;
    }
    for (int i = 1; i < colon; i++) {
      var c = value.charAt(i);
      if (!(isAsciiAlpha(c) || (c >= '0' && c <= '9') || c == '+' || c == '-' || c == '.')) {
        return false;
      }
    }
    return true;
  }

  private static boolean isAsciiAlpha(char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
  }

  /** A decoded supplied schema set, validated and ready to compile. */
  record SuppliedSet(
      String rootSystemId,
      byte[] rootBytes,
      Map<String, byte[]> resources,
      Map<String, String> aliases) {}

  /** Bounded validation failure for a decoded set (caps, aliases, root). */
  static final class SchemaSetException extends RuntimeException {
    private final BridgeError.Kind kind;
    private final String detail;

    SchemaSetException(BridgeError.Kind kind, String message) {
      this(kind, message, null);
    }

    SchemaSetException(BridgeError.Kind kind, String message, String detail) {
      super(message);
      this.kind = kind;
      this.detail = detail;
    }

    BridgeError.Kind kind() {
      return kind;
    }

    String detail() {
      return detail;
    }
  }

  /** Resolver refusal; unchecked so {@code newSchema} aborts without fetching. */
  static final class ResourceRefusedException extends RuntimeException {
    ResourceRefusedException(String message) {
      super(message);
    }
  }

  /**
   * Resolves supplied virtual keys only. Pushes a dedicated unchecked exception on any refusal so
   * {@code newSchema} aborts instead of falling back to a fetch; the fallback is caught by the
   * service and turned into a bounded bridge error.
   */
  private static final class SuppliedKeyResolver implements LSResourceResolver {
    private final Map<String, byte[]> resources;
    private final Map<String, String> aliases;

    SuppliedKeyResolver(Map<String, byte[]> resources, Map<String, String> aliases) {
      this.resources = resources;
      this.aliases = aliases;
    }

    @Override
    public LSInput resolveResource(
        String type, String namespaceURI, String publicId, String systemId, String baseURI) {
      if (!W3C_XML_SCHEMA_NS.equals(type)) {
        throw new ResourceRefusedException("refusing non-schema resource type");
      }
      if (systemId == null) {
        // An import without a location: nothing to resolve, and returning null
        // here is the only permitted null.
        return null;
      }

      String key;
      if (isAbsoluteUri(systemId)) {
        // Precedence 2: an absolute URI resolves only from the supplied alias
        // map, by exact raw key match (no percent-decoding). Never a file or
        // network read.
        var mapped = aliases.get(systemId);
        if (mapped == null) {
          throw new ResourceRefusedException("unsupplied external reference");
        }
        key = mapped;
      } else {
        // Raw URI delimiters are refused; percent-escaped delimiters are filename data.
        if (systemId.indexOf('?') >= 0 || systemId.indexOf('#') >= 0) {
          throw new ResourceRefusedException("reference contains a refused separator");
        }
        // A relative reference is percent-decoded exactly once (strict UTF-8,
        // literal '+' preserved). An encoded absolute URI or absolute path is
        // rejected before any join: an obfuscated URL or leading slash must not
        // become an importer-relative path or a fetch.
        var decoded = decodeUriReferenceOnce(systemId);
        rejectBackslash(decoded);
        if (isAbsoluteUri(decoded) || decoded.startsWith("/")) {
          throw new ResourceRefusedException(
              "relative reference resolves to an absolute or escaping path");
        }
        var importerKey = importerKeyFromBase(baseURI);
        if (importerKey == null) {
          // Precedence 3: no importing base, so only an exact root raw key is
          // eligible. This distinguished root reference never shadows a
          // base-relative key.
          key = decoded;
        } else {
          // Precedence 1: base-relative normalization wins over an exact raw
          // key. A nested importer a/parent.xsd targeting child.xsd resolves
          // a/child.xsd, never a root-level child.xsd.
          var dir = directoryOf(importerKey);
          key = normalizeVirtual(dir.isEmpty() ? decoded : dir + "/" + decoded);
        }
      }

      var bytes = resources.get(key);
      if (bytes == null) {
        throw new ResourceRefusedException("no supplied schema for the requested reference");
      }

      var input = new ByteStreamLsInput();
      input.setByteStream(new ByteArrayInputStream(bytes));
      // systemId is the absolute anchor + normalized virtual key; baseURI stays
      // UNSET so Xerces re-expands the key from the single process anchor and
      // never applies the base twice (a/a/child.xsd, nested/a/child.xsd).
      input.setSystemId(anchorUri(key).toString());
      return input;
    }

    /**
     * Derives the importing schema's normalized root-relative key from the expanded {@code file:}
     * base Xerces hands the resolver. The base carries our encoded URI, so {@code getPath()}
     * decodes exactly once and the resulting key is used literally (no extra percent-decoding). A
     * base outside the anchor, with an authority/query/fragment, or not a supplied schema is
     * refused.
     */
    private String importerKeyFromBase(String baseURI) {
      if (baseURI == null || baseURI.isEmpty()) {
        return null;
      }
      URI base;
      try {
        base = new URI(baseURI);
      } catch (URISyntaxException e) {
        throw new ResourceRefusedException("importer base is not a valid URI");
      }
      if (!"file".equalsIgnoreCase(base.getScheme())) {
        throw new ResourceRefusedException("importer base is not a file URI");
      }
      if (base.getRawAuthority() != null && !base.getRawAuthority().isEmpty()) {
        throw new ResourceRefusedException("importer base carries an authority");
      }
      if (base.getRawQuery() != null || base.getRawFragment() != null) {
        throw new ResourceRefusedException("importer base carries a query or fragment");
      }
      var path = base.getPath();
      var anchorPath = VIRTUAL_ANCHOR.getPath();
      if (path == null || !path.startsWith(anchorPath)) {
        throw new ResourceRefusedException("importer base is outside the virtual root");
      }
      var key = path.substring(anchorPath.length());
      if (key.isEmpty() || !resources.containsKey(key)) {
        throw new ResourceRefusedException("importer base is not a supplied schema");
      }
      return key;
    }
  }

  /** Minimal byte-stream-only {@link LSInput}; baseURI is intentionally unset. */
  private static final class ByteStreamLsInput implements LSInput {
    private InputStream byteStream;
    private String systemId;

    @Override
    public Reader getCharacterStream() {
      return null;
    }

    @Override
    public void setCharacterStream(Reader characterStream) {}

    @Override
    public InputStream getByteStream() {
      return byteStream;
    }

    @Override
    public void setByteStream(InputStream byteStream) {
      this.byteStream = byteStream;
    }

    @Override
    public String getStringData() {
      return null;
    }

    @Override
    public void setStringData(String stringData) {}

    @Override
    public String getSystemId() {
      return systemId;
    }

    @Override
    public void setSystemId(String systemId) {
      this.systemId = systemId;
    }

    @Override
    public String getPublicId() {
      return null;
    }

    @Override
    public void setPublicId(String publicId) {}

    @Override
    public String getBaseURI() {
      return null;
    }

    @Override
    public void setBaseURI(String baseURI) {}

    @Override
    public String getEncoding() {
      return null;
    }

    @Override
    public void setEncoding(String encoding) {}

    @Override
    public boolean getCertifiedText() {
      return false;
    }

    @Override
    public void setCertifiedText(boolean certifiedText) {}
  }

  /**
   * Turns any parser error/fatalError into a thrown exception so diagnostics never reach the
   * default handler's stderr stream.
   */
  private static final class ThrowingErrorHandler implements ErrorHandler {
    @Override
    public void warning(SAXParseException exception) {
      // warnings are not failures
    }

    @Override
    public void error(SAXParseException exception) throws SAXException {
      throw exception;
    }

    @Override
    public void fatalError(SAXParseException exception) throws SAXException {
      throw exception;
    }
  }
}
