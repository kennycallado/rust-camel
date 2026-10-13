package org.rustcamel.jms;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.channels.FileChannel;
import java.nio.channels.FileLock;
import java.nio.channels.OverlappingFileLockException;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CharsetDecoder;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileAlreadyExistsException;
import java.nio.file.FileSystems;
import java.nio.file.Files;
import java.nio.file.InvalidPathException;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import java.nio.file.StandardOpenOption;
import java.nio.file.attribute.FileAttribute;
import java.nio.file.attribute.PosixFilePermission;
import java.nio.file.attribute.PosixFilePermissions;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.function.LongSupplier;
import java.util.regex.Pattern;

/**
 * Process-wide reservation journal: versioned framing, replay, torn-tail handling, lazy
 * per-destination OS file locking, and owner-private files.
 *
 * <p>A journal file is a versioned magic header followed by framed records {@code [u32 length][u8
 * type][payload]}. The length is the payload byte count; the type is one of {@code RESERVE}, {@code
 * COMMIT}, {@code ROLLBACK}, or {@code RETIRE}; the payload carries the message id and last-touched
 * time. All multi-byte fields are big-endian.
 *
 * <p>Task 2.1 implemented framing and replay. Task 2.2 added the instance API: one service per
 * process owns every journal write; {@link #acquire(String, String, long)} lazily creates the
 * owner-private broker directory, takes a never-replaced {@code <destination>.lock} OS file lock,
 * and replays the journal into a refcounted {@link JournalHandle}. Task 2.3 makes every mutation
 * durable before it is visible: {@code reserve}, {@code commit}, and {@code rollback} append a
 * frame and {@code force(true)} before the in-memory state advances, and the lock is replayed on
 * every reacquisition so a sequential owner observes another process's durable writes. The byte
 * ceiling reserves both the append frame and the settlement frame of every live entry, so a full
 * journal can always settle. Task 2.4 adds compaction: the state is re-encoded to an owner-private
 * temporary snapshot, fsynced, atomically renamed over the journal, and the parent directory is
 * fsynced. Compaction runs at open and before any ceiling fail-closed decision, but stays best
 * effort: when the transient peak (old journal plus snapshot) does not fit the ceiling it leaves
 * the journal unchanged, so a settlement that frees reserved bytes is never rejected because of an
 * unchanged working journal. Task 2.5 adds retention: each reservation stores a last-touched time
 * that every live {@code RESERVE} refreshes. During compaction, entries untouched longer than the
 * retention window are retired (dropped from the re-encoded snapshot) unless they are the
 * runtime-current reservation of this owner. Retirement is distinct from live eviction: this
 * service never evicts a live reservation to admit another, and it fails closed only when
 * unretirable entries fill the ceiling. The reservation cap is therefore <em>not an absolute
 * forever cap</em>: a message absent past the window restarts its budget at one. Configuration
 * bounds (2.6) arrive in a later task.
 *
 * <p>Retention pruning distinguishes a <em>runtime-current</em> reservation from <em>durable
 * old-owner</em> state. A {@code RESERVE} appended while this process holds the domain lock marks
 * the entry runtime-current and is never retired. A {@code RESERVE} replayed from disk (for example
 * one left by an owner that crashed before settlement) is durable old-owner state: its count is
 * preserved on reopen, but it is not the live reservation of the current handle and may be retired
 * once it is untouched past the window. Retirement is applied to the in-memory state only after the
 * compacted snapshot has been durably renamed, so a failed compaction never loses a count.
 *
 * <p>Task 2.7 bounds corrupt and hostile input: a frame length outside the addressable range, a bad
 * magic or version, an unsupported record type, or an id that is blank or not valid UTF-8 fails
 * closed with a {@link JournalPermanentException} and never resets a confirmed count. A missing or
 * incomplete trailing frame stays a torn tail and is discarded at open before the next append. Ids
 * are bounded to the two-byte id length, so an oversized id can neither overflow the frame length
 * nor allocate without limit.
 *
 * <p>Task 3.7 fixes the accounting scope and identity. A journal domain is the tuple ({@code
 * JMS_ATTEMPT_JOURNAL_DIR}, broker key, destination key) with exactly one live owner at a time.
 * Budgets are shared concurrently only inside the owning bridge process, and a sequential owner
 * that acquires the domain through lock handoff inherits the retained counts. A reservation is one
 * durable delivery-attempt record whose durable authority is the provider {@code JMSMessageID},
 * never an exchange header or an ephemeral subscription id. Cross-process or cross-host duplicates
 * are bounded at N times {@code JMS_MAX_DELIVERY_ATTEMPTS}, where N is the number of distinct
 * journal domains — (directory, broker key, destination key) tuples — consuming the queue within
 * one retention window. Topics are non-durable: they use a subscription-scoped volatile counter in
 * {@code JmsConsumer}, never touch this service or its lock, and their poison cap is not
 * crash-stable.
 */
public final class JournalService {

  /** Four-byte file magic, {@code "RCJN"}. */
  static final byte[] MAGIC = {'R', 'C', 'J', 'N'};

  /** On-disk format version written immediately after {@link #MAGIC}. */
  static final byte FORMAT_VERSION = 1;

  /** Record type: a reservation attempt for a message id. */
  static final byte TYPE_RESERVE = 1;

  /** Record type: the reserved delivery settled. */
  static final byte TYPE_COMMIT = 2;

  /** Record type: the reserved delivery rolled back; the reservation stays. */
  static final byte TYPE_ROLLBACK = 3;

  /** Record type: a stale reservation was retired at compaction. */
  static final byte TYPE_RETIRE = 4;

  /** Environment variable named in the busy-lock failure, per the bridge contract. */
  static final String JOURNAL_DIRECTORY_ENV = "JMS_ATTEMPT_JOURNAL_DIR";

  /** Environment variable that scopes the journal namespace to one broker. */
  static final String BROKER_KEY_ENV = "JMS_BRIDGE_BROKER_KEY";

  /** Broker key shape Rust derives: the first 32 lowercase hex characters of a SHA-256 digest. */
  private static final Pattern BROKER_KEY_PATTERN = Pattern.compile("^[0-9a-f]{32}$");

  /** Bytes of a SHA-256 digest that make up the 32-character destination key. */
  private static final int DESTINATION_KEY_BYTES = 16;

  /** Offset of the one-byte format version, immediately after {@link #MAGIC}. */
  private static final int FORMAT_VERSION_OFFSET = MAGIC.length;

  /** Bytes of magic plus the format version. */
  private static final int HEADER_LENGTH = MAGIC.length + 1;

  /** Shortest possible payload: a two-byte id length and an eight-byte last-touched time. */
  private static final int MIN_PAYLOAD_LENGTH = 2 + 8;

  /** Largest message id the two-byte id length can address. */
  static final int MAX_MESSAGE_ID_BYTES = 0xFFFF;

  /**
   * Largest valid record payload: the two-byte id length, the id, and the eight-byte time. A
   * declared frame length outside {@code [MIN_PAYLOAD_LENGTH, MAX_PAYLOAD_LENGTH]} is corrupt
   * framing, not a torn tail, so replay fails closed before it can allocate.
   */
  private static final int MAX_PAYLOAD_LENGTH = MIN_PAYLOAD_LENGTH + MAX_MESSAGE_ID_BYTES;

  /**
   * Largest journal a replay buffers in one array. A larger file fails closed with a {@link
   * JournalPermanentException} before allocating, so a corrupt or hostile journal can never trigger
   * an {@link OutOfMemoryError}.
   */
  static final long MAX_REPLAY_BYTES = Integer.MAX_VALUE - 8L;

  /**
   * Bytes the ceiling always reserves for one destination: the journal header plus the empty lock
   * and snapshot files. Lock and snapshot are created empty, so only the header is non-zero, but
   * the floor names all three and guarantees the ceiling is never accounted down to zero free
   * space. The temporary snapshot's live bytes are added separately during compaction (2.4).
   */
  static final long DESTINATION_FLOOR_BYTES = HEADER_LENGTH;

  /** Poll interval for a busy lock; keeps the bounded wait responsive without spinning. */
  private static final long LOCK_POLL_INTERVAL_MS = 10L;

  /** Broker directory mode, {@code rwx------}. */
  private static final FileAttribute<Set<PosixFilePermission>> DIRECTORY_PERMISSIONS =
      PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------"));

  /** Journal, lock, and snapshot file mode, {@code rw-------}. */
  private static final FileAttribute<Set<PosixFilePermission>> FILE_PERMISSIONS =
      PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rw-------"));

  private final Path root;
  private final long maxBytes;
  private final long retentionMs;
  private final LongSupplier clockMs;
  private final JournalOpener journalOpener;
  private final Object lockMonitor = new Object();
  private final Map<Domain, Session> sessions = new HashMap<>();

  /** Opens the append channel for a journal file; a seam for tests to inject a write failure. */
  @FunctionalInterface
  interface JournalOpener {
    /**
     * Opens the read/write append channel for {@code journal}.
     *
     * @param journal the journal file
     * @return the append channel
     * @throws IOException when the channel cannot be opened
     */
    FileChannel open(Path journal) throws IOException;
  }

  /** The production append-channel opener. */
  private static final JournalOpener DEFAULT_OPENER =
      journal -> FileChannel.open(journal, StandardOpenOption.READ, StandardOpenOption.WRITE);

  /**
   * Creates a journal service rooted at {@code root}, the directory Rust resolves and passes as
   * {@code JMS_ATTEMPT_JOURNAL_DIR}.
   *
   * @param root the journal root directory; broker keys are single path components below it
   * @param maxBytes the per-destination byte ceiling; callers pass the value already
   *     range-validated by {@link BridgeConfig#journalMaxBytes()} (task 2.6)
   * @param retentionMs the retention window in milliseconds (consumed from task 2.5); callers pass
   *     the value already range-validated by {@link BridgeConfig#journalRetentionMs()} (task 2.6)
   * @param clockMs the clock used to stamp a reservation's last-touched time
   */
  public JournalService(Path root, long maxBytes, long retentionMs, LongSupplier clockMs) {
    this(root, maxBytes, retentionMs, clockMs, DEFAULT_OPENER);
  }

  /**
   * Creates a journal service with an explicit append-channel opener. Production code uses the
   * public constructor; tests use this one to inject a channel whose {@code force} fails.
   *
   * @param root the journal root directory
   * @param maxBytes the per-destination byte ceiling
   * @param retentionMs the retention window in milliseconds
   * @param clockMs the clock used to stamp a reservation's last-touched time
   * @param journalOpener the append-channel opener
   */
  JournalService(
      Path root,
      long maxBytes,
      long retentionMs,
      LongSupplier clockMs,
      JournalOpener journalOpener) {
    this.root = Objects.requireNonNull(root, "root");
    this.maxBytes = maxBytes;
    this.retentionMs = retentionMs;
    this.clockMs = Objects.requireNonNull(clockMs, "clockMs");
    this.journalOpener = Objects.requireNonNull(journalOpener, "journalOpener");
  }

  /**
   * Validates the journal startup configuration without opening, locking, or replaying a journal.
   * Unset values are accepted: the failure is deferred to a queue subscribe, so a producer-only or
   * topic-only bridge reports readiness. A set broker key must match {@code ^[0-9a-f]{32}$}; a set
   * directory must resolve to an existing owner-private directory that is not a symbolic link.
   *
   * @param dirEnv the value of {@code JMS_ATTEMPT_JOURNAL_DIR}; {@code null} or blank means unset
   * @param brokerKeyEnv the value of {@code JMS_BRIDGE_BROKER_KEY}; {@code null} or blank means
   *     unset
   * @throws JournalPermanentException when a set value is invalid; the message names the variable
   *     and never echoes the raw value
   */
  public static void validateStartup(String dirEnv, String brokerKeyEnv)
      throws JournalPermanentException {
    boolean keySet = brokerKeyEnv != null && !brokerKeyEnv.isBlank();
    if (keySet && !BROKER_KEY_PATTERN.matcher(brokerKeyEnv).matches()) {
      throw new JournalPermanentException(
          BROKER_KEY_ENV + " must be 32 lowercase hexadecimal characters");
    }
    if (dirEnv != null && !dirEnv.isBlank()) {
      validateOwnerPrivateDirectory(dirEnv);
    }
  }

  /**
   * Returns the stable destination key for a queue name: the first 32 lowercase hex characters of
   * SHA-256 over the UTF-8 bytes of {@code "queue:" + name}. Java derives it from the JDK standard
   * library so the journal path never contains a raw destination name or message ID.
   *
   * @param destinationName the queue name, without the {@code queue:} prefix
   * @return the 32-character lowercase hex destination key
   */
  public static String destinationKey(String destinationName) {
    try {
      MessageDigest digest = MessageDigest.getInstance("SHA-256");
      byte[] hash = digest.digest(("queue:" + destinationName).getBytes(StandardCharsets.UTF_8));
      StringBuilder key = new StringBuilder(DESTINATION_KEY_BYTES * 2);
      for (int i = 0; i < DESTINATION_KEY_BYTES; i++) {
        key.append(Character.forDigit((hash[i] >> 4) & 0xF, 16));
        key.append(Character.forDigit(hash[i] & 0xF, 16));
      }
      return key.toString();
    } catch (NoSuchAlgorithmException e) {
      // SHA-256 is mandated by the JDK; its absence is a broken runtime, not a config error.
      throw new IllegalStateException("SHA-256 is unavailable", e);
    }
  }

  /**
   * Rejects a directory that cannot be trusted as an owner-private journal root: an unparseable
   * path, a symbolic link (a symlink escape is never followed), an unresolvable path, a non
   * directory, or one with any group or other permission bit. Existing files are only inspected,
   * never re-permissioned.
   */
  private static void validateOwnerPrivateDirectory(String rawDir)
      throws JournalPermanentException {
    Path dir;
    try {
      dir = Paths.get(rawDir);
    } catch (InvalidPathException e) {
      throw new JournalPermanentException(JOURNAL_DIRECTORY_ENV + " is not a valid path");
    }
    if (Files.isSymbolicLink(dir)) {
      throw new JournalPermanentException(JOURNAL_DIRECTORY_ENV + " must not be a symbolic link");
    }
    Path real;
    try {
      real = dir.toRealPath();
    } catch (IOException e) {
      throw new JournalPermanentException(
          JOURNAL_DIRECTORY_ENV + " does not resolve to an existing directory", e);
    }
    if (!Files.isDirectory(real)) {
      throw new JournalPermanentException(JOURNAL_DIRECTORY_ENV + " is not a directory");
    }
    Set<PosixFilePermission> permissions;
    try {
      permissions = Files.getPosixFilePermissions(real);
    } catch (UnsupportedOperationException e) {
      // Non-POSIX filesystem: privacy bits cannot be checked, so the resolved directory stands.
      return;
    } catch (IOException e) {
      throw new JournalPermanentException(JOURNAL_DIRECTORY_ENV + " permissions are unreadable", e);
    }
    for (PosixFilePermission permission : permissions) {
      if (permission != PosixFilePermission.OWNER_READ
          && permission != PosixFilePermission.OWNER_WRITE
          && permission != PosixFilePermission.OWNER_EXECUTE) {
        throw new JournalPermanentException(JOURNAL_DIRECTORY_ENV + " is not owner-private");
      }
    }
  }

  /**
   * Lazily acquires ownership of one queue destination journal, waiting up to {@code lockWaitMs}
   * for the destination's OS file lock. The first acquire in this process creates the owner-private
   * broker directory and the journal, lock, and snapshot files (0700 and 0600 at creation), takes
   * the lock with {@code FileChannel.tryLock}, and replays the journal. Repeated acquires of the
   * same domain inside this service share one lock through a refcount; the last {@link
   * JournalHandle#close()} releases it. The lock file is never renamed, replaced, or deleted, so
   * the kernel releases the lock at process exit.
   *
   * <p>Only map lookup and file creation run under the process monitor; the bounded lock wait runs
   * under the destination's own coordination object, so a busy domain never blocks another domain.
   *
   * <p>Every reacquisition of the OS lock replays the journal, even when a previous handle left a
   * cached in-memory state behind: a prior owner may be another process that appended durable
   * records while this process held no lock. Replay is bounded by the per-destination ceiling, so a
   * journal grown past the ceiling fails closed instead of allocating.
   *
   * @param brokerKey the broker namespace, a trusted single path component
   * @param destinationKey the destination namespace, a trusted single path component
   * @param lockWaitMs the bounded wait for a busy lock, in milliseconds
   * @return a refcounted handle that owns the domain until closed
   * @throws JournalPermanentException when a path component is unsafe, files cannot be created or
   *     read, the journal framing is corrupt, or the lock stays busy past the wait
   */
  public JournalHandle acquire(String brokerKey, String destinationKey, long lockWaitMs)
      throws JournalPermanentException {
    validatePathComponent(brokerKey, "broker key");
    validatePathComponent(destinationKey, "destination key");
    Domain domain = new Domain(brokerKey, destinationKey);
    Session session;
    synchronized (lockMonitor) {
      session = sessions.get(domain);
      if (session == null) {
        session = createSession(domain);
        sessions.put(domain, session);
      }
    }
    session.acquire(lockWaitMs);
    return new JournalHandle(this, session);
  }

  /**
   * A refcounted owner of one destination journal; closing releases the lock on the last handle.
   */
  public static final class JournalHandle implements AutoCloseable {

    private final JournalService owner;
    private final Session session;
    private boolean closed;

    /** Opens a handle over one destination's shared session. */
    private JournalHandle(JournalService owner, Session session) {
      this.owner = owner;
      this.session = session;
    }

    /**
     * Appends a durable RESERVE frame and fsyncs it before returning; the message must not leave
     * the bridge until this returns.
     *
     * @param messageId the reservation key
     * @throws JournalPermanentException when the append cannot be made durable or would exceed the
     *     per-destination ceiling
     */
    public void reserve(String messageId) throws JournalPermanentException {
      session.reserve(messageId, owner.clockMs.getAsLong());
    }

    /**
     * Appends a durable COMMIT frame and fsyncs it before the in-memory entry is removed.
     *
     * @param messageId the reservation key
     * @throws JournalPermanentException when the append cannot be made durable
     */
    public void commit(String messageId) throws JournalPermanentException {
      session.commit(messageId, owner.clockMs.getAsLong());
    }

    /**
     * Appends a durable ROLLBACK frame and fsyncs it; the count stays and only the in-flight mark
     * clears.
     *
     * @param messageId the reservation key
     * @throws JournalPermanentException when the append cannot be made durable
     */
    public void rollback(String messageId) throws JournalPermanentException {
      session.rollback(messageId, owner.clockMs.getAsLong());
    }

    /** Returns the reservation count for {@code messageId}; zero when absent. */
    public long count(String messageId) {
      return session.count(messageId);
    }

    @Override
    public synchronized void close() {
      if (closed) {
        return;
      }
      closed = true;
      session.release();
    }
  }

  /** In-memory journal view produced by {@link #replay(Path)}. */
  static final class JournalState {

    private final Map<String, Entry> entries = new LinkedHashMap<>();

    /** Number of reservations recorded for {@code messageId}; zero when absent. */
    long count(String messageId) {
      Entry entry = entries.get(messageId);
      return entry == null ? 0L : entry.count;
    }

    /** Whether the last record for {@code messageId} was a RESERVE without a ROLLBACK. */
    boolean inFlight(String messageId) {
      Entry entry = entries.get(messageId);
      return entry != null && entry.inFlight;
    }

    /** Last-touched time recorded for {@code messageId}, or {@code -1} when absent. */
    long lastTouchedMs(String messageId) {
      Entry entry = entries.get(messageId);
      return entry == null ? -1L : entry.lastTouchedMs;
    }

    /** Whether {@code messageId} still has a live reservation. */
    boolean contains(String messageId) {
      return entries.containsKey(messageId);
    }

    /**
     * Bytes the ceiling must keep reserved so every in-flight reservation can still write its
     * settlement frame. Each in-flight entry reserves exactly one COMMIT-length frame; a ROLLBACK
     * frame has the same length, so the reserve covers either settlement.
     *
     * @return the reserved settlement bytes
     */
    long inFlightSettlementReserveBytes() {
      long total = 0L;
      for (Map.Entry<String, Entry> e : entries.entrySet()) {
        if (e.getValue().inFlight) {
          total += frameLength(e.getKey());
        }
      }
      return total;
    }

    /**
     * Bytes a compaction snapshot of the current, retention-pruned state occupies: the header plus,
     * per retained entry, one RESERVE frame per reservation and one ROLLBACK frame when it is not
     * in-flight. Entries stale past {@code retentionMs} that are not runtime-current are priced out
     * of the snapshot, but the live state is left untouched until a compaction succeeds. Committed
     * entries are already absent, so their history is dropped.
     *
     * @param nowMs the clock reading used to judge staleness
     * @param retentionMs the validated retention window
     * @return the pruned snapshot size in bytes
     */
    long snapshotBytes(long nowMs, long retentionMs) {
      long total = HEADER_LENGTH;
      for (Map.Entry<String, Entry> e : entries.entrySet()) {
        if (isStale(e.getValue(), nowMs, retentionMs)) {
          continue;
        }
        long frame = frameLength(e.getKey());
        total += e.getValue().count * frame;
        if (!e.getValue().inFlight) {
          total += frame;
        }
      }
      return total;
    }

    /**
     * Encodes the current, retention-pruned state as a fresh journal: the header followed by, per
     * retained entry, one RESERVE frame per reservation and a ROLLBACK frame when it is not
     * in-flight. Re-emitting from the live state, never from the old file, guarantees an entry that
     * was already committed cannot come back and that a retired entry is dropped. The live state is
     * not mutated here, so a failed compaction cannot lose a count.
     *
     * @param nowMs the clock reading used to judge staleness
     * @param retentionMs the validated retention window
     * @return the encoded snapshot
     * @throws JournalPermanentException when the snapshot exceeds the replay buffer cap
     */
    byte[] encodeSnapshot(long nowMs, long retentionMs) throws JournalPermanentException {
      long size = snapshotBytes(nowMs, retentionMs);
      if (size > MAX_REPLAY_BYTES) {
        throw new JournalPermanentException("compaction snapshot too large: " + size);
      }
      ByteBuffer buffer = ByteBuffer.allocate((int) size).order(ByteOrder.BIG_ENDIAN);
      buffer.put(header());
      for (Map.Entry<String, Entry> e : entries.entrySet()) {
        Entry entry = e.getValue();
        if (isStale(entry, nowMs, retentionMs)) {
          continue;
        }
        byte[] reserve = encodeFrame(TYPE_RESERVE, e.getKey(), entry.lastTouchedMs);
        for (long i = 0L; i < entry.count; i++) {
          buffer.put(reserve);
        }
        if (!entry.inFlight) {
          buffer.put(encodeFrame(TYPE_ROLLBACK, e.getKey(), entry.lastTouchedMs));
        }
      }
      return buffer.array();
    }

    /**
     * Removes entries untouched longer than {@code retentionMs}, except the runtime-current
     * reservation of this owner. Called only after a compacted snapshot has been durably renamed,
     * so a failed compaction can never lose an in-memory count. A returned removed count is not
     * needed by callers; the pruning is reflected in the next {@link #snapshotBytes(long, long)}.
     *
     * @param nowMs the clock reading used to judge staleness
     * @param retentionMs the validated retention window
     */
    void retireStale(long nowMs, long retentionMs) {
      entries.entrySet().removeIf(e -> isStale(e.getValue(), nowMs, retentionMs));
    }

    /**
     * Whether an entry is eligible for retention retirement: it is not this owner's runtime-current
     * reservation and it has been untouched for longer than the window. A RESERVE appended while
     * this process held the lock is runtime-current; a RESERVE replayed from disk is durable
     * old-owner state, so it is retirable without resetting its count on reopen.
     */
    private static boolean isStale(Entry entry, long nowMs, long retentionMs) {
      return !entry.liveReservation && nowMs - entry.lastTouchedMs > retentionMs;
    }

    /**
     * Applies one RESERVE. {@code live} marks a reservation appended under this process's lock
     * (runtime-current); replay passes {@code false} so durable old-owner state is retirable.
     */
    private void reserve(String messageId, long lastTouchedMs, boolean live) {
      Entry entry = entries.computeIfAbsent(messageId, ignored -> new Entry());
      entry.count++;
      entry.lastTouchedMs = lastTouchedMs;
      entry.inFlight = true;
      entry.liveReservation = live;
    }

    private void rollback(String messageId) {
      Entry entry = entries.get(messageId);
      if (entry != null) {
        entry.inFlight = false;
        entry.liveReservation = false;
      }
    }

    private void remove(String messageId) {
      entries.remove(messageId);
    }

    /** Mutable reservation entry; {@code count} grows by one per RESERVE. */
    private static final class Entry {
      private long count;
      private long lastTouchedMs = -1L;

      /** Durable in-flight mark: set by any RESERVE, cleared by ROLLBACK, true after replay. */
      private boolean inFlight;

      /**
       * Runtime-current reservation: true only for a RESERVE appended under this process's lock.
       */
      private boolean liveReservation;
    }
  }

  /**
   * One destination's coordination object: stable paths, replayed state, live OS lock, and
   * in-process refcount. Every state mutation and the refcount run under this object's monitor, so
   * concurrent handles on one domain are serialized and see each other's writes. A bounded lock
   * wait holds only this monitor, never the process monitor, so it cannot block another domain.
   */
  private static final class Session {
    private final Domain domain;
    private final Path journal;
    private final Path lockFile;
    private final Path snapshot;
    private final long maxBytes;
    private final long retentionMs;
    private final LongSupplier clockMs;
    private final JournalOpener journalOpener;
    private JournalState state;
    private FileChannel lockChannel;
    private FileLock lock;
    private FileChannel journalChannel;
    private long journalBytes;
    private long reservedSettlementBytes;
    private boolean poisoned;
    private int refCount;

    private Session(
        Domain domain,
        Path journal,
        Path lockFile,
        Path snapshot,
        long maxBytes,
        long retentionMs,
        LongSupplier clockMs,
        JournalOpener journalOpener) {
      this.domain = domain;
      this.journal = journal;
      this.lockFile = lockFile;
      this.snapshot = snapshot;
      this.maxBytes = maxBytes;
      this.retentionMs = retentionMs;
      this.clockMs = clockMs;
      this.journalOpener = journalOpener;
    }

    /**
     * Takes one owner reference. When this is the first live reference it acquires the OS lock,
     * then replays the journal unconditionally: a previous owner may have run in another process
     * and appended durable records while this process held no lock, so any cached in-memory state
     * is discarded in favor of the durable file.
     */
    private synchronized void acquire(long lockWaitMs) throws JournalPermanentException {
      if (refCount > 0) {
        refCount++;
        return;
      }
      FileChannel channel = openLockChannel(lockFile);
      lockChannel = channel;
      try {
        lock = tryLockBounded(channel, lockWaitMs, domain);
        journalBytes = journalSize(journal);
        ReplayResult replayed = replayForOpen(journal, maxBytes);
        state = replayed.state();
        reservedSettlementBytes = state.inFlightSettlementReserveBytes();
        journalChannel = journalOpener.open(journal);
        discardTornTail(replayed.validBytes());
        compactIfFits();
      } catch (JournalPermanentException e) {
        releaseLockOnly();
        throw e;
      } catch (IOException e) {
        releaseLockOnly();
        throw new JournalPermanentException(
            "cannot open journal for destination " + domain.destinationKey(), e);
      }
      refCount++;
    }

    /** Drops one owner reference, releasing the OS lock when the last handle closes. */
    private synchronized void release() {
      refCount--;
      if (refCount == 0) {
        releaseLockOnly();
      }
    }

    /**
     * Appends and fsyncs a RESERVE frame, reserving its settlement frame, before the in-memory
     * count and in-flight mark advance.
     */
    private synchronized void reserve(String messageId, long lastTouchedMs)
        throws JournalPermanentException {
      requireHeld();
      requireKeyableId(messageId);
      boolean wasInFlight = state.inFlight(messageId);
      long frame = frameLength(messageId);
      long settlement = wasInFlight ? 0L : frameLength(messageId);
      ensureAppendFits(frame, settlement);
      writeDurable(encodeFrame(TYPE_RESERVE, messageId, lastTouchedMs));
      state.reserve(messageId, lastTouchedMs, true);
      reservedSettlementBytes += settlement;
    }

    /**
     * Appends and fsyncs a COMMIT frame, then removes the in-memory entry and releases its
     * settlement reserve.
     */
    private synchronized void commit(String messageId, long lastTouchedMs)
        throws JournalPermanentException {
      requireHeld();
      requireKeyableId(messageId);
      boolean wasInFlight = state.inFlight(messageId);
      long settlement = wasInFlight ? frameLength(messageId) : 0L;
      ensureAppendFits(frameLength(messageId), -settlement);
      writeDurable(encodeFrame(TYPE_COMMIT, messageId, lastTouchedMs));
      state.remove(messageId);
      reservedSettlementBytes -= settlement;
    }

    /**
     * Appends and fsyncs a ROLLBACK frame, then clears the in-flight mark and releases its
     * settlement reserve; the count stays.
     */
    private synchronized void rollback(String messageId, long lastTouchedMs)
        throws JournalPermanentException {
      requireHeld();
      requireKeyableId(messageId);
      boolean wasInFlight = state.inFlight(messageId);
      long settlement = wasInFlight ? frameLength(messageId) : 0L;
      ensureAppendFits(frameLength(messageId), -settlement);
      writeDurable(encodeFrame(TYPE_ROLLBACK, messageId, lastTouchedMs));
      state.rollback(messageId);
      reservedSettlementBytes -= settlement;
    }

    private synchronized long count(String messageId) {
      return state.count(messageId);
    }

    /** Rejects any mutation when this process holds no lock or the journal was poisoned. */
    private void requireHeld() throws JournalPermanentException {
      if (poisoned) {
        throw new JournalPermanentException(
            "journal poisoned by a failed write for destination " + domain.destinationKey());
      }
      if (refCount == 0) {
        throw new JournalPermanentException(
            "journal not held for destination " + domain.destinationKey());
      }
    }

    /**
     * Fails closed when {@code frameBytes} plus the settlement reserve would exceed the ceiling.
     * Before that fail-closed decision it runs compaction, which may shrink the journal enough to
     * fit. A settlement frame of a live entry always fits: it consumes the bytes already reserved
     * for it, so {@code reservedDelta} is negative and the projected use never grows, and an
     * unchanged working journal never rejects it.
     */
    private void ensureAppendFits(long frameBytes, long reservedDelta)
        throws JournalPermanentException {
      if (fitsCeiling(frameBytes, reservedDelta)) {
        return;
      }
      compactIfFits();
      if (!fitsCeiling(frameBytes, reservedDelta)) {
        throw new JournalPermanentException(
            "destination journal ceiling reached for "
                + domain.destinationKey()
                + "; no live reservation is evicted");
      }
    }

    /** Whether the append plus settlement delta keeps the live journal within the ceiling. */
    private boolean fitsCeiling(long frameBytes, long reservedDelta) {
      return !exceedsCeiling(
          journalBytes + frameBytes, reservedSettlementBytes + reservedDelta, 0L, maxBytes);
    }

    /**
     * Discards an incomplete trailing frame left by a crash so a later append lands at a valid
     * offset. This is recovery, not error handling: a torn tail confirms nothing, so dropping it
     * loses no confirmed count. It runs only under the exclusive lock, before the open compaction
     * and before any append, and never touches a journal whose replay confirmed every byte.
     *
     * @param validBytes the byte length of the confirmed prefix reported by replay
     * @throws IOException when the file cannot be truncated, leaving the domain unacquired
     */
    private void discardTornTail(long validBytes) throws IOException {
      if (journalBytes <= validBytes) {
        return;
      }
      journalChannel.truncate(validBytes);
      journalChannel.force(true);
      journalBytes = validBytes;
    }

    /**
     * Rewrites the journal as a compacted, retention-pruned snapshot when that strictly shrinks it
     * and the transient peak (old journal plus snapshot) fits the ceiling. Best effort: when the
     * peak does not fit it returns without compacting, so a settlement that frees reserved bytes is
     * never rejected because of an unchanged working journal. Retirement is priced here but applied
     * only after {@link #compact(long, long)} makes the snapshot durable.
     *
     * @throws JournalPermanentException when a compaction that started fails; the session is then
     *     poisoned and fails closed
     */
    private void compactIfFits() throws JournalPermanentException {
      long nowMs = clockMs.getAsLong();
      long targetBytes = state.snapshotBytes(nowMs, retentionMs);
      if (targetBytes >= journalBytes) {
        return;
      }
      if (exceedsCeiling(journalBytes, reservedSettlementBytes, targetBytes, maxBytes)) {
        return;
      }
      compact(targetBytes, nowMs);
    }

    /**
     * Writes the compacted, retention-pruned state to the owner-private temporary snapshot, fsyncs
     * it, closes the old append channel, atomically renames the snapshot over the journal, fsyncs
     * the parent directory, and reopens the append channel. Retirement is applied to the in-memory
     * state only after every durable step succeeds, so a failure poisons the session and fails
     * closed without losing a confirmed count. The live journal is never truncated or partially
     * replaced.
     *
     * @param targetBytes the exact pruned snapshot size the caller priced
     * @param nowMs the clock reading the caller used to price and prune the snapshot
     * @throws JournalPermanentException when the snapshot cannot be made durable or replaced
     */
    private void compact(long targetBytes, long nowMs) throws JournalPermanentException {
      try {
        byte[] snapshotData = state.encodeSnapshot(nowMs, retentionMs);
        if (snapshotData.length != targetBytes) {
          throw new JournalPermanentException(
              "compaction snapshot size mismatch for destination " + domain.destinationKey());
        }
        writeSnapshotFile(snapshotData);
        // Close the old append channel before the rename: after the rename this descriptor points
        // at an unlinked inode, so any later append through it would be lost.
        closeJournalChannel();
        Files.move(snapshot, journal, StandardCopyOption.ATOMIC_MOVE);
        fsyncDirectory(journal.getParent());
        journalChannel = journalOpener.open(journal);
        journalBytes = snapshotData.length;
        // The snapshot is durable: it is now safe to drop retired entries from memory and release
        // the settlement bytes they were reserving. Nothing before this point changes live state.
        state.retireStale(nowMs, retentionMs);
        reservedSettlementBytes = state.inFlightSettlementReserveBytes();
      } catch (JournalPermanentException e) {
        poisoned = true;
        cleanupSnapshot();
        throw e;
      } catch (IOException e) {
        poisoned = true;
        cleanupSnapshot();
        throw new JournalPermanentException(
            "journal compaction failed for destination " + domain.destinationKey(), e);
      }
    }

    /** Creates the owner-private snapshot when absent, then writes and fsyncs it in full. */
    private void writeSnapshotFile(byte[] bytes) throws JournalPermanentException, IOException {
      createOwnerPrivateFile(snapshot);
      try (FileChannel channel =
          FileChannel.open(
              snapshot, StandardOpenOption.WRITE, StandardOpenOption.TRUNCATE_EXISTING)) {
        ByteBuffer buffer = ByteBuffer.wrap(bytes);
        while (buffer.hasRemaining()) {
          channel.write(buffer);
        }
        channel.force(true);
      }
    }

    /** Closes the append channel after flushing it, leaving the field null for a reopen. */
    private void closeJournalChannel() {
      closeQuietly(journalChannel);
      journalChannel = null;
    }

    /** Removes a leftover snapshot after a failed compaction; the session is already poisoned. */
    private void cleanupSnapshot() {
      try {
        Files.deleteIfExists(snapshot);
      } catch (IOException e) {
        // Already poisoned: a failed cleanup leaves the session fail-closed.
      }
    }

    /**
     * Writes the header when absent, then the frame, and fsyncs data and metadata. The in-memory
     * length advances only after the fsync succeeds; a failed write poisons the session and never
     * advances state.
     */
    private void writeDurable(byte[] frame) throws JournalPermanentException {
      int headerBytes = journalBytes < HEADER_LENGTH ? HEADER_LENGTH : 0;
      try {
        if (headerBytes > 0) {
          journalChannel.truncate(0L);
          journalChannel.position(0L);
          writeFully(ByteBuffer.wrap(header()));
        }
        journalChannel.position(journalBytes + headerBytes);
        writeFully(ByteBuffer.wrap(frame));
        // force(true), stronger than the plan's force(false): the append grows the file, so the
        // file metadata (its durable length) must be synced too, not only the frame data.
        journalChannel.force(true);
      } catch (IOException e) {
        poisoned = true;
        throw new JournalPermanentException(
            "journal append failed for destination " + domain.destinationKey(), e);
      }
      journalBytes += headerBytes + frame.length;
    }

    private void writeFully(ByteBuffer buffer) throws IOException {
      while (buffer.hasRemaining()) {
        journalChannel.write(buffer);
      }
    }

    /** Releases and closes the live lock and append channel without touching the refcount. */
    private void releaseLockOnly() {
      FileLock held = lock;
      lock = null;
      if (held != null) {
        try {
          held.release();
        } catch (IOException e) {
          // The channel close below also releases; a failed explicit release is not actionable.
        }
      }
      closeQuietly(lockChannel);
      lockChannel = null;
      closeQuietly(journalChannel);
      journalChannel = null;
    }
  }

  /** Identity of one destination journal: broker namespace plus destination namespace. */
  private record Domain(String brokerKey, String destinationKey) {}

  /** Creates the owner-private directory and files for a not-yet-cached domain. */
  private Session createSession(Domain domain) throws JournalPermanentException {
    createRootIfMissing();
    Path directory = root.resolve(domain.brokerKey());
    Path journal = directory.resolve(domain.destinationKey() + ".journal");
    Path lockFile = directory.resolve(domain.destinationKey() + ".lock");
    Path snapshot = directory.resolve(domain.destinationKey() + ".snapshot");
    createOwnerPrivateDirectory(directory);
    createOwnerPrivateFile(journal);
    createOwnerPrivateFile(lockFile);
    createOwnerPrivateFile(snapshot);
    return new Session(
        domain, journal, lockFile, snapshot, maxBytes, retentionMs, clockMs, journalOpener);
  }

  /**
   * Takes an exclusive lock, retrying until {@code waitMs} elapses. A lock owned by another channel
   * in this JVM raises {@link OverlappingFileLockException}; it is treated as busy and never leaks.
   */
  private static FileLock tryLockBounded(FileChannel channel, long waitMs, Domain domain)
      throws JournalPermanentException {
    long deadline = System.nanoTime() + Math.max(0L, waitMs) * 1_000_000L;
    while (true) {
      try {
        FileLock lock = channel.tryLock();
        if (lock != null) {
          return lock;
        }
      } catch (OverlappingFileLockException e) {
        // Held by another channel in this JVM: fall through to the bounded wait.
      } catch (IOException e) {
        throw new JournalPermanentException(
            "journal lock failed for destination " + domain.destinationKey(), e);
      }
      long remainingNanos = deadline - System.nanoTime();
      if (remainingNanos <= 0L) {
        throw new JournalPermanentException(
            "journal lock busy for destination "
                + domain.destinationKey()
                + " after "
                + waitMs
                + " ms; point "
                + JOURNAL_DIRECTORY_ENV
                + " at a distinct directory");
      }
      sleepQuietly(Math.max(1L, Math.min(LOCK_POLL_INTERVAL_MS, remainingNanos / 1_000_000L)));
    }
  }

  private static void sleepQuietly(long millis) throws JournalPermanentException {
    try {
      Thread.sleep(millis);
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new JournalPermanentException("journal lock wait interrupted", e);
    }
  }

  private static FileChannel openLockChannel(Path lockFile) throws JournalPermanentException {
    try {
      return FileChannel.open(lockFile, StandardOpenOption.READ, StandardOpenOption.WRITE);
    } catch (IOException e) {
      throw new JournalPermanentException("cannot open journal lock: " + lockFile, e);
    }
  }

  private static void closeQuietly(FileChannel channel) {
    if (channel == null) {
      return;
    }
    try {
      channel.close();
    } catch (IOException e) {
      // Best effort; a failed close leaves no recoverable action.
    }
  }

  /**
   * Fsyncs a directory so a completed rename within it is durable. Linux supports opening a
   * directory read-only and forcing it; a failure is not swallowed, because a compaction that
   * cannot make its rename durable must fail closed.
   */
  private static void fsyncDirectory(Path directory) throws IOException {
    try (FileChannel channel = FileChannel.open(directory, StandardOpenOption.READ)) {
      channel.force(true);
    }
  }

  /**
   * Creates the journal root with 0700 when it is absent; an existing root is left untouched (never
   * re-permissioned), matching the owner-private contract for the broker directory.
   */
  private void createRootIfMissing() throws JournalPermanentException {
    if (Files.isDirectory(root)) {
      return;
    }
    try {
      if (posixSupported()) {
        Files.createDirectories(root, DIRECTORY_PERMISSIONS);
      } else {
        Files.createDirectories(root);
      }
    } catch (FileAlreadyExistsException e) {
      throw new JournalPermanentException("journal root is not a directory: " + root, e);
    } catch (IOException e) {
      throw new JournalPermanentException("cannot create journal root: " + root, e);
    }
  }

  /**
   * Creates the broker directory with 0700 when it is new. An existing directory is left untouched
   * (startup validation owns the owner-private check), so arbitrary files are never
   * re-permissioned.
   */
  private static void createOwnerPrivateDirectory(Path directory) throws JournalPermanentException {
    try {
      if (posixSupported()) {
        Files.createDirectory(directory, DIRECTORY_PERMISSIONS);
      } else {
        Files.createDirectory(directory);
      }
    } catch (FileAlreadyExistsException e) {
      // Never chmod an existing directory.
    } catch (IOException e) {
      throw new JournalPermanentException("cannot create journal directory: " + directory, e);
    }
  }

  /**
   * Creates a journal, lock, or snapshot file with 0600 when it is new; existing files are kept.
   */
  private static void createOwnerPrivateFile(Path file) throws JournalPermanentException {
    try {
      if (posixSupported()) {
        Files.createFile(file, FILE_PERMISSIONS);
      } else {
        Files.createFile(file);
      }
    } catch (FileAlreadyExistsException e) {
      // Never chmod an existing file.
    } catch (IOException e) {
      throw new JournalPermanentException("cannot create journal file: " + file, e);
    }
  }

  private static boolean posixSupported() {
    return FileSystems.getDefault().supportedFileAttributeViews().contains("posix");
  }

  /** Rejects any key that could escape the journal root through a separator or a relative path. */
  private static void validatePathComponent(String value, String name)
      throws JournalPermanentException {
    if (value == null || value.isEmpty()) {
      throw new JournalPermanentException(name + " is empty");
    }
    if (value.equals(".") || value.equals("..")) {
      throw new JournalPermanentException(name + " is a relative path component");
    }
    for (int i = 0; i < value.length(); i++) {
      char c = value.charAt(i);
      boolean safe =
          (c >= 'a' && c <= 'z')
              || (c >= 'A' && c <= 'Z')
              || (c >= '0' && c <= '9')
              || c == '-'
              || c == '_'
              || c == '.';
      if (!safe) {
        throw new JournalPermanentException(name + " contains an unsafe character: " + c);
      }
    }
  }

  /**
   * Replays {@code file} into an in-memory {@link JournalState} without a ceiling. Test-only;
   * production replay always passes the per-destination ceiling.
   *
   * @param file the journal file
   * @return the replayed state
   * @throws JournalPermanentException when the file cannot be read, its framing is corrupt, or it
   *     is too large to buffer
   */
  static JournalState replay(Path file) throws JournalPermanentException {
    return replay(file, MAX_REPLAY_BYTES);
  }

  /**
   * Replays {@code file} into an in-memory {@link JournalState}. A missing or empty file yields an
   * empty state. An incomplete trailing frame is a torn tail: it is discarded and every earlier
   * confirmed count is kept. A bad magic, unsupported version, out-of-range frame length, invalid
   * UTF-8 id, or unsupported record type is corrupt framing and fails closed. The reader is bounded
   * before it allocates: a file larger than {@code maxBytes} or the {@link #MAX_REPLAY_BYTES}
   * buffer cap fails closed with a {@link JournalPermanentException}, so a real file past the
   * ceiling never triggers an {@link OutOfMemoryError}.
   *
   * @param file the journal file
   * @param maxBytes the per-destination ceiling; the file is read only when it fits below it
   * @return the replayed state
   * @throws JournalPermanentException when the file cannot be read, exceeds the ceiling or the
   *     buffer cap, or its framing is corrupt
   */
  static JournalState replay(Path file, long maxBytes) throws JournalPermanentException {
    return replayForOpen(file, maxBytes).state();
  }

  /** A replay's in-memory state plus the byte length of the confirmed (non-torn) prefix. */
  record ReplayResult(JournalState state, long validBytes) {}

  /**
   * Replays {@code file} for {@link #acquire}, reporting the confirmed prefix length so the caller
   * can discard a torn tail before it appends. Test-only callers use {@link #replay(Path, long)}.
   *
   * @param file the journal file
   * @param maxBytes the per-destination ceiling
   * @return the replayed state and the confirmed prefix length
   * @throws JournalPermanentException when the file cannot be read, exceeds the ceiling or the
   *     buffer cap, or its framing is corrupt
   */
  static ReplayResult replayForOpen(Path file, long maxBytes) throws JournalPermanentException {
    long size = journalSize(file);
    if (size > Math.min(maxBytes, MAX_REPLAY_BYTES)) {
      throw new JournalPermanentException(
          "journal exceeds the readable ceiling: " + file + " size " + size);
    }
    if (size == 0L) {
      return new ReplayResult(new JournalState(), 0L);
    }
    byte[] bytes = new byte[(int) size];
    try (FileChannel channel = FileChannel.open(file, StandardOpenOption.READ)) {
      ByteBuffer buffer = ByteBuffer.wrap(bytes);
      while (buffer.hasRemaining()) {
        if (channel.read(buffer) < 0) {
          break;
        }
      }
      int read = buffer.position();
      if (read == 0) {
        return new ReplayResult(new JournalState(), 0L);
      }
      if (read < bytes.length) {
        bytes = java.util.Arrays.copyOf(bytes, read);
      }
    } catch (IOException e) {
      throw new JournalPermanentException("journal read failed: " + file, e);
    }
    return replayBytes(bytes);
  }

  /** Returns the journal file size, or zero when it does not exist yet. */
  private static long journalSize(Path file) throws JournalPermanentException {
    try {
      return Files.exists(file) ? Files.size(file) : 0L;
    } catch (IOException e) {
      throw new JournalPermanentException("journal stat failed: " + file, e);
    }
  }

  /** The versioned on-disk header: {@link #MAGIC} plus {@link #FORMAT_VERSION}. */
  private static byte[] header() {
    byte[] header = new byte[HEADER_LENGTH];
    System.arraycopy(MAGIC, 0, header, 0, MAGIC.length);
    header[FORMAT_VERSION_OFFSET] = FORMAT_VERSION;
    return header;
  }

  /**
   * Encoded frame length for {@code messageId}: a four-byte payload length, a one-byte type, and
   * the payload (a two-byte id length, the UTF-8 id, and an eight-byte last-touched time). Every
   * record type shares this layout, so the length is type-independent.
   *
   * @param messageId the reservation key
   * @return the frame length in bytes
   */
  static long frameLength(String messageId) {
    return (long) Integer.BYTES + 1 + MIN_PAYLOAD_LENGTH + encodedIdLength(messageId);
  }

  /**
   * UTF-8 byte length of {@code messageId}, bounded so it can neither overflow nor encode more than
   * the maximum: an id longer than {@link #MAX_MESSAGE_ID_BYTES} characters is already past the
   * limit, so the exact byte count is unnecessary and the caller fails closed.
   */
  private static int encodedIdLength(String messageId) {
    if (messageId.length() > MAX_MESSAGE_ID_BYTES) {
      return MAX_MESSAGE_ID_BYTES + 1;
    }
    return messageId.getBytes(StandardCharsets.UTF_8).length;
  }

  /**
   * Rejects a blank or oversized message id before any frame is priced or encoded, so the storage
   * never manufactures an empty key, and an id past the two-byte length can neither overflow the
   * frame length nor trigger a large allocation.
   */
  private static void requireKeyableId(String messageId) throws JournalPermanentException {
    if (messageId == null || messageId.isBlank()) {
      throw new JournalPermanentException("journal message id is blank");
    }
    if (encodedIdLength(messageId) > MAX_MESSAGE_ID_BYTES) {
      throw new JournalPermanentException(
          "journal message id is too long: " + MAX_MESSAGE_ID_BYTES + " bytes max");
    }
  }

  /**
   * Whether the accounted destination bytes would exceed the ceiling. The accounting is the live
   * journal plus the settlement reserve plus the temporary snapshot's current peak plus the fixed
   * floor for the empty lock and snapshot files.
   *
   * @param journalBytes live journal bytes
   * @param reservedBytes settlement bytes reserved for live entries
   * @param snapshotBytes temporary snapshot bytes (zero until compaction, task 2.4)
   * @param maxBytes the per-destination ceiling
   * @return true when the ceiling would be exceeded
   */
  static boolean exceedsCeiling(
      long journalBytes, long reservedBytes, long snapshotBytes, long maxBytes) {
    return journalBytes + reservedBytes + snapshotBytes + DESTINATION_FLOOR_BYTES > maxBytes;
  }

  /** Encodes one framed record; the inverse of {@link #replayBytes(byte[])}. */
  private static byte[] encodeFrame(byte type, String messageId, long lastTouchedMs)
      throws JournalPermanentException {
    byte[] id = messageId.getBytes(StandardCharsets.UTF_8);
    if (id.length > MAX_MESSAGE_ID_BYTES) {
      throw new JournalPermanentException("journal message id is too long: " + id.length);
    }
    ByteBuffer payload =
        ByteBuffer.allocate(MIN_PAYLOAD_LENGTH + id.length).order(ByteOrder.BIG_ENDIAN);
    payload.putShort((short) id.length);
    payload.put(id);
    payload.putLong(lastTouchedMs);
    ByteBuffer frame =
        ByteBuffer.allocate(Integer.BYTES + 1 + payload.capacity()).order(ByteOrder.BIG_ENDIAN);
    frame.putInt(payload.capacity());
    frame.put(type);
    frame.put(payload.array());
    return frame.array();
  }

  private static ReplayResult replayBytes(byte[] bytes) throws JournalPermanentException {
    JournalState state = new JournalState();
    if (bytes.length < HEADER_LENGTH) {
      // An empty or half-written header confirms nothing; there is no confirmed count to lose.
      return new ReplayResult(state, 0L);
    }
    for (int i = 0; i < MAGIC.length; i++) {
      if (bytes[i] != MAGIC[i]) {
        throw new JournalPermanentException("journal magic mismatch");
      }
    }
    if (bytes[FORMAT_VERSION_OFFSET] != FORMAT_VERSION) {
      throw new JournalPermanentException(
          "unsupported journal format version: " + bytes[FORMAT_VERSION_OFFSET]);
    }

    ByteBuffer buffer = ByteBuffer.wrap(bytes).order(ByteOrder.BIG_ENDIAN);
    int pos = HEADER_LENGTH;
    while (pos < bytes.length) {
      int frameStart = pos;
      if (bytes.length - frameStart < Integer.BYTES) {
        pos = frameStart;
        break; // Torn tail: a partial length field.
      }
      long payloadLength = Integer.toUnsignedLong(buffer.getInt(frameStart));
      if (payloadLength < MIN_PAYLOAD_LENGTH || payloadLength > MAX_PAYLOAD_LENGTH) {
        // A complete length field outside the valid range cannot come from a crash mid-append.
        // It is corruption, not a torn tail: fail closed and never reset a confirmed count.
        throw new JournalPermanentException("journal frame length out of range: " + payloadLength);
      }
      long frameEnd = (long) frameStart + Integer.BYTES + 1L + payloadLength;
      if (frameEnd > bytes.length) {
        pos = frameStart;
        break; // Torn tail: a partial record body.
      }
      applyRecord(
          state,
          bytes[frameStart + Integer.BYTES],
          bytes,
          frameStart + Integer.BYTES + 1,
          (int) payloadLength);
      pos = (int) frameEnd;
    }
    return new ReplayResult(state, pos);
  }

  private static void applyRecord(
      JournalState state, byte type, byte[] bytes, int offset, int length)
      throws JournalPermanentException {
    if (length < MIN_PAYLOAD_LENGTH) {
      throw new JournalPermanentException("journal record payload too short: " + length);
    }
    ByteBuffer payload = ByteBuffer.wrap(bytes, offset, length).order(ByteOrder.BIG_ENDIAN);
    int idLength = Short.toUnsignedInt(payload.getShort());
    if (idLength != length - MIN_PAYLOAD_LENGTH) {
      throw new JournalPermanentException("journal record id length out of range: " + idLength);
    }
    byte[] idBytes = new byte[idLength];
    payload.get(idBytes);
    String messageId = decodeMessageId(idBytes);
    long lastTouchedMs = payload.getLong();
    switch (type) {
      case TYPE_RESERVE -> state.reserve(messageId, lastTouchedMs, false);
      case TYPE_ROLLBACK -> state.rollback(messageId);
      case TYPE_COMMIT, TYPE_RETIRE -> state.remove(messageId);
      default -> throw new JournalPermanentException("unsupported journal record type: " + type);
    }
  }

  /**
   * Decodes a record message id strictly. An empty id or invalid UTF-8 is corrupt: replay fails
   * closed rather than manufacture a key (an empty one or one with replacement characters) the
   * writer never recorded.
   */
  private static String decodeMessageId(byte[] idBytes) throws JournalPermanentException {
    if (idBytes.length == 0) {
      throw new JournalPermanentException("journal record message id is blank");
    }
    CharsetDecoder decoder =
        StandardCharsets.UTF_8
            .newDecoder()
            .onMalformedInput(CodingErrorAction.REPORT)
            .onUnmappableCharacter(CodingErrorAction.REPORT);
    try {
      return decoder.decode(ByteBuffer.wrap(idBytes)).toString();
    } catch (CharacterCodingException e) {
      throw new JournalPermanentException("journal message id is not valid UTF-8", e);
    }
  }
}
