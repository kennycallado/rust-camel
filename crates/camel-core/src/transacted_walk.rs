//! ONE recursive load walk over `BuilderStep` transacted blocks (sqltrans-exec
//! step 3, settles R3-2).
//!
//! [`walk_transacted_block`] is the single visitor both load fan-ins call — the
//! declarative lowering arm and the canonical lowering arms (camel-dsl
//! `compile.rs` and `commands.rs::canonical_step_to_builder_step`) — so the
//! BuilderStep tree is never re-validated by a second implementation. The S1
//! declarative-AST validator (`camel_dsl::transacted_validation`) stays in
//! place for the declarative route gate; this module enforces the same
//! structural set over the lowered `BuilderStep` tree plus the req-5 per-arm
//! guards that S1 does not know about.
//!
//! Precedence (binding, ruling 604c / mission 609 step 3): depth-first in
//! document order; at each node the STRUCTURAL checks run before the PER-ARM
//! checks, both before children recurse; first error wins. After the whole
//! block subtree passes, resolution runs: manager first
//! ([`TransactionError::ManagerMissing`]), then the dialect stage from the
//! datasource URL scheme ([`TransactionError::DialectUnprovable`] /
//! [`TransactionError::UnsupportedJoinDialect`]). Nested `transacted` blocks
//! are a structural rejection reached by the recursion itself, so an inner
//! block can never hit a manager gate before the outer
//! [`TransactionError::NestedBlock`] fires (R3-2: the canonical fan-in walks
//! only the outermost block; see `commands.rs`).
//!
//! Structural set (S1 parity, evaluated over the `BuilderStep` tree):
//! parallel split / multicast / stream split reaching a joined sql step,
//! `wireTap` of a joined sql step, async producer scheme and `seda:` handoffs
//! (only when the block contains a joined sql step, exactly like S1),
//! `outputType=StreamList` (unconditional), foreign datasource, unnamed
//! datasource, nested block.
//!
//! Per-arm set (req-5, every in-block sql step): static-SQL-only — a
//! statement resolved at runtime (URI path `file:` source or
//! `allowDynamicQuery=true`, the header/body opt-in the producer honors) is
//! [`TransactionError::RuntimeSqlInJoinedStep`]; transaction-control and
//! implicit-commit statement text (`BEGIN`/`START TRANSACTION`/`COMMIT`/
//! `ROLLBACK`/`SAVEPOINT`/`RELEASE`/`VACUUM`/`SET AUTOCOMMIT`/
//! `ALTER SYSTEM`, scanned per `;`-separated statement) is
//! [`TransactionError::TransactionControlSql`]; dynamic-routing targets
//! (recipient list / routing slip / dynamic router) are
//! [`TransactionError::DynamicTargetUnprovable`]. Control-keyword matching
//! strips leading SQL comments per `;`-separated statement, so
//! `/* c */ COMMIT` and `-- x\nCOMMIT` are caught (comment obfuscation must
//! not bypass a fail-closed scan).
//!
//! Dialect rules (probe R2-3): PostgreSQL (`postgres`/`postgresql` scheme) is
//! the only supported join dialect in v1; `mysql`/`sqlite` resolve to
//! [`TransactionError::UnsupportedJoinDialect`]; any other scheme is
//! [`TransactionError::DialectUnprovable`] naming the scheme; a datasource
//! whose URL cannot be resolved fails closed with
//! [`TransactionError::DialectUnprovable`] naming the DATASOURCE in the scheme
//! field (no scheme exists to name).
//!
//! Resolver modes: the camel-core canonical boundary owns the transaction
//! registry and calls [`walk_transacted_block`] with registry-backed
//! resolvers; its URL resolver is backed by the boot-installed datasource
//! catalog (bundle boot registers one manager per catalog datasource and
//! installs the catalog for URL resolution). When no catalog is installed —
//! tests, embedded callers — the URL resolver stays always-absent and the
//! dialect stage fails closed naming the datasource. DSL parse seams receive
//! raw text — no `CamelConfig`, no registry — and call
//! [`walk_transacted_block_deferred`] with the manager resolver threaded
//! from the load entry (production: always-absent): structural + per-arm
//! checks run and the manager tail decides IN ONE VISITOR (per-arm BEFORE
//! manager — the frozen precedence); the dialect stage defers to the
//! boundary that owns the catalog.

use std::sync::Arc;

use camel_api::CamelError;
use camel_api::transaction::{TransactionError, TransactionManager};

use crate::lifecycle::application::route_definition::BuilderStep;

/// Answers "what is the configured JDBC/DB URL for this datasource?".
type ResolveUrl<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Resolvers the walk consults after the structural + per-arm pass.
///
/// `resolve_url` is `Some` only when the calling boundary owns a datasource
/// catalog: without it the dialect stage cannot run and manager resolution
/// is the tail (DSL parse seams).
#[derive(Clone, Copy)]
struct WalkResolvers<'a> {
    /// Answers "is a [`TransactionManager`] registered for this datasource?".
    resolve_manager: &'a dyn Fn(&str) -> Option<Arc<dyn TransactionManager>>,
    /// Answers "what is the configured JDBC/DB URL for this datasource?".
    resolve_url: Option<ResolveUrl<'a>>,
}

/// Walk one lowered `BuilderStep::Transacted` block with full resolution.
///
/// Called from the camel-core canonical lowering arm (registry-backed
/// manager resolver) and from direct-call test seams. `resolve_manager` /
/// `resolve_url` are the fixture seams: production resolves zero managers
/// today, so every block still fails [`TransactionError::ManagerMissing`]
/// after the structural + per-arm checks pass.
///
/// Non-`Transacted` input is a caller contract violation and validates to
/// `Ok`; only transacted blocks carry block rules.
pub fn walk_transacted_block(
    step: &BuilderStep,
    resolve_manager: &dyn Fn(&str) -> Option<Arc<dyn TransactionManager>>,
    resolve_url: &dyn Fn(&str) -> Option<String>,
) -> Result<(), CamelError> {
    walk_block(
        step,
        Some(WalkResolvers {
            resolve_manager,
            resolve_url: Some(resolve_url),
        }),
    )
}

/// Walk one lowered `BuilderStep::Transacted` block: structural + per-arm
/// checks first, manager resolution LAST, in one visitor.
///
/// Declarative fan-in entry (`camel-dsl::compile`'s `DeclarativeStep::Transacted`
/// arm and the `on_exceptions` clause lowering): `resolve_manager` is threaded
/// from the load entry (production: always-absent). The frozen walk precedence
/// (per-arm BEFORE manager) needs the tail in the SAME visitor: a clean block
/// with the always-absent resolver fails [`TransactionError::ManagerMissing`]
/// from the walk tail, while a per-arm rejection (transaction-control text,
/// runtime SQL source) is never masked by it.
///
/// The dialect stage stays at the boundary that owns a datasource catalog
/// (probe R2-3) — parse seams resolve no URLs.
pub fn walk_transacted_block_deferred(
    step: &BuilderStep,
    resolve_manager: &dyn Fn(&str) -> Option<Arc<dyn TransactionManager>>,
) -> Result<(), CamelError> {
    walk_block(
        step,
        Some(WalkResolvers {
            resolve_manager,
            resolve_url: None,
        }),
    )
}

/// Walk one lowered `BuilderStep::Transacted` block WITHOUT resolution.
///
/// Canonical parse-seam entry (`camel-dsl::compile`'s `CanonicalStepSpec::Transacted`
/// arm): no canonical load entry owns a registry, so only the structural +
/// per-arm checks run here. Manager resolution for canonical loads stays at
/// the camel-core route-add backstop ([`walk_transacted_block`] with
/// registry-backed resolvers — per-arm still precedes manager there); the
/// dialect stage belongs to the boundary that can resolve URLs (probe R2-3).
pub fn walk_transacted_block_deferred_unresolved(step: &BuilderStep) -> Result<(), CamelError> {
    walk_block(step, None)
}

/// The ONE visitor: structural + per-arm checks over the whole block subtree,
/// then the resolution tail when the calling fan-in owns resolvers.
///
/// Ordering inside one visitor (binding): structural → per-arm → manager →
/// dialect. URL-None with a resolved manager is fail-closed
/// `DialectUnprovable` naming the datasource in the scheme field (probe
/// R2-3: no scheme exists to name).
fn walk_block(step: &BuilderStep, resolvers: Option<WalkResolvers<'_>>) -> Result<(), CamelError> {
    let BuilderStep::Transacted { datasource, steps } = step else {
        return Ok(());
    };
    let datasource = datasource.as_str();

    // Block-wide joined-sql presence gates the async/seda structural rules
    // exactly like S1: a handoff cannot escape a transaction no step joins.
    let joined_sql_in_block = steps_contain_joined_sql(steps, datasource);

    walk_nodes(steps, datasource, joined_sql_in_block)?;

    let Some(resolvers) = resolvers else {
        return Ok(());
    };
    // Manager first, then dialect from the URL scheme — only when this
    // boundary owns a catalog (parse seams end at the manager tail).
    let Some(_manager) = (resolvers.resolve_manager)(datasource) else {
        return Err(CamelError::Transaction(TransactionError::ManagerMissing {
            datasource: datasource.to_string(),
        }));
    };
    let Some(resolve_url) = resolvers.resolve_url else {
        return Ok(());
    };
    let Some(url) = resolve_url(datasource) else {
        return Err(CamelError::Transaction(
            TransactionError::DialectUnprovable {
                scheme: datasource.to_string(),
            },
        ));
    };
    classify_dialect(&url, datasource).map_err(CamelError::Transaction)
}

/// Depth-first, document-order walk of one node sequence.
///
/// At each node: structural checks, then per-arm checks, then children —
/// first error wins. A nested [`BuilderStep::Transacted`] is rejected at its
/// node and never descended into.
fn walk_nodes(
    steps: &[BuilderStep],
    datasource: &str,
    joined_sql_in_block: bool,
) -> Result<(), CamelError> {
    for step in steps {
        check_structural(step, datasource, joined_sql_in_block)?;
        check_per_arm(step)?;
        for child in child_steps(step) {
            walk_nodes(child, datasource, joined_sql_in_block)?;
        }
    }
    Ok(())
}

/// Structural checks (the S1 set) for one node.
fn check_structural(
    step: &BuilderStep,
    datasource: &str,
    joined_sql_in_block: bool,
) -> Result<(), CamelError> {
    // A nested block is rejected first and never traversed as a body.
    if let BuilderStep::Transacted {
        datasource: inner, ..
    } = step
    {
        return Err(CamelError::Transaction(TransactionError::NestedBlock {
            inner: inner.clone(),
        }));
    }

    // StreamList is rejected unconditionally: the streaming body cannot be
    // read inside a transaction that may still roll back.
    if let Some(uri) = static_step_uri(step)
        && let Some(facts) = sql_uri_facts(uri)
        && facts.stream_list
    {
        return Err(CamelError::Transaction(
            TransactionError::StreamListInBlock {
                step: uri.to_string(),
                datasource: datasource.to_string(),
            },
        ));
    }

    match step {
        BuilderStep::WireTap { uri } if is_joined_sql_uri(uri, datasource) => {
            return Err(CamelError::Transaction(TransactionError::WireTapInBlock {
                step: uri.clone(),
                datasource: datasource.to_string(),
            }));
        }
        BuilderStep::DeclarativeSplit {
            parallel, steps, ..
        } if *parallel && steps_contain_joined_sql(steps, datasource) => {
            return Err(CamelError::Transaction(TransactionError::ParallelInBlock {
                step: "split".to_string(),
                datasource: datasource.to_string(),
            }));
        }
        BuilderStep::Split { config, steps }
            if config.parallel && steps_contain_joined_sql(steps, datasource) =>
        {
            return Err(CamelError::Transaction(TransactionError::ParallelInBlock {
                step: "split".to_string(),
                datasource: datasource.to_string(),
            }));
        }
        // Stream splits process fragments concurrently by construction; the
        // parallel-split rule applies without a flag.
        BuilderStep::DeclarativeStreamSplit { steps, .. }
        | BuilderStep::StreamSplit { steps, .. }
            if steps_contain_joined_sql(steps, datasource) =>
        {
            return Err(CamelError::Transaction(TransactionError::ParallelInBlock {
                step: "split".to_string(),
                datasource: datasource.to_string(),
            }));
        }
        BuilderStep::Multicast { steps, .. } if steps_contain_joined_sql(steps, datasource) => {
            return Err(CamelError::Transaction(
                TransactionError::MulticastInBlock {
                    step: "multicast".to_string(),
                    datasource: datasource.to_string(),
                },
            ));
        }
        // `seda` has its own rule: an in-process queue handoff is not an
        // external broker send, so it is deliberately not routed through
        // `is_async_producer_scheme` (S1 parity).
        BuilderStep::To(uri) | BuilderStep::LayeredTo { uri, .. } if joined_sql_in_block => {
            let Some((scheme, _)) = uri.split_once(':') else {
                return Ok(());
            };
            if scheme.eq_ignore_ascii_case("seda") {
                return Err(CamelError::Transaction(TransactionError::SedaInBlock {
                    step: uri.clone(),
                    datasource: datasource.to_string(),
                }));
            }
            if crate::lifecycle::application::route_definition::is_async_producer_scheme(scheme) {
                return Err(CamelError::Transaction(TransactionError::AsyncInBlock {
                    step: uri.clone(),
                    datasource: datasource.to_string(),
                }));
            }
        }
        _ => {}
    }

    // Datasource identity. A sql step with no named datasource is rejected
    // unconditionally: the block-owned transaction cannot adopt it, and a
    // mangled or bare query must never fall through to an implicit
    // datasource.
    if let Some(uri) = static_step_uri(step)
        && let Some(facts) = sql_uri_facts(uri)
    {
        match facts.datasource.as_deref() {
            Some(step_datasource) if step_datasource != datasource => {
                return Err(CamelError::Transaction(
                    TransactionError::ForeignDatasource {
                        step: uri.to_string(),
                        block_datasource: datasource.to_string(),
                        step_datasource: step_datasource.to_string(),
                    },
                ));
            }
            Some(_) => {}
            None => {
                return Err(CamelError::Transaction(
                    TransactionError::UnnamedDatasource {
                        step: uri.to_string(),
                    },
                ));
            }
        }
    }

    Ok(())
}

/// Per-arm checks (req-5) for one node: dynamic-routing targets and the
/// joined-sql statement guards. Only reached after the node's structural
/// checks passed, so an in-block sql step here always names the block
/// datasource.
fn check_per_arm(step: &BuilderStep) -> Result<(), CamelError> {
    match step {
        BuilderStep::DeclarativeDynamicRouter { .. } | BuilderStep::DynamicRouter { .. } => {
            return Err(dynamic_target_unprovable("dynamic-router"));
        }
        BuilderStep::DeclarativeRoutingSlip { .. } | BuilderStep::RoutingSlip { .. } => {
            return Err(dynamic_target_unprovable("routing-slip"));
        }
        BuilderStep::DeclarativeRecipientList { .. } | BuilderStep::RecipientList { .. } => {
            return Err(dynamic_target_unprovable("recipient-list"));
        }
        _ => {}
    }

    if let Some(uri) = static_step_uri(step)
        && let Some(facts) = sql_uri_facts(uri)
    {
        // Runtime statement sources first (R2 precedence chain): `file:` and
        // the `allowDynamicQuery` header/body opt-in both mean the executed
        // SQL is not the route-configured text.
        if facts.runtime_source {
            return Err(CamelError::Transaction(
                TransactionError::RuntimeSqlInJoinedStep {
                    step: uri.to_string(),
                },
            ));
        }
        if let Some(statement) = find_transaction_control_statement(&facts.query) {
            return Err(CamelError::Transaction(
                TransactionError::TransactionControlSql {
                    step: uri.to_string(),
                    statement,
                },
            ));
        }
    }
    Ok(())
}

fn dynamic_target_unprovable(step: &'static str) -> CamelError {
    CamelError::Transaction(TransactionError::DynamicTargetUnprovable {
        step: step.to_string(),
    })
}

/// Facts validation needs about one authored sql step URI.
///
/// Only URIs whose scheme is `sql:` produce facts; every other step kind is
/// invisible to the sql rules. Parameter parsing mirrors S1
/// (`camel_dsl::transacted_validation`): the endpoint layer's grammar is the
/// primary parse and a literal `&`/`=` scan is the fail-closed fallback, so a
/// malformed query can never hide a sql step from these checks.
struct SqlUriFacts {
    /// Value of the `datasource` parameter, when present.
    datasource: Option<String>,
    /// Whether `outputType` is exactly `StreamList`.
    stream_list: bool,
    /// Whether the statement resolves at run time: a `file:` path (checked on
    /// the percent-DECODED path, component parity) or the
    /// `allowDynamicQuery=true` header/body opt-in.
    runtime_source: bool,
    /// The statement text (decoded when the endpoint parser accepts the URI).
    query: String,
}

fn sql_uri_facts(uri: &str) -> Option<SqlUriFacts> {
    let (scheme, rest) = uri.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("sql") {
        return None;
    }
    let (raw_path, raw_query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };

    // Raw-path seed for the no-decode fallback branch only; the decoded
    // `Ok` branch below recomputes it.
    let mut facts = SqlUriFacts {
        datasource: None,
        stream_list: false,
        runtime_source: raw_path.starts_with("file:"),
        query: raw_path.to_string(),
    };

    match camel_endpoint::parse_uri(uri) {
        Ok(parts) => {
            // The component sees the DECODED path (parse_uri percent-decodes;
            // `SqlEndpointConfig::from_components` branches on it), so the
            // `file:` source check must too: `file%3A` decodes to a runtime
            // source and can never bypass the fail-closed scan.
            facts.runtime_source = parts.path.starts_with("file:")
                || parts
                    .params
                    .get("allowDynamicQuery")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true"));
            facts.query = parts.path;
            facts.datasource = parts.params.get("datasource").cloned();
            facts.stream_list = parts
                .params
                .get("outputType")
                .is_some_and(|value| value == "StreamList");
        }
        Err(_) => {
            // Literal scan, no decoding: the keys these rules read hold
            // plain names/values (S1 parity).
            let Some(query) = raw_query else {
                return Some(facts);
            };
            for pair in query.split('&').filter(|pair| !pair.is_empty()) {
                let Some((key, value)) = pair.split_once('=') else {
                    continue;
                };
                match key {
                    "datasource" => facts.datasource = Some(value.to_string()),
                    "outputType" if value == "StreamList" => facts.stream_list = true,
                    "allowDynamicQuery" if value.eq_ignore_ascii_case("true") => {
                        facts.runtime_source = true;
                    }
                    _ => {}
                }
            }
        }
    }
    Some(facts)
}

/// The authored static endpoint URI a step carries, if any.
///
/// Every variant that addresses a statically authored endpoint is covered —
/// `to`, its middleware-wrapped twin `layered to`, `enrich`, `poll_enrich`
/// and `wire_tap` — so the StreamList, datasource and statement rules see all
/// of them.
fn static_step_uri(step: &BuilderStep) -> Option<&str> {
    match step {
        BuilderStep::To(uri)
        | BuilderStep::LayeredTo { uri, .. }
        | BuilderStep::WireTap { uri }
        | BuilderStep::Enrich { uri, .. }
        | BuilderStep::PollEnrich { uri, .. } => Some(uri),
        _ => None,
    }
}

/// Whether one step (or any descendant) is a sql step joined to `datasource`.
fn steps_contain_joined_sql(steps: &[BuilderStep], datasource: &str) -> bool {
    steps.iter().any(|step| {
        is_joined_sql_step(step, datasource)
            || child_steps(step)
                .iter()
                .any(|child| steps_contain_joined_sql(child, datasource))
    })
}

fn is_joined_sql_step(step: &BuilderStep, datasource: &str) -> bool {
    match static_step_uri(step) {
        Some(uri) => is_joined_sql_uri(uri, datasource),
        None => false,
    }
}

fn is_joined_sql_uri(uri: &str, datasource: &str) -> bool {
    match sql_uri_facts(uri) {
        Some(facts) => facts.datasource.as_deref() == Some(datasource),
        None => false,
    }
}

/// Transaction-control and implicit-commit statement text.
///
/// Each `;`-separated statement is judged by its leading keywords, AFTER
/// stripping leading SQL comments (`--` line comments, `/* */` block
/// comments): comment obfuscation must not hide a control statement from the
/// fail-closed scan. The keywords are transaction control (`BEGIN`,
/// `START TRANSACTION`, `COMMIT`, `END`, `ROLLBACK`, `ABORT`, `SAVEPOINT`,
/// `RELEASE`, PostgreSQL's two-word `PREPARE TRANSACTION` — plain `PREPARE`
/// of a statement plan stays allowed), statements PostgreSQL refuses inside
/// a transaction block (`VACUUM`), and statements that implicitly end or
/// step around a transaction (`SET AUTOCOMMIT`, `ALTER SYSTEM`). A word
/// elsewhere in the text (a column named `commits`) never matches.
///
/// The two-word pairs are matched token-exactly on the fully
/// comment-stripped statement (rc-4g25r): extra whitespace, an interleaved
/// comment, or a glued comment cannot slip the pair past the scan, and an
/// identifier continuation (`PREPARE transaction_stmt AS …`) is never a
/// false positive.
fn find_transaction_control_statement(query: &str) -> Option<String> {
    const FIRST_WORDS: &[&str] = &[
        "begin",
        "start",
        "commit",
        "end",
        "rollback",
        "abort",
        "savepoint",
        "release",
        "vacuum",
    ];
    const LEADING_PAIRS: &[&[&str; 2]] = &[
        &["set", "autocommit"],
        &["alter", "system"],
        &["prepare", "transaction"],
    ];

    for statement in query.split(';') {
        let Some(trimmed) = strip_leading_comments(statement) else {
            continue;
        };
        let lower = trimmed.to_ascii_lowercase();
        let mut words = lower.split_whitespace();
        if let Some(first_word) = words.next()
            && (FIRST_WORDS.contains(&first_word)
                // Leading keyword PAIR: judged token-exactly on the fully
                // comment-stripped statement (rc-4g25r) — extra whitespace,
                // comments between or glued to the words cannot bypass, and
                // an identifier continuation (`PREPARE transaction_stmt AS
                // …`) is never a false positive.
                || LEADING_PAIRS
                    .iter()
                    .any(|pair| leads_with_keyword_pair(&strip_all_comments(&lower), pair))
                // A comment glued to the keyword (`BEGIN--x`, `COMMIT/*y*/`)
                // makes the whole-token match above miss; the suffix is not
                // part of the keyword, so strip it and judge the head.
                || comment_suffix_head(first_word)
                    .is_some_and(|head| FIRST_WORDS.contains(&head)))
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Whether the comment-stripped statement's first two whitespace tokens are
/// exactly the pair's keywords (rc-4g25r).
///
/// Matching tokens — not a string prefix — is the point: `PREPARE
/// transaction_stmt AS …` (plan preparation whose name happens to start with
/// `transaction`) must load, while `PREPARE\t TRANSACTION` or `PREPARE
/// /*c*/ TRANSACTION` must reject. A punctuation suffix glued to the second
/// keyword still matches (`AUTOCOMMIT=0`), the operand shape
/// [`comment_suffix_head`] already tolerates on the first word.
fn leads_with_keyword_pair(stripped: &str, pair: &[&str; 2]) -> bool {
    let mut words = stripped.split_whitespace();
    let (Some(first), Some(second)) = (words.next(), words.next()) else {
        return false;
    };
    first == pair[0] && keyword_with_punctuation_suffix(second, pair[1])
}

/// `keyword` or `keyword` glued to a punctuation operand: `autocommit=0`
/// matches `autocommit`; an identifier continuation never does
/// (`transaction_stmt` does not match `transaction`). The input is already
/// lowercased ASCII, so byte indexing preserves character boundaries.
fn keyword_with_punctuation_suffix(token: &str, keyword: &str) -> bool {
    token == keyword
        || (token.len() > keyword.len()
            && token.starts_with(keyword)
            && !matches!(token.as_bytes()[keyword.len()], b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

/// Strip EVERY block/line comment from a statement (rc-4g25r).
///
/// `/*…*/` becomes one separator space (a glued keyword pair splits back
/// apart); `--…` is dropped through end of line, the newline kept as a token
/// separator. Unterminated comments swallow the rest — nothing executable
/// follows them. No string-literal awareness: a quote containing a comment
/// marker is corrupted for matching purposes only, the class recorded
/// fail-closed (bd rc-xbela). Only the leading-pair check consumes this.
fn strip_all_comments(statement: &str) -> String {
    let mut out = String::with_capacity(statement.len());
    let mut rest = statement;
    loop {
        // The earliest of `/*` and `--` opens the next comment.
        let Some(idx) = [rest.find("/*"), rest.find("--")]
            .into_iter()
            .flatten()
            .min()
        else {
            out.push_str(rest);
            return out;
        };
        let is_block = rest[idx..].starts_with("/*");
        out.push_str(&rest[..idx]);
        let after = &rest[idx + 2..];
        match after.find(if is_block { "*/" } else { "\n" }) {
            // Block comment → one separator space; line comment → dropped,
            // the newline itself kept as a separator.
            Some(end) => {
                if is_block {
                    out.push(' ');
                    rest = &after[end + 2..];
                } else {
                    out.push('\n');
                    rest = &after[end..];
                }
            }
            None => {
                out.push(' ');
                return out;
            }
        }
    }
}

/// The keyword head of a first token that carries a glued comment suffix.
///
/// `begin--x` → `begin`, `commit/*y*/` → `commit`; a token without a
/// comment marker is returned unchanged (`beginning` never becomes
/// `begin`). The input is already lowercased ASCII, so byte lengths are
/// preserved.
fn comment_suffix_head(word: &str) -> Option<&str> {
    let cut = word
        .find("--")
        .into_iter()
        .chain(word.find("/*"))
        .min()
        .unwrap_or(word.len());
    let head = &word[..cut];
    if head.is_empty() { None } else { Some(head) }
}

/// Strip leading whitespace and SQL comment tokens from one statement.
///
/// Iterates so stacked comments (`/* a */ -- b\nCOMMIT`) are peeled in order;
/// a statement left empty (comments only) yields `None` and cannot match. An
/// unterminated comment (line comment without a newline, block comment
/// without a terminator) swallows the rest of the statement — there is
/// nothing executable behind it.
fn strip_leading_comments(statement: &str) -> Option<&str> {
    let mut rest = statement.trim_start();
    loop {
        if let Some(body) = rest.strip_prefix("--") {
            // Line comment: skip past the next newline.
            rest = body[body.find('\n')? + 1..].trim_start();
        } else if let Some(body) = rest.strip_prefix("/*") {
            // Block comment: skip past the terminator.
            rest = body[body.find("*/")? + 2..].trim_start();
        } else {
            break;
        }
    }
    if rest.is_empty() {
        return None;
    }
    Some(rest)
}

/// Dialect identity comes from the URL scheme, never the provider.
///
/// PostgreSQL is the only supported join dialect in v1; `mysql` and `sqlite`
/// carry named [`TransactionError::UnsupportedJoinDialect`] rejections; every
/// other scheme is unprovable.
fn classify_dialect(url: &str, datasource: &str) -> Result<(), TransactionError> {
    let Some((scheme, _)) = url.split_once(':') else {
        return Err(TransactionError::DialectUnprovable {
            scheme: datasource.to_string(),
        });
    };
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        "postgres" | "postgresql" => Ok(()),
        "mysql" | "sqlite" => Err(TransactionError::UnsupportedJoinDialect {
            dialect: scheme,
            datasource: datasource.to_string(),
        }),
        _ => Err(TransactionError::DialectUnprovable { scheme }),
    }
}

/// Direct child step slices of one step, in document order.
///
/// A nested [`BuilderStep::Transacted`] is NOT a generic child: it is a
/// structural rejection at its own node, so its body is never traversed as
/// part of the enclosing block (S1 parity).
fn child_steps(step: &BuilderStep) -> Vec<&[BuilderStep]> {
    match step {
        BuilderStep::DeclarativeFilter { steps, .. }
        | BuilderStep::Filter { steps, .. }
        | BuilderStep::DeclarativeSplit { steps, .. }
        | BuilderStep::DeclarativeStreamSplit { steps, .. }
        | BuilderStep::StreamSplit { steps, .. }
        | BuilderStep::Split { steps, .. }
        | BuilderStep::Multicast { steps, .. }
        | BuilderStep::Throttle { steps, .. }
        | BuilderStep::LoadBalance { steps, .. }
        | BuilderStep::Loop { steps, .. }
        | BuilderStep::DeclarativeLoop { steps, .. }
        | BuilderStep::IdempotentConsumer { steps, .. }
        | BuilderStep::Layered { steps, .. } => vec![steps.as_slice()],
        BuilderStep::DeclarativeChoice { whens, otherwise } => {
            let mut children: Vec<&[BuilderStep]> = Vec::new();
            for when in whens {
                children.push(when.steps.as_slice());
            }
            if let Some(otherwise) = otherwise {
                children.push(otherwise.as_slice());
            }
            children
        }
        BuilderStep::Choice { whens, otherwise } => {
            let mut children: Vec<&[BuilderStep]> = Vec::new();
            for when in whens {
                children.push(when.steps.as_slice());
            }
            if let Some(otherwise) = otherwise {
                children.push(otherwise.as_slice());
            }
            children
        }
        BuilderStep::Cache { on_miss, .. } => vec![on_miss.as_slice()],
        BuilderStep::DeclarativeDoTry {
            try_steps,
            catch,
            finally,
        } => {
            let mut children: Vec<&[BuilderStep]> = vec![try_steps.as_slice()];
            for clause in catch {
                children.push(clause.steps.as_slice());
            }
            if let Some(finally) = finally {
                children.push(finally.steps.as_slice());
            }
            children
        }
        BuilderStep::DoTry {
            try_steps,
            catches,
            finally,
        } => {
            let mut children: Vec<&[BuilderStep]> = vec![try_steps.as_slice()];
            for clause in catches {
                children.push(clause.steps.as_slice());
            }
            if let Some(finally) = finally {
                children.push(finally.steps.as_slice());
            }
            children
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use camel_api::LanguageExpressionDef;
    use camel_api::transaction::OpenSessionFuture;

    use super::*;
    use crate::lifecycle::application::route_definition::DeclarativeWhenStep;

    struct FakeManager {
        datasource: String,
    }

    impl TransactionManager for FakeManager {
        fn datasource(&self) -> &str {
            &self.datasource
        }

        fn open_session(&self) -> OpenSessionFuture<'_> {
            panic!("the load walk must never open a transaction session");
        }

        fn acquire_deadline(&self) -> Duration {
            Duration::from_secs(1)
        }
    }

    fn fake_manager(datasource: &str) -> Arc<dyn TransactionManager> {
        Arc::new(FakeManager {
            datasource: datasource.to_string(),
        })
    }

    /// Resolver registering only `registered_ds`.
    fn resolve_registered(name: &str) -> Option<Arc<dyn TransactionManager>> {
        (name == "registered_ds").then(|| fake_manager("registered_ds"))
    }

    fn sql(uri: &str) -> BuilderStep {
        BuilderStep::To(uri.to_string())
    }

    fn block(datasource: &str, steps: Vec<BuilderStep>) -> BuilderStep {
        BuilderStep::Transacted {
            datasource: datasource.to_string(),
            steps,
        }
    }

    fn predicate() -> LanguageExpressionDef {
        LanguageExpressionDef {
            language: "simple".into(),
            source: "${body}".into(),
        }
    }

    /// Unwraps the typed transaction error so tests can assert the exact
    /// variant instead of a rendered message.
    fn transaction_error(step: &BuilderStep) -> TransactionError {
        match walk_transacted_block(step, &resolve_registered, &|_datasource| {
            Some("postgres://walk-seam/db".to_string())
        }) {
            Ok(()) => panic!("expected a typed transaction error"),
            Err(CamelError::Transaction(error)) => error,
            Err(other) => panic!("expected CamelError::Transaction, got {other:?}"),
        }
    }

    /// Same as [`transaction_error`], but the datasource resolves to an
    /// unrecognized `opaque` URL scheme.
    fn opaque_scheme_error(step: &BuilderStep) -> TransactionError {
        match walk_transacted_block(step, &resolve_registered, &|_datasource| {
            Some("opaque://host/db".to_string())
        }) {
            Err(CamelError::Transaction(error)) => error,
            other => panic!("expected DialectUnprovable, got {other:?}"),
        }
    }

    /// The violating sql step nested one level deep in each recursive
    /// container (choice arm, filter body, doTry body, loop body): the walk
    /// must reach it through every container at any depth.
    fn nested_containers(body: BuilderStep) -> Vec<BuilderStep> {
        vec![
            // choice arm
            BuilderStep::DeclarativeChoice {
                whens: vec![DeclarativeWhenStep {
                    predicate: predicate(),
                    steps: vec![body.clone()],
                }],
                otherwise: None,
            },
            // filter body
            BuilderStep::DeclarativeFilter {
                predicate: predicate(),
                steps: vec![body.clone()],
            },
            // doTry body
            BuilderStep::DeclarativeDoTry {
                try_steps: vec![body.clone()],
                catch: Vec::new(),
                finally: None,
            },
            // loop body
            BuilderStep::DeclarativeLoop {
                count: Some(1),
                while_predicate: None,
                steps: vec![body],
                max_iterations: None,
            },
        ]
    }

    #[test]
    fn walk_rejects_dialect_unprovable() {
        // Fake manager + unresolvable-scheme URL through the resolver seam:
        // resolution cannot stop at ManagerMissing, so the scheme is named.
        let step = block(
            "registered_ds",
            vec![sql("sql:select 1?datasource=registered_ds")],
        );
        let error = match walk_transacted_block(&step, &resolve_registered, &|_datasource| {
            Some("opaque://host/db".to_string())
        }) {
            Err(CamelError::Transaction(error)) => error,
            other => panic!("expected DialectUnprovable, got {other:?}"),
        };
        assert_eq!(
            error,
            TransactionError::DialectUnprovable {
                scheme: "opaque".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_unresolvable_url_naming_the_datasource() {
        // Probe R2-3: a resolved manager whose URL cannot be resolved fails
        // closed, and no scheme exists to name — the DATASOURCE is named in
        // the scheme field.
        let step = block(
            "registered_ds",
            vec![sql("sql:select 1?datasource=registered_ds")],
        );
        let error = match walk_transacted_block(&step, &resolve_registered, &|_datasource| None) {
            Err(CamelError::Transaction(error)) => error,
            other => panic!("expected DialectUnprovable, got {other:?}"),
        };
        assert_eq!(
            error,
            TransactionError::DialectUnprovable {
                scheme: "registered_ds".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_dialect_unprovable_in_every_nesting() {
        for container in nested_containers(sql("sql:select 1?datasource=registered_ds")) {
            let nested = block("registered_ds", vec![container]);
            assert_eq!(
                opaque_scheme_error(&nested),
                TransactionError::DialectUnprovable {
                    scheme: "opaque".to_string(),
                },
                "the walk must reach the sql step through choice/filter/doTry/loop"
            );
        }
    }

    #[test]
    fn walk_rejects_unsupported_join_dialect_mysql_and_sqlite() {
        for (url, dialect) in [("mysql://host/db", "mysql"), ("sqlite://mem", "sqlite")] {
            let step = block(
                "registered_ds",
                vec![sql("sql:select 1?datasource=registered_ds")],
            );
            let error = match walk_transacted_block(&step, &resolve_registered, &|_datasource| {
                Some(url.to_string())
            }) {
                Err(CamelError::Transaction(error)) => error,
                other => panic!("expected UnsupportedJoinDialect for {url}, got {other:?}"),
            };
            assert_eq!(
                error,
                TransactionError::UnsupportedJoinDialect {
                    dialect: dialect.to_string(),
                    datasource: "registered_ds".to_string(),
                }
            );
        }
    }

    #[test]
    fn walk_accepts_clean_block_on_postgres() {
        let step = block(
            "registered_ds",
            vec![
                sql("sql:select 1?datasource=registered_ds"),
                sql("sql:insert into t values (1)?datasource=registered_ds"),
            ],
        );
        assert!(
            walk_transacted_block(&step, &resolve_registered, &|_datasource| {
                Some("postgresql://host/db".to_string())
            })
            .is_ok(),
            "a clean block on a registered postgres datasource must pass the walk"
        );
    }

    #[test]
    fn walk_rejects_transaction_control_sql() {
        let step = block(
            "registered_ds",
            vec![sql("sql:update t set a=1; COMMIT?datasource=registered_ds")],
        );
        assert_eq!(
            transaction_error(&step),
            TransactionError::TransactionControlSql {
                step: "sql:update t set a=1; COMMIT?datasource=registered_ds".to_string(),
                statement: "COMMIT".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_transaction_control_sql_nested() {
        for container in nested_containers(sql("sql:select 1; begin?datasource=registered_ds")) {
            let nested = block("registered_ds", vec![container]);
            match transaction_error(&nested) {
                TransactionError::TransactionControlSql { step, statement } => {
                    // The offending `;`-separated statement is named, not the
                    // whole query text.
                    assert_eq!(statement, "begin");
                    assert!(step.contains("begin"), "error must name the step: {step}");
                }
                other => panic!("expected TransactionControlSql, got {other:?}"),
            }
        }
    }

    #[test]
    fn walk_accepts_sql_mentioning_control_words_mid_statement() {
        let step = block(
            "registered_ds",
            vec![sql(
                "sql:select * from commits where v = 'begin'?datasource=registered_ds",
            )],
        );
        assert!(
            transaction_error_free(&step),
            "mid-statement control words are not statements"
        );
    }

    fn transaction_error_free(step: &BuilderStep) -> bool {
        walk_transacted_block(step, &resolve_registered, &|_datasource| {
            Some("postgres://walk-seam/db".to_string())
        })
        .is_ok()
    }

    #[test]
    fn walk_rejects_end_and_abort_dialect_aliases() {
        // PostgreSQL equivalents of COMMIT/ROLLBACK: END commits, ABORT rolls
        // back — both are transaction control for a joined step.
        for (query, statement) in [
            ("sql:end?datasource=registered_ds", "end"),
            ("sql:ABORT?datasource=registered_ds", "ABORT"),
        ] {
            let step = block("registered_ds", vec![sql(query)]);
            match transaction_error(&step) {
                TransactionError::TransactionControlSql {
                    step: named,
                    statement: text,
                } => {
                    assert_eq!(named, query);
                    assert_eq!(text, statement);
                }
                other => panic!("expected TransactionControlSql for {query}, got {other:?}"),
            }
        }
    }

    #[test]
    fn walk_rejects_comment_obfuscated_control_sql() {
        // Leading comment tokens are stripped before keyword matching, so
        // obfuscation cannot bypass the fail-closed scan.
        for query in [
            "sql:/* c */ COMMIT?datasource=registered_ds",
            "sql:-- x\nCOMMIT?datasource=registered_ds",
            "sql:/* a */ -- b\n ROLLBACK?datasource=registered_ds",
        ] {
            let step = block("registered_ds", vec![sql(query)]);
            match transaction_error(&step) {
                TransactionError::TransactionControlSql {
                    step: named,
                    statement,
                } => {
                    assert_eq!(named, query, "the error names the authored step");
                    let lower = statement.to_ascii_lowercase();
                    assert!(
                        lower.contains("commit") || lower.contains("rollback"),
                        "the executable statement is named, got: {statement}"
                    );
                }
                other => panic!("expected TransactionControlSql for {query}, got {other:?}"),
            }
        }
    }

    #[test]
    fn walk_rejects_control_sql_with_comment_glued_to_keyword() {
        // A comment suffix glued to the keyword (no space) must not split the
        // token into a miss: `BEGIN--x` is a line-commented BEGIN,
        // `COMMIT/*y*/` a block-commented COMMIT.
        for query in [
            "sql:BEGIN--x?datasource=registered_ds",
            "sql:COMMIT/*y*/?datasource=registered_ds",
            "sql:rollback--note\nselect 1?datasource=registered_ds",
        ] {
            let step = block("registered_ds", vec![sql(query)]);
            assert!(
                matches!(
                    transaction_error(&step),
                    TransactionError::TransactionControlSql { .. }
                ),
                "a keyword with a glued comment suffix is transaction control: {query}"
            );
        }
    }

    #[test]
    fn walk_rejects_prepare_transaction_but_allows_plain_prepare() {
        // PostgreSQL's two-word `PREPARE TRANSACTION` ends the transaction —
        // rejected like COMMIT. Plain `PREPARE stmt` (plan preparation) is
        // ordinary DDL and stays allowed.
        let step = block(
            "registered_ds",
            vec![sql(
                "sql:PREPARE TRANSACTION 'tx1'?datasource=registered_ds",
            )],
        );
        match transaction_error(&step) {
            TransactionError::TransactionControlSql { statement, .. } => {
                let lower = statement.to_ascii_lowercase();
                assert!(
                    lower.starts_with("prepare transaction"),
                    "the two-word form is named, got: {statement}"
                );
            }
            other => {
                panic!("expected TransactionControlSql for PREPARE TRANSACTION, got {other:?}")
            }
        }

        let plain = block(
            "registered_ds",
            vec![sql(
                "sql:PREPARE pstmt AS SELECT 1?datasource=registered_ds",
            )],
        );
        assert!(
            transaction_error_free(&plain),
            "plain PREPARE is not transaction control"
        );
    }

    #[test]
    fn walk_rejects_prepare_transaction_despite_whitespace_and_comments() {
        // rc-4g25r (a): the leading pair is judged token-exactly on the
        // fully comment-stripped statement — extra whitespace, a comment
        // between or glued to the words, and a line comment in between
        // cannot bypass the fail-closed scan.
        for query in [
            "sql:PREPARE\t TRANSACTION?datasource=registered_ds",
            "sql:PREPARE /*c*/ TRANSACTION?datasource=registered_ds",
            "sql:PREPARE/*g*/TRANSACTION?datasource=registered_ds",
            "sql:PREPARE\n-- note\nTRANSACTION?datasource=registered_ds",
            "sql:SET AUTOCOMMIT=0?datasource=registered_ds",
        ] {
            let step = block("registered_ds", vec![sql(query)]);
            assert!(
                matches!(
                    transaction_error(&step),
                    TransactionError::TransactionControlSql { .. }
                ),
                "two-word PREPARE TRANSACTION is transaction control: {query}"
            );
        }
    }

    #[test]
    fn walk_allows_plain_prepare_with_transaction_prefixed_name() {
        // rc-4g25r (b): the old string-prefix match rejected
        // `PREPARE transaction_stmt AS …` (plan name starting with
        // `transaction`); token-exact matching never false-positives, and a
        // plain plan name still loads.
        for query in [
            "sql:PREPARE transaction_stmt AS SELECT 1?datasource=registered_ds",
            "sql:PREPARE plan AS SELECT 1?datasource=registered_ds",
        ] {
            let step = block("registered_ds", vec![sql(query)]);
            assert!(
                transaction_error_free(&step),
                "plain PREPARE of a statement plan is not transaction control: {query}"
            );
        }
    }

    #[test]
    fn walk_rejects_percent_encoded_file_source() {
        // The component branches on the percent-DECODED path (`parse_uri`
        // decodes; `file%3A` becomes `file:`), so the walk's runtime-source
        // scan must too: encoded `file%3A` can never bypass the fail-closed
        // rejection.
        let step = block(
            "registered_ds",
            vec![sql("sql:file%3A/queries/q.sql?datasource=registered_ds")],
        );
        assert_eq!(
            transaction_error(&step),
            TransactionError::RuntimeSqlInJoinedStep {
                step: "sql:file%3A/queries/q.sql?datasource=registered_ds".to_string(),
            }
        );
    }

    #[test]
    fn walk_accepts_benign_comment_prefix_on_select() {
        // No false positive: a comment prefix on a clean statement still
        // loads once the comments are stripped.
        let step = block(
            "registered_ds",
            vec![sql(
                "sql:/* load-time check */ select 1?datasource=registered_ds",
            )],
        );
        assert!(transaction_error_free(&step));
    }

    #[test]
    fn walk_reports_structural_before_per_arm() {
        // Precedence (binding, 604c): at a node the STRUCTURAL check runs
        // before the per-arm guard — a parallel split over a joined step
        // whose statement also contains COMMIT reports ParallelInBlock.
        let step = block(
            "registered_ds",
            vec![BuilderStep::DeclarativeSplit {
                expression: predicate(),
                aggregation: camel_api::splitter::AggregationStrategy::LastWins,
                parallel: true,
                parallel_limit: None,
                trace_item_threshold: None,
                stop_on_exception: false,
                steps: vec![sql("sql:update t set a=1; COMMIT?datasource=registered_ds")],
            }],
        );
        assert_eq!(
            transaction_error(&step),
            TransactionError::ParallelInBlock {
                step: "split".to_string(),
                datasource: "registered_ds".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_runtime_sql_in_joined_step() {
        // `file:` source twin.
        let file_step = block(
            "registered_ds",
            vec![sql("sql:file:/queries/q.sql?datasource=registered_ds")],
        );
        assert_eq!(
            transaction_error(&file_step),
            TransactionError::RuntimeSqlInJoinedStep {
                step: "sql:file:/queries/q.sql?datasource=registered_ds".to_string(),
            }
        );

        // Header/body-provided statement twin (`allowDynamicQuery` opt-in).
        let dynamic_step = block(
            "registered_ds",
            vec![sql(
                "sql:select 1?datasource=registered_ds&allowDynamicQuery=true",
            )],
        );
        assert_eq!(
            transaction_error(&dynamic_step),
            TransactionError::RuntimeSqlInJoinedStep {
                step: "sql:select 1?datasource=registered_ds&allowDynamicQuery=true".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_runtime_sql_in_joined_step_nested() {
        for container in nested_containers(sql("sql:file:/queries/q.sql?datasource=registered_ds"))
        {
            let nested = block("registered_ds", vec![container]);
            assert_eq!(
                transaction_error(&nested),
                TransactionError::RuntimeSqlInJoinedStep {
                    step: "sql:file:/queries/q.sql?datasource=registered_ds".to_string(),
                }
            );
        }
    }

    #[test]
    fn walk_rejects_dynamic_targets_in_block() {
        let step = block(
            "registered_ds",
            vec![
                sql("sql:select 1?datasource=registered_ds"),
                BuilderStep::DeclarativeRecipientList {
                    expression: predicate(),
                    delimiter: ",".to_string(),
                    parallel: false,
                    parallel_limit: None,
                    stop_on_exception: false,
                    aggregation: "last_wins".to_string(),
                },
            ],
        );
        assert_eq!(
            transaction_error(&step),
            TransactionError::DynamicTargetUnprovable {
                step: "recipient-list".to_string(),
            }
        );

        let slip = block(
            "registered_ds",
            vec![BuilderStep::DeclarativeRoutingSlip {
                expression: predicate(),
                uri_delimiter: ",".to_string(),
                cache_size: 0,
                ignore_invalid_endpoints: false,
            }],
        );
        assert_eq!(
            transaction_error(&slip),
            TransactionError::DynamicTargetUnprovable {
                step: "routing-slip".to_string(),
            }
        );

        let router = block(
            "registered_ds",
            vec![BuilderStep::DeclarativeDynamicRouter {
                expression: predicate(),
                uri_delimiter: ",".to_string(),
                cache_size: 0,
                ignore_invalid_endpoints: false,
                max_iterations: 1,
            }],
        );
        assert_eq!(
            transaction_error(&router),
            TransactionError::DynamicTargetUnprovable {
                step: "dynamic-router".to_string(),
            }
        );
    }

    /// Pins the decision: a dynamic target inside a transacted block is
    /// rejected at load, never accepted under a membership proof. The
    /// routing expression is evaluated at exchange time, so block membership
    /// is not statically provable; the canonical `CanonicalStepSpec` cannot
    /// author a dynamic target at all. It duplicates
    /// `walk_rejects_dynamic_targets_in_block` on purpose: mission 617 names
    /// this test, and the canonical entry cannot build its input. So this
    /// test exercises the SAME `walk_transacted_block` entry and
    /// registry-backed resolver shape the canonical `Transacted` lowering in
    /// `lifecycle/application/commands.rs` uses, which calls
    /// `walk_transacted_block` at block depth 0.
    #[test]
    fn canonical_block_dynamic_uri_is_rejected_at_load() {
        let step = block(
            "registered_ds",
            vec![
                sql("sql:select 1?datasource=registered_ds"),
                BuilderStep::DeclarativeRecipientList {
                    expression: predicate(),
                    delimiter: ",".into(),
                    parallel: false,
                    parallel_limit: None,
                    stop_on_exception: false,
                    aggregation: "last_wins".into(),
                },
            ],
        );
        assert_eq!(
            transaction_error(&step),
            TransactionError::DynamicTargetUnprovable {
                step: "recipient-list".to_string(),
            }
        );

        let slip = block(
            "registered_ds",
            vec![BuilderStep::DeclarativeRoutingSlip {
                expression: predicate(),
                uri_delimiter: ",".into(),
                cache_size: 0,
                ignore_invalid_endpoints: false,
            }],
        );
        assert_eq!(
            transaction_error(&slip),
            TransactionError::DynamicTargetUnprovable {
                step: "routing-slip".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_dynamic_target_nested_in_split() {
        let recipient_source: std::sync::Arc<dyn Fn(&camel_api::Exchange) -> String + Send + Sync> =
            std::sync::Arc::new(|_exchange| String::new());
        let step = block(
            "registered_ds",
            vec![BuilderStep::DeclarativeSplit {
                expression: predicate(),
                aggregation: camel_api::splitter::AggregationStrategy::LastWins,
                parallel: false,
                parallel_limit: None,
                trace_item_threshold: None,
                stop_on_exception: false,
                steps: vec![BuilderStep::RecipientList {
                    config: camel_api::recipient_list::RecipientListConfig::new(recipient_source),
                }],
            }],
        );
        assert_eq!(
            transaction_error(&step),
            TransactionError::DynamicTargetUnprovable {
                step: "recipient-list".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_unregistered_datasource_before_dialect() {
        // Precedence: manager resolution runs BEFORE the dialect stage, so an
        // absent manager names the datasource even when no URL exists either.
        let step = block(
            "unregistered",
            vec![sql("sql:select 1?datasource=unregistered")],
        );
        let error = match walk_transacted_block(&step, &|_datasource| None, &|_datasource| None) {
            Err(CamelError::Transaction(error)) => error,
            other => panic!("expected ManagerMissing, got {other:?}"),
        };
        assert_eq!(
            error,
            TransactionError::ManagerMissing {
                datasource: "unregistered".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_per_arm_before_manager_resolution() {
        // Precedence: per-arm checks run before manager resolution, so a
        // control-SQL violation is typed even when no manager is registered.
        let step = block(
            "unregistered",
            vec![sql("sql:commit?datasource=unregistered")],
        );
        let error = match walk_transacted_block(&step, &|_datasource| None, &|_datasource| None) {
            Err(CamelError::Transaction(error)) => error,
            other => panic!("expected TransactionControlSql, got {other:?}"),
        };
        assert!(matches!(
            error,
            TransactionError::TransactionControlSql { .. }
        ));
    }

    #[test]
    fn walk_rejects_nested_block_structurally() {
        let step = block(
            "registered_ds",
            vec![block("inner", vec![sql("sql:select 1?datasource=inner")])],
        );
        assert_eq!(
            transaction_error(&step),
            TransactionError::NestedBlock {
                inner: "inner".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_parallel_split_over_joined_sql() {
        let step = block(
            "registered_ds",
            vec![BuilderStep::DeclarativeSplit {
                expression: predicate(),
                aggregation: camel_api::splitter::AggregationStrategy::LastWins,
                parallel: true,
                parallel_limit: None,
                trace_item_threshold: None,
                stop_on_exception: false,
                steps: vec![sql("sql:insert into t values (1)?datasource=registered_ds")],
            }],
        );
        assert_eq!(
            transaction_error(&step),
            TransactionError::ParallelInBlock {
                step: "split".to_string(),
                datasource: "registered_ds".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_multicast_and_wiretap_over_joined_sql() {
        let multicast = block(
            "registered_ds",
            vec![BuilderStep::Multicast {
                steps: vec![sql("sql:insert into t values (1)?datasource=registered_ds")],
                config: camel_api::multicast::MulticastConfig::default(),
            }],
        );
        assert_eq!(
            transaction_error(&multicast),
            TransactionError::MulticastInBlock {
                step: "multicast".to_string(),
                datasource: "registered_ds".to_string(),
            }
        );

        let wiretap = block(
            "registered_ds",
            vec![BuilderStep::WireTap {
                uri: "sql:insert into audit values (1)?datasource=registered_ds".to_string(),
            }],
        );
        assert_eq!(
            transaction_error(&wiretap),
            TransactionError::WireTapInBlock {
                step: "sql:insert into audit values (1)?datasource=registered_ds".to_string(),
                datasource: "registered_ds".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_async_and_seda_handoffs_over_joined_sql() {
        let async_step = block(
            "registered_ds",
            vec![
                sql("sql:select 1?datasource=registered_ds"),
                sql("kafka:orders"),
            ],
        );
        assert_eq!(
            transaction_error(&async_step),
            TransactionError::AsyncInBlock {
                step: "kafka:orders".to_string(),
                datasource: "registered_ds".to_string(),
            }
        );

        let seda = block(
            "registered_ds",
            vec![
                sql("sql:select 1?datasource=registered_ds"),
                sql("seda:orders"),
            ],
        );
        assert_eq!(
            transaction_error(&seda),
            TransactionError::SedaInBlock {
                step: "seda:orders".to_string(),
                datasource: "registered_ds".to_string(),
            }
        );
    }

    #[test]
    fn walk_rejects_stream_list_foreign_and_unnamed_datasource() {
        let stream_list = block(
            "registered_ds",
            vec![sql(
                "sql:select 1?datasource=registered_ds&outputType=StreamList",
            )],
        );
        assert_eq!(
            transaction_error(&stream_list),
            TransactionError::StreamListInBlock {
                step: "sql:select 1?datasource=registered_ds&outputType=StreamList".to_string(),
                datasource: "registered_ds".to_string(),
            }
        );

        let foreign = block("registered_ds", vec![sql("sql:select 1?datasource=audit")]);
        assert_eq!(
            transaction_error(&foreign),
            TransactionError::ForeignDatasource {
                step: "sql:select 1?datasource=audit".to_string(),
                block_datasource: "registered_ds".to_string(),
                step_datasource: "audit".to_string(),
            }
        );

        let unnamed = block("registered_ds", vec![sql("sql:select 1")]);
        assert_eq!(
            transaction_error(&unnamed),
            TransactionError::UnnamedDatasource {
                step: "sql:select 1".to_string(),
            }
        );
    }

    #[test]
    fn walk_ignores_non_transacted_input() {
        assert!(
            walk_transacted_block_deferred(&sql("sql:select 1?datasource=x"), &|_datasource| None)
                .is_ok()
        );
    }

    /// Frozen walk precedence (per-arm BEFORE manager, one visitor): the
    /// deferred entry with an ABSENT resolver reports `ManagerMissing` for a
    /// clean block — the walk tail — while transaction-control text and a
    /// percent-encoded runtime SQL source (the decoded-path scan) reject at
    /// per-arm BEFORE the manager is ever consulted.
    #[test]
    fn deferred_walk_per_arm_precedes_manager_resolution() {
        let absent = &|_datasource: &str| -> Option<Arc<dyn TransactionManager>> { None };
        let clean = block("pg", vec![sql("sql:select 1?datasource=pg")]);
        assert_eq!(
            transaction_error(&clean),
            TransactionError::ManagerMissing {
                datasource: "pg".to_string(),
            },
            "clean block + absent resolver: ManagerMissing from the walk tail"
        );

        let commit_text = block(
            "pg",
            vec![sql("sql:update t set a=1; COMMIT?datasource=pg")],
        );
        assert!(
            matches!(
                walk_transacted_block_deferred(&commit_text, absent),
                Err(CamelError::Transaction(
                    TransactionError::TransactionControlSql { .. }
                ))
            ),
            "transaction-control text rejects before the manager is consulted"
        );

        // Percent-encoded `file%3A` DECODES to a runtime source in the
        // component (`parse_uri` percent-decodes the path) — the walk's
        // decoded-path scan must see it too.
        let encoded_file = block("pg", vec![sql("sql:file%3A/queries/q.sql?datasource=pg")]);
        assert!(
            matches!(
                walk_transacted_block_deferred(&encoded_file, absent),
                Err(CamelError::Transaction(
                    TransactionError::RuntimeSqlInJoinedStep { .. }
                ))
            ),
            "a percent-encoded file: source rejects before the manager is consulted"
        );
    }
}
