//! Authentication and authorization primitives for rust-camel.
//!
//! Provider-neutral OIDC auth service. Configurable claim mapping via [`ClaimsMapper`]
//! enables any OIDC-compliant provider. Provider-specific presets live in their
//! respective component crates (e.g. `camel-component-keycloak`).
//!
//! Core types (`SecurityPolicy`, `AuthorizationDecision`, `Principal`)
//! live in `camel-api` so `camel-core` and `camel-dsl` can reference
//! them without depending on this crate.

pub mod authn_cache;
pub mod bearer;
pub mod bearer_token_layer;
pub mod bind_gate;
pub mod built_in;
pub mod claims;
pub mod credential_source;
pub mod http_client;
pub mod introspection;
pub mod introspection_auth;
pub mod jwks;
pub mod jwt;
pub mod kernel;
pub mod native_auth;
pub mod oauth2;
pub mod permission;
pub mod permission_cache;
pub mod permission_policy;
pub mod registry;
pub mod token_authenticator;
pub mod types;

pub use authn_cache::{AuthnCache, AuthnCacheKey, AuthnCacheOptions};
pub use bearer::extract_bearer_token;
pub use bearer_token_layer::{BearerTokenLayer, BearerTokenService};
pub use bind_gate::{BindExposureAcks, enforce_bind_exposure_gate};
pub use built_in::{RolePolicy, ScopePolicy};
pub use claims::{ClaimPaths, ClaimsMapper, JsonPointerClaimsMapper, escape_json_pointer};
pub use credential_source::{CredentialSource, extract_token_multi, redact_query_params};
pub use http_client::{SsrfClientOptions, validate_uri};
pub use introspection::{
    CachingTokenIntrospector, IntrospectionCacheOptions, IntrospectionResult, TokenIntrospector,
};
pub use introspection_auth::IntrospectionAuthenticator;
pub use jwks::{Jwk, JwksProvider, RemoteJwksProvider};
pub use jwt::{JwtValidator, LocalJwtValidator};
pub use kernel::{
    AuthenticatedPrincipal, KERNEL_PRINCIPAL_KEY, enforce_dispatch, install_carrier,
    kernel_authenticate, read_carrier,
};
pub use oauth2::{ClientCredentialsProvider, TokenProvider};
pub use registry::PermissionEvaluatorRegistry;
pub use registry::ProviderEntry;
pub use registry::ProviderRegistry;
pub use registry::SecurityPolicyRegistry;
pub use token_authenticator::TokenAuthenticator;
pub use types::AuthError;

pub use permission::{
    PermissionContextConfig, PermissionDecision, PermissionEvaluator, PermissionRequest,
    PermissionValueSource,
};

pub use permission_cache::{CachingPermissionEvaluator, PermissionCacheOptions};

pub use permission_policy::PermissionPolicy;

pub use native_auth::{NativeCredential, NativeCredentialSecret, StaticTokenAuthenticator};

pub use camel_api::security_policy::{
    AuthorizationDecision, PRINCIPAL_KEY, Principal, SecurityPolicy, SecurityPolicyConfig,
};
