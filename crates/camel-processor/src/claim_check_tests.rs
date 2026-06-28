use super::*;
use camel_api::{Exchange, Message, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tower::ServiceExt;

/// In-memory test repository backed by `Mutex<HashMap>`.
#[derive(Debug)]
struct TestRepo {
    name: String,
    keys: Mutex<HashMap<String, Message>>,
    stacks: Mutex<HashMap<String, VecDeque<Message>>>,
}

impl TestRepo {
    fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            keys: Mutex::new(HashMap::new()),
            stacks: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl ClaimCheckRepository for TestRepo {
    fn name(&self) -> &str {
        &self.name
    }

    async fn set(&self, key: &str, payload: Message) -> Result<(), CamelError> {
        self.keys
            .lock()
            .expect("mutex poisoned")
            .insert(key.to_string(), payload);
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Message, CamelError> {
        self.keys
            .lock()
            .expect("mutex poisoned")
            .get(key)
            .cloned()
            .ok_or_else(|| CamelError::RouteError(format!("Claim check key not found: {key}")))
    }

    async fn get_and_remove(&self, key: &str) -> Result<Message, CamelError> {
        self.keys
            .lock()
            .expect("mutex poisoned")
            .remove(key)
            .ok_or_else(|| CamelError::RouteError(format!("Claim check key not found: {key}")))
    }

    async fn remove(&self, key: &str) -> Result<(), CamelError> {
        self.keys.lock().expect("mutex poisoned").remove(key);
        Ok(())
    }

    async fn push(&self, key: &str, payload: Message) -> Result<(), CamelError> {
        self.stacks
            .lock()
            .expect("mutex poisoned")
            .entry(key.to_string())
            .or_default()
            .push_back(payload);
        Ok(())
    }

    async fn pop(&self, key: &str) -> Result<Message, CamelError> {
        self.stacks
            .lock()
            .expect("mutex poisoned")
            .get_mut(key)
            .and_then(|s| s.pop_back())
            .ok_or_else(|| {
                CamelError::RouteError(format!("Claim check stack empty for key: {key}"))
            })
    }
}

fn make_key_expr(key: &str) -> KeyExpression {
    let k = key.to_string();
    Arc::new(move |_ex: &Exchange| Ok(k.clone()))
}

fn make_exchange(body: Body) -> Exchange {
    Exchange::new(Message::new(body))
}

#[tokio::test]
async fn set_moves_body_to_repo() {
    let repo = Arc::new(TestRepo::new("test"));
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Set, make_key_expr("mykey"));
    let exchange = make_exchange(Body::Text("secret-data".to_string()));

    let result = svc.oneshot(exchange).await.unwrap();
    assert_eq!(result.input.body, Body::Text("mykey".to_string()));

    let stashed = repo.get("mykey").await.unwrap();
    assert_eq!(stashed.body, Body::Text("secret-data".to_string()));
}

#[tokio::test]
async fn get_restores_body() {
    let repo = Arc::new(TestRepo::new("test"));
    repo.set("k1", Message::new(Body::Text("stashed-body".to_string())))
        .await
        .unwrap();

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k1"));
    let exchange = make_exchange(Body::Empty);

    let result = svc.oneshot(exchange).await.unwrap();
    assert_eq!(result.input.body, Body::Text("stashed-body".to_string()));
}

#[tokio::test]
async fn get_and_remove_restores_and_deletes() {
    let repo = Arc::new(TestRepo::new("test"));
    repo.set(
        "k2",
        Message::new(Body::Text("will-be-removed".to_string())),
    )
    .await
    .unwrap();

    let svc = ClaimCheckService::new(
        repo.clone(),
        ClaimCheckOp::GetAndRemove,
        make_key_expr("k2"),
    );
    let exchange = make_exchange(Body::Empty);

    let result = svc.oneshot(exchange).await.unwrap();
    assert_eq!(result.input.body, Body::Text("will-be-removed".to_string()));

    let err = repo.get("k2").await.unwrap_err();
    assert!(
        matches!(&err, CamelError::RouteError(msg) if msg.contains("not found")),
        "expected RouteError with 'not found', got: {err:?}"
    );
}

#[tokio::test]
async fn push_pop_lifo() {
    let repo = Arc::new(TestRepo::new("test"));
    let first = Body::Text("first".to_string());
    let second = Body::Text("second".to_string());

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Push, make_key_expr("stack-key"));
    svc.oneshot(make_exchange(first)).await.unwrap();

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Push, make_key_expr("stack-key"));
    svc.oneshot(make_exchange(second.clone())).await.unwrap();

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Pop, make_key_expr("stack-key"));
    let result = svc.oneshot(make_exchange(Body::Empty)).await.unwrap();
    assert_eq!(result.input.body, second);

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Pop, make_key_expr("stack-key"));
    let result = svc.oneshot(make_exchange(Body::Empty)).await.unwrap();
    assert_eq!(result.input.body, Body::Text("first".to_string()));
}

#[tokio::test]
async fn get_missing_propagates_error() {
    let repo = Arc::new(TestRepo::new("test"));
    let svc = ClaimCheckService::new(
        repo.clone(),
        ClaimCheckOp::Get,
        make_key_expr("nonexistent"),
    );
    let exchange = make_exchange(Body::Empty);
    let result = svc.oneshot(exchange).await;
    assert!(
        matches!(&result, Err(CamelError::RouteError(msg)) if msg.contains("not found")),
        "expected Err with 'not found', got: {result:?}"
    );
}

#[tokio::test]
async fn set_overwrites_existing() {
    let repo = Arc::new(TestRepo::new("test"));
    repo.set("k", Message::new(Body::Text("old".to_string())))
        .await
        .unwrap();

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Set, make_key_expr("k"));
    let exchange = make_exchange(Body::Text("new".to_string()));
    svc.oneshot(exchange).await.unwrap();

    let stashed = repo.get("k").await.unwrap();
    assert_eq!(stashed.body, Body::Text("new".to_string()));
}

#[tokio::test]
async fn pop_empty_stack_propagates_error() {
    let repo = Arc::new(TestRepo::new("test"));
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Pop, make_key_expr("empty"));
    let exchange = make_exchange(Body::Empty);
    let result = svc.oneshot(exchange).await;
    assert!(
        matches!(&result, Err(CamelError::RouteError(msg)) if msg.contains("empty")),
        "expected Err with 'empty', got: {result:?}"
    );
}

// --- merge tests with filter ---

#[tokio::test]
async fn get_with_filter_body_only_restores_body() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message {
        body: Body::Text("stashed".into()),
        headers: [("X-Old".into(), Value::String("old".into()))].into(),
    };
    repo.set("k", stashed_msg).await.unwrap();

    let filter = ClaimCheckFilter::parse("body").unwrap();
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"))
        .with_filter(filter);

    let current_msg = Message {
        body: Body::Empty,
        headers: [("X-Current".into(), Value::String("cur".into()))].into(),
    };
    let exchange = Exchange::new(current_msg);
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.body, Body::Text("stashed".into()));
    assert_eq!(
        result.input.headers.get("X-Current"),
        Some(&Value::String("cur".into()))
    );
    assert_eq!(result.input.headers.get("X-Old"), None);
}

#[tokio::test]
async fn get_without_filter_backward_compat() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message::new(Body::Text("stashed".into()));
    repo.set("k", stashed_msg).await.unwrap();

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"));
    let exchange = make_exchange(Body::Empty);
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.body, Body::Text("stashed".into()));
}

#[tokio::test]
async fn get_with_header_wildcard_merges_matching_headers() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message {
        body: Body::Empty,
        headers: [
            ("X-Foo".into(), Value::String("foo".into())),
            ("X-Bar".into(), Value::String("bar".into())),
            ("Other".into(), Value::String("other".into())),
        ]
        .into(),
    };
    repo.set("k", stashed_msg).await.unwrap();

    let filter = ClaimCheckFilter::parse("header:X-*").unwrap();
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"))
        .with_filter(filter);

    let exchange = make_exchange(Body::Empty);
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(
        result.input.headers.get("X-Foo"),
        Some(&Value::String("foo".into()))
    );
    assert_eq!(
        result.input.headers.get("X-Bar"),
        Some(&Value::String("bar".into()))
    );
    assert_eq!(result.input.headers.get("Other"), None);
}

#[tokio::test]
async fn get_with_remove_body_sets_empty() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message::new(Body::Text("stashed".into()));
    repo.set("k", stashed_msg).await.unwrap();

    let filter = ClaimCheckFilter::parse("--body").unwrap();
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"))
        .with_filter(filter);

    let exchange = make_exchange(Body::Text("current".into()));
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.body, Body::Empty);
}

#[tokio::test]
async fn set_preserves_headers_on_exchange() {
    let repo = Arc::new(TestRepo::new("test"));
    let mut msg = Message::new(Body::Text("body".into()));
    msg.set_header("X-Some", Value::String("header".into()));
    let exchange = Exchange::new(msg);

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Set, make_key_expr("k"));
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.body, Body::Text("k".into()));
    assert_eq!(
        result.input.headers.get("X-Some"),
        Some(&Value::String("header".into()))
    );

    let stashed = repo.get("k").await.unwrap();
    assert_eq!(stashed.body, Body::Text("body".into()));
    assert_eq!(
        stashed.headers.get("X-Some"),
        Some(&Value::String("header".into()))
    );
}

#[tokio::test]
async fn get_and_remove_with_filter() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message {
        body: Body::Text("data".into()),
        headers: [("K".into(), Value::String("v".into()))].into(),
    };
    repo.set("k", stashed_msg).await.unwrap();

    let filter = ClaimCheckFilter::parse("body,header:K").unwrap();
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::GetAndRemove, make_key_expr("k"))
        .with_filter(filter);

    let exchange = make_exchange(Body::Empty);
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.body, Body::Text("data".into()));
    assert_eq!(
        result.input.headers.get("K"),
        Some(&Value::String("v".into()))
    );

    let err = repo.get("k").await.unwrap_err();
    assert!(matches!(&err, CamelError::RouteError(msg) if msg.contains("not found")));
}

#[tokio::test]
async fn exclude_header_merges_non_matching() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message {
        body: Body::Empty,
        headers: [
            ("Secret".into(), Value::String("hidden".into())),
            ("Public".into(), Value::String("visible".into())),
        ]
        .into(),
    };
    repo.set("k", stashed_msg).await.unwrap();

    let filter = ClaimCheckFilter::parse("-header:Secret*").unwrap();
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"))
        .with_filter(filter);

    let exchange = make_exchange(Body::Empty);
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.headers.get("Secret"), None);
    assert_eq!(
        result.input.headers.get("Public"),
        Some(&Value::String("visible".into()))
    );
}

#[tokio::test]
async fn remove_headers_deletes_from_current() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message::new(Body::Empty);
    repo.set("k", stashed_msg).await.unwrap();

    let filter = ClaimCheckFilter::parse("--header:ToRemove").unwrap();
    let mut current_msg = Message::new(Body::Empty);
    current_msg.set_header("ToRemove", Value::String("remove-me".into()));
    current_msg.set_header("Keep", Value::String("keep-me".into()));

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"))
        .with_filter(filter);

    let exchange = Exchange::new(current_msg);
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.headers.get("ToRemove"), None);
    assert_eq!(
        result.input.headers.get("Keep"),
        Some(&Value::String("keep-me".into()))
    );
}

#[tokio::test]
async fn remove_all_headers_clears_current() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message::new(Body::Empty);
    repo.set("k", stashed_msg).await.unwrap();

    let filter = ClaimCheckFilter::parse("--headers").unwrap();
    let mut current_msg = Message::new(Body::Empty);
    current_msg.set_header("X-One", Value::String("one".into()));
    current_msg.set_header("X-Two", Value::String("two".into()));

    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"))
        .with_filter(filter);

    let exchange = Exchange::new(current_msg);
    let result = svc.oneshot(exchange).await.unwrap();

    assert!(result.input.headers.is_empty());
}

#[tokio::test]
async fn round_trip_set_then_get_with_filter() {
    let repo = Arc::new(TestRepo::new("test"));

    let mut msg = Message::new(Body::Text("payload".into()));
    msg.set_header("X-Route", Value::String("camel".into()));
    let exchange = Exchange::new(msg);

    let set_svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Set, make_key_expr("k"));
    set_svc.oneshot(exchange).await.unwrap();

    let filter = ClaimCheckFilter::parse("body,header:X-*").unwrap();
    let get_svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"))
        .with_filter(filter);

    let exchange = make_exchange(Body::Empty);
    let result = get_svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.body, Body::Text("payload".into()));
    assert_eq!(
        result.input.headers.get("X-Route"),
        Some(&Value::String("camel".into()))
    );
}

#[tokio::test]
async fn pop_with_filter_merges_headers() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message {
        body: Body::Text("popped".into()),
        headers: [("K".into(), Value::String("v".into()))].into(),
    };
    repo.push("s", stashed_msg).await.unwrap();

    let filter = ClaimCheckFilter::parse("body,header:K").unwrap();
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Pop, make_key_expr("s"))
        .with_filter(filter);

    let exchange = make_exchange(Body::Empty);
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.body, Body::Text("popped".into()));
    assert_eq!(
        result.input.headers.get("K"),
        Some(&Value::String("v".into()))
    );
}

// --- parser tests ---

#[test]
fn parse_body_only() {
    let f = ClaimCheckFilter::parse("body").unwrap();
    assert_eq!(f.body, FilterAction::Include);
    assert_eq!(f.headers_action, HeadersAction::All(FilterAction::Exclude));
}

#[test]
fn parse_exclude_body() {
    let f = ClaimCheckFilter::parse("-body").unwrap();
    assert_eq!(f.body, FilterAction::Exclude);
    assert_eq!(f.headers_action, HeadersAction::All(FilterAction::Include));
}

#[test]
fn parse_body_and_header_wildcard() {
    let f = ClaimCheckFilter::parse("body,header:foo*").unwrap();
    assert_eq!(f.body, FilterAction::Include);
    match &f.headers_action {
        HeadersAction::ByPattern {
            include,
            exclude,
            remove,
        } => {
            assert_eq!(include.len(), 1);
            assert!(matches!(&include[0], HeaderPattern::Prefix(p) if p == "foo"));
            assert!(exclude.is_empty());
            assert!(remove.is_empty());
        }
        other => panic!("expected ByPattern, got {other:?}"),
    }
}

#[test]
fn parse_remove_headers_wildcard() {
    let f = ClaimCheckFilter::parse("--headers:bar*").unwrap();
    assert_eq!(f.body, FilterAction::Include);
    match &f.headers_action {
        HeadersAction::ByPattern {
            include,
            exclude,
            remove,
        } => {
            assert!(include.is_empty());
            assert!(exclude.is_empty());
            assert_eq!(remove.len(), 1);
            assert!(matches!(&remove[0], HeaderPattern::Prefix(p) if p == "bar"));
        }
        other => panic!("expected ByPattern, got {other:?}"),
    }
}

#[test]
fn parse_mixed_include_exclude_rejected() {
    let err = ClaimCheckFilter::parse("header:foo*,-header:bar*").unwrap_err();
    assert!(matches!(err, FilterParseError::MixedIncludeExclude));
}

#[test]
fn parse_invalid_regex_rejected() {
    let err = ClaimCheckFilter::parse("header:[invalid").unwrap_err();
    assert!(matches!(err, FilterParseError::InvalidPattern(_)));
}

#[test]
fn parse_attachments_accepted_as_noop() {
    let f = ClaimCheckFilter::parse("attachments").unwrap();
    assert_eq!(f.body, FilterAction::Include);
    assert_eq!(f.headers_action, HeadersAction::All(FilterAction::Include));
}

#[test]
fn parse_all_headers() {
    let f = ClaimCheckFilter::parse("headers").unwrap();
    assert_eq!(f.headers_action, HeadersAction::All(FilterAction::Include));
}

#[test]
fn parse_exclude_all_headers() {
    let f = ClaimCheckFilter::parse("-headers").unwrap();
    assert_eq!(f.headers_action, HeadersAction::All(FilterAction::Exclude));
}

#[test]
fn parse_remove_all_headers() {
    let f = ClaimCheckFilter::parse("--headers").unwrap();
    assert_eq!(f.headers_action, HeadersAction::All(FilterAction::Remove));
}

#[test]
fn parse_remove_body() {
    let f = ClaimCheckFilter::parse("--body").unwrap();
    assert_eq!(f.body, FilterAction::Remove);
}

#[test]
fn parse_only_header_pattern_defaults_omitted_to_exclude() {
    let f = ClaimCheckFilter::parse("header:foo*").unwrap();
    assert_eq!(f.body, FilterAction::Exclude);
}

#[test]
fn parse_all_negative_tokens_yield_exclude() {
    let f = ClaimCheckFilter::parse("-body,-headers").unwrap();
    assert_eq!(f.body, FilterAction::Exclude);
    assert_eq!(f.headers_action, HeadersAction::All(FilterAction::Exclude));
}

#[test]
fn parse_regex_pattern() {
    let f = ClaimCheckFilter::parse("header:^x-").unwrap();
    match &f.headers_action {
        HeadersAction::ByPattern { include, .. } => {
            assert!(matches!(&include[0], HeaderPattern::Regex(_)));
        }
        other => panic!("expected ByPattern, got {other:?}"),
    }
}

#[test]
fn parse_exact_header() {
    let f = ClaimCheckFilter::parse("header:Content-Type").unwrap();
    match &f.headers_action {
        HeadersAction::ByPattern { include, .. } => {
            assert!(matches!(&include[0], HeaderPattern::Exact(s) if s == "Content-Type"));
        }
        other => panic!("expected ByPattern, got {other:?}"),
    }
}

#[test]
fn parse_all_wildcard_matches_everything() {
    let p = HeaderPattern::compile("*").unwrap();
    assert!(p.matches("anything"));
}

#[test]
fn prefix_pattern_matches_start() {
    let p = HeaderPattern::compile("X-Foo*").unwrap();
    assert!(p.matches("X-Foo-Bar"));
    assert!(!p.matches("X-Bar"));
}

// --- coverage gap: HeadersAction::All(Include) merge ---

#[tokio::test]
async fn get_with_headers_all_include_merges_all_stashed_headers() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message {
        body: Body::Text("data".into()),
        headers: [
            ("A".into(), Value::String("1".into())),
            ("B".into(), Value::String("2".into())),
        ]
        .into(),
    };
    repo.set("k", stashed_msg).await.unwrap();

    let filter = ClaimCheckFilter::parse("body,headers").unwrap();
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"))
        .with_filter(filter);

    let exchange = make_exchange(Body::Empty);
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.body, Body::Text("data".into()));
    assert_eq!(
        result.input.headers.get("A"),
        Some(&Value::String("1".into()))
    );
    assert_eq!(
        result.input.headers.get("B"),
        Some(&Value::String("2".into()))
    );
}

// --- coverage gap: include-then-remove ordering ---

#[tokio::test]
async fn include_then_remove_specific_header() {
    let repo = Arc::new(TestRepo::new("test"));
    let stashed_msg = Message {
        body: Body::Empty,
        headers: [
            ("X-Keep".into(), Value::String("keep".into())),
            ("X-Remove".into(), Value::String("remove".into())),
        ]
        .into(),
    };
    repo.set("k", stashed_msg).await.unwrap();

    // include all X-* headers but then remove X-Remove explicitly
    let filter = ClaimCheckFilter::parse("header:X-*,--header:X-Remove").unwrap();
    let svc = ClaimCheckService::new(repo.clone(), ClaimCheckOp::Get, make_key_expr("k"))
        .with_filter(filter);

    let exchange = make_exchange(Body::Empty);
    let result = svc.oneshot(exchange).await.unwrap();

    assert_eq!(
        result.input.headers.get("X-Keep"),
        Some(&Value::String("keep".into()))
    );
    assert_eq!(result.input.headers.get("X-Remove"), None);
}
