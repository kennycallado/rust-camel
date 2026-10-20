package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyBoolean;
import static org.mockito.ArgumentMatchers.anyInt;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.atLeastOnce;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.doReturn;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.inOrder;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.spy;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.io.IOException;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import javax.jms.ConnectionMetaData;
import javax.jms.ExceptionListener;
import org.apache.activemq.ActiveMQConnection;
import org.apache.activemq.ActiveMQConnectionFactory;
import org.apache.activemq.artemis.api.core.ActiveMQException;
import org.apache.activemq.artemis.api.core.TransportConfiguration;
import org.apache.activemq.artemis.api.core.client.ClientSession;
import org.apache.activemq.artemis.api.core.client.ClientSessionFactory;
import org.apache.activemq.artemis.api.core.client.FailoverEventListener;
import org.apache.activemq.artemis.api.core.client.FailoverEventType;
import org.apache.activemq.artemis.api.core.client.ServerLocator;
import org.apache.activemq.transport.TransportListener;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;
import org.mockito.InOrder;

/** Retained diagnostic lifecycle and generation semantics of {@link JmsBrokerProbe}. */
class JmsBrokerProbeTest {

  private static final String RANDOM_TOKEN_PATTERN =
      ".*[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}.*";

  private static BridgeConfig config(String type, String url, String user, String pass) {
    Map<String, String> env = new java.util.HashMap<>();
    env.put("BRIDGE_BROKER_TYPE", type);
    env.put("BRIDGE_BROKER_URL", url);
    if (user != null) {
      env.put("BRIDGE_USERNAME", user);
    }
    if (pass != null) {
      env.put("BRIDGE_PASSWORD", pass);
    }
    return new BridgeConfig(env);
  }

  private static BridgeConfig config(String type, String url) {
    return config(type, url, null, null);
  }

  private static JmsBrokerProbe newProbe(
      BridgeConfig config, JmsBrokerProbe.DiagnosticFactory factory) {
    return new JmsBrokerProbe(config, factory, () -> {}, () -> {}, () -> {}, () -> {});
  }

  private static void awaitCleanup(JmsBrokerProbe probe) throws InterruptedException {
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
    while (!probe.isCleanupIdle() && System.nanoTime() < deadline) {
      Thread.sleep(2);
    }
    assertTrue(probe.isCleanupIdle(), "probe cleanup did not quiesce");
  }

  private static final class FakeFactory implements JmsBrokerProbe.DiagnosticFactory {
    private final AtomicInteger opens = new AtomicInteger();
    private final java.util.function.Supplier<JmsBrokerProbe.Diagnostic> supplier;

    FakeFactory(java.util.function.Supplier<JmsBrokerProbe.Diagnostic> supplier) {
      this.supplier = supplier;
    }

    int opens() {
      return opens.get();
    }

    @Override
    public JmsBrokerProbe.Diagnostic open(
        String brokerType, String brokerUrl, String username, String password) {
      opens.incrementAndGet();
      return supplier.get();
    }
  }

  private static JmsBrokerProbe adapterHost() {
    return newProbe(
        config("artemis", "tcp://broker:61616"),
        (brokerType, brokerUrl, username, password) -> {
          throw new IllegalStateException("adapter host does not open diagnostics");
        });
  }

  private static JmsBrokerProbe.Diagnostic countingDiagnostic(
      AtomicInteger verifies, AtomicInteger closes) {
    return new JmsBrokerProbe.Diagnostic() {
      @Override
      public void verify() {
        verifies.incrementAndGet();
      }

      @Override
      public void close() {
        closes.incrementAndGet();
      }
    };
  }

  @Test
  void startsAtGenerationOne() throws Exception {
    AtomicInteger verifies = new AtomicInteger();
    AtomicInteger closes = new AtomicInteger();
    FakeFactory factory = new FakeFactory(() -> countingDiagnostic(verifies, closes));
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    try {
      BrokerProbeResult old = probe.probe(0L);
      assertFalse(old.connected());
      assertEquals("probe invalidated", old.message());

      BrokerProbeResult current = probe.probe(1L);
      assertTrue(current.connected());
      assertEquals("ok", current.message());
      assertEquals(1, factory.opens());
    } finally {
      probe.close();
    }
  }

  @Test
  void activeMqReusesRetainedDiagnosticAcrossChecks() throws Exception {
    AtomicInteger verifies = new AtomicInteger();
    AtomicInteger closes = new AtomicInteger();
    FakeFactory factory = new FakeFactory(() -> countingDiagnostic(verifies, closes));
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    try {
      assertTrue(probe.probe(1L).connected());
      assertTrue(probe.probe(1L).connected());
      assertEquals(1, factory.opens());
      assertEquals(2, verifies.get());
    } finally {
      probe.close();
    }
  }

  @Test
  void artemisReusesRetainedDiagnosticAcrossChecks() throws Exception {
    AtomicInteger verifies = new AtomicInteger();
    AtomicInteger closes = new AtomicInteger();
    FakeFactory factory = new FakeFactory(() -> countingDiagnostic(verifies, closes));
    JmsBrokerProbe probe = newProbe(config("artemis", "tcp://broker:61616"), factory);
    try {
      assertTrue(probe.probe(1L).connected());
      assertTrue(probe.probe(1L).connected());
      assertEquals(1, factory.opens());
      assertEquals(2, verifies.get());
    } finally {
      probe.close();
    }
  }

  @Test
  void artemisVerificationUsesConfiguredCredentialsAndReusesConnection() throws Exception {
    String url = "tcp://broker:61616";
    String user = "profile-user";
    String pass = "profile-pass";

    ServerLocator locator = mock(ServerLocator.class);
    ClientSessionFactory sessionFactory = mock(ClientSessionFactory.class);
    ClientSession session = mock(ClientSession.class);
    when(locator.getStaticTransportConfigurations())
        .thenReturn(new TransportConfiguration[] {mock(TransportConfiguration.class)});
    when(locator.isPreAcknowledge()).thenReturn(false);
    when(locator.getAckBatchSize()).thenReturn(-1);
    when(locator.createSessionFactory(any(TransportConfiguration.class)))
        .thenReturn(sessionFactory);

    AtomicInteger anonymousAttempts = new AtomicInteger();
    when(sessionFactory.createSession())
        .thenAnswer(
            invocation -> {
              anonymousAttempts.incrementAndGet();
              throw new ActiveMQException("anonymous core session rejected");
            });
    AtomicInteger explicitCreates = new AtomicInteger();
    when(sessionFactory.createSession(
            anyString(),
            anyString(),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            anyBoolean(),
            anyInt()))
        .thenAnswer(
            invocation -> {
              assertEquals(user, invocation.getArgument(0));
              assertEquals(pass, invocation.getArgument(1));
              assertEquals(false, (boolean) invocation.getArgument(2));
              assertEquals(true, (boolean) invocation.getArgument(3));
              assertEquals(true, (boolean) invocation.getArgument(4));
              assertEquals(false, (boolean) invocation.getArgument(5));
              assertEquals(-1, (int) invocation.getArgument(6));
              explicitCreates.incrementAndGet();
              return session;
            });

    AtomicInteger factoryOpens = new AtomicInteger();
    AtomicReference<org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory>
        providerFactory = new AtomicReference<>();
    JmsBrokerProbe.DiagnosticFactory diagnosticFactory =
        (brokerType, brokerUrl, username, password) -> {
          factoryOpens.incrementAndGet();
          assertEquals("artemis", brokerType);
          assertEquals(url, brokerUrl);
          org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory built =
              spy(JmsBrokerProbe.buildArtemisDiagnosticFactory(brokerUrl, username, password));
          assertEquals(username, built.getUser());
          assertEquals(password, built.getPassword());
          doReturn(locator).when(built).getServerLocator();
          providerFactory.set(built);
          return JmsBrokerProbe.artemisDiagnostic(built);
        };

    JmsBrokerProbe probe = newProbe(config("artemis", url, user, pass), diagnosticFactory);
    try {
      assertTrue(probe.probe(1L).connected());
      assertTrue(probe.probe(1L).connected());
      assertEquals(1, factoryOpens.get());
      assertEquals(2, explicitCreates.get());
      assertEquals(0, anonymousAttempts.get());
      verify(sessionFactory, times(2))
          .createSession(eq(user), eq(pass), eq(false), eq(true), eq(true), eq(false), eq(-1));
      verify(sessionFactory, never()).createSession();
      verify(locator, times(1)).createSessionFactory(any(TransportConfiguration.class));
      verify(session, times(2)).close();
      verify(providerFactory.get(), never()).close();
    } finally {
      probe.close();
    }
    awaitCleanup(probe);
    verify(sessionFactory, times(1)).close();
    verify(providerFactory.get(), times(1)).close();
  }

  @Test
  void replacesDiagnosticAfterVerifyFailure() throws Exception {
    AtomicInteger verifyCalls = new AtomicInteger();
    AtomicInteger closes = new AtomicInteger();
    CountDownLatch closeLatch = new CountDownLatch(1);
    FakeFactory factory =
        new FakeFactory(
            () ->
                new JmsBrokerProbe.Diagnostic() {
                  @Override
                  public void verify() {
                    if (verifyCalls.incrementAndGet() == 1) {
                      throw new IllegalStateException("first check failed");
                    }
                  }

                  @Override
                  public void close() {
                    closes.incrementAndGet();
                    closeLatch.countDown();
                  }
                });
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    try {
      BrokerProbeResult first = probe.probe(1L);
      assertFalse(first.connected());
      assertTrue(closeLatch.await(5, TimeUnit.SECONDS));
      awaitCleanup(probe);

      BrokerProbeResult second = probe.probe(1L);
      assertTrue(second.connected());
      assertEquals(2, factory.opens());
      assertEquals(1, closes.get());
    } finally {
      probe.close();
    }
  }

  @Test
  void interruptFlagSurvivesInterruptedProbe() throws Exception {
    Thread.interrupted();
    FakeFactory factory =
        new FakeFactory(
            () ->
                new JmsBrokerProbe.Diagnostic() {
                  @Override
                  public void verify() throws Exception {
                    throw new InterruptedException("interrupted");
                  }

                  @Override
                  public void close() {}
                });
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    try {
      BrokerProbeResult result = probe.probe(1L);
      assertFalse(result.connected());
      assertTrue(Thread.currentThread().isInterrupted());
      Thread.interrupted();
    } finally {
      probe.close();
      Thread.interrupted();
    }
  }

  @Test
  void failureMessageOmitsUrlAndCredentials() throws Exception {
    String url = "tcp://admin:secret@broker:61616";
    FakeFactory factory =
        new FakeFactory(
            () ->
                new JmsBrokerProbe.Diagnostic() {
                  @Override
                  public void verify() {
                    throw new IllegalStateException(url);
                  }

                  @Override
                  public void close() {}
                });
    JmsBrokerProbe probe = newProbe(config("activemq", url), factory);
    try {
      BrokerProbeResult result = probe.probe(1L);
      assertFalse(result.connected());
      assertFalse(result.message().contains("admin"));
      assertFalse(result.message().contains("secret"));
      assertFalse(result.message().contains("broker:61616"));
    } finally {
      probe.close();
    }
  }

  @Test
  void activeMqExplicitClientIdClearedWithoutUrlRewriting() throws Exception {
    String url =
        "failover:(tcp://a:61616,tcp://b:61616)?jms.clientID=shared&jms.clientIDPrefix=pre&jms.connectionIDPrefix=conn";
    ActiveMQConnectionFactory factory = new ActiveMQConnectionFactory(url);

    JmsBrokerProbe.clearExplicitClientId(factory);

    assertNull(factory.getClientID());
    assertEquals("pre", factory.getClientIDPrefix());
    assertFalse(hasAccessor(ActiveMQConnectionFactory.class, "getConnectionIDPrefix"));
    assertTrue(factory.getBrokerURL().startsWith("failover:("));
    assertTrue(factory.getBrokerURL().contains("tcp://a:61616"));
    assertTrue(factory.getBrokerURL().contains("tcp://b:61616"));
    assertEquals("conn", privateField(factory, "connectionIDPrefix"));
  }

  @Test
  void artemisNativePathRetainsFactoryWithoutRewritingUrl() throws Exception {
    String url = "tcp://h:61616?jms.clientIDPrefix=pre";
    AtomicReference<String> seenUrl = new AtomicReference<>();
    AtomicReference<org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory> built =
        new AtomicReference<>();
    AtomicInteger createSessionCalls = new AtomicInteger();
    AtomicInteger opens = new AtomicInteger();
    JmsBrokerProbe.DiagnosticFactory factory =
        (brokerType, brokerUrl, username, password) -> {
          opens.incrementAndGet();
          seenUrl.set(brokerUrl);
          org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory real =
              spy(JmsClientFactory.buildArtemisFactory(brokerUrl, username, password));
          built.set(real);
          assertTrue(real.getServerLocator().getStaticTransportConfigurations().length >= 1);

          ClientSessionFactory sessionFactory = mock(ClientSessionFactory.class);
          ClientSession session = mock(ClientSession.class);
          when(sessionFactory.createSession()).thenReturn(session);
          return new JmsBrokerProbe.Diagnostic() {
            @Override
            public void verify() throws Exception {
              if (sessionFactory.createSession() == null) {
                throw new IllegalStateException("no session");
              }
              createSessionCalls.incrementAndGet();
            }

            @Override
            public void close() {
              sessionFactory.close();
              real.close();
            }
          };
        };

    JmsBrokerProbe probe = newProbe(config("artemis", url), factory);
    try {
      assertTrue(probe.probe(1L).connected());
      assertEquals(1, opens.get());
      assertEquals(url, seenUrl.get());
      assertNotNull(built.get());
      assertEquals(1, createSessionCalls.get());
      verify(built.get()).getServerLocator();
      verify(built.get(), never()).createConnection();
    } finally {
      probe.close();
    }
  }

  @Test
  void artemisExplicitClientIdPolicyIsEnforced() throws Exception {
    String url = "tcp://h:61616?jms.clientID=shared&jms.clientIDPrefix=pre";
    org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory real =
        JmsBrokerProbe.buildArtemisDiagnosticFactory(url, null, null);

    assertNull(real.getClientID());
    if (hasAccessor(real.getClass(), "getClientIDPrefix")) {
      Method getter = real.getClass().getMethod("getClientIDPrefix");
      assertEquals("pre", getter.invoke(real));
    } else {
      assertFalse(hasAccessor(real.getClass(), "getClientIDPrefix"));
    }
    assertTrue(real.getServerLocator().getStaticTransportConfigurations().length >= 1);

    org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory mockedFactory =
        mock(org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory.class);
    ServerLocator locator = mock(ServerLocator.class);
    ClientSessionFactory sessionFactory = mock(ClientSessionFactory.class);
    ClientSession session = mock(ClientSession.class);
    when(mockedFactory.getServerLocator()).thenReturn(locator);
    when(locator.getStaticTransportConfigurations())
        .thenReturn(new TransportConfiguration[] {mock(TransportConfiguration.class)});
    when(locator.createSessionFactory(any(TransportConfiguration.class)))
        .thenReturn(sessionFactory);
    when(sessionFactory.createSession(
            any(), any(), anyBoolean(), anyBoolean(), anyBoolean(), anyBoolean(), anyInt()))
        .thenReturn(session);

    JmsBrokerProbe.Diagnostic diagnostic = JmsBrokerProbe.artemisDiagnostic(mockedFactory);
    try {
      assertTrue(diagnostic instanceof JmsBrokerProbe.ArtemisDiagnostic);
      JmsBrokerProbe.ArtemisDiagnostic artemis = (JmsBrokerProbe.ArtemisDiagnostic) diagnostic;
      assertSame(sessionFactory, artemis.retainedSessionFactory());
      assertFalse(artemis.retainedSessionFactory() instanceof javax.jms.Connection);
      assertDoesNotThrow(diagnostic::verify);
    } finally {
      diagnostic.close();
    }
  }

  @Test
  void artemisPartialConstructionRetiresAcquiredResources() throws Exception {
    assertArtemisFactoryFailureRetiresFactory();
    assertArtemisListenerFailureRetiresSessionFactoryThenFactory();
  }

  private static void assertArtemisFactoryFailureRetiresFactory() throws Exception {
    org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory providerFactory =
        mock(org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory.class);
    ServerLocator locator = mock(ServerLocator.class);
    when(providerFactory.getServerLocator()).thenReturn(locator);
    when(locator.getStaticTransportConfigurations())
        .thenReturn(new TransportConfiguration[] {mock(TransportConfiguration.class)});
    when(locator.createSessionFactory(any(TransportConfiguration.class)))
        .thenThrow(new IllegalStateException("no session factory"));

    CountDownLatch factoryClosed = new CountDownLatch(1);
    AtomicReference<String> closeThread = new AtomicReference<>();
    doAnswer(
            invocation -> {
              closeThread.set(Thread.currentThread().getName());
              factoryClosed.countDown();
              return null;
            })
        .when(providerFactory)
        .close();

    AtomicReference<JmsBrokerProbe> holder = new AtomicReference<>();
    JmsBrokerProbe probe =
        newProbe(
            config("artemis", "tcp://h:61616"),
            (brokerType, brokerUrl, username, password) ->
                holder.get().openArtemisDiagnostic(providerFactory));
    holder.set(probe);
    try {
      assertFalse(probe.probe(1L).connected());
      assertTrue(factoryClosed.await(2, TimeUnit.SECONDS));
      assertTrue(closeThread.get().startsWith("jms-probe-cleanup"));
      verify(providerFactory, times(1)).close();
      verify(locator, times(1)).createSessionFactory(any(TransportConfiguration.class));
      awaitCleanup(probe);
      assertTrue(probe.isCleanupIdle());
    } finally {
      probe.close();
    }
  }

  private static void assertArtemisListenerFailureRetiresSessionFactoryThenFactory()
      throws Exception {
    org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory providerFactory =
        mock(org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory.class);
    ServerLocator locator = mock(ServerLocator.class);
    ClientSessionFactory sessionFactory = mock(ClientSessionFactory.class);
    when(providerFactory.getServerLocator()).thenReturn(locator);
    when(locator.getStaticTransportConfigurations())
        .thenReturn(new TransportConfiguration[] {mock(TransportConfiguration.class)});
    when(locator.createSessionFactory(any(TransportConfiguration.class)))
        .thenReturn(sessionFactory);
    doThrow(new IllegalStateException("listener registration failed"))
        .when(sessionFactory)
        .addFailoverListener(any(FailoverEventListener.class));

    CountDownLatch sessionFactoryClosed = new CountDownLatch(1);
    CountDownLatch factoryClosed = new CountDownLatch(1);
    AtomicInteger factoryCloseCalls = new AtomicInteger();
    doAnswer(
            invocation -> {
              sessionFactoryClosed.countDown();
              return null;
            })
        .when(sessionFactory)
        .close();
    doAnswer(
            invocation -> {
              factoryCloseCalls.incrementAndGet();
              factoryClosed.countDown();
              return null;
            })
        .when(providerFactory)
        .close();

    AtomicReference<JmsBrokerProbe> holder = new AtomicReference<>();
    JmsBrokerProbe probe =
        newProbe(
            config("artemis", "tcp://h:61616"),
            (brokerType, brokerUrl, username, password) ->
                holder.get().openArtemisDiagnostic(providerFactory));
    holder.set(probe);
    try {
      assertFalse(probe.probe(1L).connected());
      assertTrue(sessionFactoryClosed.await(2, TimeUnit.SECONDS));
      assertTrue(factoryClosed.await(2, TimeUnit.SECONDS));
      assertEquals(1, factoryCloseCalls.get());
      InOrder closes = inOrder(sessionFactory, providerFactory);
      closes.verify(sessionFactory).close();
      closes.verify(providerFactory).close();
      awaitCleanup(probe);
      assertTrue(probe.isCleanupIdle());
    } finally {
      probe.close();
    }
  }

  @Test
  void diagnosticUsesCanonicalProcessUniqueIdPrefixes() throws Exception {
    assertCanonicalPrefixes("tcp://localhost:61616", null);
    assertCanonicalPrefixes(
        "tcp://localhost:61616?jms.connectionIDPrefix=explicit-sentinel"
            + "&jms.clientIDPrefix=explicit-sentinel&jms.clientID=shared",
        "explicit-sentinel");
    assertCanonicalPrefixes(
        "failover:(tcp://a:61616,tcp://b:61616)?jms.connectionIDPrefix=explicit-sentinel"
            + "&jms.clientIDPrefix=explicit-sentinel&jms.clientID=shared",
        "explicit-sentinel");
  }

  private static void assertCanonicalPrefixes(String url, String expectedBase) throws Exception {
    ActiveMQConnectionFactory diagnostic =
        JmsBrokerProbe.buildActiveMqDiagnosticFactory(url, null, null);
    ActiveMQConnectionFactory canonical =
        new JmsClientFactory().buildActiveMqFactory(url, null, null);

    assertNull(diagnostic.getClientID(), "explicit client ID must be cleared");
    assertNotNull(diagnostic.getClientIDPrefix());
    assertNotNull(privateField(diagnostic, "connectionIDPrefix"));
    assertEquals(canonical.getClientIDPrefix(), diagnostic.getClientIDPrefix());
    assertEquals(
        privateField(canonical, "connectionIDPrefix"),
        privateField(diagnostic, "connectionIDPrefix"));
    assertTrue(diagnostic.getClientIDPrefix().matches(RANDOM_TOKEN_PATTERN), "process token");
    assertTrue(
        String.valueOf(privateField(diagnostic, "connectionIDPrefix"))
            .matches(RANDOM_TOKEN_PATTERN),
        "process token");
    if (expectedBase != null) {
      assertTrue(diagnostic.getClientIDPrefix().contains(expectedBase));
      assertTrue(
          String.valueOf(privateField(diagnostic, "connectionIDPrefix")).contains(expectedBase));
    }
    if (url.startsWith("failover:")) {
      assertTrue(diagnostic.getBrokerURL().startsWith("failover:("));
      assertTrue(diagnostic.getBrokerURL().contains("tcp://a:61616"));
      assertTrue(diagnostic.getBrokerURL().contains("tcp://b:61616"));
    }
  }

  @Test
  void invalidateReturnsBeforeBlockedProviderClose() throws Exception {
    CountDownLatch closeEntered = new CountDownLatch(1);
    CountDownLatch releaseClose = new CountDownLatch(1);
    AtomicInteger closes = new AtomicInteger();
    FakeFactory factory =
        new FakeFactory(
            () ->
                new JmsBrokerProbe.Diagnostic() {
                  @Override
                  public void verify() {}

                  @Override
                  public void close() {
                    closeEntered.countDown();
                    try {
                      releaseClose.await();
                    } catch (InterruptedException ignored) {
                      // Provider close ignores interrupts on purpose.
                    }
                    closes.incrementAndGet();
                  }
                });
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    try {
      assertTrue(probe.probe(1L).connected());

      long started = System.nanoTime();
      probe.invalidate(2L);
      long elapsedMillis = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);

      assertTrue(elapsedMillis < 100, "invalidate blocked for " + elapsedMillis + " ms");
      BrokerProbeResult result = probe.probe(1L);
      assertFalse(result.connected());
      assertEquals("probe invalidated", result.message());
      assertEquals(1, factory.opens());
      assertTrue(closeEntered.await(5, TimeUnit.SECONDS));

      probe.invalidate(3L);
      probe.invalidate(4L);
      assertEquals("probe invalidated", probe.probe(1L).message());
      assertEquals(1, factory.opens());
    } finally {
      releaseClose.countDown();
      probe.close();
      probe.close();
    }
    assertEquals(1, closes.get());
    assertEquals(0, fallbackThreadCount(), "no fallback worker may be created");
    assertTrue(cleanupThreadCount() <= 1, "only the owned cleanup worker may exist");
  }

  @Test
  void invalidateCloseRaceClosesDetachedDiagnosticExactlyOnce() throws Exception {
    CountDownLatch detached = new CountDownLatch(1);
    CountDownLatch resumeTransition = new CountDownLatch(1);
    CountDownLatch providerCloseEntered = new CountDownLatch(1);
    CountDownLatch releaseProviderClose = new CountDownLatch(1);
    AtomicInteger opens = new AtomicInteger();
    AtomicInteger closes = new AtomicInteger();
    AtomicReference<String> closeThread = new AtomicReference<>();
    JmsBrokerProbe.DiagnosticFactory factory =
        (brokerType, brokerUrl, username, password) -> {
          opens.incrementAndGet();
          return new JmsBrokerProbe.Diagnostic() {
            @Override
            public void verify() {}

            @Override
            public void close() {
              closeThread.set(Thread.currentThread().getName());
              providerCloseEntered.countDown();
              try {
                releaseProviderClose.await();
              } catch (InterruptedException ignored) {
                // Provider cleanup ignores interrupts.
              }
              closes.incrementAndGet();
            }
          };
        };
    CountDownLatch closeEntered = new CountDownLatch(1);
    JmsBrokerProbe probe =
        new JmsBrokerProbe(
            config("activemq", "tcp://broker:61616"),
            factory,
            () -> {
              detached.countDown();
              try {
                resumeTransition.await();
              } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
              }
            },
            () -> {},
            closeEntered::countDown,
            () -> {});
    ExecutorService invalidationWorker = Executors.newSingleThreadExecutor();
    ExecutorService closeWorker = Executors.newSingleThreadExecutor();
    try {
      assertTrue(probe.probe(1L).connected());

      CountDownLatch closeReturned = new CountDownLatch(1);
      Future<?> invalidation = invalidationWorker.submit(() -> probe.invalidate(2L));
      assertTrue(detached.await(5, TimeUnit.SECONDS));
      Future<?> close =
          closeWorker.submit(
              () -> {
                try {
                  probe.close();
                } finally {
                  closeReturned.countDown();
                }
              });
      assertTrue(closeEntered.await(5, TimeUnit.SECONDS), "close must enter concurrently");
      // Accounting precedes detach, so close must wait for the accounted retirement instead of
      // shutting the owned executor down before submission.
      assertFalse(
          closeReturned.await(500, TimeUnit.MILLISECONDS),
          "close must not complete while an accounted retirement awaits submission");
      assertEquals(0, fallbackThreadCount(), "no fallback worker may be created");

      resumeTransition.countDown();
      invalidation.get(5, TimeUnit.SECONDS);
      close.get(5, TimeUnit.SECONDS);
      assertTrue(providerCloseEntered.await(5, TimeUnit.SECONDS));
      assertTrue(closeThread.get().startsWith("jms-probe-cleanup"));
      assertEquals(1, cleanupThreadCount(), "exactly one owned cleanup worker");
      assertEquals(0, fallbackThreadCount(), "no fallback worker may be created");

      releaseProviderClose.countDown();
      awaitCleanup(probe);
      assertEquals(1, closes.get(), "detached diagnostic must close exactly once");
      assertEquals(1, opens.get(), "no replacement may open during invalidation/close");
      assertEquals(0, cleanupThreadCount(), "terminal cleanup executor must terminate");
      assertEquals(0, fallbackThreadCount(), "no rejection fallback worker may run");

      probe.invalidate(3L);
      probe.invalidate(4L);
      probe.close();
      probe.close();
      assertEquals(0, fallbackThreadCount(), "repeated invalidation/close must stay owned");
      assertEquals(1, opens.get(), "terminal probe must never open another diagnostic");
    } finally {
      resumeTransition.countDown();
      releaseProviderClose.countDown();
      probe.close();
      invalidationWorker.shutdownNow();
      closeWorker.shutdownNow();
    }
  }

  @Test
  void losingCasRollbackAfterTerminalCleanupShutsExecutor() throws Exception {
    assertLosingRollbackAfterTerminalCleanup(false);
  }

  @Test
  void losingCloseRollbackAfterTerminalCleanupShutsExecutor() throws Exception {
    assertLosingRollbackAfterTerminalCleanup(true);
  }

  private static void assertLosingRollbackAfterTerminalCleanup(boolean terminalCallerLoses)
      throws Exception {
    CountDownLatch speculativeReserved = new CountDownLatch(1);
    CountDownLatch releaseLoser = new CountDownLatch(1);
    CountDownLatch diagnosticClosed = new CountDownLatch(1);
    CountDownLatch cleanupCompleted = new CountDownLatch(1);
    AtomicInteger opens = new AtomicInteger();
    AtomicInteger closes = new AtomicInteger();
    java.util.concurrent.atomic.AtomicBoolean pauseFirst =
        new java.util.concurrent.atomic.AtomicBoolean(true);
    JmsBrokerProbe.DiagnosticFactory factory =
        (brokerType, brokerUrl, username, password) -> {
          opens.incrementAndGet();
          return new JmsBrokerProbe.Diagnostic() {
            @Override
            public void verify() {}

            @Override
            public void close() {
              closes.incrementAndGet();
              diagnosticClosed.countDown();
            }
          };
        };
    Runnable beforeCas =
        () -> {
          if (pauseFirst.compareAndSet(true, false)) {
            speculativeReserved.countDown();
            try {
              releaseLoser.await();
            } catch (InterruptedException e) {
              Thread.currentThread().interrupt();
            }
          }
        };
    JmsBrokerProbe probe =
        new JmsBrokerProbe(
            config("activemq", "tcp://broker:61616"),
            factory,
            () -> {},
            beforeCas,
            () -> {},
            cleanupCompleted::countDown);
    ExecutorService loserWorker = Executors.newSingleThreadExecutor();
    ExecutorService winnerWorker = Executors.newSingleThreadExecutor();
    try {
      assertTrue(probe.probe(1L).connected());

      Future<?> loser =
          loserWorker.submit(
              () -> {
                if (terminalCallerLoses) {
                  probe.close();
                } else {
                  probe.invalidate(2L);
                }
              });
      assertTrue(speculativeReserved.await(5, TimeUnit.SECONDS));

      Future<?> winner = winnerWorker.submit(probe::close);
      assertTrue(diagnosticClosed.await(5, TimeUnit.SECONDS));
      // The cleanup task's finally releases the winner's reservation and re-evaluates shutdown
      // before this barrier fires. The winner must also have returned; only then may the loser
      // roll back, so terminal cleanup is provably finished first.
      assertTrue(cleanupCompleted.await(5, TimeUnit.SECONDS));
      winner.get(5, TimeUnit.SECONDS);
      assertEquals(1, closes.get());
      assertFalse(probe.isCleanupIdle(), "loser speculative reservation must still be held");
      assertEquals(
          1, cleanupThreadCount(), "owned executor must still be alive while the loser is held");

      releaseLoser.countDown();
      loser.get(5, TimeUnit.SECONDS);

      long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
      while (cleanupThreadCount() > 0 && System.nanoTime() < deadline) {
        Thread.sleep(2);
      }
      assertTrue(probe.isCleanupIdle(), "speculative reservation must be released");
      assertEquals(0, cleanupThreadCount(), "owned cleanup executor must terminate");
      assertEquals(0, fallbackThreadCount(), "no fallback worker may exist");
      assertEquals(1, opens.get(), "no replacement may open after terminal cleanup");
      assertEquals(1, closes.get(), "diagnostic must be closed exactly once");
    } finally {
      releaseLoser.countDown();
      probe.close();
      loserWorker.shutdownNow();
      winnerWorker.shutdownNow();
    }
  }

  @Test
  void lateOpenCannotRetainAfterInvalidate() throws Exception {
    CountDownLatch openEntered = new CountDownLatch(1);
    CountDownLatch releaseOpen = new CountDownLatch(1);
    CountDownLatch closed = new CountDownLatch(1);
    AtomicInteger opens = new AtomicInteger();
    JmsBrokerProbe.Diagnostic opened =
        new JmsBrokerProbe.Diagnostic() {
          @Override
          public void verify() {}

          @Override
          public void close() {
            closed.countDown();
          }
        };
    JmsBrokerProbe.DiagnosticFactory factory =
        (brokerType, brokerUrl, username, password) -> {
          opens.incrementAndGet();
          openEntered.countDown();
          try {
            releaseOpen.await();
          } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
          }
          return opened;
        };
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    ExecutorService probes = Executors.newSingleThreadExecutor();
    try {
      Future<BrokerProbeResult> late = probes.submit(() -> probe.probe(1L));
      assertTrue(openEntered.await(5, TimeUnit.SECONDS));
      probe.invalidate(2L);
      releaseOpen.countDown();

      BrokerProbeResult result = late.get(5, TimeUnit.SECONDS);
      assertFalse(result.connected());
      assertEquals("probe invalidated", result.message());
      assertTrue(closed.await(5, TimeUnit.SECONDS));
      awaitCleanup(probe);

      assertTrue(probe.probe(2L).connected());
      assertEquals(2, opens.get());
    } finally {
      releaseOpen.countDown();
      probes.shutdownNow();
      probe.close();
    }
  }

  @Test
  void reverseOrderedInvalidationsCannotRegressGeneration() throws Exception {
    AtomicInteger verifies = new AtomicInteger();
    AtomicInteger closes = new AtomicInteger();
    FakeFactory factory = new FakeFactory(() -> countingDiagnostic(verifies, closes));
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    CountDownLatch releaseSecond = new CountDownLatch(1);
    ExecutorService invalidations = Executors.newSingleThreadExecutor();
    try {
      assertTrue(probe.probe(1L).connected());

      Future<?> secondInvalidation =
          invalidations.submit(
              () -> {
                try {
                  releaseSecond.await();
                } catch (InterruptedException e) {
                  Thread.currentThread().interrupt();
                }
                probe.invalidate(2L);
              });

      probe.invalidate(3L);
      releaseSecond.countDown();
      secondInvalidation.get(5, TimeUnit.SECONDS);
      awaitCleanup(probe);

      assertTrue(probe.probe(3L).connected());
      BrokerProbeResult stale = probe.probe(2L);
      assertFalse(stale.connected());
      assertEquals("probe invalidated", stale.message());
      assertEquals(2, factory.opens());
      assertEquals(1, closes.get());
    } finally {
      releaseSecond.countDown();
      invalidations.shutdownNow();
      probe.close();
    }
  }

  @Test
  void classicExceptionListenerInvalidatesOnlyItsDiagnostic() throws Exception {
    assertClassicCallbackRetiresOnlyItsDiagnostic(
        listeners -> listeners.exception().onException(new javax.jms.JMSException("failure")));
  }

  @Test
  void classicTransportInterruptionInvalidatesOnlyItsDiagnostic() throws Exception {
    assertClassicCallbackRetiresOnlyItsDiagnostic(
        listeners -> listeners.transport().transportInterupted());
  }

  @Test
  void classicTransportExceptionInvalidatesOnlyItsDiagnostic() throws Exception {
    assertClassicCallbackRetiresOnlyItsDiagnostic(
        listeners -> listeners.transport().onException(new IOException("failure")));
  }

  @Test
  void artemisFailureInvalidatesOnlyItsDiagnostic() throws Exception {
    assertArtemisCallbackRetiresOnlyItsDiagnostic();
  }

  private record ClassicListeners(ExceptionListener exception, TransportListener transport) {}

  private interface ClassicCallback {
    void fire(ClassicListeners listeners) throws Exception;
  }

  private static void assertClassicCallbackRetiresOnlyItsDiagnostic(ClassicCallback callback)
      throws Exception {
    AtomicInteger opens = new AtomicInteger();
    AtomicInteger firstCloses = new AtomicInteger();
    CountDownLatch closeEntered = new CountDownLatch(1);
    CountDownLatch releaseClose = new CountDownLatch(1);
    AtomicReference<ClassicListeners> listeners = new AtomicReference<>();
    AtomicReference<ActiveMQConnection> firstConnection = new AtomicReference<>();
    JmsBrokerProbe.DiagnosticFactory factory =
        (brokerType, brokerUrl, username, password) -> {
          int open = opens.incrementAndGet();
          ActiveMQConnection connection = mock(ActiveMQConnection.class);
          when(connection.getMetaData()).thenReturn(mock(ConnectionMetaData.class));
          JmsBrokerProbe.Diagnostic diagnostic = JmsBrokerProbe.activeMqDiagnostic(connection);
          if (open == 1) {
            firstConnection.set(connection);
            ArgumentCaptor<ExceptionListener> exception =
                ArgumentCaptor.forClass(ExceptionListener.class);
            ArgumentCaptor<TransportListener> transport =
                ArgumentCaptor.forClass(TransportListener.class);
            verify(connection).setExceptionListener(exception.capture());
            verify(connection).addTransportListener(transport.capture());
            listeners.set(new ClassicListeners(exception.getValue(), transport.getValue()));
            doAnswer(
                    invocation -> {
                      closeEntered.countDown();
                      releaseClose.await();
                      firstCloses.incrementAndGet();
                      return null;
                    })
                .when(connection)
                .close();
          }
          return diagnostic;
        };
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    try {
      assertTrue(probe.probe(1L).connected());
      callback.fire(listeners.get());
      assertFalse(probe.probe(1L).connected());
      assertTrue(closeEntered.await(5, TimeUnit.SECONDS));
      assertEquals("probe cleanup pending", probe.probe(1L).message());
      assertEquals(1, opens.get());

      releaseClose.countDown();
      awaitCleanup(probe);
      assertTrue(probe.probe(1L).connected());
      assertEquals(2, opens.get());

      callback.fire(listeners.get());
      assertTrue(probe.probe(1L).connected());
      assertEquals(1, firstCloses.get());
      verify(firstConnection.get(), times(1)).close();
    } finally {
      releaseClose.countDown();
      probe.close();
    }
  }

  private static void assertArtemisCallbackRetiresOnlyItsDiagnostic() throws Exception {
    AtomicInteger opens = new AtomicInteger();
    AtomicInteger firstSessionFactoryCloses = new AtomicInteger();
    CountDownLatch closeEntered = new CountDownLatch(1);
    CountDownLatch releaseClose = new CountDownLatch(1);
    AtomicReference<FailoverEventListener> listener = new AtomicReference<>();
    AtomicReference<ClientSessionFactory> firstSessionFactory = new AtomicReference<>();
    AtomicReference<ClientSession> firstSession = new AtomicReference<>();
    AtomicReference<org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory>
        firstProviderFactory = new AtomicReference<>();
    JmsBrokerProbe.DiagnosticFactory factory =
        (brokerType, brokerUrl, username, password) -> {
          int open = opens.incrementAndGet();
          org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory providerFactory =
              mock(org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory.class);
          ServerLocator locator = mock(ServerLocator.class);
          ClientSessionFactory sessionFactory = mock(ClientSessionFactory.class);
          ClientSession session = mock(ClientSession.class);
          when(providerFactory.getServerLocator()).thenReturn(locator);
          when(locator.getStaticTransportConfigurations())
              .thenReturn(new TransportConfiguration[] {mock(TransportConfiguration.class)});
          when(locator.createSessionFactory(any(TransportConfiguration.class)))
              .thenReturn(sessionFactory);
          when(sessionFactory.createSession(
                  any(), any(), anyBoolean(), anyBoolean(), anyBoolean(), anyBoolean(), anyInt()))
              .thenReturn(session);
          JmsBrokerProbe.Diagnostic diagnostic = JmsBrokerProbe.artemisDiagnostic(providerFactory);
          if (open == 1) {
            ArgumentCaptor<FailoverEventListener> failover =
                ArgumentCaptor.forClass(FailoverEventListener.class);
            verify(sessionFactory).addFailoverListener(failover.capture());
            listener.set(failover.getValue());
            firstSessionFactory.set(sessionFactory);
            firstSession.set(session);
            firstProviderFactory.set(providerFactory);
            doAnswer(
                    invocation -> {
                      closeEntered.countDown();
                      releaseClose.await();
                      firstSessionFactoryCloses.incrementAndGet();
                      return null;
                    })
                .when(sessionFactory)
                .close();
          }
          return diagnostic;
        };
    JmsBrokerProbe probe = newProbe(config("artemis", "tcp://broker:61616"), factory);
    try {
      assertTrue(probe.probe(1L).connected());
      listener.get().failoverEvent(FailoverEventType.FAILURE_DETECTED);
      assertFalse(probe.probe(1L).connected());
      assertTrue(closeEntered.await(5, TimeUnit.SECONDS));
      assertEquals("probe cleanup pending", probe.probe(1L).message());
      assertEquals(1, opens.get());

      releaseClose.countDown();
      awaitCleanup(probe);
      assertTrue(probe.probe(1L).connected());
      assertEquals(2, opens.get());

      listener.get().failoverEvent(FailoverEventType.FAILURE_DETECTED);
      assertTrue(probe.probe(1L).connected());
      assertEquals(1, firstSessionFactoryCloses.get());
      verify(firstSession.get(), atLeastOnce()).close();
      InOrder closes = inOrder(firstSessionFactory.get(), firstProviderFactory.get());
      closes.verify(firstSessionFactory.get()).close();
      closes.verify(firstProviderFactory.get()).close();
      verify(firstProviderFactory.get(), never()).createConnection();
    } finally {
      releaseClose.countDown();
      probe.close();
    }
  }

  @Test
  void probeExceptionRetiresAndNeverResetsThePool() throws Exception {
    AtomicInteger opens = new AtomicInteger();
    AtomicInteger closes = new AtomicInteger();
    JmsBrokerProbe.DiagnosticFactory factory =
        (brokerType, brokerUrl, username, password) -> {
          opens.incrementAndGet();
          return new JmsBrokerProbe.Diagnostic() {
            @Override
            public void verify() {
              throw new IllegalStateException("broker failed");
            }

            @Override
            public void close() {
              closes.incrementAndGet();
            }
          };
        };
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    try {
      assertFalse(probe.probe(1L).connected());
      awaitCleanup(probe);
      assertEquals(1, closes.get());

      String source =
          Files.readString(Path.of("src/main/java/org/rustcamel/jms/JmsBrokerProbe.java"));
      assertFalse(source.contains("reset()"), "probe must never reset the client factory pool");

      assertFalse(probe.probe(1L).connected());
      awaitCleanup(probe);
      assertEquals(2, opens.get());
    } finally {
      probe.close();
    }
  }

  @Test
  void closeRetiresWithinBoundAndSpansOneCleanupThread() throws Exception {
    CountDownLatch closeEntered = new CountDownLatch(1);
    CountDownLatch releaseClose = new CountDownLatch(1);
    AtomicInteger closes = new AtomicInteger();
    AtomicInteger opens = new AtomicInteger();
    JmsBrokerProbe.DiagnosticFactory factory =
        (brokerType, brokerUrl, username, password) -> {
          opens.incrementAndGet();
          return new JmsBrokerProbe.Diagnostic() {
            @Override
            public void verify() {}

            @Override
            public void close() {
              closeEntered.countDown();
              try {
                releaseClose.await();
              } catch (InterruptedException ignored) {
                // Provider close ignores interrupts on purpose.
              }
              closes.incrementAndGet();
            }
          };
        };
    JmsBrokerProbe probe = newProbe(config("activemq", "tcp://broker:61616"), factory);
    assertTrue(probe.probe(1L).connected());

    long started = System.nanoTime();
    try {
      probe.close();
    } finally {
      long elapsedMillis = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);
      assertTrue(elapsedMillis <= 1_500, "close took " + elapsedMillis + " ms");
      BrokerProbeResult afterClose = probe.probe(1L);
      assertFalse(afterClose.connected());
      assertEquals("probe closed", afterClose.message());
      assertEquals(1, opens.get());
      assertEquals(1, cleanupThreadCount());
      assertEquals(0, fallbackThreadCount());
      assertTrue(closeEntered.await(5, TimeUnit.SECONDS));

      probe.invalidate(2L);
      probe.invalidate(3L);
      probe.close();
      probe.close();
      assertEquals(1, opens.get());
      assertEquals(1, cleanupThreadCount(), "only the owned cleanup worker may exist");
      assertEquals(0, fallbackThreadCount(), "repeated invalidation/close must stay owned");
    }
    releaseClose.countDown();

    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
    while (cleanupThreadCount() > 0 && System.nanoTime() < deadline) {
      Thread.sleep(2);
    }
    assertEquals(0, cleanupThreadCount(), "cleanup executor did not terminate");
    assertEquals(0, fallbackThreadCount(), "no fallback worker may exist");
    assertEquals(1, closes.get());
  }

  private static int cleanupThreadCount() {
    return threadCount("jms-probe-cleanup");
  }

  private static int fallbackThreadCount() {
    return threadCount("jms-probe-cleanup-fallback");
  }

  private static int threadCount(String name) {
    int count = 0;
    for (Thread thread : Thread.getAllStackTraces().keySet()) {
      if (name.equals(thread.getName()) && thread.isAlive()) {
        count++;
      }
    }
    return count;
  }

  private static Object privateField(Object target, String name) throws Exception {
    var field = target.getClass().getDeclaredField(name);
    field.setAccessible(true);
    return field.get(target);
  }

  private static boolean hasAccessor(Class<?> type, String name) {
    try {
      type.getMethod(name);
      return true;
    } catch (NoSuchMethodException e) {
      return false;
    }
  }
}
