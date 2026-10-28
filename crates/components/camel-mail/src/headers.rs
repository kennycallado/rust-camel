//! Exchange header names (ADR-0093 classes) and header-value validation.

use crate::error::MailError;

/// Subject of an outgoing mail; overrides the `subject` option (class A).
pub const CAMEL_MAIL_SUBJECT: &str = "CamelMailSubject";
/// `text/plain` or `text/html`; overrides the `contentType` option (class A).
pub const CAMEL_MAIL_CONTENT_TYPE: &str = "CamelMailContentType";
/// HTML alternative to a `text/plain` body (class A, body content).
pub const CAMEL_MAIL_HTML_BODY: &str = "CamelMailHtmlBody";
/// To recipients; needs `allowedRecipients` on the endpoint (class B).
pub const CAMEL_MAIL_TO: &str = "CamelMailTo";
/// Cc recipients; needs `allowedRecipients` on the endpoint (class B).
pub const CAMEL_MAIL_CC: &str = "CamelMailCc";
/// Bcc recipients, envelope only; needs `allowedRecipients` (class B).
pub const CAMEL_MAIL_BCC: &str = "CamelMailBcc";
/// Message-ID the producer assigned to an accepted send.
pub const CAMEL_MAIL_MESSAGE_ID: &str = "CamelMailMessageId";

/// From addresses of a received message, comma separated.
pub const CAMEL_MAIL_IN_FROM: &str = "CamelMailInFrom";
/// To addresses of a received message, comma separated.
pub const CAMEL_MAIL_IN_TO: &str = "CamelMailInTo";
/// Cc addresses of a received message, comma separated.
pub const CAMEL_MAIL_IN_CC: &str = "CamelMailInCc";
/// Subject of a received message.
pub const CAMEL_MAIL_IN_SUBJECT: &str = "CamelMailInSubject";
/// Message-ID of a received message, without angle brackets.
pub const CAMEL_MAIL_IN_MESSAGE_ID: &str = "CamelMailInMessageId";
/// IMAP UID of a received message in its folder.
pub const CAMEL_MAIL_IN_UID: &str = "CamelMailInUid";
/// Date of a received message in RFC 3339 format.
pub const CAMEL_MAIL_IN_DATE: &str = "CamelMailInDate";
/// Folder a received message came from.
pub const CAMEL_MAIL_IN_FOLDER: &str = "CamelMailInFolder";
/// HTML part of a received message that also has a text part.
pub const CAMEL_MAIL_IN_HTML_BODY: &str = "CamelMailInHtmlBody";

pub(crate) const MAX_HEADER_VALUE_BYTES: usize = 998;

/// Reject values that could inject headers or exceed the line limit.
pub(crate) fn validate_header_value(name: &str, value: &str) -> Result<(), MailError> {
    let reason = if value.contains('\r') {
        "contains CR"
    } else if value.contains('\n') {
        "contains LF"
    } else if value.contains('\0') {
        "contains NUL"
    } else if value.len() > MAX_HEADER_VALUE_BYTES {
        "longer than 998 bytes"
    } else {
        return Ok(());
    };
    Err(MailError::HeaderValidation {
        header: name.to_string(),
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason(v: &str) -> Option<&'static str> {
        match validate_header_value("h", v) {
            Err(MailError::HeaderValidation { reason, .. }) => Some(reason),
            _ => None,
        }
    }

    #[test]
    fn validate_header_value_rules() {
        assert!(validate_header_value("h", "ok").is_ok());
        assert_eq!(reason("a\rb"), Some("contains CR"));
        assert_eq!(reason("a\nb"), Some("contains LF"));
        assert_eq!(reason("a\0b"), Some("contains NUL"));
        assert_eq!(reason(&"a".repeat(999)), Some("longer than 998 bytes"));
        assert!(validate_header_value("h", &"a".repeat(998)).is_ok());
    }
}
