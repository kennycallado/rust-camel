package org.rustcamel.cxf;

import com.google.protobuf.ByteString;
import cxf_bridge.ConsumerRequest;
import cxf_bridge.ConsumerResponse;
import io.quarkus.runtime.StartupEvent;
import io.vertx.core.Vertx;
import io.vertx.core.buffer.Buffer;
import io.vertx.core.http.ClientAuth;
import io.vertx.core.http.HttpMethod;
import io.vertx.core.http.HttpServer;
import io.vertx.core.http.HttpServerOptions;
import io.vertx.core.http.HttpServerRequest;
import io.vertx.core.net.PemKeyCertOptions;
import io.vertx.core.net.PemTrustOptions;
import io.vertx.core.net.PfxOptions;
import jakarta.annotation.PreDestroy;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import jakarta.inject.Inject;
import java.net.URI;
import java.util.Map;
import java.util.OptionalInt;
import java.util.UUID;
import java.util.concurrent.TimeUnit;
import java.util.logging.Level;
import java.util.logging.Logger;
import javax.xml.parsers.ParserConfigurationException;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.xml.sax.SAXParseException;

@ApplicationScoped
public class SoapEndpointPublisher {
  private static final Logger LOG = Logger.getLogger(SoapEndpointPublisher.class.getName());
  private static final String DEFAULT_ADDRESS = "http://0.0.0.0:9000/cxf";

  static final String SOAP11_NS = "http://schemas.xmlsoap.org/soap/envelope/";
  static final String SOAP12_NS = "http://www.w3.org/2003/05/soap-envelope";
  static final String SOAP_VERSION_11 = "1.1";
  static final String SOAP_VERSION_12 = "1.2";

  /** The five top-level SOAP 1.2 fault codes; no other code is valid in that version. */
  private static final java.util.Set<String> SOAP12_TOP_LEVEL_CODES =
      java.util.Set.of(
          "Sender", "Receiver", "VersionMismatch", "MustUnderstand", "DataEncodingUnknown");

  private static final String BODY_LIMIT_REJECT_MESSAGE = "request body exceeds CXF_MAX_BODY_BYTES";

  /** Fault class from the fixed matrix: Sender maps to 400 on SOAP 1.2, Receiver to 500. */
  enum FaultClass {
    SENDER,
    RECEIVER
  }

  /**
   * One request's outcome, produced on the blocking worker and applied on the event loop: the HTTP
   * status, the detected SOAP version (for a version-matched content type), and the response body.
   * A fault never carries status 200.
   */
  record SoapHttpResult(int status, String soapVersion, String xml) {}

  /** Keeps response construction/signing failures distinct from inbound validation failures. */
  private static final class OutboundResponseException extends Exception {
    private static final long serialVersionUID = 1L;

    OutboundResponseException(Exception cause) {
      super("SOAP response processing failed", cause);
    }
  }

  /**
   * Thrown when the request envelope parses but has no SOAP {@code Body}. Detected before WSS so a
   * profile's {@code fault_status} override can never rewrite it: a missing Body is a Sender fault.
   */
  private static final class MissingSoapBodyException extends Exception {
    private static final long serialVersionUID = 1L;

    MissingSoapBodyException(String message) {
      super(message);
    }
  }

  @Inject BridgeConfig bridgeConfig;

  @Inject SecurityProfileStore profileStore;

  @Inject CxfServerManager cxfServerManager;

  @Inject Vertx vertx;

  private HttpServer server;

  /** The actual bound SOAP port from the {@link HttpServer} listen result; empty until bound. */
  private volatile OptionalInt soapPort = OptionalInt.empty();

  /**
   * Test seam: pins the request-body cap deterministically. {@code -1} (production) lets each
   * request resolve the cap from {@link #maxBodyBytes()} at request time.
   */
  long pinnedMaxBodyBytes = -1;

  /**
   * The actual bound SOAP port (from {@code HttpServer.actualPort()} after a successful listen), or
   * empty when no listener is bound. Never a reserve-bind-drop probe: the port is held by this
   * server from the first moment the number exists.
   */
  public OptionalInt soapPort() {
    return soapPort;
  }

  /**
   * Startup publication (Task 2.7): publish the configured SOAP listener synchronously from the
   * Quarkus {@link StartupEvent}, BEFORE any gRPC {@code ready}/{@code soap-ready} announcement and
   * independent of any consumer stream. Publishing from {@code
   * CxfServerManager.setConsumerRequestObserver} would make {@code soap-ready} wait for {@code
   * OpenConsumerStream}, a circular wait against {@code BridgeProcess::start}. When no bind is
   * configured (producer-only pool), nothing binds — no implicit default port.
   */
  void onStart(@Observes StartupEvent ev) {
    String configuredAddress = bridgeConfig.address();
    if (configuredAddress != null && !configuredAddress.isBlank()) {
      publish();
    } else {
      // No bind (producer-only pool): nothing binds — no implicit default port — but listener
      // TLS material must still fail closed at startup (Task 2.6 / F3). Validation otherwise
      // lived only inside publish(), so a no-bind startup with an incomplete or inapplicable
      // soap_tls_* shape would silently start.
      validateConfiguredListenerTls();
    }
  }

  private long resolveMaxBodyBytes() {
    return pinnedMaxBodyBytes > 0 ? pinnedMaxBodyBytes : maxBodyBytes();
  }

  /**
   * One WSS processor per security profile: keeps Crypto instances and the inbound replay cache
   * shared across requests of the same profile.
   */
  private final java.util.concurrent.ConcurrentHashMap<String, WssSecurityProcessor>
      wssProcessorsByProfile = new java.util.concurrent.ConcurrentHashMap<>();

  WssSecurityProcessor wssProcessorFor(String profileName, SecurityProfile profile) {
    return wssProcessorsByProfile.computeIfAbsent(
        profileName, n -> new WssSecurityProcessor(profile));
  }

  synchronized void publish() {
    if (server != null) {
      return;
    }

    // Fail-closed listener TLS shape (cxf-listener-tls): validated against the CONFIGURED
    // address (no address = no bind) before any socket binds, mirroring the Rust-side
    // CxfPoolConfig::validate so a bad shape dies at startup, not at first handshake.
    // Listener TLS never reuses the WSS message-layer stores.
    validateConfiguredListenerTls();

    String configuredAddress = bridgeConfig.address();
    String address =
        configuredAddress != null && !configuredAddress.isBlank()
            ? configuredAddress
            : DEFAULT_ADDRESS;
    URI uri;
    try {
      uri = URI.create(address);
    } catch (IllegalArgumentException e) {
      throw new IllegalStateException("CXF_ADDRESS invalid: " + address, e);
    }
    validateAddressScheme(uri);

    // Startup validation (ADR-0033): a malformed CXF_MAX_BODY_BYTES must fail loud before any
    // socket binds; requests resolve the cap again at request time.
    maxBodyBytes();

    try {
      String host = uri.getHost();
      int port = uri.getPort();
      if (host == null || host.isBlank()) {
        throw new IllegalStateException("CXF address must include host: " + address);
      }
      if (port < 0) {
        throw new IllegalStateException("CXF address must include explicit port: " + address);
      }

      HttpServerOptions options = listenerOptions(uri, address);
      HttpServer httpServer = vertx.createHttpServer(options);
      httpServer.requestHandler(
          req -> {
            String profileName = extractProfileName(req.path());
            if (profileName == null) {
              req.response().setStatusCode(404).end();
              return;
            }

            SecurityProfile profile;
            try {
              profile = profileStore.getProfile(profileName);
            } catch (IllegalArgumentException e) {
              req.response().setStatusCode(404).end();
              return;
            }

            if (req.method() == HttpMethod.GET || req.method() == HttpMethod.HEAD) {
              req.response().setStatusCode(200).putHeader("content-type", "text/plain").end("ok");
              return;
            }

            if (req.method() != HttpMethod.POST) {
              req.response().setStatusCode(405).end();
              return;
            }

            WssSecurityProcessor wssProcessor = wssProcessorFor(profileName, profile);

            // Upfront gate: a declared Content-Length over the cap never enters aggregation.
            long maxBodyBytes = resolveMaxBodyBytes();
            String declaredLength = req.getHeader("Content-Length");
            if (declaredLength != null && !declaredLength.isBlank()) {
              try {
                if (Long.parseLong(declaredLength.trim()) > maxBodyBytes) {
                  LOG.log(
                      Level.WARNING,
                      "Rejecting request with declared {0} body bytes on {1}",
                      new Object[] {declaredLength.trim(), req.path()});
                  rejectBodyLimit(req);
                  return;
                }
              } catch (NumberFormatException e) {
                // Malformed header cannot be trusted; the mid-stream gate still protects us.
              }
            }

            // Bounded accumulator: chunk-by-chunk accounting aborts mid-stream once the
            // aggregate would pass the cap, so memory stays bounded even for lying
            // Content-Length headers. Vert.x drives one request's handlers sequentially on a
            // single context, so plain effectively-final captures are sufficient.
            final long[] received = {0};
            final boolean[] rejected = {false};
            final Buffer[] aggregate = {Buffer.buffer()};
            req.handler(
                chunk -> {
                  if (rejected[0]) {
                    return;
                  }
                  received[0] += chunk.length();
                  if (received[0] > maxBodyBytes) {
                    rejected[0] = true;
                    aggregate[0] = null;
                    req.pause();
                    LOG.log(
                        Level.WARNING,
                        "Request streamed past the CXF_MAX_BODY_BYTES cap ({0}); closing",
                        new Object[] {req.path()});
                    rejectBodyLimit(req);
                    return;
                  }
                  aggregate[0].appendBuffer(chunk);
                });
            req.endHandler(
                v -> {
                  if (!rejected[0]) {
                    handleRequestBody(req, wssProcessor, profileName, profile, aggregate[0]);
                  }
                });
          });
      java.util.concurrent.CountDownLatch latch = new java.util.concurrent.CountDownLatch(1);
      java.util.concurrent.atomic.AtomicReference<Throwable> listenError =
          new java.util.concurrent.atomic.AtomicReference<>();
      httpServer.listen(
          port,
          host,
          ar -> {
            if (ar.succeeded()) {
              server = httpServer;
              // The ACTUAL bound port (port 0 → OS-assigned): held by this server from the
              // first moment the number exists — no bind-read-drop window (ADR-0070).
              int boundPort = ar.result().actualPort();
              soapPort = boundPort > 0 ? OptionalInt.of(boundPort) : OptionalInt.empty();
            } else {
              listenError.set(ar.cause());
            }
            latch.countDown();
          });
      latch.await(bridgeConfig.connectionTimeoutMs(), TimeUnit.MILLISECONDS);
      if (listenError.get() != null) {
        throw new IllegalStateException("Failed to publish SOAP endpoint", listenError.get());
      }
      if (server == null) {
        throw new IllegalStateException("Timed out publishing SOAP endpoint");
      }

      LOG.log(
          Level.INFO,
          "SOAP endpoint published at {0} (bound port {1})",
          new Object[] {address, soapPort});
    } catch (Exception e) {
      LOG.log(Level.SEVERE, "SOAP endpoint publish failed", e);
      throw new IllegalStateException("Failed to publish SOAP endpoint", e);
    }
  }

  @PreDestroy
  void stop() {
    if (server != null) {
      HttpServer current = server;
      server = null;
      java.util.concurrent.CountDownLatch latch = new java.util.concurrent.CountDownLatch(1);
      current.close(ar -> latch.countDown());
      try {
        latch.await(2, TimeUnit.SECONDS);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
      }
      LOG.info("SOAP endpoint stopped");
    }
    soapPort = OptionalInt.empty();
  }

  /**
   * Listener TLS for the one Vert.x listener serving all profiles (cxf-listener-tls): {@code https}
   * enables SSL with {@code ClientAuth.REQUIRED} (mutual TLS, always) and exactly one identity —
   * PEM {@code cert}+{@code key} ({@link PemKeyCertOptions}) or PKCS#12 {@code path}+{@code
   * password} ({@link PfxOptions}) — plus {@link PemTrustOptions} for {@code
   * soap_tls_client_ca_path}. These pool keys are the ONLY material source: the WSS message-layer
   * stores are never reused here. Plaintext {@code http} binds without TLS (interim
   * ingress-termination path). Shape is pre-validated by {@link #validateListenerTls}.
   */
  private HttpServerOptions listenerOptions(URI uri, String address) {
    HttpServerOptions options = new HttpServerOptions();
    if (!"https".equalsIgnoreCase(uri.getScheme())) {
      return options;
    }
    options.setSsl(true);
    options.setClientAuth(ClientAuth.REQUIRED);
    if (isSet(bridgeConfig.soapTlsCertPath())) {
      options.setPemKeyCertOptions(
          new PemKeyCertOptions()
              .setCertPath(bridgeConfig.soapTlsCertPath())
              .setKeyPath(bridgeConfig.soapTlsKeyPath()));
    } else {
      options.setPfxKeyCertOptions(
          new PfxOptions()
              .setPath(bridgeConfig.soapTlsPkcs12Path())
              .setPassword(bridgeConfig.soapTlsPkcs12Password()));
    }
    options.setPemTrustOptions(
        new PemTrustOptions().addCertPath(bridgeConfig.soapTlsClientCaPath()));
    LOG.log(Level.FINE, "SOAP listener TLS enabled (client auth REQUIRED) for {0}", address);
    return options;
  }

  /**
   * Rejects listener schemes other than {@code http}/{@code https}. The TLS material interplay
   * (https needs material, material needs https) is validated by {@link #validateListenerTls}
   * before any socket binds.
   */
  static void validateAddressScheme(URI address) {
    String scheme = address.getScheme();
    if (scheme == null || !(scheme.equalsIgnoreCase("http") || scheme.equalsIgnoreCase("https"))) {
      throw new IllegalStateException(
          "CXF_ADDRESS scheme not supported: "
              + (scheme == null ? "(missing)" : scheme)
              + "; use http:// or https://");
    }
  }

  private static boolean isSet(String value) {
    return value != null && !value.isBlank();
  }

  /**
   * Runs the blank-value raw-env check plus {@link #validateListenerTls} against the current bridge
   * configuration, from the configured address. Shared by the startup observer (which validates
   * even with no bind) and {@link #publish} (which validates immediately before it binds), so the
   * two paths cannot drift.
   */
  private void validateConfiguredListenerTls() {
    rejectConfiguredBlankTlsEnv(System::getenv);
    validateListenerTls(
        bridgeConfig.address(),
        bridgeConfig.soapTlsCertPath(),
        bridgeConfig.soapTlsKeyPath(),
        bridgeConfig.soapTlsClientCaPath(),
        bridgeConfig.soapTlsPkcs12Path(),
        bridgeConfig.soapTlsPkcs12Password());
  }

  /** The five listener-TLS env vars — the raw operator / Rust-forwarding surface. */
  private static final String[] SOAP_TLS_ENV_KEYS = {
    "CXF_SOAP_TLS_CERT_PATH",
    "CXF_SOAP_TLS_KEY_PATH",
    "CXF_SOAP_TLS_CLIENT_CA_PATH",
    "CXF_SOAP_TLS_PKCS12_PATH",
    "CXF_SOAP_TLS_PKCS12_PASSWORD",
  };

  /**
   * N4 fail-closed blank check on the RAW configured env values. The {@code Optional<String>}
   * config injection erases a configured-empty ("") env value to {@code Optional.empty()} — so
   * presence of an explicit empty value is only observable at the raw env-var boundary (verified by
   * {@code SoapTlsConfiguredEmptyValueBoundaryTest}). Whitespace-only values survive the Optional
   * lookup and are rejected by {@link #validateListenerTls}; this check rejects the
   * configured-empty case and re-rejects any other blank for defense in depth.
   */
  static void rejectConfiguredBlankTlsEnv(java.util.function.Function<String, String> getenv) {
    for (String key : SOAP_TLS_ENV_KEYS) {
      String raw = getenv.apply(key);
      if (BlankTlsValue.isBlank(raw)) {
        throw new IllegalStateException(
            "CXF listener TLS: "
                + key
                + " is empty or whitespace-only; supply a nonblank value or remove the variable"
                + " (blank TLS values are rejected, not treated as unset)");
      }
    }
  }

  /**
   * Fail-closed listener TLS shape validation (pool keys {@code soap_tls_*}, env {@code
   * CXF_SOAP_TLS_*}), run before any socket binds. Mirrors the Rust-side {@code
   * CxfPoolConfig::validate}: exactly one key-material form (PEM {@code cert}+{@code key} XOR
   * PKCS12 {@code path}+{@code password}), no partial pairs in either direction, {@code client_ca}
   * always required (mutual TLS), TLS material only with an {@code https} bind, {@code https} only
   * with material, and TLS material without a configured bind is not applicable. Blank values are
   * invalid, not silently unset: an explicitly supplied empty or whitespace-only (Unicode {@code
   * White_Space}, see {@link BlankTlsValue}) value is rejected here and by the Rust-side validation
   * — only a genuinely absent value counts as unset. A configured-empty ("") env value that the
   * {@code Optional} config layer erased to {@code Optional.empty()} before this method is caught
   * by {@link #rejectConfiguredBlankTlsEnv} at the raw env boundary. Listener TLS material is
   * independent of the WSS message-layer stores and is never reused from them.
   */
  static void validateListenerTls(
      String configuredAddress,
      String certPath,
      String keyPath,
      String clientCaPath,
      String pkcs12Path,
      String pkcs12Password) {
    rejectBlankListenerTlsValue("soap_tls_cert_path", certPath);
    rejectBlankListenerTlsValue("soap_tls_key_path", keyPath);
    rejectBlankListenerTlsValue("soap_tls_client_ca_path", clientCaPath);
    rejectBlankListenerTlsValue("soap_tls_pkcs12_path", pkcs12Path);
    rejectBlankListenerTlsValue("soap_tls_pkcs12_password", pkcs12Password);
    boolean cert = certPath != null;
    boolean key = keyPath != null;
    boolean ca = clientCaPath != null;
    boolean pkcs12 = pkcs12Path != null;
    boolean pkcs12Pw = pkcs12Password != null;
    boolean any = cert || key || ca || pkcs12 || pkcs12Pw;
    boolean https =
        isSet(configuredAddress)
            && configuredAddress.trim().toLowerCase(java.util.Locale.ROOT).startsWith("https:");

    if (!any) {
      if (https) {
        throw new IllegalStateException(
            "CXF listener TLS: https bind requires key material — set soap_tls_cert_path +"
                + " soap_tls_key_path (PEM) or soap_tls_pkcs12_path + soap_tls_pkcs12_password"
                + " (PKCS#12)");
      }
      return;
    }
    if (cert != key) {
      throw new IllegalStateException(
          "CXF listener TLS: supply soap_tls_cert_path and soap_tls_key_path together (got "
              + (cert ? "cert only" : "key only")
              + ")");
    }
    if (pkcs12 != pkcs12Pw) {
      throw new IllegalStateException(
          "CXF listener TLS: supply soap_tls_pkcs12_path and soap_tls_pkcs12_password together (got"
              + (pkcs12 ? " path only" : " password only")
              + ")");
    }
    boolean pemForm = cert && key;
    boolean pkcs12Form = pkcs12 && pkcs12Pw;
    if (pemForm && pkcs12Form) {
      throw new IllegalStateException(
          "CXF listener TLS: supply exactly one key-material form — soap_tls_cert_path +"
              + " soap_tls_key_path (PEM) or soap_tls_pkcs12_path + soap_tls_pkcs12_password"
              + " (PKCS#12) — never both");
    }
    if (!pemForm && !pkcs12Form) {
      throw new IllegalStateException(
          "CXF listener TLS: supply exactly one key-material form — soap_tls_cert_path +"
              + " soap_tls_key_path (PEM) or soap_tls_pkcs12_path + soap_tls_pkcs12_password"
              + " (PKCS#12) — none is configured");
    }
    if (!ca) {
      throw new IllegalStateException(
          "CXF listener TLS: soap_tls_client_ca_path is always required (mutual TLS client"
              + " authentication)");
    }
    if (!isSet(configuredAddress)) {
      throw new IllegalStateException(
          "CXF listener TLS: soap_tls_* material is configured but bind_address is absent;"
              + " the listener TLS configuration is not applicable without a bind");
    }
    if (!https) {
      throw new IllegalStateException(
          "CXF listener TLS: bind_address must use https when soap_tls_* material is configured");
    }
  }

  /**
   * Rejects one explicitly supplied empty or whitespace-only (Unicode {@code White_Space}) listener
   * TLS value. The message names the pool key and never echoes the value (ADR-0051: the PKCS12
   * password is a credential).
   */
  private static void rejectBlankListenerTlsValue(String poolKey, String value) {
    if (BlankTlsValue.isBlank(value)) {
      throw new IllegalStateException(
          "CXF listener TLS: "
              + poolKey
              + " is empty or whitespace-only; supply a nonblank value or omit the key (blank"
              + " TLS values are rejected, not treated as unset)");
    }
  }

  /**
   * Blocked-pipeline processing of one fully accumulated request body: validate the envelope and
   * require a Body, WSS-in, extract, IPC call to the consumer, wrap, WSS-out, respond. Malformed
   * input and a missing Body are detected before WSS so a profile's security {@code fault_status}
   * override can never swallow them.
   */
  private void handleRequestBody(
      HttpServerRequest req,
      WssSecurityProcessor wssProcessor,
      String profileName,
      SecurityProfile profile,
      Buffer body) {
    // Transport-availability gate FIRST — before any XML parsing, SOAP version classification,
    // and before any WS-Security processing. Without a live consumer stream the routes are
    // unavailable: HTTP 503 + Retry-After + plaintext, never a SOAP fault (not the fault matrix's
    // 400/500) and never a hang. Unconditional and version-independent: a Signature-required
    // profile whose signature is missing or invalid still takes this 503. Transport body limits
    // were already enforced during accumulation (before this method runs).
    if (!cxfServerManager.hasConsumerObserver()) {
      respondTransportUnavailable(req);
      return;
    }
    String requestXml = body.toString(java.nio.charset.StandardCharsets.UTF_8);
    Map<String, String> headers =
        req.headers().entries().stream()
            .collect(
                java.util.stream.Collectors.toMap(
                    h -> h.getKey().toLowerCase(), Map.Entry::getValue, (a, b) -> a));
    // Provisional version from the content type, cheap and parsing-free on the event loop; the
    // blocking callable refines it from the envelope root. The failure branch reads the refined
    // value so it still renders the version-matched envelope, and the WSS processor receives the
    // detected version instead of re-detecting it.
    java.util.concurrent.atomic.AtomicReference<String> detectedVersion =
        new java.util.concurrent.atomic.AtomicReference<>(versionFromContentType(headers));

    vertx.executeBlocking(
        () -> {
          String soapVersion = detectSoapVersion(requestXml, headers);
          detectedVersion.set(soapVersion);

          // Envelope and Body validation before WSS: both are Sender faults that must not be
          // rewritten by a profile's security fault_status override.
          org.w3c.dom.Document requestDoc = SoapEnvelopeHelper.parseResponse(requestXml);
          if (!SoapEnvelopeHelper.hasBody(requestDoc)) {
            throw new MissingSoapBodyException("request envelope has no SOAP Body");
          }

          String securedXml = requestXml;
          java.util.Optional<cxf_bridge.SignerIdentity> signer = java.util.Optional.empty();
          if (wssProcessor.canVerifyInbound()) {
            WssSecurityProcessor.VerifiedInboundMessage verified =
                wssProcessor.processInbound(securedXml, soapVersion);
            securedXml = verified.xml();
            signer = verified.signer();
          }

          String requestBody =
              SoapEnvelopeHelper.extractBody(SoapEnvelopeHelper.parseResponse(securedXml));

          String soapAction = extractSoapAction(headers);

          ConsumerRequest.Builder consumerRequestBuilder =
              ConsumerRequest.newBuilder()
                  .setRequestId(UUID.randomUUID().toString())
                  .setOperation(soapAction)
                  .setPayload(ByteString.copyFromUtf8(requestBody))
                  .putAllHeaders(headers)
                  .setSoapAction(soapAction)
                  .setSecurityProfile(profileName);
          // Only a verified Body-covering signature may populate the field; when no signer is
          // present the field stays absent (unknown).
          if (signer.isPresent()) {
            consumerRequestBuilder.setSigner(signer.get());
          }
          ConsumerRequest consumerRequest = consumerRequestBuilder.build();

          ConsumerResponse response =
              cxfServerManager
                  .handleSoapRequest(consumerRequest)
                  .get(bridgeConfig.consumerTimeoutMs(), TimeUnit.MILLISECONDS);

          // Defense in depth: assert profile echo matches
          if (!profileName.equals(response.getSecurityProfile())) {
            LOG.log(
                Level.WARNING,
                "Security profile mismatch: expected={0}, got={1}",
                new Object[] {profileName, response.getSecurityProfile()});
          }

          try {
            String responseXml;
            int statusCode;
            if (response.getFault()) {
              // A Rust fault maps through the same matrix but is never a WSS fault, so the
              // per-profile override does not apply.
              FaultClass faultClass = faultClassForRustCode(response.getFaultCode());
              statusCode = defaultFaultStatus(faultClass, soapVersion);
              responseXml =
                  buildSoapFault(response.getFaultCode(), response.getFaultString(), soapVersion);
            } else {
              statusCode = 200;
              responseXml = wrapEnvelope(response.getPayload().toStringUtf8(), soapVersion);
            }

            // Sign with the resolved profile (defense in depth)
            if (wssProcessor.canSignOutbound()) {
              responseXml = wssProcessor.processOutbound(responseXml);
            }

            return new SoapHttpResult(statusCode, soapVersion, responseXml);
          } catch (Exception e) {
            throw new OutboundResponseException(e);
          }
        },
        ar -> {
          if (ar.succeeded()) {
            SoapHttpResult result = (SoapHttpResult) ar.result();
            req.response()
                .setStatusCode(result.status())
                .putHeader("content-type", contentTypeFor(result.soapVersion()))
                .end(result.xml());
          } else {
            Throwable cause = ar.cause();
            LOG.log(Level.SEVERE, "SOAP request processing failed", cause);

            // Late stream loss after the availability gate: the SAME HTTP 503
            // transport-availability response as the pre-stream gate — not the generic
            // Receiver 500 and never a SOAP fault envelope.
            if (hasInChain(cause, CxfServerManager.StreamUnavailableException.class)) {
              respondTransportUnavailable(req);
              return;
            }

            String soapVersion = detectedVersion.get();
            boolean isMalformedEnvelope =
                hasInChain(cause, SAXParseException.class, ParserConfigurationException.class);
            boolean isMissingBody = hasInChain(cause, MissingSoapBodyException.class);
            boolean isSecurityFailure = hasInChain(cause, WSSecurityException.class);
            boolean isTimeout = hasInChain(cause, java.util.concurrent.TimeoutException.class);

            FaultClass faultClass;
            boolean isWssFault = false;
            String faultCode;
            String faultString;
            if (hasInChain(cause, OutboundResponseException.class)) {
              // Never expose response payload/key diagnostics or retry signing this fault.
              faultClass = FaultClass.RECEIVER;
              faultCode = "soap:Server";
              faultString = "Internal server error";
            } else if (isMalformedEnvelope) {
              faultClass = FaultClass.SENDER;
              faultCode = "soap:Client";
              faultString = "malformed XML envelope: " + sanitize(cause.getMessage());
            } else if (isMissingBody) {
              faultClass = FaultClass.SENDER;
              faultCode = "soap:Client";
              faultString = "missing SOAP Body: " + sanitize(cause.getMessage());
            } else if (isSecurityFailure) {
              faultClass = FaultClass.SENDER;
              isWssFault = true;
              faultCode = "soap:Client";
              faultString = "WS-Security processing failed: " + sanitize(cause.getMessage());
            } else if (isTimeout) {
              faultClass = FaultClass.RECEIVER;
              faultCode = "soap:Server";
              faultString = "request timed out";
            } else {
              faultClass = FaultClass.RECEIVER;
              faultCode = "soap:Server";
              faultString = "Internal server error";
            }

            int statusCode = defaultFaultStatus(faultClass, soapVersion);
            if (isWssFault && profile.faultStatus() != null) {
              statusCode = profile.faultStatus();
            }

            req.response()
                .setStatusCode(statusCode)
                .putHeader("content-type", contentTypeFor(soapVersion))
                .end(buildSoapFault(faultCode, faultString, soapVersion));
          }
        });
  }

  /** Default matrix status: SOAP 1.1 any fault 500; SOAP 1.2 Sender 400, Receiver 500. */
  static int defaultFaultStatus(FaultClass faultClass, String soapVersion) {
    if (SOAP_VERSION_11.equals(soapVersion)) {
      return 500;
    }
    return faultClass == FaultClass.SENDER ? 400 : 500;
  }

  /** Rust fault-code classification: {@code soap:Client} Sender; everything else Receiver/500. */
  static FaultClass faultClassForRustCode(String faultCode) {
    String local = localName(faultCode);
    if (local.equalsIgnoreCase("Client") || local.equalsIgnoreCase("Sender")) {
      return FaultClass.SENDER;
    }
    return FaultClass.RECEIVER;
  }

  /** Response content type matching the detected SOAP version. */
  static String contentTypeFor(String soapVersion) {
    return SOAP_VERSION_12.equals(soapVersion)
        ? "application/soap+xml; charset=utf-8"
        : "text/xml; charset=utf-8";
  }

  /** Aborts a request past the body cap: HTTP 413 with a short plain-text reason. */
  private static void rejectBodyLimit(HttpServerRequest req) {
    req.response().setStatusCode(413).end(BODY_LIMIT_REJECT_MESSAGE);
    req.connection().close();
  }

  /**
   * HTTP 503 transport-availability response (Task 2.7): no consumer stream means the routes are
   * unavailable. Plaintext body, {@code Retry-After} header, and NEVER a SOAP fault envelope — this
   * is not part of the fault-class matrix (which maps Sender to 400 and Receiver to 500). The same
   * response serves the pre-stream gate and a stream lost after the gate.
   */
  private static void respondTransportUnavailable(HttpServerRequest req) {
    req.response()
        .setStatusCode(503)
        .putHeader("Retry-After", "1")
        .putHeader("content-type", "text/plain; charset=utf-8")
        .end("routes unavailable");
  }

  /**
   * Reads the listener request-body cap from {@code CXF_MAX_BODY_BYTES} in bytes. Fails loud on a
   * malformed or non-positive value so a typo never silently disables the cap.
   */
  static long maxBodyBytes() {
    return BridgeConfig.parseMaxBodyBytes();
  }

  /**
   * Parses one {@code CXF_MAX_BODY_BYTES} value, failing loud with the env name on malformed,
   * non-positive, or above-ceiling input.
   */
  static long parseCap(String raw) {
    return BridgeConfig.parseMaxBodyBytes(raw);
  }

  /**
   * Extracts profile name from URL path. Path format: /cxf/&lt;profile_name&gt;/... Returns null
   * for /cxf, /cxf/, non-matching paths, or null path.
   */
  static String extractProfileName(String path) {
    if (path == null) return null;
    if (!path.startsWith("/cxf")) return null;
    String after = path.substring("/cxf".length());
    if (after.isEmpty()) return null;
    if (!after.startsWith("/")) return null;
    String rest = after.substring(1);
    if (rest.isEmpty()) return null;
    int slash = rest.indexOf('/');
    String segment = slash >= 0 ? rest.substring(0, slash) : rest;
    return segment.isEmpty() ? null : segment;
  }

  private static String sanitize(String msg) {
    if (msg == null) return "unknown";
    return msg.length() > 200 ? msg.substring(0, 200) : msg;
  }

  /**
   * Renders a fault envelope whose namespace matches the detected SOAP version. SOAP 1.1 uses
   * {@code faultcode}/{@code faultstring} with a declared QName prefix; SOAP 1.2 uses {@code
   * Code}/{@code Value} and {@code Reason}/{@code Text} with {@code xml:lang}.
   */
  static String buildSoapFault(String faultCode, String faultString, String soapVersion) {
    String code = escapeXml(faultCodeFor(faultCode, soapVersion));
    String text = escapeXml(faultString == null ? "SOAP fault" : faultString);
    if (SOAP_VERSION_12.equals(soapVersion)) {
      return "<soapenv:Envelope xmlns:soapenv=\""
          + SOAP12_NS
          + "\"><soapenv:Header/><soapenv:Body><soapenv:Fault>"
          + "<soapenv:Code><soapenv:Value>"
          + code
          + "</soapenv:Value></soapenv:Code>"
          + "<soapenv:Reason><soapenv:Text xml:lang=\"en\">"
          + text
          + "</soapenv:Text></soapenv:Reason>"
          + "</soapenv:Fault></soapenv:Body></soapenv:Envelope>";
    }
    // SOAP 1.1: both the envelope prefix and the faultcode QName prefix are declared to the
    // SOAP 1.1 namespace, so the rendered fault never carries an undeclared prefix.
    return "<soapenv:Envelope xmlns:soapenv=\""
        + SOAP11_NS
        + "\" xmlns:soap=\""
        + SOAP11_NS
        + "\"><soapenv:Header/><soapenv:Body><soapenv:Fault>"
        + "<faultcode>"
        + code
        + "</faultcode>"
        + "<faultstring>"
        + text
        + "</faultstring>"
        + "</soapenv:Fault></soapenv:Body></soapenv:Envelope>";
  }

  /**
   * Maps a fault code to the detected version's namespace. SOAP 1.2 admits only the five top-level
   * codes; the legacy {@code Client}/{@code Server} aliases map to {@code Sender}/ {@code
   * Receiver}, and any unknown or unbound custom code falls back to {@code Receiver}. SOAP 1.1
   * preserves the local part, rebinding an undeclared prefix to the declared {@code soap} prefix so
   * the rendered QName is always namespace-correct.
   */
  static String faultCodeFor(String faultCode, String soapVersion) {
    if (SOAP_VERSION_12.equals(soapVersion)) {
      String local = localName(faultCode);
      if (local.isEmpty()) {
        return "soapenv:Receiver";
      }
      if (local.equalsIgnoreCase("Client")) {
        return "soapenv:Sender";
      }
      if (local.equalsIgnoreCase("Server")) {
        return "soapenv:Receiver";
      }
      for (String topLevel : SOAP12_TOP_LEVEL_CODES) {
        if (topLevel.equalsIgnoreCase(local)) {
          return "soapenv:" + topLevel;
        }
      }
      return "soapenv:Receiver";
    }
    String raw = faultCode == null ? "" : faultCode.trim();
    int colon = raw.lastIndexOf(':');
    String local = colon >= 0 ? raw.substring(colon + 1).trim() : raw;
    if (local.isEmpty()) {
      local = "Server";
    }
    String prefix = colon >= 0 ? raw.substring(0, colon).trim() : "";
    if (prefix.equals("soap") || prefix.equals("soapenv")) {
      return prefix + ":" + local;
    }
    return "soap:" + local;
  }

  private static String localName(String qname) {
    if (qname == null) {
      return "";
    }
    String raw = qname.trim();
    int colon = raw.lastIndexOf(':');
    return colon >= 0 ? raw.substring(colon + 1).trim() : raw;
  }

  private static String extractSoapAction(Map<String, String> headers) {
    String action = headers.getOrDefault("soapaction", "");
    if (action != null && !action.isBlank()) {
      return action.replace("\"", "").trim();
    }
    String contentType = headers.getOrDefault("content-type", "");
    int idx = contentType.toLowerCase().indexOf("action=");
    if (idx >= 0) {
      String tail = contentType.substring(idx + "action=".length()).trim();
      int semi = tail.indexOf(';');
      String raw = semi >= 0 ? tail.substring(0, semi) : tail;
      return raw.replace("\"", "").trim();
    }
    return "";
  }

  /**
   * Detects the request SOAP version from the envelope root namespace; an unreadable root falls
   * back to the content type ({@code application/soap+xml} selects 1.2, otherwise 1.1). Parsing
   * uses the secure, DTD-disabled helper, so untrusted input never triggers external resolution.
   */
  static String detectSoapVersion(String requestXml, Map<String, String> headers) {
    if (requestXml != null && !requestXml.isBlank()) {
      try {
        org.w3c.dom.Element root =
            SoapEnvelopeHelper.parseResponse(requestXml).getDocumentElement();
        if (root != null) {
          String namespace = root.getNamespaceURI();
          if (SOAP12_NS.equals(namespace)) {
            return SOAP_VERSION_12;
          }
          if (SOAP11_NS.equals(namespace)) {
            return SOAP_VERSION_11;
          }
        }
      } catch (Exception e) {
        // Unreadable root: fall through to the content-type fallback.
      }
    }
    return versionFromContentType(headers);
  }

  /** Content-type fallback: {@code application/soap+xml} selects 1.2, everything else 1.1. */
  static String versionFromContentType(Map<String, String> headers) {
    String contentType =
        headers == null ? "" : headers.getOrDefault("content-type", "").toLowerCase();
    return contentType.contains("application/soap+xml") ? SOAP_VERSION_12 : SOAP_VERSION_11;
  }

  private static String escapeXml(String value) {
    return value
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&apos;");
  }

  /** True when any {@link Throwable} in the cause chain is an instance of one of {@code types}. */
  private static boolean hasInChain(Throwable thrown, Class<?>... types) {
    java.util.Set<Throwable> seen =
        java.util.Collections.newSetFromMap(new java.util.IdentityHashMap<>());
    for (Throwable t = thrown; t != null && seen.add(t); t = t.getCause()) {
      for (Class<?> type : types) {
        if (type.isInstance(t)) {
          return true;
        }
      }
    }
    return false;
  }

  private static String wrapEnvelope(String xmlBody, String soapVersion) {
    String ns =
        "1.2".equals(soapVersion)
            ? "http://www.w3.org/2003/05/soap-envelope"
            : "http://schemas.xmlsoap.org/soap/envelope/";
    return "<soapenv:Envelope xmlns:soapenv=\""
        + ns
        + "\"><soapenv:Header/><soapenv:Body>"
        + (xmlBody == null ? "" : xmlBody)
        + "</soapenv:Body></soapenv:Envelope>";
  }
}
