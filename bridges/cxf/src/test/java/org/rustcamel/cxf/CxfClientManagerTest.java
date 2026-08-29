package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import jakarta.xml.ws.Dispatch;
import jakarta.xml.ws.Service;
import java.net.URL;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicInteger;
import javax.xml.namespace.QName;
import javax.xml.transform.Source;
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

  @Mock SecurityProfileStore profileStore;

  RecordingManager manager;
  final AtomicInteger creations = new AtomicInteger();
  AutoCloseable openMocks;
  MockedStatic<Service> serviceStatic;

  @BeforeEach
  void setUp() {
    openMocks = MockitoAnnotations.openMocks(this);
    when(profileStore.getProfile(anyString())).thenReturn(mock(SecurityProfile.class));

    manager = new RecordingManager();
    manager.profileStore = profileStore;

    Service mockService = mock(Service.class);
    when(mockService.createDispatch(any(QName.class), eq(Source.class), eq(Service.Mode.PAYLOAD)))
        .thenAnswer(inv -> newMockDispatch());
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
    Dispatch<Source> dispatch = mock(Dispatch.class);
    Map<String, Object> context = new HashMap<>();
    when(dispatch.getRequestContext()).thenReturn(context);
    return dispatch;
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
    manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, null, PROFILE, DEFAULT_TIMEOUT);
    manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "opX", PROFILE, DEFAULT_TIMEOUT);
    manager.getDispatch(WSDL, ADDRESS, SERVICE, PORT, "   ", PROFILE, DEFAULT_TIMEOUT);

    assertEquals(2, manager.cacheSize(), "whitespace-only operation must normalize to blank key");
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
    assertEquals("opA", context.get(SOAPACTION_URI));

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

  private Thread lookupWorker(String operation, AtomicInteger crossings, List<String> failures) {
    return new Thread(
        () -> {
          try {
            for (int i = 0; i < 200; i++) {
              Dispatch<Source> dispatch =
                  manager.getDispatch(
                      WSDL, ADDRESS, SERVICE, PORT, operation, PROFILE, DEFAULT_TIMEOUT);
              Object uri = dispatch.getRequestContext().get(SOAPACTION_URI);
              if (!operation.equals(uri)) {
                crossings.incrementAndGet();
              }
            }
          } catch (Exception e) {
            failures.add(operation + ": " + e);
          }
        },
        "worker-" + operation);
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
