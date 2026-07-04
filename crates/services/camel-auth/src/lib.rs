//! Authentication and authorization primitives for rust-camel.
//!
//! Provider-neutral OIDC auth service. Configurable claim mapping via [`ClaimsMapper`]
//! enables any OIDC-compliant provider. Provider-specific presets live in their
//! respective component crates (e.g. `camel-component-keycloak`).
//!
//! Core types (`SecurityPolicy`, `AuthorizationDecision`, `Principal`)
//! live in `camel-api` so `camel-core` and `camel-dsl` can reference
//! them without depending on this crate.

pub mod bearer;
pub mod bearer_token_layer;
pub mod built_in;
pub mod claims;
pub mod credential_source;
pub mod http_client;
pub mod introspection;
pub mod introspection_auth;
pub mod jwks;
pub mod jwt;
pub mod native_auth;
pub mod native_client_store;
pub mod native_issuer;
pub mod native_jwks;
pub mod oauth2;
pub mod permission;
pub mod permission_cache;
pub mod permission_policy;
pub mod registry;
pub mod token_authenticator;
pub mod types;

pub use bearer::extract_bearer_token;
pub use bearer_token_layer::{BearerTokenLayer, BearerTokenService};
pub use built_in::{RolePolicy, ScopePolicy};
pub use claims::{ClaimPaths, ClaimsMapper, JsonPointerClaimsMapper};
pub use credential_source::{CredentialSource, extract_token_multi, redact_query_params};
pub use http_client::validate_https_public_uri;
pub use introspection::{
    CachingTokenIntrospector, IntrospectionCacheOptions, IntrospectionResult, TokenIntrospector,
};
pub use introspection_auth::IntrospectionAuthenticator;
pub use jwks::{Jwk, JwksProvider, RemoteJwksProvider};
pub use jwt::{JwtValidator, LocalJwtValidator};
pub use oauth2::{ClientCredentialsProvider, TokenProvider};
pub use registry::PermissionEvaluatorRegistry;
pub use registry::SecurityPolicyRegistry;
pub use token_authenticator::TokenAuthenticator;
pub use types::AuthError;

pub use permission::{
    PermissionContextConfig, PermissionDecision, PermissionEvaluator, PermissionRequest,
    PermissionValueSource,
};

pub use permission_cache::{CachingPermissionEvaluator, PermissionCacheOptions};

pub use permission_policy::PermissionPolicy;

pub use native_auth::{
    ApiKeyAuthenticator, NativeCredential, NativeCredentialSecret, StaticTokenAuthenticator,
};
pub use native_client_store::{M2mClient, M2mClientSecret, M2mClientStore};
pub use native_issuer::{IssuerError, NativeSigningKey, NativeTokenIssuer, TokenResponse};
pub use native_jwks::NativeJwksProvider;

pub use camel_api::security_policy::{
    AuthorizationDecision, PRINCIPAL_KEY, Principal, SecurityPolicy, SecurityPolicyConfig,
};
