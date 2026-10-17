package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import jakarta.xml.ws.Dispatch;
import jakarta.xml.ws.Service;
import java.net.URL;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Supplier;
import javax.xml.namespace.QName;
import javax.xml.transform.Source;
import org.apache.cxf.BusFactory;
import org.apache.cxf.binding.soap.model.SoapOperationInfo;
import org.apache.cxf.configuration.jsse.TLSClientParameters;
import org.apache.cxf.endpoint.Client;
import org.apache.cxf.endpoint.Endpoint;
import org.apache.cxf.jaxws.DispatchImpl;
import org.apache.cxf.service.model.BindingInfo;
import org.apache.cxf.service.model.BindingOperationInfo;
import org.apache.cxf.service.model.EndpointInfo;
import org.apache.cxf.transport.http.HTTPConduit;
import org.apache.cxf.transport.http.URLConnectionHTTPConduit;
import org.apache.cxf.ws.addressing.EndpointReferenceType;
import org.apache.cxf.wsdl11.WSDLServiceFactory;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.mockito.Mock;
import org.mockito.MockedStatic;
import org.mockito.MockitoAnnotations;

@DisplayName("CxfClientManager")
class CxfClientManagerTest {

  private static final String WSDL = "/fake.wsdl";
  private static final String ADDRESS = "http://localhost:8080/svc";
  private static final String SERVICE = "Svc";
  private static final String PORT = "Port";
  private static final String PROFILE = "prof";
  private static final long DEFAULT_TIMEOUT = 30000L;
  private static final String SOAPACTION_URI = "jakarta.xml.ws.soap.http.soapaction.uri";
  private static final String RECEIVE_TIMEOUT = "jakarta.xml.ws.client.receiveTimeout";
  private static final String ROBUST = "org.apache.cxf.oneway.robust";
  private static final String NS = "urn:oneway:363";
  private static final String FIXTURE_WSDL = "/soap-oneway.wsdl";

  @Mock SecurityProfileStore profileStore;

  RecordingManager manager;
  Service mockService;
  final AtomicInteger creations = new AtomicInteger();
  final List<Dispatch<Source>> createdDispatches = new CopyOnWriteArrayList<>();
  Supplier<Dispatch<Source>> dispatchFactory = CxfClientManagerTest::newMockDispatch;
  AutoCloseable openMocks;
  MockedStatic<Service> serviceStatic;

  @BeforeEach
  void setUp() {
    openMocks = MockitoAnnotations.openMocks(this);
    when(profileStore.getProfile(anyString())).thenReturn(mock(SecurityProfile.class));

    manager = new RecordingManager();
    manager.profileStore = profileStore;

    mockService = mock(Service.class);
    when(mockService.createDispatch(any(QName.class), eq(Source.class), eq(Service.Mode.PAYLOAD)))
        .thenAnswer(
            inv -> {
              Dispatch<Source> created = dispatchFactory.get();
              createdDispatches.add(created);
              return created;
            });
    serviceStatic = mockStatic(Service.class);
    serviceStatic
        .when(() -> Service.create(any(URL.class), any(QName.class)))
        .thenAnswer(
            inv -> {
              creations.incrementAndGet();
              return mockService;
            });
  }

  @AfterEach
  void tearDown() throws Exception {
    serviceStatic.close();
    openMocks.close();
  }

  @SuppressWarnings("unchecked")
  private static Dispatch<Source> newMockDispatch() {
    DispatchImpl<Source> dispatch = mock(DispatchImpl.class);
    Map<String, Object> context = new HashMap<>();
    when(dispatch.getRequestContext()).thenReturn(context);
    Client client = mock(Client.class);
    stubBindingOperations(client);
    when(dispatch.getClient()).thenReturn(client);
    return dispatch;
  }

  private static void stubBindingOperations(Client client) {
    Endpoint endpoint = mock(Endpoint.class);
    EndpointInfo endpointInfo = mock(EndpointInfo.class);
    BindingInfo binding = mock(BindingInfo.class);
    when(client.getEndpoint()).thenReturn(endpoint);
    when(endpoint.getEndpointInfo()).thenReturn(endpointInfo);
    when(endpointInfo.getBinding()).thenReturn(binding);
    List<BindingOperationInfo> operations =
        List.of("opX", "opT", "opA", "opB", "opC", "k1", "k2", "k3", "opTls", "opPlain").stream()
            .map(
                name -> {
                  BindingOperationInfo operation = mock(BindingOperationInfo.class);
                  when(operation.getName()).thenReturn(new QName("urn:test", name));
                  SoapOperationInfo soap = new SoapOperationInfo();
                  soap.setAction("urn:test:" + name);
                  when(operation.getExtensor(SoapOperationInfo.class)).thenReturn(soap);
                  return operation;
                })
            .toList();
    when(binding.getOperations()).thenReturn(operations);
  }

  /** Builds a mocked CXF dispatch whose selected binding is the real fixture binding. */
  @SuppressWarnings("unchecked")
  private static Dispatch<Source> newFixtureDispatch(BindingInfo binding) {
    DispatchImpl<Source> dispatch = mock(DispatchImpl.class);
    when(dispatch.getRequestContext()).thenReturn(new HashMap<>());
    Client client = mock(Client.class);
    Endpoint endpoint = mock(Endpoint.class);
    EndpointInfo endpointInfo = mock(EndpointInfo.class);
    when(client.getEndpoint()).thenReturn(endpoint);
    when(endpoint.getEndpointInfo()).thenReturn(endpointInfo);
    when(endpointInfo.getBinding()).thenReturn(binding);
    when(dispatch.getClient()).thenReturn(client);
    return dispatch;
  }

  private static BindingInfo fixtureBinding(String service, String port) {
    URL wsdl = CxfClientManagerTest.class.getResource(FIXTURE_WSDL);
    assertNotNull(wsdl, "fixture WSDL must be on the test classpath");
    WSDLServiceFactory factory =
        new WSDLServiceFactory(
            BusFactory.getDefaultBus(), wsdl.toExternalForm(), new QName(NS, service));
    return factory.create().getServiceInfos().get(0).getEndpoint(new QName(NS, port)).getBinding();
  }

  private static BindingInfo mainBinding() {
    return fixtureBinding("OneWayService", "MainPort");
  }

  private static BindingInfo aliasBinding() {
    return fixtureBinding("AliasService", "AliasPort");
  }

  @Test
  @DisplayName("cacheSize returns 0 initially")
  void cacheSizeReturnsZeroInitially() {
    assertEquals(0, manager.cacheSize());
  }

  @Test
  @DisplayName("getDispatch with unknown profile throws")
  void getDispatchWithUnknownProfileThrows() {
    when(profileStore.getProfile("unknown"))
        .thenThrow(new IllegalArgumentException("Unknown security profile: unknown"));

    assertThrows(
        IllegalArgumentException.class,
        () -> manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "op", "unknown", DEFAULT_TIMEOUT));
  }

  @Test
  @DisplayName("profiles is the injected profileStore")
  void profileStoreIsInjected() {
    // Verify the manager references the mock profileStore
    assertNotNull(manager.profileStore);
    assertSame(profileStore, manager.profileStore);
  }

  @Test
  @DisplayName("operation participates in the cache key")
  void operationParticipatesInCacheKey() throws Exception {
    dispatchFactory = () -> newFixtureDispatch(mainBinding());
    QName receiveQName = new QName(NS, "receiveMessage");

    manager.getResolvedDispatch(
        WSDL, ADDRESS, SERVICE, PORT, null, PROFILE, DEFAULT_TIMEOUT, receiveQName);
    manager.getResolvedDispatch(
        WSDL, ADDRESS, SERVICE, PORT, "EchoOp", PROFILE, DEFAULT_TIMEOUT, null);
    manager.getResolvedDispatch(
        WSDL, ADDRESS, SERVICE, PORT, "   ", PROFILE, DEFAULT_TIMEOUT, receiveQName);

    assertEquals(2, manager.cacheSize(), "whitespace-only operation must normalize to blank key");
  }

  @Test
  @DisplayName("resolved dispatch carries shape and fixed non-robust mode")
  void resolvedDispatchCarriesShapeAndFixedNonRobustMode() throws Exception {
    dispatchFactory = () -> newFixtureDispatch(mainBinding());

    CxfClientManager.ResolvedDispatch receive =
        manager.getResolvedDispatch(
            WSDL, ADDRESS, SERVICE, PORT, "ReceiveMessageOp", PROFILE, DEFAULT_TIMEOUT, null);
    assertEquals(new QName(NS, "ReceiveMessageOp"), receive.operation().name());
    assertEquals("urn:receive", receive.operation().soapAction());
    assertTrue(receive.operation().oneWay(), "ReceiveMessageOp has input and no output");
    Map<String, Object> receiveContext = receive.dispatch().getRequestContext();
    assertEquals("urn:receive", receiveContext.get(SOAPACTION_URI));
    assertEquals(Boolean.FALSE, receiveContext.get(ROBUST), "one-way dispatch must be non-robust");

    CxfClientManager.ResolvedDispatch echo =
        manager.getResolvedDispatch(
            WSDL, ADDRESS, SERVICE, PORT, "EchoOp", PROFILE, DEFAULT_TIMEOUT, null);
    assertEquals(new QName(NS, "EchoOp"), echo.operation().name());
    assertEquals("urn:echo", echo.operation().soapAction());
    assertFalse(echo.operation().oneWay(), "EchoOp declares an output");
    Map<String, Object> echoContext = echo.dispatch().getRequestContext();
    assertEquals("urn:echo", echoContext.get(SOAPACTION_URI));
    assertNotEquals(
        Boolean.FALSE, echoContext.get(ROBUST), "request-reply must not force non-robust mode");
  }

  @Test
  @DisplayName("inferred operations do not cross-contaminate cached context")
  void inferredOperationsDoNotCrossContaminateCachedContext() throws Exception {
    manager.pinnedMaxDispatches = 64;
    QName receiveQName = new QName(NS, "receiveMessage");
    QName echoQName = new QName(NS, "echoRequest");
    dispatchFactory = () -> newFixtureDispatch(mainBinding());

    // Pre-seed on the main thread: Mockito static Service mocks are thread-confined.
    CxfClientManager.ResolvedDispatch receive =
        manager.getResolvedDispatch(
            WSDL, ADDRESS, SERVICE, PORT, null, PROFILE, DEFAULT_TIMEOUT, receiveQName);
    CxfClientManager.ResolvedDispatch echo =
        manager.getResolvedDispatch(
            WSDL, ADDRESS, SERVICE, PORT, null, PROFILE, DEFAULT_TIMEOUT, echoQName);
    assertEquals(2, manager.cacheSize());

    Map<String, Object> receiveSnapshot = new HashMap<>(receive.dispatch().getRequestContext());
    Map<String, Object> echoSnapshot = new HashMap<>(echo.dispatch().getRequestContext());

    for (int i = 0; i < 50; i++) {
      CxfClientManager.ResolvedDispatch r =
          manager.getResolvedDispatch(
              WSDL, ADDRESS, SERVICE, PORT, null, PROFILE, DEFAULT_TIMEOUT, receiveQName);
      assertSame(
          receive.dispatch(), r.dispatch(), "receive QName must resolve to its own dispatch");
      assertTrue(r.operation().oneWay());
      assertEquals("urn:receive", r.operation().soapAction());
      assertEquals("urn:receive", r.dispatch().getRequestContext().get(SOAPACTION_URI));

      CxfClientManager.ResolvedDispatch e =
          manager.getResolvedDispatch(
              WSDL, ADDRESS, SERVICE, PORT, null, PROFILE, DEFAULT_TIMEOUT, echoQName);
      assertSame(echo.dispatch(), e.dispatch(), "echo QName must resolve to its own dispatch");
      assertFalse(e.operation().oneWay());
      assertEquals("urn:echo", e.operation().soapAction());
      assertEquals("urn:echo", e.dispatch().getRequestContext().get(SOAPACTION_URI));
    }

    AtomicInteger crossings = new AtomicInteger();
    List<String> failures = new CopyOnWriteArrayList<>();
    Thread workerReceive =
        inferredLookupWorker(receiveQName, "urn:receive", true, crossings, failures);
    Thread workerEcho = inferredLookupWorker(echoQName, "urn:echo", false, crossings, failures);
    workerReceive.start();
    workerEcho.start();
    workerReceive.join();
    workerEcho.join();

    assertTrue(failures::isEmpty, () -> "worker failures: " + failures);
    assertEquals(0, crossings.get(), "inferred operations crossed cached context");
    assertEquals(
        receiveSnapshot,
        receive.dispatch().getRequestContext(),
        "no post-creation write to the receive context");
    assertEquals(
        echoSnapshot,
        echo.dispatch().getRequestContext(),
        "no post-creation write to the echo context");
  }

  @Test
  @DisplayName("invalid resolution closes the created dispatch without caching")
  void invalidResolutionClosesDispatchWithoutCaching() throws Exception {
    manager.pinnedMaxDispatches = 64;
    dispatchFactory =
        () -> {
          Dispatch<Source> created = newFixtureDispatch(mainBinding());
          createdDispatches.add(created);
          return created;
        };

    assertThrows(
        IllegalArgumentException.class,
        () ->
            manager.getResolvedDispatch(
                WSDL, ADDRESS, SERVICE, PORT, "MissingOp", PROFILE, DEFAULT_TIMEOUT, null),
        "an explicit name absent from the binding must be rejected");
    assertThrows(
        IllegalArgumentException.class,
        () ->
            manager.getResolvedDispatch(
                WSDL,
                ADDRESS,
                SERVICE,
                PORT,
                null,
                PROFILE,
                DEFAULT_TIMEOUT,
                new QName(NS, "notThere")),
        "an inferred QName matching no operation must be rejected");

    dispatchFactory =
        () -> {
          Dispatch<Source> created = newFixtureDispatch(aliasBinding());
          createdDispatches.add(created);
          return created;
        };
    assertThrows(
        IllegalArgumentException.class,
        () ->
            manager.getResolvedDispatch(
                WSDL,
                ADDRESS,
                SERVICE,
                PORT,
                null,
                PROFILE,
                DEFAULT_TIMEOUT,
                new QName(NS, "receiveMessage")),
        "an inferred QName matching multiple operations must be rejected");

    assertEquals(
        3,
        manager.closedDispatches.size(),
        "each created dispatch rejected by resolution must be closed exactly once");
    assertEquals(0, manager.cacheSize(), "a failed selection must never be cached");
    for (Dispatch<Source> created : createdDispatches) {
      verify(created, never()).invoke(any(Source.class));
    }
  }

  @Test
  @DisplayName("timeout participates in the cache key")
  void timeoutParticipatesInCacheKey() throws Exception {
    Dispatch<Source> first =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opT", PROFILE, 5000L);
    Dispatch<Source> second =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opT", PROFILE, 9000L);

    assertEquals(2, manager.cacheSize());
    assertEquals("9000", second.getRequestContext().get(RECEIVE_TIMEOUT));

    Dispatch<Source> firstReRead =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opT", PROFILE, 5000L);
    assertSame(first, firstReRead);
    assertEquals("5000", first.getRequestContext().get(RECEIVE_TIMEOUT));
  }

  @Test
  @DisplayName("soapaction is set at creation, never after cache lookup")
  void soapActionSetAtCreationNotAfterLookup() throws Exception {
    Dispatch<Source> dispatch =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opA", PROFILE, DEFAULT_TIMEOUT);
    Map<String, Object> context = dispatch.getRequestContext();
    assertEquals("urn:test:opA", context.get(SOAPACTION_URI));

    Map<String, Object> snapshot = new HashMap<>(context);
    manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opA", PROFILE, DEFAULT_TIMEOUT);
    assertEquals(snapshot, context, "cached dispatch context must not be mutated after publish");
  }

  @Test
  @DisplayName("concurrent distinct operations do not cross-contaminate")
  void concurrentDistinctOperationsDoNotCrossContaminate() throws Exception {
    // Pre-seed both entries on the main thread: MockedStatic is thread-confined,
    // so workers must only perform cache-hit lookups (no createDispatch / Service.create).
    manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opA", PROFILE, DEFAULT_TIMEOUT);
    manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opB", PROFILE, DEFAULT_TIMEOUT);
    assertEquals(2, manager.cacheSize());

    AtomicInteger crossings = new AtomicInteger();
    List<String> failures = new CopyOnWriteArrayList<>();
    Thread workerA = lookupWorker("opA", crossings, failures);
    Thread workerB = lookupWorker("opB", crossings, failures);
    workerA.start();
    workerB.start();
    workerA.join();
    workerB.join();

    assertTrue(failures::isEmpty, () -> "worker failures: " + failures);
    assertEquals(0, crossings.get(), "soapaction crossed between concurrent operations");
  }

  @Test
  @DisplayName("eviction bounds the cache at the pinned cap")
  void evictionBoundsCacheAtCap() throws Exception {
    manager.pinnedMaxDispatches = 2;

    Dispatch<Source> first =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "k1", PROFILE, DEFAULT_TIMEOUT);
    Dispatch<Source> second =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "k2", PROFILE, DEFAULT_TIMEOUT);
    Dispatch<Source> third =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "k3", PROFILE, DEFAULT_TIMEOUT);

    assertEquals(2, manager.cacheSize(), "cache must stay at the cap");
    assertEquals(3, creations.get(), "third distinct key forces a new creation");

    Dispatch<Source> firstReRequested =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "k1", PROFILE, DEFAULT_TIMEOUT);
    assertNotSame(first, firstReRequested, "k1 was the LRU entry and must have been evicted");
    assertEquals(4, creations.get(), "re-requesting an evicted key creates a fresh dispatch");

    Dispatch<Source> thirdReRequested =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "k3", PROFILE, DEFAULT_TIMEOUT);
    assertSame(third, thirdReRequested, "k3 must still be served from the cache");
    assertEquals(
        List.of(first, second), manager.closedDispatches, "evicted entries must be closed");
  }

  @Test
  @DisplayName("LRU order follows access, not insertion")
  void lruOrderFollowsAccess() throws Exception {
    manager.pinnedMaxDispatches = 2;
    Dispatch<Source> a =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opA", PROFILE, DEFAULT_TIMEOUT);
    Dispatch<Source> b =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opB", PROFILE, DEFAULT_TIMEOUT);
    Dispatch<Source> aAgain =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opA", PROFILE, DEFAULT_TIMEOUT);
    assertSame(a, aAgain, "A must be served from cache");

    manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opC", PROFILE, DEFAULT_TIMEOUT);

    assertEquals(2, manager.cacheSize());
    assertEquals(
        List.of(b), manager.closedDispatches, "B was least recently used and must be evicted");

    Dispatch<Source> aThird =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opA", PROFILE, DEFAULT_TIMEOUT);
    assertSame(a, aThird, "A remains cached after C's insertion evicted B");
  }

  @Test
  @DisplayName("shutdown closes all cached dispatches")
  void closeClosesAllDispatches() throws Exception {
    manager.pinnedMaxDispatches = 64;
    Dispatch<Source> a =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opA", PROFILE, DEFAULT_TIMEOUT);
    Dispatch<Source> b =
        manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opB", PROFILE, DEFAULT_TIMEOUT);
    assertEquals(2, manager.cacheSize());

    manager.close();

    assertEquals(
        List.of(a, b), manager.closedDispatches, "shutdown must close every cached dispatch");
    assertEquals(0, manager.cacheSize());
  }

  @Test
  @DisplayName("malformed dispatch cap env fails loud")
  void malformedDispatchCapFailsLoud() {
    IllegalStateException ex =
        assertThrows(IllegalStateException.class, () -> BridgeConfig.parseMaxDispatches("0"));

    assertTrue(ex.getMessage().contains("CXF_MAX_DISPATCHES"), ex.getMessage());
    assertTrue(ex.getMessage().contains("0"), ex.getMessage());
  }

  @Test
  @DisplayName("TLSClientParameters are attached to the HTTP conduit when TLS is configured")
  void tlsParametersAttachedWhenTlsConfigured() throws Exception {
    Path store = TestKeystoreHelper.createTestKeystore();
    SecurityProfile profile =
        SecurityProfile.builder(PROFILE)
            .tlsKeystore(store.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD, "JKS")
            .tlsTruststore(store.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD, "JKS")
            .build();
    when(profileStore.getProfile(PROFILE)).thenReturn(profile);

    HTTPConduit conduit = realConduit();
    stubConduitDispatch(conduit);

    manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opTls", PROFILE, DEFAULT_TIMEOUT);

    TLSClientParameters tls = conduit.getTlsClientParameters();
    assertNotNull(tls, "configured tls must install explicit TLSClientParameters on the conduit");
    assertNotNull(tls.getKeyManagers(), "the keystore must yield key managers");
    assertNotNull(tls.getTrustManagers(), "the truststore must yield trust managers");
    assertFalse(tls.isDisableCNCheck(), "hostname verification must stay enabled");
  }

  @Test
  @DisplayName("No TLS block leaves the conduit on JVM defaults")
  void noTlsBlockLeavesJvmDefaults() throws Exception {
    SecurityProfile profile = SecurityProfile.builder(PROFILE).build();
    when(profileStore.getProfile(PROFILE)).thenReturn(profile);

    HTTPConduit conduit = realConduit();
    stubConduitDispatch(conduit);

    manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opPlain", PROFILE, DEFAULT_TIMEOUT);

    assertNull(
        conduit.getTlsClientParameters(),
        "a profile without a tls block must not install explicit TLSClientParameters");
  }

  @Test
  @DisplayName("A wrong store password fails initTlsStores before any dispatch")
  void wrongStorePasswordFailsBeforeReady() throws Exception {
    Path store = TestKeystoreHelper.createTestKeystore();
    SecurityProfile profile =
        SecurityProfile.builder(PROFILE)
            .tlsKeystore(store.toString(), "not-the-store-password", "JKS")
            .build();
    when(profileStore.getProfile(PROFILE)).thenReturn(profile);
    when(profileStore.profiles()).thenReturn(Map.of(PROFILE, profile));

    assertThrows(IllegalStateException.class, () -> manager.initTlsStores());
    assertEquals(
        0, manager.cacheSize(), "no dispatch may be created before the store is validated");
    assertEquals(0, creations.get(), "initTlsStores must not create a dispatch");
  }

  /**
   * Returns a real CXF {@link HTTPConduit} through the mocked dispatch/client so the assertion
   * exercises the actual TLS-material application rather than verifying a mock method call.
   */
  private static HTTPConduit realConduit() throws Exception {
    return new URLConnectionHTTPConduit(
        BusFactory.getDefaultBus(), new EndpointInfo(), new EndpointReferenceType());
  }

  @SuppressWarnings({"unchecked", "rawtypes"})
  private void stubConduitDispatch(HTTPConduit conduit) {
    DispatchImpl<Source> dispatch = mock(DispatchImpl.class);
    when(dispatch.getRequestContext()).thenReturn(new HashMap<>());
    Client client = mock(Client.class);
    stubBindingOperations(client);
    when(client.getConduit()).thenReturn(conduit);
    when(dispatch.getClient()).thenReturn(client);
    when(mockService.createDispatch(any(QName.class), eq(Source.class), eq(Service.Mode.PAYLOAD)))
        .thenReturn((Dispatch) dispatch);
  }

  private Thread lookupWorker(String operation, AtomicInteger crossings, List<String> failures) {
    return new Thread(
        () -> {
          try {
            for (int i = 0; i < 200; i++) {
              Dispatch<Source> dispatch =
                  manager.getDispatch(
                      WSDL, ADDRESS, SERVICE, PORT, operation, PROFILE, DEFAULT_TIMEOUT);
              Object uri = dispatch.getRequestContext().get(SOAPACTION_URI);
              if (!("urn:test:" + operation).equals(uri)) {
                crossings.incrementAndGet();
              }
            }
          } catch (Exception e) {
            failures.add(operation + ": " + e);
          }
        },
        "worker-" + operation);
  }

  private Thread inferredLookupWorker(
      QName inputQName,
      String expectedAction,
      boolean expectedOneWay,
      AtomicInteger crossings,
      List<String> failures) {
    return new Thread(
        () -> {
          try {
            for (int i = 0; i < 200; i++) {
              CxfClientManager.ResolvedDispatch resolved =
                  manager.getResolvedDispatch(
                      WSDL, ADDRESS, SERVICE, PORT, null, PROFILE, DEFAULT_TIMEOUT, inputQName);
              Object uri = resolved.dispatch().getRequestContext().get(SOAPACTION_URI);
              if (!expectedAction.equals(uri) || resolved.operation().oneWay() != expectedOneWay) {
                crossings.incrementAndGet();
              }
            }
          } catch (Exception e) {
            failures.add(inputQName + ": " + e);
          }
        },
        "worker-" + inputQName.getLocalPart());
  }

  /** Records every close routed through the single closeQuietly implementation. */
  private static final class RecordingManager extends CxfClientManager {
    final List<Dispatch<Source>> closedDispatches = new CopyOnWriteArrayList<>();

    @Override
    void closeQuietly(Dispatch<Source> dispatch) {
      closedDispatches.add(dispatch);
    }
  }
}
