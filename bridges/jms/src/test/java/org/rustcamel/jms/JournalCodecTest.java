package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.ByteBuffer;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Pins the pure framing contract extracted into {@link JournalCodec}: the versioned header, the
 * exact record bytes, the replay state transitions, torn-tail truncation, and fail-closed
 * corruption handling. The vectors are hand-derived from the on-disk layout documented by {@code
 * crates/camel-test/tests/support/jms_journal.rs} ({@code [u32 BE payload_len][u8 type][u16 BE
 * id_len][id utf8][i64 BE last_touched_ms]}), so the reader is verified against independently
 * constructed bytes rather than its own writer.
 *
 * <p>The purity boundary test is a plain source-text assertion: the codec must stay free of file
 * channels, file locking, and monitor synchronization, which belong to {@link JournalService}.
 */
@DisplayName("JournalCodec framing, replay, and purity boundary")
class JournalCodecTest {

  private static final String GOLDEN_M1_RESERVE = "0000000C0100026D310000000000000001";

  private static final String GOLDEN_ORDER_1_RESERVE =
      "000000110100076F726465722D310000018EAFC18435";

  @Test
  void codec_source_contains_no_io_or_synchronization_tokens() throws Exception {
    Path source =
        Path.of(
            System.getProperty("user.dir"), "src/main/java/org/rustcamel/jms/JournalCodec.java");
    assertTrue(Files.isRegularFile(source), "codec source must exist at " + source);
    String text = Files.readString(source);
    for (String token :
        new String[] {"FileChannel", "Files.", "RandomAccessFile", "synchronized", "FileLock"}) {
      assertFalse(
          text.contains(token),
          "JournalCodec must not reference " + token + " (pure framing only)");
    }
  }

  @Test
  void header_bytes_match_pinned_format() {
    assertArrayEquals(bytes("52434A4E01"), JournalCodec.header());
  }

  @Test
  void encodes_reserve_frame_byte_for_byte() throws Exception {
    assertArrayEquals(
        bytes(GOLDEN_M1_RESERVE), JournalCodec.encodeFrame(JournalCodec.TYPE_RESERVE, "m1", 1L));
  }

  @Test
  void encodes_reserve_frame_with_realistic_timestamp() throws Exception {
    assertArrayEquals(
        bytes(GOLDEN_ORDER_1_RESERVE),
        JournalCodec.encodeFrame(JournalCodec.TYPE_RESERVE, "order-1", 1712345678901L));
  }

  @Test
  void replay_applies_record_type_semantics() throws Exception {
    byte[] stream =
        journal(
            JournalCodec.encodeFrame(JournalCodec.TYPE_RESERVE, "m1", 1L),
            JournalCodec.encodeFrame(JournalCodec.TYPE_RESERVE, "m1", 2L),
            JournalCodec.encodeFrame(JournalCodec.TYPE_ROLLBACK, "m1", 3L),
            JournalCodec.encodeFrame(JournalCodec.TYPE_RESERVE, "m2", 4L));

    JournalCodec.ReplayResult result = JournalCodec.replayBytes(stream);
    JournalCodec.JournalState state = result.state();

    assertEquals(2L, state.count("m1"), "each RESERVE increments the count");
    assertFalse(state.inFlight("m1"), "ROLLBACK keeps the count and clears in-flight");
    assertEquals(1L, state.count("m2"));
    assertTrue(state.inFlight("m2"));

    byte[] committed =
        journal(
            JournalCodec.encodeFrame(JournalCodec.TYPE_RESERVE, "m1", 5L),
            JournalCodec.encodeFrame(JournalCodec.TYPE_COMMIT, "m1", 6L));
    assertEquals(0L, JournalCodec.replayBytes(committed).state().count("m1"), "COMMIT removes");
  }

  @Test
  void replay_discards_truncated_length_tail() throws Exception {
    byte[] stream =
        journal(JournalCodec.encodeFrame(JournalCodec.TYPE_RESERVE, "m1", 1L), bytes("000000"));

    JournalCodec.ReplayResult result = JournalCodec.replayBytes(stream);

    assertEquals(1L, result.state().count("m1"));
    assertEquals(22L, result.validBytes(), "header 5 + frame 17");
  }

  @Test
  void replay_discards_truncated_body_tail() throws Exception {
    byte[] stream =
        journal(JournalCodec.encodeFrame(JournalCodec.TYPE_RESERVE, "m1", 1L), bytes("0000000C01"));

    JournalCodec.ReplayResult result = JournalCodec.replayBytes(stream);

    assertEquals(1L, result.state().count("m1"));
    assertEquals(22L, result.validBytes(), "header 5 + frame 17");
  }

  @Test
  void replay_rejects_wrong_magic_without_publishing_state() {
    byte[] stream = bytes("52434A4D01");

    JournalPermanentException thrown =
        assertThrows(JournalPermanentException.class, () -> JournalCodec.replayBytes(stream));

    assertTrue(
        thrown.getMessage().contains("magic mismatch"),
        "unexpected message: " + thrown.getMessage());
  }

  @Test
  void replay_rejects_unsupported_version() {
    byte[] stream = bytes("52434A4E02");

    JournalPermanentException thrown =
        assertThrows(JournalPermanentException.class, () -> JournalCodec.replayBytes(stream));

    assertTrue(
        thrown.getMessage().contains("unsupported journal format version"),
        "unexpected message: " + thrown.getMessage());
  }

  @Test
  void replay_rejects_out_of_range_payload_length() {
    byte[] stream = journal(bytes("00000009"));

    JournalPermanentException thrown =
        assertThrows(JournalPermanentException.class, () -> JournalCodec.replayBytes(stream));

    assertTrue(
        thrown.getMessage().contains("out of range"), "unexpected message: " + thrown.getMessage());
  }

  @Test
  void byte_sink_reproduces_golden_bytes() throws Exception {
    byte[] frame = JournalCodec.encodeFrame(JournalCodec.TYPE_RESERVE, "m1", 1L);

    ByteBuffer sink = ByteBuffer.allocate(frame.length);
    sink.put(frame);

    assertArrayEquals(bytes(GOLDEN_M1_RESERVE), sink.array());
  }

  /** Decodes an upper-case hex string into the bytes it denotes. */
  private static byte[] bytes(String hex) {
    int length = hex.length() / 2;
    byte[] out = new byte[length];
    for (int i = 0; i < length; i++) {
      out[i] = (byte) Integer.parseInt(hex.substring(i * 2, i * 2 + 2), 16);
    }
    return out;
  }

  /** Builds a journal stream: the versioned header followed by {@code frames}. */
  private static byte[] journal(byte[]... frames) {
    byte[] header = JournalCodec.header();
    int size = header.length;
    for (byte[] frame : frames) {
      size += frame.length;
    }
    byte[] out = new byte[size];
    System.arraycopy(header, 0, out, 0, header.length);
    int offset = header.length;
    for (byte[] frame : frames) {
      System.arraycopy(frame, 0, out, offset, frame.length);
      offset += frame.length;
    }
    return out;
  }
}
