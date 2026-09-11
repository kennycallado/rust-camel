use std::fmt;

/// Validation and parse errors for test documents.
#[derive(Debug)]
pub enum TestDocError {
    /// Malformed YAML or a type mismatch at the serde layer.
    Yaml(String),
    /// `deny_unknown_fields` rejection.
    UnknownField(String),
    /// The declared route source keys (`routeFiles`, `routeFilesFromRoot`,
    /// `routes`) number zero or more than one; `present` lists the declared
    /// keys and is empty when none is declared.
    RouteSourceConflict { present: Vec<&'static str> },
    /// `routeFilesFromRoot` is declared but no `Camel.toml` exists in any
    /// ancestor directory of the document. Raised during runner route
    /// resolution, not during parsing.
    NoProjectRoot { doc_dir: String },
    /// `expects` is missing or empty.
    ExpectsEmpty,
    /// An `expects` key lacks the required `mock:` scheme.
    ExpectKeyMissingScheme { key: String },
    /// `sequence:` holds fewer than two entries.
    SequenceTooShort,
    /// A `sequence:` entry lacks the `mock:` scheme or names an empty
    /// endpoint path.
    SequenceBadRef { entry: String },
    /// One `expects` entry sets both `count` and `minCount`.
    CountAndMinCount(String),
    /// One `expects` entry sets both `count` and `maxCount`.
    CountAndMaxCount(String),
    /// One `expects` entry sets a `minCount` above its `maxCount` (an
    /// empty range).
    MinCountAboveMaxCount {
        /// Bare endpoint name of the entry.
        endpoint: String,
        /// Declared `minCount`.
        min: usize,
        /// Declared `maxCount`.
        max: usize,
    },
    /// `settle` failed to parse or falls outside `0 < settle <= 5s`.
    SettleOutOfRange(String),
    /// An input `to` target lacks the required `direct:` scheme.
    UnsupportedInputScheme { target: String },
    /// A body scalar (null/boolean/number) is not a supported body form.
    UnsupportedBodyScalar(String),
    /// Intercept source URI is empty.
    InterceptEmptySource,
    /// Intercept source uses the `mock:` scheme.
    InterceptMockSource { key: String },
    /// Intercept action has both or neither keys (`skipTo` / `divertCopyTo`).
    InterceptActionKeys { key: String, problem: &'static str },
    /// Intercept target is `mock:` with an empty endpoint path.
    InterceptEmptyTargetPath { key: String },
    /// Intercept target failed Stage A validation (e.g. non-`mock:` target).
    InterceptInvalid(String),
    /// A `beans:` declaration failed validation; the message carries the
    /// precise reason verbatim.
    InvalidBeans(String),
    /// A `repositories:` declaration failed validation; the message carries
    /// the precise reason verbatim.
    InvalidRepositories(String),
    /// An `expectReply` block failed validation; the message carries the
    /// precise reason verbatim.
    InvalidReply(String),
    /// A matcher entry failed grammar validation; the message carries the
    /// precise reason verbatim.
    InvalidMatcher(String),
    /// A `${env:NAME}` placeholder in an identifier field resolved to
    /// nothing (default-only lookup, ambient env never consulted).
    EnvUnresolved { var: String, field: String },
}

impl fmt::Display for TestDocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Yaml(raw) => write!(f, "invalid test document: {raw}"),
            Self::UnknownField(raw) => write!(f, "unknown field in test document: {raw}"),
            Self::RouteSourceConflict { present } => {
                if present.is_empty() {
                    write!(
                        f,
                        "exactly one route source (`routeFiles`, `routeFilesFromRoot`, \
                         or `routes`) is required"
                    )
                } else {
                    write!(
                        f,
                        "route sources {} are mutually exclusive; exactly one route \
                         source is required",
                        present
                            .iter()
                            .map(|key| format!("`{key}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
            Self::NoProjectRoot { doc_dir } => write!(
                f,
                "NoProjectRoot: routeFilesFromRoot requires a Camel.toml in an ancestor \
                 directory of {doc_dir}; none was found."
            ),
            Self::ExpectsEmpty => write!(
                f,
                "expects must declare at least one mock: endpoint unless an input declares expectReply"
            ),
            Self::ExpectKeyMissingScheme { key } => {
                write!(f, "expects key `{key}` must start with `mock:`")
            }
            Self::SequenceTooShort => {
                write!(f, "sequence needs at least two entries")
            }
            Self::SequenceBadRef { entry } => {
                write!(
                    f,
                    "sequence entry `{entry}` must be a mock: URI with a non-empty endpoint path"
                )
            }
            Self::CountAndMinCount(endpoint) => write!(
                f,
                "expects entry `{endpoint}` must not set both count and minCount"
            ),
            Self::CountAndMaxCount(endpoint) => write!(
                f,
                "expects entry `{endpoint}` must not set both count and maxCount"
            ),
            Self::MinCountAboveMaxCount { endpoint, min, max } => write!(
                f,
                "expects entry `{endpoint}` must not set minCount {min} above maxCount {max}"
            ),
            Self::SettleOutOfRange(raw) => {
                write!(
                    f,
                    "settle `{raw}` out of range: must satisfy 0 < settle <= 5s"
                )
            }
            Self::UnsupportedInputScheme { target } => {
                write!(f, "input target `{target}` must start with `direct:`")
            }
            Self::UnsupportedBodyScalar(raw) => write!(
                f,
                "unsupported body scalar `{raw}`: only string, object, and array bodies are supported"
            ),
            Self::InterceptEmptySource => write!(f, "intercept source URI must not be empty"),
            Self::InterceptMockSource { key } => {
                write!(f, "intercept source `{key}` must not start with `mock:`")
            }
            Self::InterceptActionKeys { key, problem } => write!(
                f,
                "intercept action for `{key}`: exactly one of `skipTo` or `divertCopyTo` is required (got {problem})"
            ),
            Self::InterceptEmptyTargetPath { key } => write!(
                f,
                "intercept target for `{key}` needs a mock endpoint name: `mock:` requires a non-empty endpoint path"
            ),
            Self::InterceptInvalid(msg) => write!(f, "invalid intercept: {msg}"),
            Self::InvalidBeans(msg) => write!(f, "{msg}"),
            Self::InvalidRepositories(msg) => write!(f, "{msg}"),
            Self::InvalidReply(msg) => write!(f, "{msg}"),
            Self::InvalidMatcher(msg) => write!(f, "{msg}"),
            Self::EnvUnresolved { var, field } => {
                write!(
                    f,
                    "Environment variable '{var}' not set (required by {field})"
                )
            }
        }
    }
}

impl std::error::Error for TestDocError {}
