//! Expectations recorded on a mock endpoint for batch-style assertion.

use camel_matchers::CountBound;

use crate::matcher::{BodyMatcher, HeaderMatcher};

/// One ordered entry in the expected-body list.
///
/// The two setters share one insertion-ordered list: mixed sequences of
/// [`crate::MockEndpointInner::expect_body`] and
/// [`crate::MockEndpointInner::expect_body_matcher`] keep their slots.
pub(crate) enum BodyExpectation {
    /// Exact body equality.
    Exact(camel_component_api::Body),
    /// Matcher evaluation.
    Matcher(BodyMatcher),
}

/// Expectations set on a mock endpoint for batch-style assertion.
///
/// Use [`crate::MockEndpointInner::expect_body`],
/// [`crate::MockEndpointInner::expect_body_matcher`],
/// [`crate::MockEndpointInner::expect_header`] and
/// [`crate::MockEndpointInner::expect_header_matcher`] to populate
/// expectations, then call [`crate::MockEndpointInner::assert_satisfied`]
/// after exchanges have been received.
pub struct MockExpectations {
    pub(crate) expected_bodies: Vec<BodyExpectation>,
    pub(crate) expected_headers: Vec<(String, serde_json::Value)>,
    pub(crate) expected_header_regexes: Vec<(String, String)>,
    pub(crate) expected_header_matchers: Vec<(String, HeaderMatcher)>,
    /// Count bound (from the shared `camel_matchers` algebra) enforced by
    /// [`crate::MockEndpointInner::assert_satisfied`]; `None` means no count
    /// expectation. One bound per endpoint: a later setter replaces the
    /// earlier one.
    pub(crate) count_bound: Option<CountBound>,
}

impl Default for MockExpectations {
    fn default() -> Self {
        Self::new()
    }
}

impl MockExpectations {
    /// Create an empty set of expectations.
    pub fn new() -> Self {
        Self {
            expected_bodies: Vec::new(),
            expected_headers: Vec::new(),
            expected_header_regexes: Vec::new(),
            expected_header_matchers: Vec::new(),
            count_bound: None,
        }
    }

    /// Add an expected body value.
    pub fn push_body(&mut self, body: camel_component_api::Body) {
        self.expected_bodies.push(BodyExpectation::Exact(body));
    }

    /// Add an expected body matcher.
    pub fn push_body_matcher(&mut self, matcher: BodyMatcher) {
        self.expected_bodies.push(BodyExpectation::Matcher(matcher));
    }

    /// Add an expected header key-value pair.
    pub fn push_header(&mut self, key: String, value: serde_json::Value) {
        self.expected_headers.push((key, value));
    }

    /// Add an expected header regex pattern.
    pub fn push_header_regex(&mut self, key: String, pattern: String) {
        self.expected_header_regexes.push((key, pattern));
    }

    /// Add an expected header matcher.
    pub fn push_header_matcher(&mut self, key: String, matcher: HeaderMatcher) {
        self.expected_header_matchers.push((key, matcher));
    }

    /// Set the count bound enforced at assertion time. Replaces any
    /// previously recorded bound (one bound per endpoint).
    pub(crate) fn set_count_bound(&mut self, bound: CountBound) {
        self.count_bound = Some(bound);
    }
}
