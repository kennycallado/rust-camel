//! Mail component configuration: accounts, schemes and URI authority parsing.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use camel_api::CamelError;
use camel_component_api::NetworkRetryPolicy;
use serde::Deserialize;

use crate::adapter::smtp;
use crate::address::MailAddress;
use crate::fence::AllowedRecipients;
use crate::headers;

/// A string whose `Debug` output is always `<redacted>`.
#[derive(Clone, Deserialize)]
pub struct SecretString(String);

impl SecretString {
    /// Borrow the wrapped secret. Callers must not log the result.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// One `[components.mail.accounts.<name>]` entry.
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MailAccountConfig {
    /// Login name. IMAP endpoints need it; SMTP sends anonymously without.
    pub username: Option<String>,
    /// Login password; `${env:...}` placeholders resolve at config load.
    pub password: Option<SecretString>,
    /// PEM file with extra trust anchors added to the webpki roots
    /// (`caFile` in TOML).
    pub ca_file: Option<std::path::PathBuf>,
}

impl fmt::Debug for MailAccountConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MailAccountConfig")
            .field("username", &self.username)
            .field("password", &self.password)
            .field("caFile", &self.ca_file)
            .finish()
    }
}

/// `[components.mail]` section.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailComponentConfig {
    /// Named accounts, selected per endpoint with `?account=<name>`.
    #[serde(default)]
    pub accounts: HashMap<String, MailAccountConfig>,
    /// Reconnect policy for the IMAP consumer; `None` means unlimited
    /// attempts, 5 s doubling to 60 s.
    #[serde(default)]
    pub reconnect: Option<NetworkRetryPolicy>,
}

/// Default IMAP reconnect policy: unlimited attempts, 5 s doubling to 60 s.
pub(crate) fn mail_reconnect_default() -> NetworkRetryPolicy {
    NetworkRetryPolicy {
        max_attempts: 0,
        initial_delay: Duration::from_secs(5),
        multiplier: 2.0,
        max_delay: Duration::from_secs(60),
        jitter_factor: 0.0,
        ..Default::default()
    }
}

/// Pick the account for an endpoint: the requested one, or the only one.
pub(crate) fn resolve_account_name(
    accounts: &HashMap<String, MailAccountConfig>,
    requested: Option<&str>,
) -> Result<Option<String>, CamelError> {
    match requested {
        Some(n) => {
            if accounts.contains_key(n) {
                Ok(Some(n.to_string()))
            } else {
                Err(CamelError::Config(format!(
                    "unknown mail account '{n}' — declare it in [components.mail.accounts.{n}] in Camel.toml"
                )))
            }
        }
        None => match accounts.len() {
            0 => Ok(None),
            1 => Ok(accounts.keys().next().cloned()),
            count => {
                let mut names: Vec<&str> = accounts.keys().map(String::as_str).collect();
                names.sort_unstable();
                Err(CamelError::Config(format!(
                    "multiple mail accounts configured ({count}: {}); specify one with ?account=<name> in the URI",
                    names.join(", ")
                )))
            }
        },
    }
}

/// Mail URI scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailScheme {
    /// SMTP with required STARTTLS (port 587).
    Smtp,
    /// SMTP over implicit TLS (port 465).
    Smtps,
    /// IMAP with required STARTTLS (port 143).
    Imap,
    /// IMAP over implicit TLS (port 993).
    Imaps,
}

impl MailScheme {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "smtp" => Some(Self::Smtp),
            "smtps" => Some(Self::Smtps),
            "imap" => Some(Self::Imap),
            "imaps" => Some(Self::Imaps),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Smtp => "smtp",
            Self::Smtps => "smtps",
            Self::Imap => "imap",
            Self::Imaps => "imaps",
        }
    }

    pub(crate) fn default_port(self) -> u16 {
        match self {
            Self::Smtp => 587,
            Self::Smtps => 465,
            Self::Imap => 143,
            Self::Imaps => 993,
        }
    }

    pub(crate) fn implicit_tls(self) -> bool {
        matches!(self, Self::Smtps | Self::Imaps)
    }
}

/// Server host and port from a mail URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MailServerAddress {
    pub(crate) host: String,
    pub(crate) port: u16,
}

/// Parse the `//host[:port]` part of a mail URI (everything after `scheme:`).
pub(crate) fn parse_authority(
    scheme: MailScheme,
    path: &str,
) -> Result<MailServerAddress, CamelError> {
    let rest = path.strip_prefix("//").ok_or_else(|| {
        CamelError::InvalidUri("mail URIs use scheme://host[:port]?options".to_string())
    })?;
    let (authority, tail) = match rest.split_once('/') {
        Some((a, t)) => (a, t),
        None => (rest, ""),
    };
    if !tail.is_empty() {
        return Err(CamelError::InvalidUri("mail URIs take no path".to_string()));
    }
    if authority.contains('@') {
        return Err(CamelError::InvalidUri(
            "mail URIs must not carry userinfo; configure login in [components.mail.accounts] (one [components.mail.accounts.<name>] table per account) in Camel.toml".to_string(),
        ));
    }
    let (host, raw_port) = if let Some(inner) = authority.strip_prefix('[') {
        let (h, after) = inner.split_once(']').ok_or_else(|| {
            CamelError::InvalidUri("mail URI has an unterminated IPv6 literal".to_string())
        })?;
        match after {
            "" => (h, None),
            _ => match after.strip_prefix(':') {
                Some(p) => (h, Some(p)),
                None => {
                    return Err(CamelError::InvalidUri(
                        "mail URI has trailing text after IPv6 literal".to_string(),
                    ));
                }
            },
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, _)) if h.contains(':') => {
                return Err(CamelError::InvalidUri(
                    "mail URI IPv6 host must be written in brackets".to_string(),
                ));
            }
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err(CamelError::InvalidUri(
            "mail URI requires a host".to_string(),
        ));
    }
    let port = match raw_port {
        None => scheme.default_port(),
        Some(raw) => match raw.parse::<u16>() {
            Ok(p) if p != 0 => p,
            _ => {
                return Err(CamelError::InvalidUri(format!(
                    "invalid port '{raw}' in mail URI"
                )));
            }
        },
    };
    Ok(MailServerAddress {
        host: host.to_string(),
        port,
    })
}

/// Body content kind of an outgoing mail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentKind {
    Plain,
    Html,
}

impl ContentKind {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        if raw.eq_ignore_ascii_case("text/plain") {
            Some(Self::Plain)
        } else if raw.eq_ignore_ascii_case("text/html") {
            Some(Self::Html)
        } else {
            None
        }
    }
}

/// Transport security posture of an endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TlsMode {
    StartTls,
    Implicit,
    Cleartext,
}

impl TlsMode {
    fn resolve(scheme: MailScheme, allow_cleartext: bool) -> Result<Self, CamelError> {
        if scheme.implicit_tls() {
            if allow_cleartext {
                return Err(CamelError::InvalidUri(format!(
                    "allowCleartext is not valid on {}: the scheme always uses implicit TLS",
                    scheme.as_str()
                )));
            }
            Ok(Self::Implicit)
        } else if allow_cleartext {
            Ok(Self::Cleartext)
        } else {
            Ok(Self::StartTls)
        }
    }
}

/// URI options of `smtp` and `smtps`; `metadata` must declare the same set.
pub(crate) const SMTP_OPTIONS: &[&str] = &[
    "account",
    "from",
    "to",
    "cc",
    "bcc",
    "subject",
    "contentType",
    "allowedRecipients",
    "maxRecipients",
    "allowCleartext",
];

const DEFAULT_MAX_RECIPIENTS: usize = 50;

/// Parsed `smtp://` / `smtps://` endpoint.
#[derive(Clone, Debug)]
pub(crate) struct SmtpEndpointConfig {
    pub(crate) scheme: MailScheme,
    pub(crate) server: MailServerAddress,
    pub(crate) account: Option<String>,
    pub(crate) from: MailAddress,
    pub(crate) to: Vec<MailAddress>,
    pub(crate) cc: Vec<MailAddress>,
    pub(crate) bcc: Vec<MailAddress>,
    pub(crate) subject: Option<String>,
    pub(crate) content_type: ContentKind,
    pub(crate) allowed_recipients: Option<AllowedRecipients>,
    pub(crate) max_recipients: usize,
    pub(crate) tls: TlsMode,
}

fn opt_list(params: &HashMap<String, String>, key: &str) -> Result<Vec<MailAddress>, CamelError> {
    match params.get(key) {
        None => Ok(Vec::new()),
        Some(raw) => smtp::parse_mailbox_list(raw)
            .map_err(|e| CamelError::InvalidUri(format!("invalid '{key}' address list: {e}"))),
    }
}

/// The URI's scheme when it is one of `allowed`.
fn parse_scheme(raw: &str, allowed: [MailScheme; 2]) -> Result<MailScheme, CamelError> {
    MailScheme::parse(raw)
        .filter(|s| allowed.contains(s))
        .ok_or_else(|| {
            CamelError::InvalidUri(format!(
                "expected {} or {} scheme, got '{raw}'",
                allowed[0].as_str(),
                allowed[1].as_str()
            ))
        })
}

/// Reject query parameters outside `known`, naming the first in sort order.
fn reject_unknown(
    params: &HashMap<String, String>,
    known: &[&str],
    scheme: MailScheme,
) -> Result<(), CamelError> {
    let mut keys: Vec<&String> = params.keys().collect();
    keys.sort();
    match keys.into_iter().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(CamelError::InvalidUri(format!(
            "unknown query parameter '{key}' for scheme '{}'",
            scheme.as_str()
        ))),
        None => Ok(()),
    }
}

fn bool_param(params: &HashMap<String, String>, key: &str) -> Result<bool, CamelError> {
    match params.get(key) {
        None => Ok(false),
        Some(raw) => camel_component_api::uri::parse_bool_param(raw)
            .map_err(|e| CamelError::InvalidUri(format!("{key}: {e}"))),
    }
}

impl SmtpEndpointConfig {
    pub(crate) fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parsed = camel_component_api::parse_uri(uri)?;
        let scheme = parse_scheme(&parsed.scheme, [MailScheme::Smtp, MailScheme::Smtps])?;
        let params = &parsed.params;
        reject_unknown(params, SMTP_OPTIONS, scheme)?;
        let server = parse_authority(scheme, &parsed.path)?;
        let from_raw = params.get("from").ok_or_else(|| {
            CamelError::InvalidUri("smtp endpoint requires the 'from' parameter".to_string())
        })?;
        let from = smtp::parse_mailbox(from_raw)
            .map_err(|e| CamelError::InvalidUri(format!("invalid 'from' address: {e}")))?;
        let subject = match params.get("subject") {
            None => None,
            Some(s) => {
                headers::validate_header_value("subject", s)
                    .map_err(|e| CamelError::InvalidUri(e.to_string()))?;
                Some(s.clone())
            }
        };
        let content_type = match params.get("contentType") {
            None => ContentKind::Plain,
            Some(raw) => ContentKind::parse(raw).ok_or_else(|| {
                CamelError::InvalidUri(format!(
                    "invalid contentType '{raw}': expected text/plain or text/html"
                ))
            })?,
        };
        let allowed_recipients = params
            .get("allowedRecipients")
            .map(|raw| AllowedRecipients::parse(raw))
            .transpose()?;
        let max_recipients = match params.get("maxRecipients") {
            None => DEFAULT_MAX_RECIPIENTS,
            Some(raw) => match raw.parse::<usize>() {
                Ok(n) if n >= 1 => n,
                _ => {
                    return Err(CamelError::InvalidUri(format!(
                        "invalid maxRecipients '{raw}': expected an integer >= 1"
                    )));
                }
            },
        };
        let tls = TlsMode::resolve(scheme, bool_param(params, "allowCleartext")?)?;
        Ok(Self {
            scheme,
            server,
            account: params.get("account").cloned(),
            from,
            to: opt_list(params, "to")?,
            cc: opt_list(params, "cc")?,
            bcc: opt_list(params, "bcc")?,
            subject,
            content_type,
            allowed_recipients,
            max_recipients,
            tls,
        })
    }
}

/// URI options of `imap` and `imaps`; `metadata` must declare the same set.
pub(crate) const IMAP_OPTIONS: &[&str] = &[
    "account",
    "allowCleartext",
    "folder",
    "delay",
    "initialDelay",
    "maxMessagesPerPoll",
    "maxMessageSize",
    "maxAttempts",
    "delete",
    "moveTo",
    "copyTo",
    "idempotentRepository",
];

const DEFAULT_FOLDER: &str = "INBOX";
const DEFAULT_DELAY_MS: u64 = 60_000;
const DEFAULT_INITIAL_DELAY_MS: u64 = 1_000;
const DEFAULT_MAX_MESSAGES_PER_POLL: usize = 100;
const DEFAULT_MAX_MESSAGE_SIZE: u64 = 10_485_760;
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Reject empty folder names and names carrying CR, LF or NUL. Other bytes
/// reach the server as an IMAP quoted string; a non-ASCII folder needs the
/// modified UTF-7 name the server lists.
fn validate_folder_name(option: &str, value: &str) -> Result<(), CamelError> {
    if value.is_empty() {
        return Err(CamelError::InvalidUri(format!(
            "{option} must not be empty"
        )));
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(CamelError::InvalidUri(format!(
            "{option} must not contain CR, LF or NUL"
        )));
    }
    Ok(())
}

fn parse_min_one<T>(
    params: &HashMap<String, String>,
    key: &str,
    default: T,
) -> Result<T, CamelError>
where
    T: std::str::FromStr + PartialOrd + From<u8>,
{
    match params.get(key) {
        None => Ok(default),
        Some(raw) => match raw.parse::<T>() {
            Ok(n) if n >= T::from(1) => Ok(n),
            _ => Err(CamelError::InvalidUri(format!(
                "invalid {key} '{raw}': expected an integer >= 1"
            ))),
        },
    }
}

/// Parsed `imap://` / `imaps://` endpoint.
#[derive(Clone, Debug)]
pub(crate) struct ImapEndpointConfig {
    pub(crate) scheme: MailScheme,
    pub(crate) server: MailServerAddress,
    pub(crate) account: Option<String>,
    pub(crate) tls: TlsMode,
    pub(crate) folder: String,
    pub(crate) delay: Duration,
    pub(crate) initial_delay: Duration,
    pub(crate) max_messages_per_poll: usize,
    pub(crate) max_message_size: u64,
    pub(crate) max_attempts: u32,
    pub(crate) delete: bool,
    pub(crate) move_to: Option<String>,
    pub(crate) copy_to: Option<String>,
    pub(crate) idempotent_repository: Option<String>,
}

impl ImapEndpointConfig {
    pub(crate) fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parsed = camel_component_api::parse_uri(uri)?;
        let scheme = parse_scheme(&parsed.scheme, [MailScheme::Imap, MailScheme::Imaps])?;
        let params = &parsed.params;
        reject_unknown(params, IMAP_OPTIONS, scheme)?;
        let server = parse_authority(scheme, &parsed.path)?;
        let tls = TlsMode::resolve(scheme, bool_param(params, "allowCleartext")?)?;
        let folder = match params.get("folder") {
            None => DEFAULT_FOLDER.to_string(),
            Some(f) => {
                validate_folder_name("folder", f)?;
                f.clone()
            }
        };
        let delay = parse_min_one(params, "delay", DEFAULT_DELAY_MS)?;
        let initial_delay = match params.get("initialDelay") {
            None => DEFAULT_INITIAL_DELAY_MS,
            Some(raw) => raw.parse::<u64>().map_err(|_| {
                CamelError::InvalidUri(format!(
                    "invalid initialDelay '{raw}': expected milliseconds as an integer >= 0"
                ))
            })?,
        };
        let delete = bool_param(params, "delete")?;
        let folder_opt = |key: &str| -> Result<Option<String>, CamelError> {
            params
                .get(key)
                .map(|v| validate_folder_name(key, v).map(|()| v.clone()))
                .transpose()
        };
        let move_to = folder_opt("moveTo")?;
        let copy_to = folder_opt("copyTo")?;
        let idempotent_repository = match params.get("idempotentRepository") {
            None => None,
            Some(v) if v.is_empty() => {
                return Err(CamelError::InvalidUri(
                    "idempotentRepository must not be empty".to_string(),
                ));
            }
            Some(v) => Some(v.clone()),
        };
        if delete && move_to.is_some() {
            return Err(CamelError::InvalidUri(
                "delete and moveTo are mutually exclusive; copyTo combines with either".to_string(),
            ));
        }
        Ok(Self {
            scheme,
            server,
            account: params.get("account").cloned(),
            tls,
            folder,
            delete,
            move_to,
            copy_to,
            idempotent_repository,
            delay: Duration::from_millis(delay),
            initial_delay: Duration::from_millis(initial_delay),
            max_messages_per_poll: parse_min_one(
                params,
                "maxMessagesPerPoll",
                DEFAULT_MAX_MESSAGES_PER_POLL,
            )?,
            max_message_size: parse_min_one(params, "maxMessageSize", DEFAULT_MAX_MESSAGE_SIZE)?,
            max_attempts: parse_min_one(params, "maxAttempts", DEFAULT_MAX_ATTEMPTS)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct() -> MailAccountConfig {
        MailAccountConfig {
            username: None,
            password: None,
            ca_file: None,
        }
    }

    fn map(names: &[&str]) -> HashMap<String, MailAccountConfig> {
        names.iter().map(|n| (n.to_string(), acct())).collect()
    }

    #[test]
    fn resolve_account_single_selects_implicitly() {
        assert_eq!(
            resolve_account_name(&map(&["main"]), None).unwrap(),
            Some("main".to_string())
        );
    }

    #[test]
    fn resolve_account_ambiguous_names_account_param() {
        let e = resolve_account_name(&map(&["a", "b"]), None).unwrap_err();
        let s = e.to_string();
        assert!(s.contains("?account="), "{s}");
        assert!(s.contains('2'), "{s}");
    }

    #[test]
    fn resolve_account_explicit_unknown_errors() {
        let e = resolve_account_name(&map(&["main"]), Some("ghost")).unwrap_err();
        assert!(e.to_string().contains("[components.mail.accounts"));
    }

    #[test]
    fn resolve_account_none_configured_is_anonymous() {
        assert_eq!(resolve_account_name(&map(&[]), None).unwrap(), None);
    }

    #[test]
    fn account_debug_redacts_password() {
        let mut a = acct();
        a.password = Some(SecretString::from("hunter2"));
        let s = format!("{:?}", a);
        assert!(s.contains("<redacted>"));
        assert!(!s.contains("hunter2"));
    }

    #[test]
    fn component_config_debug_redacts_password() {
        let c: MailComponentConfig =
            toml::from_str("[accounts.main]\nusername = \"u\"\npassword = \"hunter2\"").unwrap();
        assert!(!format!("{:?}", c).contains("hunter2"));
    }

    #[test]
    fn account_rejects_unknown_field() {
        assert!(toml::from_str::<MailComponentConfig>("[accounts.main]\nbogus = 1").is_err());
    }

    #[test]
    fn account_parses_ca_file_camel_case() {
        let c: MailComponentConfig =
            toml::from_str("[accounts.main]\ncaFile = \"/tmp/ca.pem\"").unwrap();
        assert_eq!(c.accounts["main"].ca_file, Some("/tmp/ca.pem".into()));
    }

    #[test]
    fn authority_default_ports() {
        for (s, p) in [
            (MailScheme::Smtp, 587),
            (MailScheme::Smtps, 465),
            (MailScheme::Imap, 143),
            (MailScheme::Imaps, 993),
        ] {
            assert_eq!(parse_authority(s, "//h").unwrap().port, p);
        }
    }

    #[test]
    fn authority_explicit_port() {
        let a = parse_authority(MailScheme::Smtp, "//mail.example:2525").unwrap();
        assert_eq!(a.host, "mail.example");
        assert_eq!(a.port, 2525);
    }

    #[test]
    fn authority_rejects_userinfo_without_echo() {
        let e = parse_authority(MailScheme::Smtp, "//user:secret@mail.example:587").unwrap_err();
        let s = e.to_string();
        assert!(s.contains("[components.mail.accounts]"), "{s}");
        assert!(!s.contains("secret"), "{s}");
    }

    #[test]
    fn authority_rejects_empty_host() {
        assert!(parse_authority(MailScheme::Smtp, "//:25").is_err());
    }

    #[test]
    fn authority_rejects_unbracketed_ipv6() {
        assert!(parse_authority(MailScheme::Smtp, "//::1").is_err());
    }

    #[test]
    fn authority_parses_ipv6_literal() {
        let a = parse_authority(MailScheme::Smtp, "//[::1]:2525").unwrap();
        assert_eq!(a.host, "::1");
        assert_eq!(a.port, 2525);
    }

    fn err(uri: &str) -> String {
        SmtpEndpointConfig::from_uri(uri).unwrap_err().to_string()
    }

    #[test]
    fn smtp_config_requires_from() {
        assert!(err("smtp://h?to=a@x.test").contains("from"));
    }

    #[test]
    fn smtp_config_rejects_unknown_param() {
        assert!(err("smtp://h?from=a@x.test&bogus=1").contains("bogus"));
    }

    #[test]
    fn smtp_config_defaults() {
        let c = SmtpEndpointConfig::from_uri("smtp://h?from=bot@corp.test").unwrap();
        assert_eq!(c.server.port, 587);
        assert_eq!(c.tls, TlsMode::StartTls);
        assert_eq!(c.content_type, ContentKind::Plain);
        assert_eq!(c.max_recipients, 50);
        assert!(c.to.is_empty());
        assert_eq!(c.from.email, "bot@corp.test");
        assert!(c.bcc.is_empty());
    }

    #[test]
    fn smtp_config_parses_lists() {
        let c = SmtpEndpointConfig::from_uri(
            "smtp://h?from=bot@corp.test&to=a@corp.test,b@corp.test&cc=c@corp.test",
        )
        .unwrap();
        assert_eq!(c.to.len(), 2);
        assert_eq!(c.cc.len(), 1);
        let c =
            SmtpEndpointConfig::from_uri("smtp://h?from=bot@corp.test&bcc=d@corp.test").unwrap();
        assert_eq!(c.bcc.len(), 1);
    }

    #[test]
    fn smtp_config_allowlist_permits() {
        let c = SmtpEndpointConfig::from_uri(
            "smtp://h?from=a@x.test&allowedRecipients=ops@corp.test,@Dom.Test",
        )
        .unwrap();
        let f = c.allowed_recipients.unwrap();
        let addr = |s: &str| smtp::parse_mailbox(s).unwrap();
        assert!(f.permits(&addr("ops@CORP.test")));
        assert!(f.permits(&addr("x@dom.test")));
        assert!(!f.permits(&addr("dev@corp.test")));
    }

    #[test]
    fn smtps_rejects_allow_cleartext() {
        assert!(err("smtps://h?from=a@x.test&allowCleartext=true").contains("allowCleartext"));
    }

    #[test]
    fn smtp_allow_cleartext_selects_cleartext() {
        let c = SmtpEndpointConfig::from_uri("smtp://h?from=a@x.test&allowCleartext=true").unwrap();
        assert_eq!(c.tls, TlsMode::Cleartext);
        let c = SmtpEndpointConfig::from_uri("smtps://h?from=a@x.test").unwrap();
        assert_eq!(c.tls, TlsMode::Implicit);
        assert_eq!(c.server.port, 465);
    }

    #[test]
    fn smtp_config_rejects_bad_content_type() {
        assert!(err("smtp://h?from=a@x.test&contentType=application/json").contains("contentType"));
    }

    #[test]
    fn smtp_config_rejects_zero_max_recipients() {
        assert!(err("smtp://h?from=a@x.test&maxRecipients=0").contains("maxRecipients"));
    }

    #[test]
    fn smtp_config_rejects_crlf_subject() {
        assert!(err("smtp://h?from=a@x.test&subject=a%0D%0ABcc:x").contains("subject"));
    }

    #[test]
    fn smtp_config_malformed_allowlist_fails_creation() {
        assert!(
            SmtpEndpointConfig::from_uri("smtp://h?from=a@x.test&allowedRecipients=not-an-address")
                .is_err()
        );
    }

    fn imap_err(uri: &str) -> String {
        ImapEndpointConfig::from_uri(uri).unwrap_err().to_string()
    }

    #[test]
    fn imap_config_defaults() {
        let c = ImapEndpointConfig::from_uri("imap://h?account=main").unwrap();
        assert_eq!(c.server.port, 143);
        assert_eq!(c.tls, TlsMode::StartTls);
        assert_eq!(c.account.as_deref(), Some("main"));
        assert_eq!(c.folder, "INBOX");
        assert_eq!(c.delay, Duration::from_secs(60));
        assert_eq!(c.initial_delay, Duration::from_secs(1));
        assert_eq!(c.max_messages_per_poll, 100);
        assert_eq!(c.max_message_size, 10_485_760);
        assert_eq!(c.max_attempts, 3);
        assert_eq!(c.scheme, MailScheme::Imap);
    }

    #[test]
    fn imaps_default_port() {
        let c = ImapEndpointConfig::from_uri("imaps://h").unwrap();
        assert_eq!(c.server.port, 993);
        assert_eq!(c.tls, TlsMode::Implicit);
    }

    #[test]
    fn imaps_rejects_allow_cleartext() {
        assert!(imap_err("imaps://h?allowCleartext=true").contains("allowCleartext"));
    }

    #[test]
    fn imap_rejects_smtp_only_option() {
        assert!(imap_err("imap://h?from=a@x.test").contains("from"));
    }

    #[test]
    fn imap_rejects_invalid_folder() {
        assert!(ImapEndpointConfig::from_uri("imap://h?folder=").is_err());
        assert!(ImapEndpointConfig::from_uri("imap://h?folder=a%0Ab").is_err());
    }

    #[test]
    fn imap_rejects_zero_limits() {
        for opt in [
            "maxAttempts",
            "maxMessagesPerPoll",
            "maxMessageSize",
            "delay",
        ] {
            let e = imap_err(&format!("imap://h?{opt}=0"));
            assert!(e.contains(opt), "{opt}: {e}");
        }
    }

    #[test]
    fn delete_with_move_to_rejected() {
        let e = imap_err("imap://h?delete=true&moveTo=Done");
        assert!(e.contains("mutually exclusive"), "{e}");
    }

    #[test]
    fn copy_to_combines_with_delete_and_move() {
        assert!(ImapEndpointConfig::from_uri("imap://h?delete=true&copyTo=Archive").is_ok());
        assert!(ImapEndpointConfig::from_uri("imap://h?moveTo=Done&copyTo=Archive").is_ok());
    }

    #[test]
    fn empty_move_to_rejected() {
        let e = imap_err("imap://h?moveTo=");
        assert!(e.contains("moveTo"), "{e}");
    }

    #[test]
    fn lf_in_folder_rejected() {
        let e = imap_err("imap://h?copyTo=a%0Ab");
        assert!(e.contains("copyTo"), "{e}");
    }
}
