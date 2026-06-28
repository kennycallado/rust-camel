//! Claim Check EIP processor.
//!
//! Stashes the message body into a `ClaimCheckRepository` and replaces it with
//! a lightweight key reference (text body containing the key). Later steps can
//! retrieve (Get) or retrieve+remove (GetAndRemove) the body, or use LIFO
//! stack operations (Push/Pop).
//!
//! # Process mode
//!
//! This is a **Process-mode** Tower `Service<Exchange>` — transforms the body
//! in place, no child sub-pipeline. No `StepLifecycle` (holds only
//! `Arc<dyn ClaimCheckRepository>`, no background work).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::Service;

use camel_api::body::Body;
use camel_api::{CamelError, ClaimCheckRepository, Exchange, Message};

/// Extracts a claim-check key string from the exchange (e.g. from header/property/body).
/// Returns an error if the key cannot be resolved (null or empty).
pub type KeyExpression = Arc<dyn Fn(&Exchange) -> Result<String, CamelError> + Send + Sync>;

/// Claim Check operation variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimCheckOp {
    /// Stash the body in the repository; replace exchange body with key reference.
    Set,
    /// Retrieve the body from the repository by key; replace exchange body.
    Get,
    /// Retrieve and remove in one atomic step.
    GetAndRemove,
    /// Push current body onto a LIFO stack for this key; replace body with key reference.
    Push,
    /// Pop body from a LIFO stack for this key; replace exchange body.
    Pop,
}

/// Claim Check EIP processor.
///
/// Transforms the exchange body to/from a `ClaimCheckRepository` using the
/// configured operation and key expression. An optional `filter` controls
/// which parts of the stashed Message are merged back during checkout
/// operations (Get/GetAndRemove/Pop).
#[derive(Clone)]
pub struct ClaimCheckService {
    repository: Arc<dyn ClaimCheckRepository>,
    operation: ClaimCheckOp,
    key_expression: KeyExpression,
    filter: Option<ClaimCheckFilter>,
}

impl std::fmt::Debug for ClaimCheckService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimCheckService")
            .field("repository", &self.repository.name())
            .field("operation", &self.operation)
            .field("filter", &self.filter)
            .finish()
    }
}

impl ClaimCheckService {
    /// Create a new `ClaimCheckService` without a filter.
    pub fn new(
        repository: Arc<dyn ClaimCheckRepository>,
        operation: ClaimCheckOp,
        key_expression: KeyExpression,
    ) -> Self {
        Self {
            repository,
            operation,
            key_expression,
            filter: None,
        }
    }

    /// Attach a filter for selective merge-back during checkout operations.
    pub fn with_filter(mut self, filter: ClaimCheckFilter) -> Self {
        self.filter = Some(filter);
        self
    }
}

/// Merge cached headers from a stashed message into the current message
/// according to filter rules. Called from the async `call` future with
/// all data cloned in advance.
fn merge_stashed(current: &mut Message, stashed: &Message, filter: &ClaimCheckFilter) {
    match filter.body {
        FilterAction::Include => current.body = stashed.body.clone(),
        FilterAction::Exclude => {}
        FilterAction::Remove => current.body = Body::Empty,
    }

    match &filter.headers_action {
        HeadersAction::All(action) => match action {
            FilterAction::Include => {
                for (k, v) in &stashed.headers {
                    current.headers.insert(k.clone(), v.clone());
                }
            }
            FilterAction::Exclude => {}
            FilterAction::Remove => current.headers.clear(),
        },
        HeadersAction::ByPattern {
            include,
            exclude,
            remove,
        } => {
            if !include.is_empty() {
                for (k, v) in &stashed.headers {
                    if include.iter().any(|p| p.matches(k)) {
                        current.headers.insert(k.clone(), v.clone());
                    }
                }
            }
            if !exclude.is_empty() {
                for (k, v) in &stashed.headers {
                    if !exclude.iter().any(|p| p.matches(k)) {
                        current.headers.insert(k.clone(), v.clone());
                    }
                }
            }
            if !remove.is_empty() {
                current
                    .headers
                    .retain(|k, _| !remove.iter().any(|p| p.matches(k)));
            }
        }
    }
}

impl Service<Exchange> for ClaimCheckService {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let repository = self.repository.clone();
        let operation = self.operation.clone();
        let key_result = (self.key_expression)(&exchange);
        let filter = self.filter.clone();

        Box::pin(async move {
            let key = key_result?;
            match operation {
                ClaimCheckOp::Set => {
                    let stashed = exchange.input.clone();
                    repository.set(&key, stashed).await?;
                    exchange.input.body = Body::Text(key);
                    Ok(exchange)
                }
                ClaimCheckOp::Get => {
                    let stashed = repository.get(&key).await?;
                    if let Some(ref f) = filter {
                        merge_stashed(&mut exchange.input, &stashed, f);
                    } else {
                        exchange.input.body = stashed.body;
                    }
                    Ok(exchange)
                }
                ClaimCheckOp::GetAndRemove => {
                    let stashed = repository.get_and_remove(&key).await?;
                    if let Some(ref f) = filter {
                        merge_stashed(&mut exchange.input, &stashed, f);
                    } else {
                        exchange.input.body = stashed.body;
                    }
                    Ok(exchange)
                }
                ClaimCheckOp::Push => {
                    let stashed = exchange.input.clone();
                    repository.push(&key, stashed).await?;
                    exchange.input.body = Body::Text(key);
                    Ok(exchange)
                }
                ClaimCheckOp::Pop => {
                    let stashed = repository.pop(&key).await?;
                    if let Some(ref f) = filter {
                        merge_stashed(&mut exchange.input, &stashed, f);
                    } else {
                        exchange.input.body = stashed.body;
                    }
                    Ok(exchange)
                }
            }
        })
    }
}

fn has_regex_metachars(s: &str) -> bool {
    s.contains(['^', '$', '(', ')', '[', ']', '{', '}', '|', '+', '.', '\\'])
}

#[derive(Debug, Clone)]
pub enum HeaderPattern {
    All,
    Prefix(String),
    Exact(String),
    Regex(regex::Regex),
}

impl PartialEq for HeaderPattern {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::All, Self::All) => true,
            (Self::Prefix(a), Self::Prefix(b)) => a == b,
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            _ => false,
        }
    }
}

impl HeaderPattern {
    fn compile(pattern: &str) -> Result<Self, FilterParseError> {
        if pattern == "*" {
            return Ok(Self::All);
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return Ok(Self::Prefix(prefix.to_string()));
        }
        if has_regex_metachars(pattern) {
            match regex::Regex::new(pattern) {
                Ok(re) => Ok(Self::Regex(re)),
                Err(_) => Err(FilterParseError::InvalidPattern(pattern.to_string())),
            }
        } else {
            Ok(Self::Exact(pattern.to_string()))
        }
    }

    fn matches(&self, header_key: &str) -> bool {
        match self {
            Self::All => true,
            Self::Prefix(prefix) => header_key.starts_with(prefix),
            Self::Exact(exact) => header_key == exact,
            Self::Regex(re) => re.is_match(header_key),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimCheckFilter {
    pub body: FilterAction,
    pub headers_action: HeadersAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    Include,
    Exclude,
    Remove,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HeadersAction {
    All(FilterAction),
    ByPattern {
        include: Vec<HeaderPattern>,
        exclude: Vec<HeaderPattern>,
        remove: Vec<HeaderPattern>,
    },
}

#[derive(Debug)]
pub enum FilterParseError {
    InvalidToken(String),
    InvalidPattern(String),
    MixedIncludeExclude,
}

impl std::fmt::Display for FilterParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidToken(tok) => write!(f, "invalid filter segment '{tok}'"),
            Self::InvalidPattern(pat) => write!(f, "invalid header pattern '{pat}'"),
            Self::MixedIncludeExclude => {
                write!(f, "cannot mix include (+) and exclude (-) header patterns")
            }
        }
    }
}

impl ClaimCheckFilter {
    /// Parse a Camel 4.x filter string into structured rules.
    /// `attachments` tokens are accepted as no-op but do not affect body/headers defaults.
    pub fn parse(input: &str) -> Result<Self, FilterParseError> {
        let mut body_action: Option<FilterAction> = None;
        let mut headers_action: Option<HeadersAction> = None;
        let mut has_positive_include = false;

        for token in input.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }

            let (prefix, rule) = if let Some(rest) = token.strip_prefix("--") {
                (FilterAction::Remove, rest)
            } else if let Some(rest) = token.strip_prefix('-') {
                (FilterAction::Exclude, rest)
            } else {
                let rest = token.strip_prefix('+').unwrap_or(token);
                (FilterAction::Include, rest)
            };

            match rule {
                "body" => {
                    if prefix == FilterAction::Include {
                        has_positive_include = true;
                    }
                    body_action = Some(prefix);
                }
                "headers" | "header" => {
                    if prefix == FilterAction::Include {
                        has_positive_include = true;
                    }
                    headers_action = Some(HeadersAction::All(prefix));
                }
                "attachments" | "attachment" => {
                    // Accepted as no-op; never affects has_positive_include or defaults
                }
                r if r.starts_with("header:") || r.starts_with("headers:") => {
                    let (_, pattern_str) = r
                        .split_once(':')
                        .ok_or_else(|| FilterParseError::InvalidToken(r.to_string()))?;
                    let pattern = HeaderPattern::compile(pattern_str)?;

                    if prefix == FilterAction::Include {
                        has_positive_include = true;
                    }

                    let (mut include, mut exclude, mut remove) = match headers_action.take() {
                        Some(HeadersAction::ByPattern {
                            include,
                            exclude,
                            remove,
                        }) => (include, exclude, remove),
                        _ => (vec![], vec![], vec![]),
                    };
                    match prefix {
                        FilterAction::Include => {
                            if !exclude.is_empty() {
                                return Err(FilterParseError::MixedIncludeExclude);
                            }
                            include.push(pattern);
                        }
                        FilterAction::Exclude => {
                            if !include.is_empty() {
                                return Err(FilterParseError::MixedIncludeExclude);
                            }
                            exclude.push(pattern);
                        }
                        FilterAction::Remove => remove.push(pattern),
                    }
                    headers_action = Some(HeadersAction::ByPattern {
                        include,
                        exclude,
                        remove,
                    });
                }
                other => return Err(FilterParseError::InvalidToken(other.to_string())),
            }
        }

        let default_if_omitted = if has_positive_include {
            FilterAction::Exclude
        } else {
            FilterAction::Include
        };

        Ok(ClaimCheckFilter {
            body: body_action.unwrap_or(default_if_omitted),
            headers_action: headers_action.unwrap_or(HeadersAction::All(default_if_omitted)),
        })
    }
}

#[cfg(test)]
#[path = "claim_check_tests.rs"]
mod tests;
