package org.rustcamel.jms;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CharsetDecoder;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * Pure journal framing: the on-disk record format, the replay parser, and the in-memory reservation
 * state it produces. Every operation works on byte arrays and maps only; file channels, locks, and
 * durability ordering stay in {@link JournalService}.
 *
 * <p>A journal file is a versioned magic header followed by framed records {@code [u32 length][u8
 * type][payload]}. The length is the payload byte count; the type is one of {@code RESERVE}, {@code
 * COMMIT}, {@code ROLLBACK}, or {@code RETIRE}; the payload carries the message id and last-touched
 * time. All multi-byte fields are big-endian.
 */
final class JournalCodec {

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

  /** Offset of the one-byte format version, immediately after {@link #MAGIC}. */
  private static final int FORMAT_VERSION_OFFSET = MAGIC.length;

  /** Bytes of magic plus the format version. */
  static final int HEADER_LENGTH = MAGIC.length + 1;

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

  private JournalCodec() {}

  /** In-memory journal view produced by {@link JournalService#replay(java.nio.file.Path)}. */
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
    void reserve(String messageId, long lastTouchedMs, boolean live) {
      Entry entry = entries.computeIfAbsent(messageId, ignored -> new Entry());
      entry.count++;
      entry.lastTouchedMs = lastTouchedMs;
      entry.inFlight = true;
      entry.liveReservation = live;
    }

    void rollback(String messageId) {
      Entry entry = entries.get(messageId);
      if (entry != null) {
        entry.inFlight = false;
        entry.liveReservation = false;
      }
    }

    void remove(String messageId) {
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

  /** A replay's in-memory state plus the byte length of the confirmed (non-torn) prefix. */
  record ReplayResult(JournalState state, long validBytes) {}

  /** The versioned on-disk header: {@link #MAGIC} plus {@link #FORMAT_VERSION}. */
  static byte[] header() {
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
  static int encodedIdLength(String messageId) {
    if (messageId.length() > MAX_MESSAGE_ID_BYTES) {
      return MAX_MESSAGE_ID_BYTES + 1;
    }
    return messageId.getBytes(StandardCharsets.UTF_8).length;
  }

  /** Encodes one framed record; the inverse of {@link #replayBytes(byte[])}. */
  static byte[] encodeFrame(byte type, String messageId, long lastTouchedMs)
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

  static ReplayResult replayBytes(byte[] bytes) throws JournalPermanentException {
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
