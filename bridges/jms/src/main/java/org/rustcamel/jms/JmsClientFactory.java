package org.rustcamel.jms;

import jakarta.annotation.PreDestroy;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import java.io.IOException;
import java.net.URI;
import java.net.URISyntaxException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicBoolean;
import javax.jms.Connection;
import javax.jms.ConnectionFactory;
import javax.net.ssl.KeyManager;
import javax.net.ssl.TrustManager;
import org.apache.activemq.ActiveMQConnectionFactory;
import org.apache.activemq.artemis.api.core.TransportConfiguration;
import org.apache.activemq.artemis.core.remoting.impl.netty.NettyConnectorFactory;
import org.apache.activemq.artemis.core.remoting.impl.netty.TransportConstants;
import org.apache.activemq.openwire.OpenWireFormatFactory;
import org.apache.activemq.pool.PooledConnectionFactory;
import org.apache.activemq.transport.TransportFactory;
import org.apache.activemq.transport.tcp.TcpTransportFactory;
import org.apache.activemq.util.FactoryFinder;
import org.apache.activemq.util.URISupport;
import org.jboss.logging.Logger;
import org.messaginghub.pooled.jms.JmsPoolConnectionFactory;

@ApplicationScoped
public class JmsClientFactory {
  @Inject BridgeConfig config;

  private static final Logger LOG = Logger.getLogger(JmsClientFactory.class);

  // Broker-facing TLS material contract (PKCS12, operator-provided). Distinct
  // from the IPC mTLS PEM pair used for gRPC transport security.
  private static final String KEYSTORE_PATH_ENV = "BRIDGE_BROKER_KEYSTORE_PATH";
  private static final String TRUSTSTORE_PATH_ENV = "BRIDGE_BROKER_TRUSTSTORE_PATH";
  private static final String KEYSTORE_PASSWORD_ENV = "BRIDGE_BROKER_KEYSTORE_PASSWORD";

  // Broker-facing TLS PEM material contract (task 5.1): CA bundle, optional client identity pair,
  // and the broker identity name for SNI/endpoint-identification binding.
  private static final String BROKER_TLS_CA_CERT_PATH_ENV = "BRIDGE_BROKER_TLS_CA_CERT_PATH";
  private static final String BROKER_TLS_CLIENT_CERT_PATH_ENV =
      "BRIDGE_BROKER_TLS_CLIENT_CERT_PATH";
  private static final String BROKER_TLS_CLIENT_KEY_PATH_ENV = "BRIDGE_BROKER_TLS_CLIENT_KEY_PATH";
  private static final String BROKER_TLS_SERVER_NAME_ENV = "BRIDGE_BROKER_TLS_SERVER_NAME";

  private static final AtomicBoolean NATIVE_INIT_DONE = new AtomicBoolean(false);
  private static final AtomicBoolean TRANSPORTS_REGISTERED = new AtomicBoolean(false);

  /**
   * Native-safe FactoryFinder service map. Promoted from the local in {@link
   * #initNativeImageWorkarounds} (identical contents, behavior-preserving) so tests can read it
   * through {@link #knownServices()}.
   */
  private static final Map<String, java.util.function.Supplier<Object>> KNOWN_SERVICES =
      new ConcurrentHashMap<>();

  /**
   * Broker-URL redaction key substrings, mirroring Rust {@code BROKER_SENSITIVE_KEY_SUBSTRINGS}
   * ({@code crates/components/camel-jms/src/config.rs}): a query pair whose lowercased key contains
   * any of these renders as {@code key=<redacted>}.
   */
  private static final String[] REDACTED_QUERY_KEY_SUBSTRINGS = {
    "password", "passwd", "secret", "credential", "token", "username", "user"
  };

  private volatile ConnectionFactory factory;
  // Raw (non-pooled) factory kept for long-lived consumer connections.
  // The pool silently recycles connections after idle timeout, breaking
  // MessageListeners without any error. Consumers use this directly.
  private volatile ConnectionFactory rawFactory;

  private volatile JmsBrokerProbe brokerProbe;
  private volatile Runnable resetHook;

  public ConnectionFactory get() {
    if (factory == null) {
      synchronized (this) {
        if (factory == null) {
          factory = createFactory();
        }
      }
    }
    return factory;
  }

  /**
   * Lazily creates the single retained broker probe. Constructing it performs no broker I/O; the
   * first {@code probe(...)} call opens the provider diagnostic.
   *
   * @return the process-wide retained probe
   */
  JmsBrokerProbe healthProbe() {
    JmsBrokerProbe probe = brokerProbe;
    if (probe == null) {
      synchronized (this) {
        probe = brokerProbe;
        if (probe == null) {
          probe = new JmsBrokerProbe(config);
          brokerProbe = probe;
        }
      }
    }
    return probe;
  }

  /**
   * Creates a JMS connection from the pool.
   *
   * <p>Credentials are configured on the underlying factory (ActiveMQConnectionFactory for Artemis,
   * ActiveMQConnectionFactory for Classic) via setUser/setPassword or setUserName/setPassword. The
   * pool propagates them automatically when it creates new physical connections — no need to pass
   * them here.
   *
   * <p>Passing credentials to JmsPoolConnectionFactory.createConnection(user, pass) creates a
   * separate pool bucket keyed by (user, pass), which can cause pool exhaustion and deadlocks under
   * concurrent access in GraalVM native image.
   */
  public Connection createConnection() throws javax.jms.JMSException {
    return get().createConnection();
  }

  /**
   * Creates a dedicated (non-pooled) connection for long-lived consumers. The pool silently
   * recycles idle connections, breaking MessageListeners.
   */
  public Connection createDedicatedConnection() throws javax.jms.JMSException {
    if (rawFactory == null) {
      get(); // ensure createFactory() has run and rawFactory is set
    }
    return rawFactory.createConnection();
  }

  /** Registers the non-blocking hook that invalidates cached broker status before a pool reset. */
  void setResetHook(Runnable hook) {
    resetHook = hook;
  }

  public void reset() {
    Runnable hook = resetHook;
    if (hook != null) {
      try {
        // Run outside the factory lock: the hook only invalidates the cache generation and never
        // performs provider I/O or calls reset() again.
        hook.run();
      } catch (RuntimeException ignored) {
      }
    }
    synchronized (this) {
      if (factory instanceof PooledConnectionFactory pool) {
        try {
          pool.stop();
        } catch (Exception ignored) {
        }
      } else if (factory instanceof JmsPoolConnectionFactory pool) {
        try {
          pool.stop();
        } catch (Exception ignored) {
        }
      }
      factory = null;
    }
  }

  private ConnectionFactory createFactory() {
    String type = config.brokerType();
    String url = config.brokerUrl();
    String user = config.username();
    String pass = config.password();

    switch (type) {
      case "activemq":
        {
          ActiveMQConnectionFactory cf = buildActiveMqFactory(url, user, pass);
          rawFactory = cf;
          return buildPooledActiveMqFactory(cf, config);
        }
      case "artemis":
        {
          var cf = buildArtemisFactory(url, user, pass);
          rawFactory = cf;
          return buildPooledArtemisFactory(cf, config);
        }
      default:
        {
          throw new IllegalArgumentException(
              "Unsupported broker_type: '" + type + "'. Valid values: activemq, artemis");
        }
    }
  }

  // Package-private (not private) testability seam mirroring buildActiveMqFactory: constructs and
  // starts the Classic producer pool with the validated pool knobs. Setters run before start().
  static PooledConnectionFactory buildPooledActiveMqFactory(
      ActiveMQConnectionFactory cf, BridgeConfig config) {
    PooledConnectionFactory pool = new PooledConnectionFactory(cf);
    pool.setMaxConnections(5);
    pool.setIdleTimeout((int) config.poolIdleTimeoutMs());
    pool.setTimeBetweenExpirationCheckMillis(config.poolExpirationCheckMs());
    pool.start();
    return pool;
  }

  // Package-private (not private) testability seam for the Artemis broker branch. The pooled-jms
  // setters differ from the Classic PooledConnectionFactory names. Setters run before start().
  static JmsPoolConnectionFactory buildPooledArtemisFactory(
      ConnectionFactory cf, BridgeConfig config) {
    JmsPoolConnectionFactory pool = new JmsPoolConnectionFactory();
    pool.setConnectionFactory(cf);
    pool.setMaxConnections(5);
    pool.setConnectionIdleTimeout((int) config.poolIdleTimeoutMs());
    pool.setConnectionCheckInterval(config.poolExpirationCheckMs());
    pool.start();
    return pool;
  }

  // Package-private (not private) so JmsClientFactoryPrefixTest can exercise the effective prefix;
  // mirrors the existing transportConfig testability seam.
  static ActiveMQConnectionFactory buildActiveMqFactory(String url, String user, String pass) {
    return buildActiveMqFactory(url, user, pass, readBrokerTlsEnv());
  }

  /**
   * Factory selection by URL transport security (task 5.1), with explicit TLS env values as the
   * testability seam ({@link System#getenv} is immutable in tests). When the URL — composite
   * members included — has an {@code ssl} member the result is always a {@link
   * BridgeActiveMqConnectionFactory}, in pinned-material mode when TLS env is present (CA or client
   * pair or both) and in platform-trust mode otherwise; that single class is what makes the
   * composite downgrade guard class-wide. Anything else keeps the plain {@link
   * ActiveMQConnectionFactory} exactly as before.
   */
  static ActiveMQConnectionFactory buildActiveMqFactory(
      String url, String user, String pass, BrokerTlsEnv tlsEnv) {
    rejectTlsStoreUrlOptions(url);
    initNativeImageWorkarounds();
    verifySchemesResolvable(url);
    ActiveMQConnectionFactory cf;
    if (urlHasSslMember(url)) {
      KeyManager[] km = null;
      TrustManager[] tm = null;
      if (tlsEnv.anyPresent()) {
        // TM from the CA path, or the platform default trust store when the CA path is null;
        // KM only when the client cert+key pair is present.
        tm = BrokerTlsMaterial.buildTrustManagers(tlsEnv.caCertPath());
        if (tlsEnv.hasClientPair()) {
          km = BrokerTlsMaterial.buildKeyManagers(tlsEnv.clientCertPath(), tlsEnv.clientKeyPath());
        } else if (tlsEnv.clientCertPath() != null || tlsEnv.clientKeyPath() != null) {
          throw new IllegalStateException(
              "TLS client identity needs both "
                  + BROKER_TLS_CLIENT_CERT_PATH_ENV
                  + " and "
                  + BROKER_TLS_CLIENT_KEY_PATH_ENV);
        }
        LOG.info("broker TLS material configured (pinned PEM material)");
      } else {
        LOG.info(
            "broker TLS env not set: platform default trust, no client identity"
                + " (composite downgrade guard still active)");
      }
      cf = new BridgeActiveMqConnectionFactory(url, km, tm, tlsEnv.serverName());
    } else {
      cf = new ActiveMQConnectionFactory(url);
    }
    // Keep connection and client IDs unique across processes even if the native-image IdGenerator
    // run-time initialization setting is lost (rc-trxge). An explicit jms.connectionIDPrefix /
    // jms.clientIDPrefix is preserved as the base and the per-process token is appended; without
    // one the token stands alone. ActiveMQ 5.19.x has no getConnectionIDPrefix() getter (the
    // client getter does exist), so the explicit connection option is read from the URI with
    // ActiveMQ's own parser while the client base comes from the factory. Direct setters avoid
    // native-image reflection requirements.
    cf.setConnectionIDPrefix(withProcessToken(explicitUriOption(url, "jms.connectionIDPrefix")));
    cf.setClientIDPrefix(withProcessToken(cf.getClientIDPrefix()));
    // Settlement contract (rc-47gr5): acknowledge() must return only after the broker applied the
    // ack. The Classic client defaults to sendAcksAsync=true, where acknowledge() returns once the
    // packet is queued, so a crash right after the journal COMMIT could redeliver a message whose
    // reservation was already cleared.
    cf.setSendAcksAsync(false);
    if (user != null) cf.setUserName(user);
    if (pass != null) cf.setPassword(pass);
    return cf;
  }

  /**
   * Rejects {@code jms.*} TLS-store URL options (task 6.1): ActiveMQ's bean-setting applies {@code
   * jms.trustStore}/{@code jms.keyStore} (and password/type variants) to the connection factory
   * superclass, whose {@code createTransport()} then rebuilds the TLS managers from those stores —
   * overriding the tls-table PEM pinning (expert-reproduced: pinned issuer count 1 -&gt; 111 with
   * JDK cacerts). TLS material comes only from the tls table. The rejected option NAME is decoded
   * via {@link URISupport#parseParameters(URI)} and echoed; the option value never is.
   */
  private static void rejectTlsStoreUrlOptions(String url) {
    Map<String, String> params;
    try {
      params = URISupport.parseParameters(URI.create(url));
    } catch (URISyntaxException e) {
      // Nominally unreachable: the ActiveMQConnectionFactory constructor parses the same URL first.
      // Never echo the raw URL — it can carry jms.password or other credentials. Preserve the
      // cause.
      throw new IllegalArgumentException("Invalid broker URL query options", e);
    }
    for (String name : params.keySet()) {
      String lower = name.toLowerCase(Locale.ROOT);
      if (lower.startsWith("jms.")
          && (lower.contains("truststore") || lower.contains("keystore"))) {
        throw new IllegalStateException(
            "UNSUPPORTED_TRANSPORT: jms TLS-store setting "
                + name
                + " rejected; TLS material comes only from the tls table");
      }
    }
  }

  /**
   * Broker TLS env contract (PEM material + identity binding), read once per factory build. The
   * explicit-values {@code buildActiveMqFactory} overload is the testability seam for it — {@link
   * System#getenv} is process-immutable.
   */
  record BrokerTlsEnv(
      String caCertPath, String clientCertPath, String clientKeyPath, String serverName) {
    boolean hasClientPair() {
      return clientCertPath != null && clientKeyPath != null;
    }

    /** CA path or any client identity value present — the pinned-material branch. */
    boolean anyPresent() {
      return caCertPath != null || clientCertPath != null || clientKeyPath != null;
    }
  }

  /** Single env read point for the broker TLS PEM contract (package-visible indirection). */
  static BrokerTlsEnv readBrokerTlsEnv() {
    return new BrokerTlsEnv(
        System.getenv(BROKER_TLS_CA_CERT_PATH_ENV),
        System.getenv(BROKER_TLS_CLIENT_CERT_PATH_ENV),
        System.getenv(BROKER_TLS_CLIENT_KEY_PATH_ENV),
        System.getenv(BROKER_TLS_SERVER_NAME_ENV));
  }

  /**
   * Per-JVM-process token, generated lazily on first use. It must NOT be a static final initialized
   * in {@code <clinit>}: GraalVM would fold that value into the image heap, so every process would
   * share one token and the rc-trxge duplicate-ID regression would return. Lazily assigned static
   * state stays runtime-only. Within a process the per-factory IdGenerator counter still keeps
   * individual connections distinct.
   */
  private static volatile String processIdToken;

  private static String processIdToken() {
    String token = processIdToken;
    if (token == null) {
      synchronized (JmsClientFactory.class) {
        token = processIdToken;
        if (token == null) {
          token = "jms-bridge-" + UUID.randomUUID();
          processIdToken = token;
        }
      }
    }
    return token;
  }

  /**
   * Appends the per-process token to an explicit prefix base, or uses the token alone when the base
   * is unset. The spec's condition is non-null and non-empty, so a whitespace-only base is a real
   * explicit prefix and is preserved ({@code isEmpty}, not {@code isBlank}). ActiveMQ Classic's
   * default is null.
   */
  private static String withProcessToken(String base) {
    String token = processIdToken();
    return (base == null || base.isEmpty()) ? token : base + "-" + token;
  }

  /**
   * Reads an explicit {@code jms.*} broker-URL option exactly as {@link ActiveMQConnectionFactory}
   * parses it in {@code setBrokerURL}, via {@link URISupport#parseParameters(URI)}: that method
   * dispatches on {@code URISupport.isCompositeURI} — a composite URI ({@code failover:}/{@code
   * fanout:}) contributes its outer parameters, a plain URI contributes its query, and a plain URI
   * with no query contributes nothing. Composite parsing must never run on a plain URI: {@code
   * parseComposite} strips {@code //} and reparses the scheme-specific part as a URI, so a plain
   * IP-literal URL ({@code tcp://127.0.0.1:61616}) fails with "Illegal character in scheme name".
   * Used only for the connection prefix — ActiveMQ 5.19.x exposes no {@code
   * getConnectionIDPrefix()} getter; {@code getClientIDPrefix()} does exist and is used instead for
   * the client base.
   */
  private static String explicitUriOption(String url, String option) {
    try {
      URI uri = URI.create(url);
      Map<String, String> params = URISupport.parseParameters(uri);
      return params.get(option);
    } catch (URISyntaxException e) {
      // Nominally unreachable: the ActiveMQConnectionFactory constructor parses the same URL first
      // and rejects unapplied jms.* options. Never echo the raw URL — it can carry jms.password or
      // other credentials. Preserve the cause.
      throw new IllegalArgumentException("Invalid broker URL query for option " + option, e);
    }
  }

  /**
   * Builds the Netty connector transport config for {@code brokerUri}, mapping the URI scheme
   * honestly onto TLS: only {@code ssl://}/{@code wss://} activate SSL properties, sourced from the
   * BRIDGE_BROKER_* env contract. Plaintext schemes ({@code tcp}/{@code ws}) get no SSL properties.
   *
   * <p>The scheme is dispatched exhaustively and fail-loud: an unsupported scheme (for example an
   * outer {@code failover:}/{@code fanout:} wrapper) aborts startup, as does a URL with no scheme
   * or no host — no default host is assumed.
   *
   * <p>This is the production entry point: it reads System.getenv() and BridgeConfig directly.
   * Tests use the explicit-values overload below because System.getenv() is immutable.
   */
  static Map<String, Object> transportConfig(URI brokerUri) {
    return transportConfig(
        brokerUri,
        System.getenv(KEYSTORE_PATH_ENV),
        System.getenv(TRUSTSTORE_PATH_ENV),
        System.getenv(KEYSTORE_PASSWORD_ENV),
        // Direct instantiation is safe: BridgeConfig is stateless (env-backed accessors only).
        new BridgeConfig().brokerType());
  }

  /**
   * Testability seam for {@link #transportConfig(URI)} — explicit values instead of env reads
   * (mirrors how task 1.3's pinnedMaxBodyBytes keeps config testable).
   */
  static Map<String, Object> transportConfig(
      URI brokerUri,
      String keystorePath,
      String truststorePath,
      String keystorePassword,
      String brokerType) {
    String scheme = brokerUri.getScheme();
    if (scheme == null) {
      throw new IllegalStateException(
          "Broker URL '" + brokerUri + "' has no scheme; a complete URL is required");
    }
    boolean secure;
    switch (scheme) {
      case "tcp", "ws" -> secure = false;
      case "ssl", "wss" -> secure = true;
      default ->
          throw new IllegalStateException(
              "Unsupported broker URL scheme '"
                  + scheme
                  + "' (URL: "
                  + brokerUri
                  + "): unwrap failover:/fanout: wrappers to a single primary broker URL; configure HA broker-side or as multiple broker entries");
    }

    String host = brokerUri.getHost();
    if (host == null || host.isBlank()) {
      throw new IllegalStateException(
          "Broker URL '"
              + brokerUri
              + "' has no host; a complete URL is required — no default host is assumed");
    }
    int port = brokerUri.getPort() > 0 ? brokerUri.getPort() : 61616;

    Map<String, Object> params = new HashMap<>();
    params.put(TransportConstants.HOST_PROP_NAME, host);
    params.put(TransportConstants.PORT_PROP_NAME, port);
    // Force NIO transport — Epoll/KQueue don't work in GraalVM native image
    params.put(TransportConstants.USE_EPOLL_PROP_NAME, false);
    params.put(TransportConstants.USE_KQUEUE_PROP_NAME, false);
    // Bound connection and call timeouts so health checks never block indefinitely.
    // GraalVM native image Netty initialization can stall under mandatory auth
    // without these — causing Rust's wait_for_health to time out.
    // Artemis expects handshake-timeout in milliseconds. Using `5` here
    // means 5ms (not 5s) and causes connection setup to fail repeatedly.
    params.put(TransportConstants.HANDSHAKE_TIMEOUT, 5_000); // ms (int)
    params.put(TransportConstants.NETTY_CONNECT_TIMEOUT, 5_000); // ms (int)

    if (!secure) {
      return params;
    }

    // Fail-loud before any locator/factory construction: secure scheme without
    // complete, real material must abort startup, never fall back to plaintext.
    requireMaterial(scheme, KEYSTORE_PATH_ENV, keystorePath);
    requireMaterial(scheme, TRUSTSTORE_PATH_ENV, truststorePath);
    if (keystorePassword == null || keystorePassword.isBlank()) {
      throw new IllegalStateException(
          "Secure scheme '" + scheme + "' requires TLS material: set " + KEYSTORE_PASSWORD_ENV);
    }
    // Broker-type guard: the Classic path passes URLs to ActiveMQConnectionFactory
    // outside this contract, so a secure scheme under any non-Artemis broker type
    // would silently produce a plaintext connection.
    if (!"artemis".equals(brokerType)) {
      throw new IllegalStateException(
          "Secure scheme '"
              + scheme
              + "' requires broker_type 'artemis' but found broker_type '"
              + brokerType
              + "' which does not implement the "
              + KEYSTORE_PATH_ENV
              + "/"
              + TRUSTSTORE_PATH_ENV
              + " TLS contract");
    }

    params.put(TransportConstants.SSL_ENABLED_PROP_NAME, true);
    // Artemis 2.36 names these without the SSL_ prefix ("sslEnabled" gates them).
    params.put(TransportConstants.KEYSTORE_PATH_PROP_NAME, keystorePath);
    params.put(TransportConstants.TRUSTSTORE_PATH_PROP_NAME, truststorePath);
    params.put(TransportConstants.KEYSTORE_PASSWORD_PROP_NAME, keystorePassword);
    return params;
  }

  /**
   * Fail-closed material validation mirroring PortAnnouncer's placeholder guard: paths carrying the
   * {@code placeholder-} marker are rejected even when the file exists.
   */
  private static void requireMaterial(String scheme, String envName, String value) {
    if (value == null || value.isBlank()) {
      throw new IllegalStateException(
          "Secure scheme '"
              + scheme
              + "' requires TLS material: set "
              + envName
              + " (PKCS12 path)");
    }
    if (value.contains("placeholder-")) {
      throw new IllegalStateException(
          "Secure scheme '"
              + scheme
              + "' started with placeholder TLS material: "
              + envName
              + "="
              + value
              + ". Aborting startup.");
    }
    if (!Files.exists(Path.of(value))) {
      throw new IllegalStateException(
          "Secure scheme '"
              + scheme
              + "' TLS material does not exist: "
              + envName
              + "="
              + value
              + ". Aborting startup.");
    }
  }

  /**
   * Builds an Artemis connection factory by constructing TransportConfiguration directly, bypassing
   * URI parsing and BeanSupport.
   *
   * <p>BeanSupport uses commons-beanutils which triggers Class.forName() chains that fail in
   * GraalVM native image. By constructing the transport config manually from the URL, we eliminate
   * that dependency.
   *
   * <p>The connector parameters come from {@link #transportConfig(URI)} so the URI scheme is never
   * discarded: ssl/wss URLs actually enable SSL.
   *
   * <p>Key native-image considerations: - useEpoll/useKQueue forced to false (Epoll not supported
   * in SubstrateVM) - reconnectAttempts set to allow retries on transient failures
   */
  static org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory buildArtemisFactory(
      String url, String user, String pass) {
    URI uri = URI.create(url);

    Map<String, Object> params = transportConfig(uri);

    TransportConfiguration tc =
        new TransportConfiguration(NettyConnectorFactory.class.getName(), params);

    var cf = new org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory(false, tc);
    cf.setReconnectAttempts(3);
    cf.setRetryInterval(1000);
    // Disable consumer-side pre-fetching. By default Artemis buffers up to
    // 1 MiB of messages on the client. With 0 the broker pushes one message
    // at a time, which avoids the scenario where the consumer's internal
    // buffer stalls in GraalVM native image and receive() never returns
    // new messages even though they exist on the broker.
    cf.setConsumerWindowSize(0);
    // Set TTL on the factory itself (not in TransportConstants where
    // it is silently ignored for client-side connectors).
    cf.setConnectionTTL(300_000); // 5 min
    // Disable producer-side flow control. Artemis Core protocol assigns a
    // limited credit window (producerWindowSize, default ~64 KiB) to each
    // producer. When credits are exhausted the send() call blocks waiting
    // for the broker to grant more. In GraalVM native image + Netty NIO
    // the credit-grant callback can stall, causing send() to block
    // indefinitely after a few messages. Setting -1 disables the credit
    // mechanism entirely — the producer never waits for credits.
    cf.setProducerWindowSize(-1);
    // Settlement contract (rc-47gr5): block acknowledge() until the broker confirms the ack
    // (default false), so the journal COMMIT implies the broker applied it.
    cf.setBlockOnAcknowledge(true);
    if (user != null) cf.setUser(user);
    if (pass != null) cf.setPassword(pass);
    return cf;
  }

  /**
   * One-time workarounds for GraalVM native image.
   *
   * <p>ActiveMQ Classic uses FactoryFinder (a custom service-loader) that reads
   * META-INF/services/... files at runtime via ClassLoader.getResourceAsStream(), then does
   * Class.forName(name).getConstructor().newInstance().
   *
   * <p>In native image this chain is fragile: resource loading may fail silently, and reflective
   * constructor access needs explicit registration. We replace the entire
   * FactoryFinder.ObjectFactory with one that knows about all ActiveMQ service classes and
   * instantiates them directly — zero reflection, zero resource loading.
   */
  static void initNativeImageWorkarounds() {
    if (!NATIVE_INIT_DONE.compareAndSet(false, true)) return;

    // Register TCP transport factory eagerly (bypass TRANSPORT_FACTORY_FINDER)
    TransportFactory.registerTransportFactory("tcp", new TcpTransportFactory());

    registerTransportFactories();

    // Replace FactoryFinder's ObjectFactory with a native-safe version
    final FactoryFinder.ObjectFactory originalFactory = FactoryFinder.getObjectFactory();
    final Map<String, java.util.function.Supplier<Object>> knownServices = KNOWN_SERVICES;

    // Wire format factories
    knownServices.put(
        "META-INF/services/org/apache/activemq/wireformat/default", OpenWireFormatFactory::new);

    // Transport factories
    knownServices.put(
        "META-INF/services/org/apache/activemq/transport/tcp", TcpTransportFactory::new);
    knownServices.put(
        "META-INF/services/org/apache/activemq/transport/failover",
        org.apache.activemq.transport.failover.FailoverTransportFactory::new);
    knownServices.put(
        "META-INF/services/org/apache/activemq/transport/nio",
        org.apache.activemq.transport.nio.NIOTransportFactory::new);

    FactoryFinder.setObjectFactory(
        path -> {
          java.util.function.Supplier<Object> supplier = knownServices.get(path);
          if (supplier != null) {
            return supplier.get();
          }
          return originalFactory.create(path);
        });
  }

  /**
   * Eager transport registrations shared by {@link #initNativeImageWorkarounds} and tests (task
   * 4.1). Idempotent: guarded so repeated calls never double-register. Registers the bridge-owned
   * {@code ssl} factory next to {@code tcp} and lists its FactoryFinder service path so native
   * resolution needs no reflective fallback.
   */
  static void registerTransportFactories() {
    if (!TRANSPORTS_REGISTERED.compareAndSet(false, true)) return;
    TransportFactory.registerTransportFactory("ssl", new CamelSslTransportFactory());
    KNOWN_SERVICES.put(
        "META-INF/services/org/apache/activemq/transport/ssl", CamelSslTransportFactory::new);
  }

  /** Package-visible accessor for the native-safe FactoryFinder service map (tests). */
  static Map<String, java.util.function.Supplier<Object>> knownServices() {
    return KNOWN_SERVICES;
  }

  /**
   * Startup self-check (task 4.1): every scheme in {@code brokerUrl} must resolve to a registered
   * transport factory before any connect attempt. Member parsing mirrors Rust {@code
   * composite_members}/{@code url_scheme} ({@code config.rs}): {@code failover://member?opts} keeps
   * its query on the member, {@code failover:(m1,m2)?opts} yields the top-level comma-split list,
   * and a plain {@code scheme://…} URL yields itself. An unresolvable scheme aborts startup with
   * {@code UNSUPPORTED_TRANSPORT} and the redacted URL — credentials never echo.
   */
  static void verifySchemesResolvable(String brokerUrl) {
    String redacted = redactUrl(brokerUrl);
    for (String member : compositeMembers(brokerUrl)) {
      String scheme = memberScheme(member);
      try {
        TransportFactory.findTransportFactory(new URI(member));
      } catch (IOException | URISyntaxException e) {
        throw new IllegalStateException(
            "UNSUPPORTED_TRANSPORT: no transport factory for scheme "
                + (scheme == null ? "<none>" : scheme)
                + " in "
                + redacted);
      }
    }
  }

  /**
   * Members of a broker URL, verbatim (never rewritten): {@code failover://member?opts} yields the
   * prefix-stripped member, {@code failover:(m1,m2)?opts} yields the parenthesized list split on
   * top-level commas, and a plain URL yields itself. An empty or malformed member list yields an
   * empty list (a validation error at the call site).
   */
  private static List<String> compositeMembers(String brokerUrl) {
    String url = brokerUrl == null ? "" : brokerUrl.trim();
    if (!url.startsWith("failover:")) {
      return url.isEmpty() ? List.of() : List.of(url);
    }
    String after = url.substring("failover:".length());
    if (after.startsWith("//")) {
      after = after.substring(2);
    }
    after = after.stripLeading();
    if (!after.startsWith("(")) {
      String tail = after.trim();
      return tail.isEmpty() ? List.of() : List.of(tail);
    }
    String inner = after.substring(1);
    int close = -1;
    int depth = 1;
    for (int i = 0; i < inner.length(); i++) {
      char c = inner.charAt(i);
      if (c == '(') {
        depth++;
      } else if (c == ')') {
        depth--;
        if (depth == 0) {
          close = i;
          break;
        }
      }
    }
    if (close < 0) {
      return List.of();
    }
    return splitTopLevelMembers(inner.substring(0, close));
  }

  /** Splits a composite member list on top-level commas, trimming and dropping empty members. */
  private static List<String> splitTopLevelMembers(String list) {
    List<String> members = new ArrayList<>();
    int start = 0;
    int nested = 0;
    for (int i = 0; i < list.length(); i++) {
      char c = list.charAt(i);
      if (c == '(') {
        nested++;
      } else if (c == ')') {
        nested = Math.max(0, nested - 1);
      } else if (c == ',' && nested == 0) {
        members.add(list.substring(start, i).trim());
        start = i + 1;
      }
    }
    members.add(list.substring(start).trim());
    members.removeIf(String::isEmpty);
    return members;
  }

  /**
   * Transport scheme of a broker URL or composite member, mirroring Rust {@code url_scheme}: the
   * text before the first {@code ://}; the parenthesized failover form {@code failover:(...)} has
   * no {@code ://} boundary for its list, so {@code (} terminates the scheme and it yields {@code
   * failover} (otherwise it would read {@code failover:(ssl}). Returns {@code null} for schemeless
   * members.
   */
  private static String memberScheme(String member) {
    String url = member.trim();
    if (url.startsWith("failover:(")) {
      return "failover";
    }
    int sep = url.indexOf("://");
    if (sep < 0) {
      return null;
    }
    String scheme = url.substring(0, sep).trim();
    return scheme.isEmpty() ? null : scheme;
  }

  /**
   * Whether the broker URL — composite members included — has an {@code ssl} member (task 5.1 TLS
   * seam selection and {@link BridgeActiveMqConnectionFactory}'s downgrade guard): the same
   * member-splitting logic as {@link #verifySchemesResolvable}.
   */
  static boolean urlHasSslMember(String brokerUrl) {
    for (String member : compositeMembers(brokerUrl)) {
      if ("ssl".equalsIgnoreCase(memberScheme(member))) {
        return true;
      }
    }
    return false;
  }

  /**
   * Redacts a broker URL for diagnostics, mirroring Rust {@code redact_broker_url} ({@code
   * config.rs}, the {@code camel_api::redact} contract): userinfo masked as {@code ***@} through
   * the last {@code @} of every authority window, sensitive query pairs rendered as {@code
   * key=<redacted>}, fragment dropped as {@code #[redacted]}, capped at 256 chars. The same policy
   * as {@link HeaderPolicy}'s credential substrings — over-masking is safe, under-masking is not.
   */
  private static String redactUrl(String raw) {
    if (isCompositeUrl(raw)) {
      return redactCompositeUrl(raw);
    }
    return redactSingleUrl(raw);
  }

  /**
   * Whether {@code raw} is the parenthesized composite form ({@code failover:(m1,m2)}) whose
   * members need independent redaction: a single-member pass would split at the first {@code ?} and
   * miss userinfo in later members.
   */
  private static boolean isCompositeUrl(String raw) {
    String url = raw == null ? "" : raw.trim();
    if (!url.startsWith("failover:")) {
      return false;
    }
    String after = url.substring("failover:".length());
    if (after.startsWith("//")) {
      after = after.substring(2);
    }
    return after.stripLeading().startsWith("(");
  }

  /**
   * Redacts each member of a parenthesized composite independently (recursively, so nested
   * composites are covered) and reassembles the wrapper with any trailing options redacted.
   */
  private static String redactCompositeUrl(String raw) {
    int open = raw.indexOf('(');
    if (open < 0) {
      return redactSingleUrl(raw);
    }
    int close = -1;
    int depth = 0;
    for (int i = open; i < raw.length(); i++) {
      char c = raw.charAt(i);
      if (c == '(') {
        depth++;
      } else if (c == ')') {
        depth--;
        if (depth == 0) {
          close = i;
          break;
        }
      }
    }
    if (close < 0) {
      return redactSingleUrl(raw);
    }
    StringBuilder out = new StringBuilder(raw.substring(0, open + 1));
    List<String> members = splitTopLevelMembers(raw.substring(open + 1, close));
    for (int i = 0; i < members.size(); i++) {
      if (i > 0) {
        out.append(',');
      }
      out.append(redactUrl(members.get(i)));
    }
    out.append(')');
    String suffix = raw.substring(close + 1);
    if (!suffix.isEmpty()) {
      out.append(redactSingleUrl(suffix));
    }
    return cap(out.toString());
  }

  /**
   * Single-member redaction: fragments dropped, userinfo masked in every {@code
   * scheme://user:pass@} authority window (including windows in query values), sensitive query keys
   * as {@code key=<redacted>}, capped at 256 chars.
   */
  private static String redactSingleUrl(String raw) {
    String body = raw;
    String fragment = "";
    int hash = body.indexOf('#');
    if (hash >= 0) {
      fragment = "#[redacted]";
      body = body.substring(0, hash);
    }
    body = maskUserinfo(body);
    int queryAt = body.indexOf('?');
    String base = queryAt >= 0 ? body.substring(0, queryAt) : body;
    String query = queryAt >= 0 ? body.substring(queryAt + 1) : "";
    String redacted = base + (query.isEmpty() ? "" : "?" + redactQuery(query)) + fragment;
    return cap(redacted);
  }

  private static String cap(String redacted) {
    if (redacted.length() > 256) {
      return redacted.substring(0, Character.isHighSurrogate(redacted.charAt(255)) ? 255 : 256);
    }
    return redacted;
  }

  /**
   * Masks userinfo of every authority window as {@code ***@}: a window opens after a run of two or
   * more slashes and ends at the next {@code /}, {@code ?}, {@code #}, or end of input; the bytes
   * from window start through the LAST {@code @} are replaced (over-masking is safe).
   */
  private static String maskUserinfo(String base) {
    StringBuilder out = new StringBuilder();
    int pos = 0;
    while (pos < base.length()) {
      char c = base.charAt(pos);
      if (c != '/') {
        out.append(c);
        pos++;
        continue;
      }
      int run = pos;
      while (run < base.length() && base.charAt(run) == '/') {
        run++;
      }
      int slashes = run - pos;
      out.append(base, pos, run);
      pos = run;
      if (slashes < 2) {
        continue;
      }
      int windowEnd = pos;
      while (windowEnd < base.length()
          && base.charAt(windowEnd) != '/'
          && base.charAt(windowEnd) != '?'
          && base.charAt(windowEnd) != '#') {
        windowEnd++;
      }
      String window = base.substring(pos, windowEnd);
      int at = window.lastIndexOf('@');
      if (at >= 0) {
        out.append("***@").append(window.substring(at + 1));
      } else {
        out.append(window);
      }
      pos = windowEnd;
    }
    return out.toString();
  }

  /**
   * Renders a query string with sensitive pairs redacted as {@code key=<redacted>}: the key bytes
   * themselves can embed a secret, but the pair position (including {@code jms.*} keys) stays
   * visible so transport policy remains diagnosable, mirroring Rust {@code redact_broker_url}.
   */
  private static String redactQuery(String query) {
    String[] pairs = query.split("&", -1);
    StringBuilder out = new StringBuilder();
    for (int i = 0; i < pairs.length; i++) {
      if (i > 0) {
        out.append('&');
      }
      String pair = pairs[i];
      int eq = pair.indexOf('=');
      String key = eq >= 0 ? pair.substring(0, eq) : pair;
      out.append(isSensitiveQueryKey(key) ? key + "=<redacted>" : pair);
    }
    return out.toString();
  }

  /** Whether a query key matches the {@link #REDACTED_QUERY_KEY_SUBSTRINGS} policy. */
  private static boolean isSensitiveQueryKey(String key) {
    String lower = key.toLowerCase(Locale.ROOT);
    for (String needle : REDACTED_QUERY_KEY_SUBSTRINGS) {
      if (lower.contains(needle)) {
        return true;
      }
    }
    return false;
  }

  @PreDestroy
  public void close() {
    JmsBrokerProbe probe = brokerProbe;
    if (probe != null) {
      probe.close();
    }
    reset();
  }
}
