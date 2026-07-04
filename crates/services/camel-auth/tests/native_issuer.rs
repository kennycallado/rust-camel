use camel_auth::claims::ClaimPaths;
use camel_auth::claims::JsonPointerClaimsMapper;
use camel_auth::jwt::JwtValidator;
use camel_auth::native_client_store::{M2mClient, M2mClientSecret, M2mClientStore};
use camel_auth::native_issuer::NativeSigningKey;
use camel_auth::native_issuer::NativeTokenIssuer;
use camel_auth::native_jwks::NativeJwksProvider;
use camel_auth::token_authenticator::TokenAuthenticator;
use std::sync::Arc;
use zeroize::Zeroizing;

fn setup() -> (NativeTokenIssuer, Arc<NativeJwksProvider>) {
    let pem = include_str!("fixtures/test_rsa_private.pem");
    let signing_key_for_jwks =
        NativeSigningKey::from_pem(pem, "integration-kid".to_string()).unwrap();
    let jwks = Arc::new(NativeJwksProvider::new(signing_key_for_jwks).unwrap());

    let signing_key_for_issuer =
        NativeSigningKey::from_pem(pem, "integration-kid".to_string()).unwrap();
    let store = M2mClientStore::try_new(vec![M2mClient {
        client_id: "order-worker".into(),
        secret: M2mClientSecret::Plaintext {
            value: Zeroizing::new("worker-secret".into()),
        },
        roles: vec!["orders".into()],
        scopes: vec!["orders:read".into(), "orders:write".into()],
    }])
    .unwrap();

    let issuer = NativeTokenIssuer::try_new(
        "https://orders.local".into(),
        vec!["orders-api".into()],
        std::time::Duration::from_secs(300),
        signing_key_for_issuer,
        store,
    )
    .unwrap();

    (issuer, jwks)
}

#[tokio::test]
async fn full_round_trip_issue_and_validate() {
    let (issuer, jwks) = setup();

    let token_resp = issuer
        .issue_token("order-worker", "worker-secret", None, None)
        .await
        .unwrap();

    let claim_paths = ClaimPaths {
        subject: "/sub".to_string(),
        roles: vec!["/roles".to_string()],
        scopes: Some("/scope".to_string()),
    };
    let mapper = Arc::new(JsonPointerClaimsMapper::new(claim_paths));
    let validator = camel_auth::LocalJwtValidator::new(
        vec!["orders-api".to_string()],
        "https://orders.local".to_string(),
        jwks as Arc<dyn camel_auth::JwksProvider>,
        mapper,
    );

    let principal = validator.validate(&token_resp.access_token).await.unwrap();
    assert_eq!(principal.subject, "order-worker");
    assert_eq!(principal.roles, vec!["orders"]);
    assert!(principal.has_scope("orders:read"));
}

#[tokio::test]
async fn round_trip_via_token_authenticator() {
    let (issuer, jwks) = setup();

    let token_resp = issuer
        .handle_token_request(
            "grant_type=client_credentials&client_id=order-worker&client_secret=worker-secret&scope=orders%3Aread",
        )
        .await
        .unwrap();

    let claim_paths = ClaimPaths {
        subject: "/sub".to_string(),
        roles: vec!["/roles".to_string()],
        scopes: Some("/scope".to_string()),
    };
    let mapper = Arc::new(JsonPointerClaimsMapper::new(claim_paths));
    let validator = camel_auth::LocalJwtValidator::new(
        vec!["orders-api".to_string()],
        "https://orders.local".to_string(),
        jwks as Arc<dyn camel_auth::JwksProvider>,
        mapper,
    );

    let principal = validator
        .authenticate_bearer(&token_resp.access_token)
        .await
        .unwrap();
    assert_eq!(principal.subject, "order-worker");
    assert!(principal.has_scope("orders:read"));
    assert!(!principal.has_scope("orders:write"));
}
