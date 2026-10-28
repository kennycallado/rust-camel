//! Recipient allowlist (class B fence).

use std::collections::HashMap;

use camel_api::CamelError;

use crate::adapter::smtp;
use crate::address::MailAddress;
use crate::config::SmtpEndpointConfig;
use crate::error::MailError;
use crate::headers::{CAMEL_MAIL_BCC, CAMEL_MAIL_CC, CAMEL_MAIL_TO};

#[derive(Clone, Debug)]
pub(crate) struct AllowedRecipients {
    exact: Vec<MailAddress>,
    domains: Vec<String>,
}

fn valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !d.starts_with('.')
        && !d.ends_with('.')
        && !d.contains("..")
}

impl AllowedRecipients {
    pub(crate) fn parse(raw: &str) -> Result<Self, CamelError> {
        let mut exact = Vec::new();
        let mut domains = Vec::new();
        for (index, entry) in raw.split(',').map(str::trim).enumerate() {
            if entry.is_empty() {
                return Err(CamelError::InvalidUri(format!(
                    "allowedRecipients entry {index} is empty"
                )));
            }
            let bad = || {
                CamelError::InvalidUri(format!(
                    "allowedRecipients entry '{entry}' is neither an address nor an @domain"
                ))
            };
            if let Some(d) = entry.strip_prefix('@') {
                let d = d.to_ascii_lowercase();
                if !valid_domain(&d) {
                    return Err(bad());
                }
                domains.push(d);
            } else {
                if !entry.contains('@') {
                    return Err(bad());
                }
                exact.push(smtp::parse_mailbox(entry).map_err(|_| bad())?);
            }
        }
        Ok(Self { exact, domains })
    }

    pub(crate) fn permits(&self, addr: &MailAddress) -> bool {
        self.exact
            .iter()
            .any(|e| e.local() == addr.local() && e.domain().eq_ignore_ascii_case(addr.domain()))
            || self.domains.contains(&addr.domain().to_ascii_lowercase())
    }
}

/// Resolved envelope recipients.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Recipients {
    pub(crate) to: Vec<MailAddress>,
    pub(crate) cc: Vec<MailAddress>,
    pub(crate) bcc: Vec<MailAddress>,
}

impl Recipients {
    pub(crate) fn total(&self) -> usize {
        self.to.len() + self.cc.len() + self.bcc.len()
    }

    pub(crate) fn envelope(&self) -> Vec<&MailAddress> {
        self.to
            .iter()
            .chain(self.cc.iter())
            .chain(self.bcc.iter())
            .collect()
    }
}

/// Merge endpoint recipients with fenced routing headers (exact-key lookup).
pub(crate) fn resolve_recipients(
    cfg: &SmtpEndpointConfig,
    headers: &HashMap<String, camel_api::Value>,
) -> Result<Recipients, MailError> {
    let mut r = Recipients {
        to: cfg.to.clone(),
        cc: cfg.cc.clone(),
        bcc: cfg.bcc.clone(),
    };
    for (name, slot) in [
        (CAMEL_MAIL_TO, &mut r.to),
        (CAMEL_MAIL_CC, &mut r.cc),
        (CAMEL_MAIL_BCC, &mut r.bcc),
    ] {
        let Some(value) = headers.get(name) else {
            continue;
        };
        let Some(allowed) = cfg.allowed_recipients.as_ref() else {
            return Err(MailError::RecipientFence(format!(
                "routing header {name} requires allowedRecipients on the endpoint"
            )));
        };
        let camel_api::Value::String(raw) = value else {
            return Err(MailError::RecipientFence(format!(
                "{name} must be a string"
            )));
        };
        let parsed = smtp::parse_mailbox_list(raw).map_err(|e| {
            MailError::RecipientFence(format!("{name} is not a valid address list: {e}"))
        })?;
        for addr in &parsed {
            if !allowed.permits(addr) {
                return Err(MailError::RecipientFence(format!(
                    "{name} address {} is outside allowedRecipients",
                    addr.email
                )));
            }
        }
        *slot = parsed;
    }
    let total = r.total();
    if total == 0 {
        return Err(MailError::NoRecipient);
    }
    if total > cfg.max_recipients {
        return Err(MailError::RecipientFence(format!(
            "{total} recipients exceed maxRecipients={}",
            cfg.max_recipients
        )));
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(uri: &str) -> SmtpEndpointConfig {
        SmtpEndpointConfig::from_uri(uri).expect("test uri")
    }

    fn hdr(name: &str, v: &str) -> HashMap<String, camel_api::Value> {
        HashMap::from([(name.to_string(), camel_api::Value::String(v.to_string()))])
    }

    fn emails(v: &[MailAddress]) -> Vec<&str> {
        v.iter().map(|a| a.email.as_str()).collect()
    }

    #[test]
    fn routing_header_on_unarmed_endpoint_fails() {
        let c = cfg("smtp://h?from=bot@corp.test&to=own@corp.test");
        let e = resolve_recipients(&c, &hdr("CamelMailTo", "a@x.test")).unwrap_err();
        assert!(matches!(&e, MailError::RecipientFence(m) if m.contains("CamelMailTo")));
    }

    #[test]
    fn address_outside_allowlist_fails() {
        let c = cfg("smtp://h?from=bot@corp.test&allowedRecipients=@corp.test");
        let e = resolve_recipients(&c, &hdr("CamelMailTo", "a@evil.test")).unwrap_err();
        assert!(matches!(&e, MailError::RecipientFence(m) if m.contains("a@evil.test")));
    }

    #[test]
    fn allowed_domain_matches_case_insensitively() {
        let c = cfg("smtp://h?from=bot@corp.test&allowedRecipients=@Corp.Test");
        let r = resolve_recipients(&c, &hdr("CamelMailTo", "a@corp.test")).unwrap();
        assert_eq!(emails(&r.to), ["a@corp.test"]);
    }

    #[test]
    fn header_recipients_replace_endpoint_recipients() {
        let c = cfg("smtp://h?from=bot@corp.test&to=a@corp.test&allowedRecipients=@corp.test");
        let r = resolve_recipients(&c, &hdr("CamelMailTo", "b@corp.test")).unwrap();
        assert_eq!(emails(&r.to), ["b@corp.test"]);
        let env: Vec<&str> = r.envelope().iter().map(|a| a.email.as_str()).collect();
        assert_eq!(env, ["b@corp.test"]);
    }

    #[test]
    fn endpoint_recipients_not_fenced() {
        let c = cfg("smtp://h?from=bot@corp.test&to=x@other.test&allowedRecipients=@corp.test");
        let r = resolve_recipients(&c, &HashMap::new()).unwrap();
        assert_eq!(emails(&r.to), ["x@other.test"]);
    }

    #[test]
    fn recipient_cap_enforced() {
        let c = cfg("smtp://h?from=bot@corp.test&allowedRecipients=@corp.test&maxRecipients=2");
        let e = resolve_recipients(
            &c,
            &hdr("CamelMailTo", "a@corp.test,b@corp.test,c@corp.test"),
        )
        .unwrap_err();
        assert!(matches!(&e, MailError::RecipientFence(m) if m.contains("maxRecipients=2")));
    }

    #[test]
    fn no_recipient_fails() {
        let c = cfg("smtp://h?from=bot@corp.test");
        let e = resolve_recipients(&c, &HashMap::new()).unwrap_err();
        assert!(matches!(e, MailError::NoRecipient));
    }

    #[test]
    fn title_case_to_has_no_effect() {
        let c = cfg("smtp://h?from=bot@corp.test&to=own@corp.test");
        let r = resolve_recipients(&c, &hdr("To", "a@x.test")).unwrap();
        let env: Vec<&str> = r.envelope().iter().map(|a| a.email.as_str()).collect();
        assert_eq!(env, ["own@corp.test"]);
    }

    #[test]
    fn bcc_header_lands_in_bcc_slot() {
        let c = cfg("smtp://h?from=bot@corp.test&to=a@corp.test&allowedRecipients=@corp.test");
        let r = resolve_recipients(&c, &hdr("CamelMailBcc", "b@corp.test")).unwrap();
        assert_eq!(emails(&r.bcc), ["b@corp.test"]);
        assert_eq!(emails(&r.to), ["a@corp.test"]);
    }

    #[test]
    fn subdomain_not_matched_by_domain_entry() {
        let c = cfg("smtp://h?from=bot@corp.test&allowedRecipients=@corp.test");
        let e = resolve_recipients(&c, &hdr("CamelMailTo", "x@sub.corp.test")).unwrap_err();
        assert!(matches!(e, MailError::RecipientFence(_)));
    }

    #[test]
    fn multi_at_address_rejected() {
        let c = cfg("smtp://h?from=bot@corp.test&allowedRecipients=@corp.test,@x.test,@evil.test");
        let e = resolve_recipients(&c, &hdr("CamelMailTo", "a@x.test@evil.test")).unwrap_err();
        assert!(matches!(
            e,
            MailError::RecipientFence(_) | MailError::Address(_)
        ));
    }

    #[test]
    fn quoted_local_part_judged_by_real_domain() {
        // lettre rejects quoted local parts, so the smuggling form never parses.
        let raw = "\"a@evil.test\"@corp.test";
        for allow in ["@corp.test", "@evil.test"] {
            let c = cfg(&format!(
                "smtp://h?from=bot@corp.test&allowedRecipients={allow}"
            ));
            let e = resolve_recipients(&c, &hdr("CamelMailTo", raw)).unwrap_err();
            assert!(matches!(e, MailError::RecipientFence(_)));
        }
    }

    #[test]
    fn cap_counts_cc_and_bcc() {
        let c = cfg("smtp://h?from=bot@corp.test&allowedRecipients=@corp.test&maxRecipients=2");
        let h = HashMap::from([
            (
                "CamelMailTo".to_string(),
                camel_api::Value::String("a@corp.test".into()),
            ),
            (
                "CamelMailCc".to_string(),
                camel_api::Value::String("b@corp.test".into()),
            ),
            (
                "CamelMailBcc".to_string(),
                camel_api::Value::String("c@corp.test".into()),
            ),
        ]);
        let e = resolve_recipients(&c, &h).unwrap_err();
        assert!(matches!(&e, MailError::RecipientFence(m) if m.contains("maxRecipients=2")));
    }

    #[test]
    fn non_string_routing_header_fails() {
        let c = cfg("smtp://h?from=bot@corp.test&allowedRecipients=@corp.test&to=a@corp.test");
        let h = HashMap::from([("CamelMailTo".to_string(), camel_api::Value::from(5))]);
        let e = resolve_recipients(&c, &h).unwrap_err();
        assert!(matches!(e, MailError::RecipientFence(_)));
    }

    fn addr(s: &str) -> MailAddress {
        smtp::parse_mailbox(s).unwrap()
    }

    #[test]
    fn allowed_recipients_rejects_malformed() {
        for raw in ["not-an-address", "@", "@corp..test", "a@corp.test,"] {
            assert!(AllowedRecipients::parse(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn allowed_recipients_domain_case_insensitive() {
        let f = AllowedRecipients::parse("@Corp.Test").unwrap();
        assert!(f.permits(&addr("a@corp.test")));
        assert!(!f.permits(&addr("a@evil.test")));
    }

    #[test]
    fn allowed_recipients_exact_address() {
        let f = AllowedRecipients::parse("ops@corp.test").unwrap();
        assert!(f.permits(&addr("ops@CORP.test")));
        assert!(!f.permits(&addr("dev@corp.test")));
    }
}
