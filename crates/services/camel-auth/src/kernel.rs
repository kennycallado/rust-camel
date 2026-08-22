//! Authentication kernel: the sealed principal, minting, and the dispatch guard.
//!
//! This module is the trust boundary between transport-layer token extraction
//! and route authorization. [`AuthenticatedPrincipal`] is a sealed type: it is
//! nameable (so `Exchange::get_extension::<AuthenticatedPrincipal>` downcasts
//! across crates) but not constructible outside this crate — all fields are
//! private and there is no public constructor. A principal can therefore only
//! come from [`kernel_authenticate`], which mints it after a real token passes
//! a registered provider's authenticator.

use std::sync::Arc;

use camel_api::security_policy::{AccessMode, AuthPrincipal, Principal, RouteSecurityPlan};
use camel_api::{CamelError, Exchange};

use crate::authn_cache::AuthnCacheKey;
use crate::credential_source::ExtractedToken;
use crate::registry::ProviderRegistry;
use crate::token_authenticator::AuthnRequest;

/// The typed, unforgeable authentication identity stored on an [`Exchange`].
///
/// # Sealing
///
/// This type is intentionally sealed: the fields are private and there is no
/// public constructor (no `__mint`, no doc-hidden escape, no feature-gated
/// test constructor — Cargo feature unification would make those unsound). The
/// only sound seal is same-crate construction, so the only way any code —
/// including transports and authorization policies — can obtain an
/// `AuthenticatedPrincipal` is through [`kernel_authenticate`], which verifies
/// a credential against a registered provider first. This guards against
/// accidental construction and, combined with [`enforce_dispatch`]'s
/// route-binding, against cross-provider principal spoofing. A hostile actor
/// editing this crate itself is the only remaining spoofing vector, and that
/// is a review-level concern, not a type-system one.
#[derive(Clone)]
pub struct AuthenticatedPrincipal {
    principal: Principal,
    provider_id: String,
}

impl AuthenticatedPrincipal {
    /// Private mint path. The only caller is [`kernel_authenticate`], after a
    /// credential has passed a provider's authenticator.
    fn mint(principal: Principal, provider_id: String) -> Self {
        Self {
            principal,
            provider_id,
        }
    }
}

impl AuthPrincipal for AuthenticatedPrincipal {
    fn principal(&self) -> &Principal {
        &self.principal
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }
}

/// Exchange extension key under which [`install_carrier`] stores the
/// authenticated principal.
///
/// Values stored under this key are unforgeable: producing one requires an
/// [`AuthenticatedPrincipal`], which no external code can construct. A
/// wrong-type value fails the `get_extension::<AuthenticatedPrincipal>`
/// downcast and is treated as absent.
pub const KERNEL_PRINCIPAL_KEY: &str = "camel.auth.principal.typed";

/// Resolve the route's provider and authenticate the extracted token.
///
/// Returns an [`AuthenticatedPrincipal`] minted by the provider named in
/// `plan.provider_ref`. An unresolved provider is an [`CamelError::Unauthenticated`]
/// whose message names the missing provider (fail-closed).
pub async fn kernel_authenticate(
    plan: &RouteSecurityPlan,
    providers: &ProviderRegistry,
    credentials: &ExtractedToken,
) -> Result<AuthenticatedPrincipal, CamelError> {
    let provider_ref = plan.provider_ref.as_deref().ok_or_else(|| {
        CamelError::Unauthenticated("route has no provider_ref; cannot authenticate".to_string())
    })?;

    let entry = providers.resolve(provider_ref).ok_or_else(|| {
        CamelError::Unauthenticated(format!("unknown auth provider: {provider_ref}"))
    })?;

    // Build the per-request authn context from the plan's audience binding
    // (route-level precedence already merged in Task 1.8), falling back to the
    // resolved provider's binding when the plan carries none.
    let binding = plan
        .audience_binding
        .as_ref()
        .or(entry.audience_binding.as_ref());
    let audiences: &[String] = binding.map(|b| b.audiences.as_slice()).unwrap_or(&[]);
    let issuers: &[String] = binding.map(|b| b.issuers.as_slice()).unwrap_or(&[]);

    // Task 3.2: consult the authn result cache before hitting the provider. A
    // hit returns the cached minted principal; denials are never cached.
    if let Some(cache) = providers.authn_cache() {
        let key = AuthnCacheKey::new(
            provider_ref,
            audiences,
            issuers,
            plan.transport,
            &credentials.token,
        );
        if let Some(principal) = cache.get(&key) {
            tracing::debug!(target: "camel_auth::authn_cache", cache_outcome = "hit");
            return Ok(principal);
        }
    }

    let req = AuthnRequest {
        token: &credentials.token,
        audiences,
        accepted_issuers: issuers,
        transport: plan.transport,
    };

    let principal = entry.authenticator.authenticate(req).await?;

    let minted = AuthenticatedPrincipal::mint(principal, provider_ref.to_string());

    // Cache the minted principal (denials returned Err above and are never
    // inserted). The entry never outlives the token's exp.
    if let Some(cache) = providers.authn_cache() {
        let key = AuthnCacheKey::new(
            provider_ref,
            audiences,
            issuers,
            plan.transport,
            &credentials.token,
        );
        cache.insert(key, minted.clone());
    }

    Ok(minted)
}

/// Guard a dispatch against the route's security plan.
///
/// A [`AccessMode::Public`] route passes through with no extraction and no
/// carrier requirement. Any non-Public route requires the carrier to be present
/// AND the carrier's `provider_id()` to equal `plan.provider_ref` — a principal
/// minted for provider A does not satisfy provider B's route (route-bound,
/// cross-provider replay denied). Otherwise the guard fails closed with
/// [`CamelError::Unauthenticated`].
pub fn enforce_dispatch(plan: &RouteSecurityPlan, exchange: &Exchange) -> Result<(), CamelError> {
    if matches!(&plan.access_mode, AccessMode::Public) {
        return Ok(());
    }

    let carrier = read_carrier(exchange).ok_or_else(|| {
        CamelError::Unauthenticated("no authenticated principal present".to_string())
    })?;

    if plan.provider_ref.as_deref() == Some(carrier.provider_id()) {
        Ok(())
    } else {
        Err(CamelError::Unauthenticated(format!(
            "principal from provider {:?} does not satisfy route provider {:?}",
            carrier.provider_id(),
            plan.provider_ref
        )))
    }
}

/// Install the authenticated principal as the exchange's typed carrier.
///
/// Stores an `Arc`'d clone under [`KERNEL_PRINCIPAL_KEY`] via
/// `Exchange::set_extension`.
pub fn install_carrier(exchange: &mut Exchange, principal: &AuthenticatedPrincipal) {
    exchange.set_extension(KERNEL_PRINCIPAL_KEY, Arc::new(principal.clone()));
}

/// Read the typed carrier back off the exchange, cloning it out.
///
/// Returns `None` when the key is absent or the stored value is not an
/// [`AuthenticatedPrincipal`] (wrong-type values fail the downcast).
pub fn read_carrier(exchange: &Exchange) -> Option<AuthenticatedPrincipal> {
    exchange
        .get_extension::<AuthenticatedPrincipal>(KERNEL_PRINCIPAL_KEY)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_source::CredentialSource;
    use crate::native_auth::{NativeCredential, NativeCredentialSecret, StaticTokenAuthenticator};
    use crate::registry::ProviderEntry;
    use camel_api::Message;
    use camel_api::security_policy::TransportId;
    use zeroize::Zeroizing;

    fn test_principal() -> Principal {
        Principal {
            subject: "svc-user".into(),
            issuer: "test".into(),
            audience: vec![],
            scopes: vec![],
            roles: vec![],
            claims: serde_json::Value::Null,
        }
    }

    /// A registry holding a single static provider whose token is `token`.
    fn static_provider(id: &str, token: &str) -> ProviderRegistry {
        let store = crate::native_auth::NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new(token.to_string()),
            },
            principal: test_principal(),
        }])
        .unwrap();
        let registry = ProviderRegistry::new();
        registry.register(
            id,
            ProviderEntry {
                authenticator: Arc::new(StaticTokenAuthenticator::new(store)),
                audience_binding: None,
            },
        );
        registry
    }

    fn authenticated_plan(provider_ref: &str) -> RouteSecurityPlan {
        RouteSecurityPlan {
            access_mode: AccessMode::Authenticated,
            provider_ref: Some(provider_ref.to_string()),
            transport: TransportId::Http,
            credential_sources: vec![CredentialSource::AuthorizationHeader],
            audience_binding: None,
        }
    }

    fn public_plan() -> RouteSecurityPlan {
        RouteSecurityPlan {
            access_mode: AccessMode::Public,
            provider_ref: None,
            transport: TransportId::Http,
            credential_sources: vec![],
            audience_binding: None,
        }
    }

    fn credentials(token: &str) -> ExtractedToken {
        ExtractedToken {
            token: token.to_string(),
            source: CredentialSource::AuthorizationHeader,
        }
    }

    fn empty_exchange() -> Exchange {
        Exchange::new(Message::default())
    }

    #[tokio::test]
    async fn kernel_authenticate_mints_with_provider() {
        let providers = static_provider("idp-a", "t-a");
        let plan = authenticated_plan("idp-a");
        let principal = kernel_authenticate(&plan, &providers, &credentials("t-a"))
            .await
            .unwrap();
        assert_eq!(principal.provider_id(), "idp-a");
        assert_eq!(principal.principal().subject, "svc-user");
    }

    #[tokio::test]
    async fn kernel_authenticate_denies_wrong_token() {
        let providers = static_provider("idp-a", "t-a");
        let plan = authenticated_plan("idp-a");
        let result = kernel_authenticate(&plan, &providers, &credentials("wrong")).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[tokio::test]
    async fn kernel_authenticate_names_unresolved_provider() {
        let providers = static_provider("idp-a", "t-a");
        let plan = authenticated_plan("idp-ghost");
        match kernel_authenticate(&plan, &providers, &credentials("t-a")).await {
            Ok(_) => panic!("expected Unauthenticated"),
            Err(CamelError::Unauthenticated(msg)) => assert!(msg.contains("idp-ghost")),
            Err(other) => panic!("expected Unauthenticated, got: {other}"),
        }
    }

    #[test]
    fn enforce_dispatch_public_passes_without_carrier() {
        let plan = public_plan();
        let exchange = empty_exchange();
        assert!(enforce_dispatch(&plan, &exchange).is_ok());
    }

    #[tokio::test]
    async fn enforce_dispatch_nonpublic_requires_carrier() {
        let providers = static_provider("idp-a", "t-a");
        let plan = authenticated_plan("idp-a");
        let mut exchange = empty_exchange();

        // No carrier yet: fail closed.
        assert!(matches!(
            enforce_dispatch(&plan, &exchange),
            Err(CamelError::Unauthenticated(_))
        ));

        let principal = kernel_authenticate(&plan, &providers, &credentials("t-a"))
            .await
            .unwrap();
        install_carrier(&mut exchange, &principal);

        assert!(enforce_dispatch(&plan, &exchange).is_ok());
    }

    #[tokio::test]
    async fn enforce_dispatch_rejects_cross_provider_carrier() {
        let providers = static_provider("idp-a", "t-a");
        let mint_plan = authenticated_plan("idp-a");
        let principal = kernel_authenticate(&mint_plan, &providers, &credentials("t-a"))
            .await
            .unwrap();

        let mut exchange = empty_exchange();
        install_carrier(&mut exchange, &principal);

        let target_plan = authenticated_plan("idp-b");
        assert!(matches!(
            enforce_dispatch(&target_plan, &exchange),
            Err(CamelError::Unauthenticated(_))
        ));
    }

    #[test]
    fn read_carrier_returns_none_when_absent() {
        let exchange = empty_exchange();
        assert!(read_carrier(&exchange).is_none());
    }

    #[tokio::test]
    async fn enforce_dispatch_fails_closed_when_provider_ref_none() {
        // Missing wiring yields deny, not bypass: a non-Public plan compiled
        // without a provider_ref can never be satisfied by any carrier.
        let providers = static_provider("idp-a", "t-a");
        let mint_plan = authenticated_plan("idp-a");
        let principal = kernel_authenticate(&mint_plan, &providers, &credentials("t-a"))
            .await
            .unwrap();

        let mut exchange = empty_exchange();
        install_carrier(&mut exchange, &principal);

        let mut unwired_plan = authenticated_plan("idp-a");
        unwired_plan.provider_ref = None;
        assert!(matches!(
            enforce_dispatch(&unwired_plan, &exchange),
            Err(CamelError::Unauthenticated(_))
        ));
    }

    #[test]
    fn wrong_type_value_under_carrier_key_does_not_authorize() {
        // Spoof resistance: a forged marker stored under KERNEL_PRINCIPAL_KEY
        // fails the downcast and is treated as absent.
        let mut exchange = empty_exchange();
        exchange.set_extension(
            KERNEL_PRINCIPAL_KEY,
            std::sync::Arc::new("forged".to_string()),
        );

        assert!(read_carrier(&exchange).is_none());
        let plan = authenticated_plan("idp-a");
        assert!(matches!(
            enforce_dispatch(&plan, &exchange),
            Err(CamelError::Unauthenticated(_))
        ));
    }
}
