//! Bounded `std::io::Write` shared by S6 (output limit) and S9 (context limit).
//! Aborts mid-write so the limit trips before the full payload is materialised.

use std::io::{self, Write};

pub struct LimitedWriter<W> {
    inner: W,
    remaining: u64,
}

impl<W> LimitedWriter<W> {
    /// Creates a new writer that allows at most `limit` bytes through.
    pub fn new(inner: W, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
        }
    }

    /// Consumes the wrapper and returns the inner writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() as u64 > self.remaining {
            return Err(io::Error::other("limit exceeded"));
        }
        let n = self.inner.write(buf)?;
        self.remaining -= n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod limited_writer_tests {
    use super::LimitedWriter;
    use std::io::Write;

    #[test]
    fn writes_under_limit_succeed() {
        let mut buf = Vec::new();
        let mut w = LimitedWriter::new(&mut buf, 10);
        assert!(w.write_all(b"hello").is_ok());
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn writes_at_limit_succeed() {
        let mut buf = Vec::new();
        let mut w = LimitedWriter::new(&mut buf, 5);
        assert!(w.write_all(b"hello").is_ok());
    }

    #[test]
    fn writes_over_limit_fail_and_do_not_grow_buf() {
        let mut buf = Vec::new();
        let mut w = LimitedWriter::new(&mut buf, 3);
        let err = w.write_all(b"hello").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
        assert!(buf.len() <= 3, "buffer must not exceed the limit");
    }

    #[test]
    fn cumulative_writes_across_calls_trip_limit() {
        let mut buf = Vec::new();
        let mut w = LimitedWriter::new(&mut buf, 5);
        assert!(w.write_all(b"abc").is_ok());
        assert!(w.write_all(b"de").is_ok());
        assert!(w.write_all(b"f").is_err(), "third write must trip");
    }
}
