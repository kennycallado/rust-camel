//! URI metadata descriptor for the mail schemes.

use camel_component_api::{ComponentMetadata, UriConfig};

use crate::config::MailScheme;

/// SMTP option set; must equal `config::SMTP_OPTIONS`.
#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "smtp"]
#[uri_config(
    skip_impl,
    descriptor,
    metadata(
        scheme = "smtp",
        description = "SMTP mail producer (send mail)",
        producer
    ),
    crate = "camel_component_api"
)]
pub(super) struct SmtpMetadataDescriptor {
    #[uri_param(name = "account")]
    pub _account: Option<String>,

    #[uri_param(name = "from", required)]
    pub _from: String,

    #[uri_param(name = "to")]
    pub _to: Option<String>,

    #[uri_param(name = "cc")]
    pub _cc: Option<String>,

    #[uri_param(name = "bcc")]
    pub _bcc: Option<String>,

    #[uri_param(name = "subject")]
    pub _subject: Option<String>,

    #[uri_param(name = "contentType", default = "text/plain")]
    pub _content_type: String,

    #[uri_param(name = "allowedRecipients")]
    pub _allowed_recipients: Option<String>,

    #[uri_param(name = "maxRecipients", default = "50")]
    pub _max_recipients: u32,

    #[uri_param(name = "allowCleartext", default = "false")]
    pub _allow_cleartext: bool,
}

pub(crate) fn smtp_metadata(scheme: MailScheme) -> ComponentMetadata {
    let mut meta = SmtpMetadataDescriptor::metadata();
    if scheme == MailScheme::Smtps {
        meta.scheme = "smtps".to_string();
        meta.description = "SMTP over implicit TLS (send mail)".to_string();
    }
    meta
}

/// IMAP option set; must equal `config::IMAP_OPTIONS`.
#[allow(dead_code)]
#[derive(UriConfig)]
#[uri_scheme = "imap"]
#[uri_config(
    skip_impl,
    descriptor,
    metadata(
        scheme = "imap",
        description = "IMAP mailbox polling consumer (receive mail)",
        consumer,
        polling_consumer
    ),
    crate = "camel_component_api"
)]
pub(super) struct ImapMetadataDescriptor {
    #[uri_param(name = "account")]
    pub _account: Option<String>,

    #[uri_param(name = "allowCleartext", default = "false")]
    pub _allow_cleartext: bool,

    #[uri_param(name = "folder", default = "INBOX")]
    pub _folder: String,

    #[uri_param(name = "delay", default = "60000")]
    pub _delay: u64,

    #[uri_param(name = "initialDelay", default = "1000")]
    pub _initial_delay: u64,

    #[uri_param(name = "maxMessagesPerPoll", default = "100")]
    pub _max_messages_per_poll: u32,

    #[uri_param(name = "maxMessageSize", default = "10485760")]
    pub _max_message_size: u64,

    #[uri_param(name = "maxAttempts", default = "3")]
    pub _max_attempts: u32,

    #[uri_param(name = "delete", default = "false")]
    pub _delete: bool,

    #[uri_param(name = "moveTo")]
    pub _move_to: Option<String>,

    #[uri_param(name = "copyTo")]
    pub _copy_to: Option<String>,

    #[uri_param(name = "idempotentRepository")]
    pub _idempotent_repository: Option<String>,
}

pub(crate) fn imap_metadata(scheme: MailScheme) -> ComponentMetadata {
    let mut meta = ImapMetadataDescriptor::metadata();
    if scheme == MailScheme::Imaps {
        meta.scheme = "imaps".to_string();
        meta.description = "IMAP over implicit TLS (receive mail)".to_string();
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{IMAP_OPTIONS, SMTP_OPTIONS};

    #[test]
    fn smtp_metadata_parity() {
        let meta = smtp_metadata(MailScheme::Smtp);
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        let mut want: Vec<&str> = SMTP_OPTIONS.to_vec();
        want.sort_unstable();
        assert_eq!(names, want);
        for o in &meta.uri_options {
            assert_eq!(o.required, o.name == "from", "{}", o.name);
            let d = match o.name.as_str() {
                "contentType" => Some("text/plain"),
                "maxRecipients" => Some("50"),
                "allowCleartext" => Some("false"),
                _ => None,
            };
            if d.is_some() {
                assert_eq!(o.default_value.as_deref(), d, "{}", o.name);
            }
        }
        assert!(meta.capabilities.supports_producer && !meta.capabilities.supports_consumer);
        assert_eq!(smtp_metadata(MailScheme::Smtps).scheme, "smtps");
    }

    #[test]
    fn imap_metadata_parity() {
        let meta = imap_metadata(MailScheme::Imap);
        let mut names: Vec<&str> = meta.uri_options.iter().map(|o| o.name.as_str()).collect();
        names.sort_unstable();
        let mut want: Vec<&str> = IMAP_OPTIONS.to_vec();
        want.sort_unstable();
        assert_eq!(names, want);
        for o in &meta.uri_options {
            assert!(!o.required, "{}", o.name);
            let d = match o.name.as_str() {
                "allowCleartext" => Some("false"),
                "folder" => Some("INBOX"),
                "delay" => Some("60000"),
                "initialDelay" => Some("1000"),
                "maxMessagesPerPoll" => Some("100"),
                "maxMessageSize" => Some("10485760"),
                "maxAttempts" => Some("3"),
                "delete" => Some("false"),
                _ => None,
            };
            assert_eq!(o.default_value.as_deref(), d, "{}", o.name);
        }
        assert!(meta.capabilities.supports_consumer && !meta.capabilities.supports_producer);
        assert!(meta.capabilities.supports_polling_consumer);
        assert_eq!(imap_metadata(MailScheme::Imaps).scheme, "imaps");
    }
}
