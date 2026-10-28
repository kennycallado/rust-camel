//! Mail component error type.

use camel_api::CamelError;

/// Errors raised by the mail component.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[must_use]
pub enum MailError {
    /// Invalid mail configuration.
    #[error("mail configuration error: {0}")]
    Config(String),
    /// Unparsable mail address.
    #[error("invalid mail address: {0}")]
    Address(String),
    /// A header value that must not reach the wire.
    #[error("header validation failed for {header}: {reason}")]
    HeaderValidation {
        /// Exchange header name.
        header: String,
        /// Why the value was rejected.
        reason: &'static str,
    },
    /// The recipient fence rejected the exchange.
    #[error("recipient fence rejected the exchange: {0}")]
    RecipientFence(String),
    /// The body cannot be sent as mail text.
    #[error("unsupported mail body: {0}")]
    BodyType(String),
    /// HTML content type combined with an HTML alternative.
    #[error("conflicting body: contentType text/html cannot be combined with CamelMailHtmlBody")]
    ConflictingBody,
    /// No recipient resolved.
    #[error(
        "no recipient: set to, cc or bcc on the endpoint, or an allowed CamelMailTo/CamelMailCc/CamelMailBcc header"
    )]
    NoRecipient,
    /// TLS setup failure (CA file, trust store).
    #[error("mail TLS error: {0}")]
    Tls(String),
    /// Transport-level failure (message build, connection, server reply).
    #[error("mail transport error: {0}")]
    Transport(String),
    /// The IMAP session broke: I/O failure, timeout, lost connection or an
    /// unparsable reply. The session must not be reused.
    #[error("IMAP store error: {0}")]
    Store(String),
    /// The IMAP server answered a command with a tagged `NO` or `BAD`. The
    /// session stays usable.
    #[error("IMAP server refused the command: {0}")]
    Refused(String),
    /// IMAP connection failure (connect, TLS handshake, a login that broke
    /// the connection).
    #[error("IMAP connect error: {0}")]
    Connect(String),
    /// The IMAP server refused the account login (tagged `NO` or `BAD` on
    /// `LOGIN`). Permanent: a reconnect sends the same credentials.
    #[error("IMAP server refused the login; check the account credentials")]
    LoginRefused,
    /// The IMAP server lacks a capability the endpoint options require.
    #[error("IMAP server lacks required capability: {0}")]
    CapabilityMissing(String),
    /// Idempotent repository failure (read or write).
    #[error("idempotent repository error: {0}")]
    Idempotent(String),
}

impl MailError {
    /// Whether the connection is suspect, so a reconnect can plausibly
    /// succeed. A refused command or a missing capability is not retryable:
    /// a new connection gets the same answer.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, MailError::Connect(_) | MailError::Store(_))
    }
}

/// Longest server-derived text an error carries, in characters.
const MAX_SERVER_TEXT: usize = 200;

/// Bound and clean text derived from a server reply or a library error:
/// control characters are dropped and the result is cut to
/// [`MAX_SERVER_TEXT`] characters, so a hostile or broken server cannot
/// flood logs or forge log lines through an error message.
pub(crate) fn server_text(text: &str) -> String {
    let mut chars = text.chars().filter(|c| !c.is_control());
    let mut clean: String = chars.by_ref().take(MAX_SERVER_TEXT).collect();
    if chars.next().is_some() {
        clean.push_str("...");
    }
    clean
}

impl From<MailError> for CamelError {
    fn from(err: MailError) -> Self {
        CamelError::ProcessorError(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_classification() {
        assert!(MailError::Connect("x".to_string()).is_retryable());
        assert!(MailError::Store("x".to_string()).is_retryable());
        assert!(!MailError::Refused("x".to_string()).is_retryable());
        assert!(!MailError::CapabilityMissing("x".to_string()).is_retryable());
        assert!(!MailError::Idempotent("x".to_string()).is_retryable());
        assert!(!MailError::Tls("x".to_string()).is_retryable());
        assert!(!MailError::LoginRefused.is_retryable());
    }

    #[test]
    fn server_text_is_bounded_and_clean() {
        let raw = format!("a\r\nb\x00c\x1b[31m{}", "z".repeat(5000));
        let out = server_text(&raw);
        assert!(!out.chars().any(char::is_control), "{out:?}");
        assert!(out.starts_with("abc[31m"), "{out:?}");
        assert_eq!(out.chars().count(), MAX_SERVER_TEXT + 3, "{out:?}");
        assert!(out.ends_with("..."), "{out:?}");
        assert_eq!(server_text("short"), "short");
    }
}
