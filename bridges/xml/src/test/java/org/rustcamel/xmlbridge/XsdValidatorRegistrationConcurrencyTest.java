package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.protobuf.ByteString;
import java.lang.management.ManagementFactory;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import java.util.concurrent.locks.LockSupport;
import javax.xml.validation.Schema;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import org.xml.sax.SAXException;
import xml_bridge.BridgeError;
import xml_bridge.RegisterSchemaRequest;
import xml_bridge.RegisterSchemaResponse;
import xml_bridge.UnregisterSchemaRequest;
import xml_bridge.ValidateResponse;
import xml_bridge.ValidateWithRequest;

/**
 * Concurrency contract of {@link XsdValidatorService} registration: compiles run outside every
 * monitor, same-digest registrations compile once, a failed compile leaves no state, replacement
 * evicts only unreferenced digests, publication re-asserts the pin, and the legacy single-blob path
 * still works. Plain JUnit: the service is constructed directly and its compile hooks are
 * overridden as a test seam; no Quarkus context is booted. Coordination uses latches and bounded
 * thread-state loops only — no sleeps.
 */
@Timeout(value = 10, threadMode = Timeout.ThreadMode.SEPARATE_THREAD)
class XsdValidatorRegistrationConcurrencyTest {

  private static final String XS = "http://www.w3.org/2001/XMLSchema";

  // ── Compile outside the registry monitor ────────────────────────────────────

  @Test
  void unrelatedSnapshotNotBlockedByLargeRegistration() throws Exception {
    var entered = new CountDownLatch(1);
    var release = new CountDownLatch(1);
    var park = new AtomicBoolean(false);
    var service =
        new XsdValidatorService() {
          @Override
          Schema compileSuppliedSet(SchemaSetCompiler.SuppliedSet set) throws Exception {
            if (park.compareAndSet(true, false)) {
              entered.countDown();
              if (!release.await(30, TimeUnit.SECONDS)) {
                throw new IllegalStateException("test never released the parked compile");
              }
            }
            return super.compileSuppliedSet(set);
          }
        };
    service.schemaCache = new SchemaCache(256);

    var existing = set("urn:existing", "first");
    var existingDocument = document("urn:existing", "first");
    assertFalse(registerOn(service, "existing", "root.xsd", existing, Map.of()).hasError());
    assertNotNull(service.snapshotFor("existing"));
    assertTrue(validateWithOn(service, "existing", existingDocument).getValid());

    var response = new AtomicReference<RegisterSchemaResponse>();
    var done = new CountDownLatch(1);
    var large = largeResourceSet();
    park.set(true);
    var worker =
        new Thread(
            () -> {
              try {
                response.set(registerOn(service, "large", "root.xsd", large, Map.of()));
              } finally {
                done.countDown();
              }
            });
    worker.setDaemon(true);
    try {
      worker.start();
      assertTrue(entered.await(5, TimeUnit.SECONDS), "the large compile must enter");

      // The compile is parked. A snapshot for an unrelated id must not wait for it.
      var snapshot = service.snapshotFor("existing");
      assertNotNull(snapshot);
      assertNotNull(snapshot.schema());
      assertTrue(validateWithOn(service, "existing", existingDocument).getValid());
    } finally {
      release.countDown();
      worker.join(5000);
    }
    assertTrue(done.await(5, TimeUnit.SECONDS), "the large registration must finish");
    assertFalse(response.get().hasError());
    assertFalse(worker.isAlive());
  }

  // ── Single-flight ───────────────────────────────────────────────────────────

  @Test
  void sameDigestCompilesOnce() throws Exception {
    var compileCount = new AtomicInteger();
    var entered = new CountDownLatch(1);
    var release = new CountDownLatch(1);
    var service =
        new XsdValidatorService() {
          @Override
          Schema compileSuppliedSet(SchemaSetCompiler.SuppliedSet set) throws Exception {
            if (compileCount.incrementAndGet() == 1) {
              entered.countDown();
              if (!release.await(30, TimeUnit.SECONDS)) {
                throw new IllegalStateException("test never released the parked compile");
              }
            }
            return super.compileSuppliedSet(set);
          }
        };
    service.schemaCache = new SchemaCache(256);

    var shared = set("urn:shared", "first");
    var sharedDocument = document("urn:shared", "first");
    var firstResponse = new AtomicReference<RegisterSchemaResponse>();
    var firstDone = new CountDownLatch(1);
    var first =
        new Thread(
            () -> {
              try {
                firstResponse.set(registerOn(service, "one", "root.xsd", shared, Map.of()));
              } finally {
                firstDone.countDown();
              }
            });
    first.setDaemon(true);
    var secondStarted = new CountDownLatch(1);
    var secondResponse = new AtomicReference<RegisterSchemaResponse>();
    var secondDone = new CountDownLatch(1);
    var second =
        new Thread(
            () -> {
              secondStarted.countDown();
              try {
                secondResponse.set(registerOn(service, "two", "root.xsd", shared, Map.of()));
              } finally {
                secondDone.countDown();
              }
            });
    second.setDaemon(true);
    try {
      first.start();
      assertTrue(entered.await(5, TimeUnit.SECONDS), "the first compile must start");

      second.start();
      assertTrue(secondStarted.await(5, TimeUnit.SECONDS), "the second registration must start");
      // T1 is parked inside the compile hook, before publication: the digest has no
      // cache entry and T1 holds no registryLock. T2 therefore cannot take the
      // getAndPin fast path or block on registryLock; the only reachable wait is
      // joinFlight's WAITING on the leader future. Observing that state proves a
      // joiner exists before the compile is released.
      awaitWaiting(second, System.nanoTime() + TimeUnit.SECONDS.toNanos(5));
    } finally {
      release.countDown();
      first.join(5000);
      second.join(5000);
    }
    assertTrue(firstDone.await(5, TimeUnit.SECONDS), "the first registration must finish");
    assertTrue(secondDone.await(5, TimeUnit.SECONDS), "the second registration must finish");

    assertFalse(firstResponse.get().hasError());
    assertFalse(secondResponse.get().hasError());
    assertEquals(1, compileCount.get(), "same-digest registrations must compile exactly once");
    assertTrue(validateWithOn(service, "one", sharedDocument).getValid());
    assertTrue(validateWithOn(service, "two", sharedDocument).getValid());
  }

  // ── Failure atomicity ───────────────────────────────────────────────────────

  @Test
  void failedCompileLeavesStateUntouched() {
    var fail = new AtomicBoolean(true);
    var service =
        new XsdValidatorService() {
          @Override
          Schema compileSuppliedSet(SchemaSetCompiler.SuppliedSet set) throws Exception {
            if (fail.get()) {
              throw new SAXException("boom");
            }
            return super.compileSuppliedSet(set);
          }
        };
    service.schemaCache = new SchemaCache(256);

    var broken = set("urn:broken", "first");
    var brokenDigest = digestOf("root.xsd", broken, Map.of());
    var sizeBefore = service.schemaCache.size();

    var failed = registerOn(service, "f", "root.xsd", broken, Map.of());
    assertTrue(failed.hasError());
    assertEquals(BridgeError.Kind.COMPILATION_FAILED, failed.getError().getKind());
    assertNull(service.snapshotFor("f"));
    assertNull(service.schemaCache.get(brokenDigest), "a failed compile must cache nothing");
    assertFalse(service.schemaCache.isPinned(brokenDigest), "a failed compile must pin nothing");
    assertEquals(sizeBefore, service.schemaCache.size());

    fail.set(false);
    var recovered = set("urn:recovered", "ok");
    assertFalse(registerOn(service, "f", "root.xsd", recovered, Map.of()).hasError());
    assertTrue(validateWithOn(service, "f", document("urn:recovered", "ok")).getValid());
  }

  // ── Failed-compile joiner parity ────────────────────────────────────────────

  @Test
  void failedCompileJoinerSurfacesLeaderError() throws Exception {
    var armFail = new AtomicBoolean(true);
    var park = new AtomicBoolean(false);
    var entered = new CountDownLatch(1);
    var release = new CountDownLatch(1);
    var service =
        new XsdValidatorService() {
          @Override
          Schema compileSuppliedSet(SchemaSetCompiler.SuppliedSet set) throws Exception {
            if (park.compareAndSet(true, false)) {
              entered.countDown();
              if (!release.await(30, TimeUnit.SECONDS)) {
                throw new IllegalStateException("test never released the parked compile");
              }
            }
            if (armFail.get()) {
              throw new SAXException("boom-joiner");
            }
            return super.compileSuppliedSet(set);
          }
        };
    service.schemaCache = new SchemaCache(256);

    var shared = set("urn:joiner", "first");
    var digest = digestOf("root.xsd", shared, Map.of());

    var firstResponse = new AtomicReference<RegisterSchemaResponse>();
    var firstDone = new CountDownLatch(1);
    var first =
        new Thread(
            () -> {
              try {
                firstResponse.set(registerOn(service, "j1", "root.xsd", shared, Map.of()));
              } finally {
                firstDone.countDown();
              }
            });
    first.setDaemon(true);
    var secondStarted = new CountDownLatch(1);
    var secondResponse = new AtomicReference<RegisterSchemaResponse>();
    var secondDone = new CountDownLatch(1);
    var second =
        new Thread(
            () -> {
              secondStarted.countDown();
              try {
                secondResponse.set(registerOn(service, "j2", "root.xsd", shared, Map.of()));
              } finally {
                secondDone.countDown();
              }
            });
    second.setDaemon(true);
    park.set(true);
    try {
      first.start();
      assertTrue(entered.await(5, TimeUnit.SECONDS), "the failing compile must start");
      second.start();
      assertTrue(secondStarted.await(5, TimeUnit.SECONDS), "the joiner must start");
      // T1 is parked in the hook before caching or publishing, so T2 can only be
      // WAITING inside joinFlight on the leader future.
      awaitWaiting(second, System.nanoTime() + TimeUnit.SECONDS.toNanos(5));
    } finally {
      release.countDown();
      first.join(5000);
      second.join(5000);
    }
    assertTrue(firstDone.await(5, TimeUnit.SECONDS), "the leader must finish");
    assertTrue(secondDone.await(5, TimeUnit.SECONDS), "the joiner must finish");
    assertFalse(first.isAlive());
    assertFalse(second.isAlive());

    // The joiner must surface the leader's exception type, so registerSchema's
    // catch chain maps it to COMPILATION_FAILED rather than INTERNAL.
    assertTrue(firstResponse.get().hasError());
    assertEquals(BridgeError.Kind.COMPILATION_FAILED, firstResponse.get().getError().getKind());
    assertTrue(secondResponse.get().hasError());
    assertEquals(BridgeError.Kind.COMPILATION_FAILED, secondResponse.get().getError().getKind());
    assertNull(service.snapshotFor("j1"));
    assertNull(service.snapshotFor("j2"));
    assertNull(service.schemaCache.get(digest), "a failed joiner compile must cache nothing");
    assertFalse(service.schemaCache.isPinned(digest), "a failed joiner compile must pin nothing");

    armFail.set(false);
    var recovered = set("urn:recovered-joiner", "ok");
    assertFalse(registerOn(service, "j1", "root.xsd", recovered, Map.of()).hasError());
    assertTrue(validateWithOn(service, "j1", document("urn:recovered-joiner", "ok")).getValid());
  }

  // ── Reference-counted eviction ──────────────────────────────────────────────

  @Test
  void replacementEvictsOnlyUnreferencedDigest() {
    var service = new XsdValidatorService();
    service.schemaCache = new SchemaCache(256);

    var setX = set("urn:x", "first");
    var setY = set("urn:y", "second");
    var setZ = set("urn:z", "third");
    var digestX = digestOf("root.xsd", setX, Map.of());
    var digestY = digestOf("root.xsd", setY, Map.of());
    var digestZ = digestOf("root.xsd", setZ, Map.of());

    assertFalse(registerOn(service, "a", "root.xsd", setX, Map.of()).hasError());
    assertFalse(registerOn(service, "b", "root.xsd", setX, Map.of()).hasError());
    assertNotNull(service.schemaCache.get(digestX));
    assertTrue(service.schemaCache.isPinned(digestX));

    assertFalse(registerOn(service, "a", "root.xsd", setY, Map.of()).hasError());
    assertNotNull(service.schemaCache.get(digestX), "b still references the old digest");
    assertTrue(service.schemaCache.isPinned(digestX));
    assertEquals(digestY, service.snapshotFor("a").digest());

    assertFalse(registerOn(service, "b", "root.xsd", setZ, Map.of()).hasError());
    assertNull(service.schemaCache.get(digestX), "the unreferenced digest must be removed");
    assertFalse(service.schemaCache.isPinned(digestX));
    assertEquals(digestZ, service.snapshotFor("b").digest());

    assertTrue(validateWithOn(service, "a", document("urn:y", "second")).getValid());
    assertTrue(validateWithOn(service, "b", document("urn:z", "third")).getValid());
  }

  // ── Legacy single-blob path ─────────────────────────────────────────────────

  @Test
  void legacySingleBlobPathStillWorks() {
    var service = new XsdValidatorService();
    service.schemaCache = new SchemaCache(256);

    var oldXsd = legacySchema("legacy", "v");
    var oldDigest = SchemaSetCompiler.sha256Hex(oldXsd.getBytes(StandardCharsets.UTF_8));

    assertFalse(registerBlobOn(service, "legacy", oldXsd).hasError());
    assertTrue(validateWithOn(service, "legacy", "<legacy><v>x</v></legacy>").getValid());
    assertTrue(service.schemaCache.isPinned(oldDigest));

    assertFalse(registerBlobOn(service, "legacy", legacySchema("legacy2", "w")).hasError());
    assertNull(service.schemaCache.get(oldDigest), "the replaced blob digest must be evicted");
    assertFalse(service.schemaCache.isPinned(oldDigest));
    assertTrue(validateWithOn(service, "legacy", "<legacy2><w>x</w></legacy2>").getValid());
  }

  // ── Publication re-asserts the pin ──────────────────────────────────────────

  @Test
  void pinSurvivesRacingReplacementUntilPublication() throws Exception {
    var service = new XsdValidatorService();
    service.schemaCache = new SchemaCache(256);

    var setX = set("urn:race", "first");
    var raceDocument = document("urn:race", "first");
    var digest = digestOf("root.xsd", setX, Map.of());
    assertFalse(registerOn(service, "a", "root.xsd", setX, Map.of()).hasError());
    assertEquals(digest, service.snapshotFor("a").digest());

    var response = new AtomicReference<RegisterSchemaResponse>();
    var done = new CountDownLatch(1);
    var worker =
        new Thread(
            () -> {
              try {
                response.set(registerOn(service, "b", "root.xsd", setX, Map.of()));
              } finally {
                done.countDown();
              }
            });
    worker.setDaemon(true);
    try {
      synchronized (service.registryLock()) {
        worker.start();
        // B already pinned D with getAndPin and is blocked entering the publish
        // section. On a service that does not re-pin at publication, the eviction
        // below removes D and B then publishes id->D with no cache entry, so the
        // snapshot assertions fail. Monitors are reentrant, so the eviction is
        // legal on this thread while it owns the monitor.
        awaitBlockedOn(
            service.registryLock(),
            worker,
            Thread.currentThread(),
            System.nanoTime() + TimeUnit.SECONDS.toNanos(5));
        service
            .unregisterSchema(UnregisterSchemaRequest.newBuilder().setSchemaId("a").build())
            .await()
            .atMost(Duration.ofSeconds(5));
      }
      assertTrue(done.await(5, TimeUnit.SECONDS), "the racing registration must finish");
    } finally {
      worker.join(5000);
    }

    assertFalse(response.get().hasError());
    var snapshot = service.snapshotFor("b");
    assertNotNull(snapshot);
    assertNotNull(snapshot.schema(), "publication must re-insert the pinned schema");
    assertTrue(validateWithOn(service, "b", raceDocument).getValid());
    assertNotNull(service.schemaCache.get(digest));
    assertTrue(service.schemaCache.isPinned(digest));
  }

  // ── Helpers ─────────────────────────────────────────────────────────────────

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
    return service.registerSchema(builder.build()).await().atMost(Duration.ofSeconds(5));
  }

  private static RegisterSchemaResponse registerBlobOn(
      XsdValidatorService service, String schemaId, String xsd) {
    return service
        .registerSchema(
            RegisterSchemaRequest.newBuilder().setSchemaId(schemaId).setSchema(bytes(xsd)).build())
        .await()
        .atMost(Duration.ofSeconds(5));
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
        .atMost(Duration.ofSeconds(5));
  }

  private static String digestOf(
      String rootSystemId, Map<String, String> resources, Map<String, String> aliases) {
    var raw = new HashMap<String, byte[]>();
    resources.forEach((key, value) -> raw.put(key, value.getBytes(StandardCharsets.UTF_8)));
    return SchemaSetCompiler.schemaSetDigest(rootSystemId, raw, aliases);
  }

  /**
   * Boundedly waits until {@code thread} is WAITING, without sleeping or an unbounded spin. Used to
   * prove a joiner reached {@code joinFlight} before the leader's compile is released.
   */
  private static void awaitWaiting(Thread thread, long deadlineNanos) {
    while (System.nanoTime() < deadlineNanos) {
      if (!thread.isAlive()) {
        throw new AssertionError("thread finished without waiting for the in-flight compile");
      }
      if (thread.getState() == Thread.State.WAITING) {
        return;
      }
      LockSupport.parkNanos(1_000_000L);
    }
    throw new AssertionError("thread did not reach WAITING before the deadline");
  }

  /**
   * Boundedly proves {@code worker} is BLOCKED entering the {@code monitor} owned by {@code owner},
   * without sleeping or an unbounded spin. Adapted from the integration suite's helper.
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

  /** A valid set: a root that imports a child type requiring {@code requiredElement}. */
  private static Map<String, String> set(String childNamespace, String requiredElement) {
    return res(
        "root.xsd",
        importSchema(childNamespace, "child.xsd", "noteType"),
        "child.xsd",
        schemaType(childNamespace, "noteType", requiredElement));
  }

  /** 256 trivially valid resources: one standalone root plus 255 extra parsed resources. */
  private static Map<String, String> largeResourceSet() {
    var resources = new LinkedHashMap<String, String>();
    resources.put(
        "root.xsd",
        """
        <xs:schema xmlns:xs="%s" targetNamespace="urn:large" elementFormDefault="qualified">
          <xs:element name="root" type="xs:string"/>
        </xs:schema>
        """
            .formatted(XS));
    for (int i = 0; i < 255; i++) {
      resources.put("r/" + i + ".xsd", "<xs:schema xmlns:xs=\"" + XS + "\"/>");
    }
    return resources;
  }

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

  private static String legacySchema(String rootElement, String childElement) {
    return """
        <xs:schema xmlns:xs="%s">
          <xs:element name="%s">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="%s" type="xs:string"/>
              </xs:sequence>
            </xs:complexType>
          </xs:element>
        </xs:schema>
        """
        .formatted(XS, rootElement, childElement);
  }

  private static String document(String childNamespace, String element) {
    return "<note xmlns=\"urn:root\" xmlns:c=\""
        + childNamespace
        + "\"><c:"
        + element
        + ">value</c:"
        + element
        + "></note>";
  }

  private static Map<String, String> res(String... pairs) {
    var map = new LinkedHashMap<String, String>();
    for (int i = 0; i < pairs.length; i += 2) {
      map.put(pairs[i], pairs[i + 1]);
    }
    return map;
  }

  private static ByteString bytes(String value) {
    return ByteString.copyFrom(value, StandardCharsets.UTF_8);
  }
}
