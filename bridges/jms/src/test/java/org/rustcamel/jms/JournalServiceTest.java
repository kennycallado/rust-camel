package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.spy;

import java.io.IOException;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.channels.FileChannel;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.nio.file.attribute.BasicFileAttributes;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.Callable;
import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.condition.DisabledOnOs;
import org.junit.jupiter.api.condition.OS;
import org.junit.jupiter.api.io.TempDir;

/**
 * Unit tests for {@link JournalService} framing, replay, and torn-tail handling (jms-bridge-080
 * task 2.1) plus the process-wide instance API, lazy OS file lock, and owner-private files (task
 * 2.2). Frames are written directly against the on-disk contract the reader parses, so the reader
 * is verified against independently constructed bytes rather than its own writer.
 *
 * <p>The owner-private permission assertions and the lock-file inode check are POSIX syscall
 * assertions; they are disabled on Windows, which has no POSIX mode and no file key. The busy-lock
 * and concurrent-acquire tests use bounded waits: the busy path asserts a bounded elapsed interval
 * and the concurrent path bounds {@code ExecutorService.invokeAll}, so a regression fails fast
 * instead of hanging.
 */
@DisplayName("JournalService framing, replay, and torn tail")
class JournalServiceTest {

  @TempDir Path tempDir;

  @Test
  void replays_framed_records_preserving_counts() throws Exception {
    Path journal = tempDir.resolve("journal.bin");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:1", 1000L);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:1", 2000L);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:2", 3000L);
    }

    JournalService.JournalState state = JournalService.replay(journal);

    assertEquals(2L, state.count("ID:1"), "two reservations for ID:1");
    assertEquals(1L, state.count("ID:2"), "one reservation for ID:2");
    assertEquals(0L, state.count("ID:absent"), "an absent id has no count");
    assertEquals(2000L, state.lastTouchedMs("ID:1"), "the last RESERVE refreshes last-touched");
    assertTrue(state.inFlight("ID:1"), "a RESERVE marks the entry in-flight");
  }

  @Test
  void torn_tail_is_discarded_and_confirmed_counts_kept() throws Exception {
    Path journal = tempDir.resolve("journal.bin");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:1", 1000L);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:2", 2000L);
      writeTornRecord(out, JournalService.TYPE_RESERVE, "ID:torn", 3000L);
    }

    JournalService.JournalState state = JournalService.replay(journal);

    assertEquals(1L, state.count("ID:1"), "an earlier confirmed count survives a torn tail");
    assertEquals(1L, state.count("ID:2"), "an earlier confirmed count survives a torn tail");
    assertEquals(0L, state.count("ID:torn"), "the torn frame is discarded");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:1", 1000L);
      out.write(new byte[] {0, 0});
    }
    assertEquals(1L, JournalService.replay(journal).count("ID:1"));
  }

  @Test
  void rollback_clears_in_flight_only() throws Exception {
    Path journal = tempDir.resolve("journal.bin");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:1", 1000L);
      writeRecord(out, JournalService.TYPE_ROLLBACK, "ID:1", 2000L);
    }

    JournalService.JournalState state = JournalService.replay(journal);

    assertEquals(1L, state.count("ID:1"), "a ROLLBACK does not change the count");
    assertFalse(state.inFlight("ID:1"), "a ROLLBACK clears in-flight");
    assertTrue(state.contains("ID:1"), "the entry is kept");
  }

  @Test
  void commit_and_retire_remove_entries() throws Exception {
    Path journal = tempDir.resolve("journal.bin");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:commit", 1000L);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:commit", 1100L);
      writeRecord(out, JournalService.TYPE_COMMIT, "ID:commit", 1200L);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:retire", 2000L);
      writeRecord(out, JournalService.TYPE_RETIRE, "ID:retire", 2100L);
    }

    JournalService.JournalState state = JournalService.replay(journal);

    assertEquals(0L, state.count("ID:commit"), "COMMIT removes the entry");
    assertFalse(state.contains("ID:commit"), "COMMIT removes the entry");
    assertEquals(0L, state.count("ID:retire"), "RETIRE removes the entry");
    assertFalse(state.contains("ID:retire"), "RETIRE removes the entry");
  }

  @Test
  void lazy_acquire_release_allows_sequential_owner() throws Exception {
    JournalService service = newService();
    try (JournalService.JournalHandle first = service.acquire("broker-a", "queue-a", 1000L)) {
      first.reserve("ID:1");
      first.reserve("ID:1");
    }
    try (JournalService.JournalHandle second = service.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(2L, second.count("ID:1"), "a sequential owner inherits the retained count");
      assertEquals(0L, second.count("ID:absent"), "an absent id has no count");
    }
  }

  @Test
  void busy_lock_waits_bounded_then_fails_precondition() throws Exception {
    JournalService owner = newService();
    JournalService other = newService();
    try (JournalService.JournalHandle held = owner.acquire("broker-a", "queue-a", 1000L)) {
      long start = System.nanoTime();
      JournalPermanentException failure =
          assertThrows(
              JournalPermanentException.class, () -> other.acquire("broker-a", "queue-a", 150L));
      long elapsedMs = (System.nanoTime() - start) / 1_000_000L;
      assertTrue(
          failure.getMessage().contains("JMS_ATTEMPT_JOURNAL_DIR"),
          "the busy message names the journal directory variable");
      assertTrue(elapsedMs >= 100L, "the wait is bounded below; actual " + elapsedMs + " ms");
      assertTrue(elapsedMs < 5000L, "the wait is bounded above; actual " + elapsedMs + " ms");
      assertTrue(held.count("ID:1") == 0L, "the busy acquire does not disturb the owner");
    }
  }

  @Test
  @DisabledOnOs(OS.WINDOWS)
  void lock_file_is_never_replaced() throws Exception {
    JournalService service = newService();
    Path lockFile = tempDir.resolve("broker-a").resolve("queue-a.lock");
    JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L);
    Object before = Files.readAttributes(lockFile, BasicFileAttributes.class).fileKey();
    assertNotNull(before, "the filesystem reports a file key for the lock");
    handle.reserve("ID:1");
    handle.close();
    Object after = Files.readAttributes(lockFile, BasicFileAttributes.class).fileKey();
    assertEquals(before, after, "the lock file inode is never replaced");
    assertTrue(Files.exists(lockFile), "the lock file is never deleted");
  }

  @Test
  @DisabledOnOs(OS.WINDOWS)
  void journal_files_are_owner_private() throws Exception {
    JournalService service = newService();
    JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L);
    try {
      handle.reserve("ID:1");
      Path dir = tempDir.resolve("broker-a");
      assertEquals(
          PosixFilePermissions.fromString("rwx------"),
          Files.getPosixFilePermissions(dir),
          "the broker directory is 0700");
      for (String name : List.of("queue-a.journal", "queue-a.lock", "queue-a.snapshot")) {
        assertEquals(
            PosixFilePermissions.fromString("rw-------"),
            Files.getPosixFilePermissions(dir.resolve(name)),
            name + " is 0600");
      }
    } finally {
      handle.close();
    }
  }

  @Test
  void repeated_acquire_shares_one_process_owner() throws Exception {
    JournalService service = newService();
    JournalService.JournalHandle first = service.acquire("broker-a", "queue-a", 1000L);
    JournalService.JournalHandle second = service.acquire("broker-a", "queue-a", 1000L);
    first.reserve("ID:1");
    assertEquals(1L, second.count("ID:1"), "the second handle sees the shared count");
    second.reserve("ID:1");
    assertEquals(2L, first.count("ID:1"), "the first handle sees the shared count");
    first.close();
    assertEquals(2L, second.count("ID:1"), "the lock stays while one handle remains");
    second.close();
    try (JournalService.JournalHandle third = service.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(2L, third.count("ID:1"), "a later owner sees the retained count");
    }
  }

  @Test
  void concurrent_acquire_same_domain_is_serialized() throws Exception {
    JournalService service = newService();
    ExecutorService pool = Executors.newFixedThreadPool(8);
    List<Future<JournalService.JournalHandle>> futures = List.of();
    try {
      List<Callable<JournalService.JournalHandle>> tasks = new ArrayList<>();
      for (int i = 0; i < 8; i++) {
        tasks.add(() -> service.acquire("broker-a", "queue-a", 5000L));
      }
      futures = pool.invokeAll(tasks, 5, TimeUnit.SECONDS);
    } finally {
      pool.shutdownNow();
    }
    List<JournalService.JournalHandle> handles = new ArrayList<>();
    try {
      for (Future<JournalService.JournalHandle> future : futures) {
        assertFalse(future.isCancelled(), "acquire did not hang past the bounded await");
        handles.add(future.get());
      }
      handles.get(0).reserve("ID:1");
      assertEquals(1L, handles.get(7).count("ID:1"), "all concurrent owners share one journal");
    } finally {
      for (JournalService.JournalHandle handle : handles) {
        handle.close();
      }
    }
  }

  @Test
  void acquire_rejects_path_traversal() throws Exception {
    JournalService service = newService();
    assertThrows(
        JournalPermanentException.class, () -> service.acquire("../escape", "queue-a", 1000L));
    assertThrows(
        JournalPermanentException.class,
        () -> service.acquire("broker-a", "../../etc/passwd", 1000L));
    assertThrows(
        JournalPermanentException.class, () -> service.acquire("broker-a", "nested/queue", 1000L));
    assertFalse(
        Files.exists(tempDir.getParent().resolve("escape.journal")),
        "no file escapes the journal root");
  }

  @Test
  void journal_path_uses_hashed_keys() throws Exception {
    // The broker key is Rust's 32-hex digest; the destination key is Java's SHA-256 prefix. Both
    // are opaque, so no raw broker URL, credential, or destination name can reach a file path.
    String brokerKey = "0123456789abcdef0123456789abcdef";
    String rawDestination = "queue:orders/secret user:pass@broker";
    JournalService service = newService();
    String destinationKey = JournalService.destinationKey(rawDestination);
    assertTrue(brokerKey.matches("^[0-9a-f]{32}$"), "the broker key is 32 lowercase hex");
    assertTrue(
        destinationKey.matches("^[0-9a-f]{32}$"),
        "the destination key is 32 lowercase hex: " + destinationKey);

    try (JournalService.JournalHandle handle = service.acquire(brokerKey, destinationKey, 1000L)) {
      handle.reserve("ID:1");
      Path journal = tempDir.resolve(brokerKey).resolve(destinationKey + ".journal");
      assertTrue(
          Files.exists(journal),
          "the journal path must be <dir>/<broker-key>/<destination-key>.journal");
      String path = journal.toString();
      assertFalse(path.contains(rawDestination), "no raw destination may appear in a path");
      assertFalse(path.contains("user:pass"), "no credential may appear in a path");
      assertFalse(path.contains("://"), "no raw broker URL may appear in a path");
      assertEquals(
          destinationKey + ".journal",
          journal.getFileName().toString(),
          "the file name is the hashed destination key plus .journal");
      assertEquals(
          brokerKey,
          journal.getParent().getFileName().toString(),
          "the broker namespace is the hashed key");
    }
  }

  @Test
  void busy_lock_on_one_domain_does_not_block_others() throws Exception {
    JournalService owner = newService();
    JournalService service = newService();
    try (JournalService.JournalHandle busy = owner.acquire("broker-a", "busy-queue", 1000L)) {
      AtomicReference<Throwable> busyFailure = new AtomicReference<>();
      Thread busyThread =
          new Thread(
              () -> {
                try {
                  service.acquire("broker-a", "busy-queue", 1500L);
                } catch (Throwable t) {
                  busyFailure.set(t);
                }
              },
              "busy-domain-acquire");
      busyThread.start();
      awaitBoundedWait(busyThread);
      ExecutorService pool = Executors.newSingleThreadExecutor();
      Future<JournalService.JournalHandle> other =
          pool.submit(() -> service.acquire("broker-a", "other-queue", 1000L));
      try {
        JournalService.JournalHandle handle = other.get(1, TimeUnit.SECONDS);
        handle.reserve("ID:y");
        assertEquals(1L, handle.count("ID:y"));
        handle.close();
      } catch (TimeoutException e) {
        fail("acquiring and closing another domain must not wait for the busy-domain lock");
      } finally {
        other.cancel(true);
        pool.shutdownNow();
        busyThread.join(5_000L);
      }
      assertTrue(
          busyFailure.get() instanceof JournalPermanentException,
          "the busy acquire still fails after its bounded wait");
      assertFalse(busyThread.isAlive(), "the busy acquire thread finished");
    }
  }

  @Test
  void shared_handles_reserve_increments_are_deterministic() throws Exception {
    JournalService service = newService();
    JournalService.JournalHandle first = service.acquire("broker-a", "queue-a", 1000L);
    JournalService.JournalHandle second = service.acquire("broker-a", "queue-a", 1000L);
    int perThread = 100;
    CyclicBarrier barrier = new CyclicBarrier(2);
    List<Throwable> failures = Collections.synchronizedList(new ArrayList<>());
    Thread one = new Thread(() -> reserveRepeatedly(first, perThread, barrier, failures));
    Thread two = new Thread(() -> reserveRepeatedly(second, perThread, barrier, failures));
    one.start();
    two.start();
    one.join(30_000L);
    two.join(30_000L);
    first.close();
    second.close();
    assertFalse(one.isAlive(), "the first reserving thread finished");
    assertFalse(two.isAlive(), "the second reserving thread finished");
    assertEquals(List.of(), failures, "no concurrent reservation failed");
    assertEquals(
        2L * perThread, first.count("ID:shared"), "shared reserves are all counted exactly");
  }

  private static void reserveRepeatedly(
      JournalService.JournalHandle handle, int times, CyclicBarrier barrier, List<Throwable> out) {
    try {
      barrier.await(5, TimeUnit.SECONDS);
      for (int i = 0; i < times; i++) {
        handle.reserve("ID:shared");
      }
    } catch (Throwable t) {
      out.add(t);
    }
  }

  /** Spins (bounded) until the acquire thread is sleeping in its bounded lock wait. */
  private static void awaitBoundedWait(Thread thread) {
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2);
    while (thread.getState() != Thread.State.TIMED_WAITING) {
      if (System.nanoTime() > deadline) {
        fail("the busy acquire did not reach its bounded wait; state=" + thread.getState());
      }
      Thread.onSpinWait();
    }
  }

  @Test
  void reserve_persists_before_return() throws Exception {
    JournalService service = newService();
    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      handle.reserve("ID:1");
    }

    Path journal = tempDir.resolve("broker-a").resolve("queue-a.journal");
    JournalService.JournalState onDisk = JournalService.replay(journal);
    assertEquals(1L, onDisk.count("ID:1"), "the reservation is durable on disk");
    assertTrue(onDisk.inFlight("ID:1"), "the durable RESERVE is in-flight");

    try (JournalService.JournalHandle reopened = service.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(1L, reopened.count("ID:1"), "a fresh open sees the durable reservation");
    }
  }

  @Test
  void reacquire_replays_durable_appends_from_another_owner() throws Exception {
    JournalService first = newService();
    try (JournalService.JournalHandle handle = first.acquire("broker-a", "queue-a", 1000L)) {
      handle.reserve("ID:1");
    }

    JournalService second = newService();
    try (JournalService.JournalHandle handle = second.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(1L, handle.count("ID:1"), "the second owner replays the durable count");
      handle.reserve("ID:1");
    }

    try (JournalService.JournalHandle handle = first.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(2L, handle.count("ID:1"), "a re-lock replays another owner's durable append");
    }
  }

  @Test
  void commit_and_rollback_are_durable() throws Exception {
    JournalService service = newService();
    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      handle.reserve("ID:commit");
      handle.commit("ID:commit");
      handle.reserve("ID:rollback");
      handle.rollback("ID:rollback");
      assertEquals(1L, handle.count("ID:rollback"), "a ROLLBACK keeps the count in memory");
    }

    Path journal = tempDir.resolve("broker-a").resolve("queue-a.journal");
    JournalService.JournalState onDisk = JournalService.replay(journal);
    assertEquals(0L, onDisk.count("ID:commit"), "the durable COMMIT removes the entry");
    assertFalse(onDisk.contains("ID:commit"), "the durable COMMIT removes the entry");
    assertEquals(1L, onDisk.count("ID:rollback"), "the durable ROLLBACK keeps the count");
    assertFalse(onDisk.inFlight("ID:rollback"), "the durable ROLLBACK clears in-flight");
  }

  @Test
  void failed_fsync_leaves_the_reservation_unrecorded() throws Exception {
    JournalService service =
        new JournalService(
            tempDir,
            4_194_304L,
            86_400_000L,
            System::currentTimeMillis,
            journal -> {
              FileChannel real =
                  FileChannel.open(journal, StandardOpenOption.READ, StandardOpenOption.WRITE);
              FileChannel failing = spy(real);
              doThrow(new IOException("simulated fsync failure")).when(failing).force(true);
              return failing;
            });

    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      assertThrows(JournalPermanentException.class, () -> handle.reserve("ID:1"));
      assertEquals(0L, handle.count("ID:1"), "a failed fsync never advances in-memory state");
      assertThrows(
          JournalPermanentException.class,
          () -> handle.reserve("ID:2"),
          "the poisoned session rejects the follow-up mutation");
    }

    // The poison lives on the cached per-domain session, so it survives the lock handoff. Reacquire
    // itself only replays and does not check the flag, so it returns; the next mutation still fails
    // closed.
    try (JournalService.JournalHandle reopened = service.acquire("broker-a", "queue-a", 1000L)) {
      assertThrows(
          JournalPermanentException.class,
          () -> reopened.reserve("ID:2"),
          "a reacquired poisoned session stays fail-closed on mutation");
    }
  }

  @Test
  void reserve_fails_closed_when_the_ceiling_is_exhausted() throws Exception {
    long frame = JournalService.frameLength("ID:a");
    long maxBytes = JournalService.DESTINATION_FLOOR_BYTES + frame + frame;
    JournalService service =
        new JournalService(tempDir, maxBytes, 86_400_000L, System::currentTimeMillis);

    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      handle.reserve("ID:a");
      assertEquals(1L, handle.count("ID:a"), "the first reservation fits exactly");
      assertThrows(JournalPermanentException.class, () -> handle.reserve("ID:b"));
      assertEquals(0L, handle.count("ID:b"), "the rejected reservation is not recorded");
      assertEquals(1L, handle.count("ID:a"), "no live reservation is evicted");
    }
  }

  @Test
  void ceiling_includes_snapshot_peak() {
    long maxBytes = 1000L;
    long journalBytes = 500L;
    long reservedBytes = 100L;

    assertFalse(
        JournalService.exceedsCeiling(journalBytes, reservedBytes, 0L, maxBytes),
        "the live journal fits without a snapshot");
    assertTrue(
        JournalService.exceedsCeiling(journalBytes, reservedBytes, 400L, maxBytes),
        "the temporary snapshot peak is counted in the ceiling");
    assertFalse(
        JournalService.exceedsCeiling(
            journalBytes,
            reservedBytes,
            maxBytes - journalBytes - reservedBytes - JournalService.DESTINATION_FLOOR_BYTES,
            maxBytes),
        "exactly the ceiling is still allowed");
  }

  @Test
  void snapshot_replace_is_atomic_and_parent_dir_synced() throws Exception {
    JournalService service = newService();
    Path dir = tempDir.resolve("broker-a");
    Path journal = dir.resolve("queue-a.journal");
    Path lock = dir.resolve("queue-a.lock");
    Path snapshot = dir.resolve("queue-a.snapshot");
    try (JournalService.JournalHandle warm = service.acquire("broker-a", "queue-a", 1000L)) {
      warm.reserve("ID:warm");
      warm.commit("ID:warm");
    }

    writeGarbageJournal(journal, 40, "ID:dead", "ID:live");
    long garbageBytes = Files.size(journal);
    Object lockBefore = Files.readAttributes(lock, BasicFileAttributes.class).fileKey();

    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      assertTrue(Files.size(journal) < garbageBytes, "compaction shrank the journal at open");
      assertFalse(Files.exists(snapshot), "the temp snapshot is gone after the atomic replace");
      JournalService.JournalState onDisk = JournalService.replay(journal);
      assertEquals(1L, onDisk.count("ID:live"), "the live count survives compaction");
      assertTrue(onDisk.inFlight("ID:live"), "the durable RESERVE stays in-flight");
      assertEquals(0L, onDisk.count("ID:dead"), "a committed entry cannot come back");
      assertEquals(1L, handle.count("ID:live"), "the in-memory count matches after compaction");
    }

    Object lockAfter = Files.readAttributes(lock, BasicFileAttributes.class).fileKey();
    assertEquals(lockBefore, lockAfter, "compaction never replaces the lock file inode");

    try (JournalService.JournalHandle reopened = service.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(1L, reopened.count("ID:live"), "a reopen replays the compacted journal");
    }
  }

  @Test
  void compaction_runs_at_open_and_before_ceiling() throws Exception {
    // A 16-byte junk id gives a wide frame, so the window between "an append would fail" and "the
    // compaction peak would fail" is wider than the two-frame step each committed pair adds.
    String junk = "DEADDEADDEADDEAD";
    int committedPairs = 40;
    long junkFrame = JournalService.frameLength(junk);
    Path dir = tempDir.resolve("broker-a");
    Files.createDirectories(dir);
    Path journal = dir.resolve("queue-a.journal");
    Path snapshot = dir.resolve("queue-a.snapshot");
    writeGarbageJournal(journal, committedPairs, junk, null);
    long garbageBytes = Files.size(journal);
    // Ceiling sized so the at-open peak fits and, after compaction, repeated appends reach a point
    // where the append itself would fail but the compaction peak still fits.
    long maxBytes = garbageBytes + 2 * junkFrame + 4;
    JournalService service =
        new JournalService(tempDir, maxBytes, 86_400_000L, System::currentTimeMillis);

    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      assertTrue(Files.size(journal) < garbageBytes, "compaction ran at open");
      assertFalse(Files.exists(snapshot), "the temp snapshot is gone after the atomic replace");

      // Repeated commits accumulate removable history until an append would fail closed; compaction
      // must run before that decision, so no iteration observes a rejection. The extra pair reaches
      // the point where the append itself no longer fits and forces the near-ceiling compaction.
      for (int i = 0; i <= committedPairs; i++) {
        handle.reserve(junk);
        handle.commit(junk);
      }
      handle.reserve("ID:live");
      assertEquals(
          1L, handle.count("ID:live"), "a reserve near the ceiling compacts before failing");
      assertEquals(0L, handle.count(junk), "committed history stays removed");
      assertTrue(
          Files.size(journal) < garbageBytes, "the near-ceiling compaction shrank the journal");
    }
  }

  @Test
  void compaction_peak_is_counted_under_a_real_ceiling() throws Exception {
    int committedPairs = 40;
    long floor = JournalService.DESTINATION_FLOOR_BYTES;
    long frameLive = JournalService.frameLength("ID:live");
    Path dir = tempDir.resolve("broker-a");
    Files.createDirectories(dir);
    Path journal = dir.resolve("queue-a.journal");

    writeGarbageJournal(journal, committedPairs, "ID:dead", "ID:live");
    long garbageBytes = Files.size(journal);
    long snapshotBytes = floor + frameLive; // one in-flight count-1 entry: one RESERVE frame
    long reservedBytes = frameLive;
    long peak = garbageBytes + reservedBytes + snapshotBytes + floor;

    JournalService fits = new JournalService(tempDir, peak, 86_400_000L, System::currentTimeMillis);
    try (JournalService.JournalHandle handle = fits.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(1L, handle.count("ID:live"));
    }
    assertEquals(
        snapshotBytes, Files.size(journal), "at exactly the peak budget the journal is compacted");

    writeGarbageJournal(journal, committedPairs, "ID:dead", "ID:live");
    assertEquals(garbageBytes, Files.size(journal), "the garbage journal is rebuilt");
    JournalService noRoom =
        new JournalService(tempDir, peak - 1L, 86_400_000L, System::currentTimeMillis);
    try (JournalService.JournalHandle handle = noRoom.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(
          1L, handle.count("ID:live"), "the journal still opens with a peak one byte too large");
    }
    assertEquals(
        garbageBytes,
        Files.size(journal),
        "the real snapshot peak gates compaction; an over-budget peak leaves it unchanged");
  }

  @Test
  void compaction_reopens_the_replaced_journal() throws Exception {
    List<FileChannel> opened = new ArrayList<>();
    JournalService service =
        new JournalService(
            tempDir,
            4_194_304L,
            86_400_000L,
            System::currentTimeMillis,
            journal -> {
              FileChannel channel =
                  FileChannel.open(journal, StandardOpenOption.READ, StandardOpenOption.WRITE);
              opened.add(channel);
              return channel;
            });
    Path dir = tempDir.resolve("broker-a");
    Files.createDirectories(dir);
    Path journal = dir.resolve("queue-a.journal");
    writeGarbageJournal(journal, 40, "ID:dead", "ID:live");

    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      assertTrue(
          opened.size() >= 2,
          "compaction reopened the replaced journal through the opener seam; opens="
              + opened.size());
      handle.reserve("ID:after");
      assertEquals(1L, handle.count("ID:after"));
    }

    JournalService.JournalState onDisk = JournalService.replay(journal);
    assertEquals(
        1L, onDisk.count("ID:after"), "the append after compaction is durable in the new inode");
    assertEquals(1L, onDisk.count("ID:live"), "the live reservation is preserved");
  }

  @Test
  void stale_entries_retire_at_compaction() throws Exception {
    long retentionMs = 86_400_000L;
    long staleAt = 1_000_000L;
    Path dir = tempDir.resolve("broker-a");
    Files.createDirectories(dir);
    Path journal = dir.resolve("queue-a.journal");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:stale", staleAt);
    }

    // A replayed RESERVE is durable old-owner state, not a live handle: once untouched past the
    // window it retires at the open compaction without resetting anything first.
    AtomicLong now = new AtomicLong(staleAt + retentionMs + 1L);
    JournalService service = new JournalService(tempDir, 4_194_304L, retentionMs, now::get);

    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(
          0L, handle.count("ID:stale"), "a crash-left stale reservation is retired at open");
      handle.reserve("ID:stale");
      assertEquals(1L, handle.count("ID:stale"), "a later receipt restarts the budget at one");
    }

    JournalService.JournalState onDisk = JournalService.replay(journal);
    assertEquals(1L, onDisk.count("ID:stale"), "only the fresh reservation is durable");
    assertTrue(onDisk.inFlight("ID:stale"), "the fresh reservation is in-flight");
  }

  @Test
  void current_reservation_is_not_retired() throws Exception {
    long retentionMs = 86_400_000L;
    String live = "ID:aaaa";
    long frame = JournalService.frameLength(live);
    long floor = JournalService.DESTINATION_FLOOR_BYTES;
    // Sized so the third reserve fails its own ceiling check, forcing a compaction whose peak
    // (old journal + settled reserve + pruned snapshot + floor) fits exactly.
    long maxBytes = 3 * floor + 5 * frame;
    AtomicLong now = new AtomicLong(1_000_000L);
    JournalService service = new JournalService(tempDir, maxBytes, retentionMs, now::get);

    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      handle.reserve(live);
      handle.reserve("ID:bbbb");
      handle.rollback("ID:bbbb");
      now.addAndGet(retentionMs + 1L); // both entries are now stale; only one is runtime-current
      handle.reserve("ID:cccc");

      assertEquals(1L, handle.count(live), "the runtime-current reservation is never retired");
      assertEquals(0L, handle.count("ID:bbbb"), "the stale, non-current reservation is retired");
      assertEquals(1L, handle.count("ID:cccc"), "the triggering reservation is recorded");
    }
  }

  @Test
  void no_live_eviction_fails_closed() throws Exception {
    long retentionMs = 86_400_000L;
    String a = "ID:aaaa";
    String b = "ID:bbbb";
    long frame = JournalService.frameLength(a);
    long floor = JournalService.DESTINATION_FLOOR_BYTES;
    long maxBytes = 2 * floor + 4 * frame;
    AtomicLong now = new AtomicLong(1_000_000L);
    JournalService service = new JournalService(tempDir, maxBytes, retentionMs, now::get);

    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      handle.reserve(a);
      handle.reserve(b);
      now.addAndGet(retentionMs + 1L); // stale, but live-current, so unretirable by contract
      assertThrows(JournalPermanentException.class, () -> handle.reserve("ID:cccc"));
      assertEquals(1L, handle.count(a), "a live reservation is never evicted to admit another");
      assertEquals(1L, handle.count(b), "a live reservation is never evicted to admit another");
      assertEquals(0L, handle.count("ID:cccc"), "the rejected reservation is not recorded");
    }
  }

  @Test
  void bad_magic_fails_closed() throws Exception {
    Path journal = tempDir.resolve("journal.bin");
    try (OutputStream out = Files.newOutputStream(journal)) {
      out.write(new byte[] {'X', 'X', 'X', 'X'});
      out.write(JournalService.FORMAT_VERSION);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:1", 1000L);
    }

    assertThrows(JournalPermanentException.class, () -> JournalService.replay(journal));
  }

  @Test
  void unsupported_version_fails_closed() throws Exception {
    Path journal = tempDir.resolve("journal.bin");
    try (OutputStream out = Files.newOutputStream(journal)) {
      out.write(JournalService.MAGIC);
      out.write(JournalService.FORMAT_VERSION + 1);
    }

    assertThrows(JournalPermanentException.class, () -> JournalService.replay(journal));
  }

  @Test
  void unsupported_record_type_fails_closed() throws Exception {
    Path journal = tempDir.resolve("journal.bin");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      writeRecord(out, (byte) 9, "ID:1", 1000L);
    }

    assertThrows(JournalPermanentException.class, () -> JournalService.replay(journal));
  }

  @Test
  void invalid_utf8_message_id_fails_closed() throws Exception {
    Path journal = tempDir.resolve("journal.bin");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      out.write(rawFrame(JournalService.TYPE_RESERVE, new byte[] {(byte) 0xFF}, 1000L));
    }

    // 0xFF is a UTF-8 continuation byte with no lead byte: a corrupt payload, not an id to replace
    // with U+FFFD at replay time (which would manufacture a key the writer never wrote).
    assertThrows(JournalPermanentException.class, () -> JournalService.replay(journal));
  }

  @Test
  void corrupt_journal_fails_closed() throws Exception {
    Path dir = tempDir.resolve("broker-a");
    Files.createDirectories(dir);
    Path journal = dir.resolve("queue-a.journal");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      writeRecord(out, JournalService.TYPE_RESERVE, "ID:good", 1000L);
      // A complete length field of 1 is structurally impossible: it is below the minimum payload,
      // so it is corruption, not the partial frame a crash leaves behind.
      out.write(ByteBuffer.allocate(4).order(ByteOrder.BIG_ENDIAN).putInt(1).array());
    }
    byte[] original = Files.readAllBytes(journal);

    JournalService service = newService();
    assertThrows(
        JournalPermanentException.class, () -> service.acquire("broker-a", "queue-a", 1000L));
    assertArrayEquals(
        original, Files.readAllBytes(journal), "a corrupt journal is never truncated or replaced");
  }

  @Test
  void io_error_is_permanent() throws Exception {
    // A regular file where the broker directory must be: the journal path cannot be opened. An I/O
    // failure at open is permanent; it must not be swallowed as an empty journal.
    Path root = tempDir.resolve("journal-root");
    Files.createDirectories(root);
    Files.writeString(root.resolve("broker-a"), "not a directory");

    JournalService service =
        new JournalService(root, 4_194_304L, 86_400_000L, System::currentTimeMillis);
    assertThrows(
        JournalPermanentException.class, () -> service.acquire("broker-a", "queue-a", 1000L));
  }

  @Test
  void blank_message_id_fails_closed_without_count_loss() throws Exception {
    JournalService service = newService();
    Path journal = tempDir.resolve("broker-a").resolve("queue-a.journal");
    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      handle.reserve("ID:real");
      long before = handle.count("ID:real");
      assertThrows(JournalPermanentException.class, () -> handle.reserve(""));
      assertThrows(JournalPermanentException.class, () -> handle.reserve("   "));
      assertEquals(before, handle.count("ID:real"), "a rejected blank id never loses a count");
      assertEquals(0L, handle.count(""), "the storage never manufactures an empty id");
    }
    JournalService.JournalState onDisk = JournalService.replay(journal);
    assertEquals(1L, onDisk.count("ID:real"), "the valid reservation is durable");
    assertEquals(0L, onDisk.count(""), "no empty-id record was ever written");
  }

  @Test
  void oversized_message_id_fails_closed_bounded() throws Exception {
    JournalService service = newService();
    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      handle.reserve("ID:real");
      // The check rejects an id longer than the two-byte length before encoding it, so a hostile
      // id can neither overflow the frame length nor trigger an OutOfMemoryError.
      String asciiTooLong = "a".repeat(0xFFFF + 1);
      String multibyteTooLong = "é".repeat(0xFFFF / 2 + 1);
      assertThrows(JournalPermanentException.class, () -> handle.reserve(asciiTooLong));
      assertThrows(JournalPermanentException.class, () -> handle.reserve(multibyteTooLong));
      assertEquals(1L, handle.count("ID:real"), "an oversized id never loses a count");
    }
  }

  @Test
  void torn_tail_is_discarded_at_open_and_append_is_durable() throws Exception {
    // A crash leaves a large partial frame after the header. Its length field is complete and in
    // range, so it is a torn tail, not corruption: replay keeps the confirmed prefix (the header)
    // and open discards the partial frame so a later append lands at a valid offset.
    long floor = JournalService.DESTINATION_FLOOR_BYTES;
    String tornId = "x".repeat(100);
    String newId = "ID:x";
    long tornBytes = JournalService.frameLength(tornId) - 3;
    long newFrame = JournalService.frameLength(newId);
    // The ceiling admits the truncating open and the later append (2*floor + 2*newFrame), but not
    // the pre-fix compaction peak (3*floor + tornBytes). Without the truncation the append is
    // rejected, or lands after a partial frame that hides it on replay.
    long maxBytes = floor + tornBytes + 3;
    Path dir = tempDir.resolve("broker-a");
    Files.createDirectories(dir);
    Path journal = dir.resolve("queue-a.journal");
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      writeTornRecord(out, JournalService.TYPE_RESERVE, tornId, System.currentTimeMillis());
    }
    assertTrue(
        2 * floor + 2 * newFrame <= maxBytes && maxBytes < 3 * floor + tornBytes,
        "the ceiling admits the append but not the compaction peak");

    JournalService service =
        new JournalService(tempDir, maxBytes, 86_400_000L, System::currentTimeMillis);
    try (JournalService.JournalHandle handle = service.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(floor, Files.size(journal), "open discards the torn partial frame");
      handle.reserve(newId);
      assertEquals(1L, handle.count(newId));
    }
    JournalService.JournalState onDisk = JournalService.replay(journal);
    assertEquals(1L, onDisk.count(newId), "an append after a discarded torn tail is durable");
    assertTrue(onDisk.inFlight(newId), "the appended reservation is in-flight");
  }

  @Test
  void compaction_failure_poisons_and_preserves_counts() throws Exception {
    AtomicLong opens = new AtomicLong();
    JournalService service =
        new JournalService(
            tempDir,
            4_194_304L,
            86_400_000L,
            System::currentTimeMillis,
            journal -> {
              if (opens.incrementAndGet() == 2L) {
                throw new IOException("simulated compaction reopen failure");
              }
              return FileChannel.open(journal, StandardOpenOption.READ, StandardOpenOption.WRITE);
            });
    Path dir = tempDir.resolve("broker-a");
    Files.createDirectories(dir);
    Path journal = dir.resolve("queue-a.journal");
    Path snapshot = dir.resolve("queue-a.snapshot");
    writeGarbageJournal(journal, 40, "ID:dead", "ID:live");
    long before = Files.size(journal);

    // The reopen after the atomic replace fails, exercising the compaction failure path through the
    // existing JournalOpener seam. A failed compaction poisons the session and cleans up.
    assertThrows(
        JournalPermanentException.class, () -> service.acquire("broker-a", "queue-a", 1000L));
    assertTrue(
        Files.size(journal) < before, "the atomic replace completed before the injected fault");
    assertFalse(Files.exists(snapshot), "a failed compaction removes its temporary snapshot");

    try (JournalService.JournalHandle reopened = service.acquire("broker-a", "queue-a", 1000L)) {
      assertEquals(
          1L, reopened.count("ID:live"), "a poisoned domain still replays persisted counts");
      assertThrows(
          JournalPermanentException.class,
          () -> reopened.reserve("ID:new"),
          "mutation is rejected after a compaction failure");
      assertThrows(
          JournalPermanentException.class,
          () -> reopened.commit("ID:live"),
          "mutation is rejected after a compaction failure");
      assertEquals(
          1L, reopened.count("ID:live"), "the persisted count survives the rejected mutation");
    }
  }

  /**
   * Writes a journal with committed history plus one live RESERVE, as accumulated use leaves it.
   */
  private static void writeGarbageJournal(
      Path journal, int committedPairs, String junkId, String liveId) throws IOException {
    try (OutputStream out = Files.newOutputStream(journal)) {
      writeHeader(out);
      for (int i = 0; i < committedPairs; i++) {
        writeRecord(out, JournalService.TYPE_RESERVE, junkId, 1000L);
        writeRecord(out, JournalService.TYPE_COMMIT, junkId, 1000L);
      }
      if (liveId != null) {
        // A recent last-touched time keeps a replayed live reservation out of the retention window;
        // these compaction tests exercise 2.4 behavior, not 2.5 retirement.
        writeRecord(out, JournalService.TYPE_RESERVE, liveId, System.currentTimeMillis());
      }
    }
  }

  private JournalService newService() {
    return new JournalService(tempDir, 4_194_304L, 86_400_000L, System::currentTimeMillis);
  }

  private static void writeHeader(OutputStream out) throws IOException {
    out.write(JournalService.MAGIC);
    out.write(JournalService.FORMAT_VERSION);
  }

  private static void writeRecord(OutputStream out, byte type, String messageId, long lastTouchedMs)
      throws IOException {
    out.write(frame(type, messageId, lastTouchedMs));
  }

  /** Writes a frame truncated by three payload bytes, as a crash mid-append would leave it. */
  private static void writeTornRecord(
      OutputStream out, byte type, String messageId, long lastTouchedMs) throws IOException {
    byte[] frame = frame(type, messageId, lastTouchedMs);
    out.write(frame, 0, frame.length - 3);
  }

  private static byte[] frame(byte type, String messageId, long lastTouchedMs) {
    return rawFrame(type, messageId.getBytes(StandardCharsets.UTF_8), lastTouchedMs);
  }

  /** Builds a frame from raw id bytes, so a test can inject an invalid UTF-8 id. */
  private static byte[] rawFrame(byte type, byte[] idBytes, long lastTouchedMs) {
    ByteBuffer payload = ByteBuffer.allocate(2 + idBytes.length + 8).order(ByteOrder.BIG_ENDIAN);
    payload.putShort((short) idBytes.length);
    payload.put(idBytes);
    payload.putLong(lastTouchedMs);
    ByteBuffer frame = ByteBuffer.allocate(4 + 1 + payload.capacity()).order(ByteOrder.BIG_ENDIAN);
    frame.putInt(payload.capacity());
    frame.put(type);
    frame.put(payload.array());
    return frame.array();
  }
}
