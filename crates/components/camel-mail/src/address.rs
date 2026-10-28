//! Parsed mail address.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MailAddress {
    pub(crate) name: Option<String>,
    pub(crate) email: String,
}

impl MailAddress {
    /// Part before the last `@`.
    pub(crate) fn local(&self) -> &str {
        self.email
            .rsplit_once('@')
            .map_or(self.email.as_str(), |(l, _)| l)
    }

    /// Part after the last `@`.
    pub(crate) fn domain(&self) -> &str {
        self.email.rsplit_once('@').map_or("", |(_, d)| d)
    }
}
