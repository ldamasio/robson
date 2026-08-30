//! OIDC bearer-token authentication and Robson role authorization.
//!
//! This module deliberately depends only on standard OIDC/JWT inputs. ZITADEL
//! brokers Google and other upstream identity providers; `robsond` trusts the
//! configured RBX issuer, audience, client ids, and organization-scoped roles.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;

use crate::config::OidcConfig;

pub const ROLE_OBSERVER: &str = "rbx:robson:observer";
pub const ROLE_OPERATOR: &str = "rbx:robson:operator";
pub const ROLE_FUNDING: &str = "rbx:robson:funding";
pub const ROLE_EMERGENCY: &str = "rbx:robson:emergency";

const JWKS_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const JWKS_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(30);
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_JWKS_KEYS: usize = 32;
const MAX_JWKS_RESPONSE_BYTES: usize = 256 * 1024;
const CLOCK_SKEW_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredPermission {
    Observer,
    Operator,
    Funding,
    Emergency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    Disabled,
    LegacyToken,
    Oidc,
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub method: AuthMethod,
    pub issuer: String,
    pub subject: String,
    pub client_id: Option<String>,
    pub roles: BTreeSet<String>,
    grants_all: bool,
}

impl AuthContext {
    fn disabled() -> Self {
        Self {
            method: AuthMethod::Disabled,
            issuer: "robsond-development".to_owned(),
            subject: "development-operator".to_owned(),
            client_id: None,
            roles: BTreeSet::new(),
            grants_all: true,
        }
    }

    fn legacy() -> Self {
        Self {
            method: AuthMethod::LegacyToken,
            issuer: "robsond-legacy".to_owned(),
            subject: "legacy-operator".to_owned(),
            client_id: None,
            roles: BTreeSet::new(),
            grants_all: true,
        }
    }

    fn allows(&self, required: RequiredPermission) -> bool {
        if self.grants_all {
            return true;
        }

        match required {
            RequiredPermission::Observer => self.roles.iter().any(|role| {
                matches!(
                    role.as_str(),
                    ROLE_OBSERVER | ROLE_OPERATOR | ROLE_FUNDING | ROLE_EMERGENCY
                )
            }),
            RequiredPermission::Operator => self.roles.contains(ROLE_OPERATOR),
            RequiredPermission::Funding => self.roles.contains(ROLE_FUNDING),
            RequiredPermission::Emergency => self.roles.contains(ROLE_EMERGENCY),
        }
    }

    pub fn permissions(&self) -> Vec<&'static str> {
        [
            RequiredPermission::Observer,
            RequiredPermission::Operator,
            RequiredPermission::Funding,
            RequiredPermission::Emergency,
        ]
        .into_iter()
        .filter(|permission| self.allows(*permission))
        .map(|permission| match permission {
            RequiredPermission::Observer => "observer",
            RequiredPermission::Operator => "operator",
            RequiredPermission::Funding => "funding",
            RequiredPermission::Emergency => "emergency",
        })
        .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing or invalid Authorization header")]
    MissingBearer,
    #[error("bearer token is invalid")]
    InvalidToken,
    #[error("authenticated identity lacks the required Robson role")]
    Forbidden,
    #[error("identity provider is temporarily unavailable")]
    IdentityUnavailable,
}

#[derive(Clone)]
pub struct AuthService {
    legacy_token: Option<Arc<str>>,
    oidc: Option<Arc<OidcVerifier>>,
}

impl AuthService {
    pub fn new(legacy_token: Option<String>, oidc: Option<OidcConfig>) -> Self {
        Self {
            legacy_token: legacy_token.map(Arc::<str>::from),
            oidc: oidc.map(|config| Arc::new(OidcVerifier::new(config))),
        }
    }

    pub async fn authenticate(
        &self,
        authorization: Option<&str>,
        required: RequiredPermission,
    ) -> Result<AuthContext, AuthError> {
        if self.legacy_token.is_none() && self.oidc.is_none() {
            return Ok(AuthContext::disabled());
        }

        let token = authorization
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty())
            .ok_or(AuthError::MissingBearer)?;

        if let Some(expected) = &self.legacy_token {
            if expected.as_bytes().ct_eq(token.as_bytes()).into() {
                return Ok(AuthContext::legacy());
            }
        }

        let context = match &self.oidc {
            Some(verifier) => verifier.verify(token).await?,
            None => return Err(AuthError::InvalidToken),
        };

        if context.allows(required) {
            Ok(context)
        } else {
            Err(AuthError::Forbidden)
        }
    }
}

pub struct OidcVerifier {
    issuer: String,
    audience: String,
    jwks_uri: String,
    allowed_client_ids: HashSet<String>,
    allowed_organization_ids: HashSet<String>,
    client: reqwest::Client,
    cache: RwLock<CachedJwks>,
    refresh_lock: Mutex<()>,
}

impl OidcVerifier {
    fn new(config: OidcConfig) -> Self {
        Self {
            issuer: config.issuer,
            audience: config.audience,
            jwks_uri: config.jwks_uri,
            allowed_client_ids: config.allowed_client_ids.into_iter().collect(),
            allowed_organization_ids: config.allowed_organization_ids.into_iter().collect(),
            client: reqwest::Client::builder()
                .timeout(JWKS_FETCH_TIMEOUT)
                .build()
                .expect("static reqwest client configuration must be valid"),
            cache: RwLock::new(CachedJwks::default()),
            refresh_lock: Mutex::new(()),
        }
    }

    async fn verify(&self, bearer_token: &str) -> Result<AuthContext, AuthError> {
        let header = decode_header(bearer_token).map_err(|_| AuthError::InvalidToken)?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::InvalidToken);
        }
        let kid = header.kid.as_deref().ok_or(AuthError::InvalidToken)?;
        let jwk = self.key_for(kid).await?;

        if jwk.kty != "RSA"
            || jwk.alg.as_deref().is_some_and(|algorithm| algorithm != "RS256")
            || jwk.use_.as_deref().is_some_and(|usage| usage != "sig")
        {
            return Err(AuthError::InvalidToken);
        }

        let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
            .map_err(|_| AuthError::InvalidToken)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.leeway = CLOCK_SKEW_SECS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.required_spec_claims =
            ["aud", "exp", "iss", "sub"].into_iter().map(str::to_owned).collect();

        let claims = decode::<JwtClaims>(bearer_token, &decoding_key, &validation)
            .map_err(|_| AuthError::InvalidToken)?
            .claims;

        let client_id = claims.azp.or(claims.client_id);
        if !client_id.as_ref().is_some_and(|value| self.allowed_client_ids.contains(value)) {
            return Err(AuthError::InvalidToken);
        }

        let roles = claims
            .zitadel_project_roles
            .map(ZitadelProjectRolesClaim::into_role_map)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(role, organizations)| {
                organizations
                    .keys()
                    .any(|org_id| self.allowed_organization_ids.contains(org_id))
                    .then_some(role)
            })
            .collect();

        Ok(AuthContext {
            method: AuthMethod::Oidc,
            issuer: claims.iss,
            subject: claims.sub,
            client_id,
            roles,
            grants_all: false,
        })
    }

    async fn key_for(&self, kid: &str) -> Result<Jwk, AuthError> {
        self.refresh_jwks(false).await?;
        if let Some(key) = self.cached_key(kid).await {
            return Ok(key);
        }

        // A missing kid can mean key rotation. This path is rate-limited so an
        // attacker cannot turn arbitrary kid values into one IdP call each.
        self.refresh_jwks(true).await?;
        self.cached_key(kid).await.ok_or(AuthError::InvalidToken)
    }

    async fn cached_key(&self, kid: &str) -> Option<Jwk> {
        self.cache.read().await.keys.iter().find(|key| key.kid == kid).cloned()
    }

    async fn refresh_jwks(&self, force: bool) -> Result<(), AuthError> {
        {
            let cache = self.cache.read().await;
            if !force && cache.is_fresh() {
                return Ok(());
            }
            if force && cache.refresh_was_recent() {
                return Ok(());
            }
            if !force && cache.refresh_was_recent() {
                return Err(AuthError::IdentityUnavailable);
            }
        }

        let _refresh_guard = self.refresh_lock.lock().await;
        {
            let cache = self.cache.read().await;
            if !force && cache.is_fresh() {
                return Ok(());
            }
            if force && cache.refresh_was_recent() {
                return Ok(());
            }
            if !force && cache.refresh_was_recent() {
                return Err(AuthError::IdentityUnavailable);
            }
        }

        self.cache.write().await.last_refresh_attempt = Some(Instant::now());
        let mut response = self
            .client
            .get(&self.jwks_uri)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| {
                warn!(error = %error, "OIDC JWKS refresh failed");
                AuthError::IdentityUnavailable
            })?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            warn!(error = %error, "OIDC JWKS response could not be read");
            AuthError::IdentityUnavailable
        })? {
            if body.len().saturating_add(chunk.len()) > MAX_JWKS_RESPONSE_BYTES {
                warn!("OIDC JWKS response exceeded the size limit");
                return Err(AuthError::IdentityUnavailable);
            }
            body.extend_from_slice(&chunk);
        }
        let jwks = serde_json::from_slice::<JwkSet>(&body).map_err(|error| {
            warn!(error = %error, "OIDC JWKS response was invalid");
            AuthError::IdentityUnavailable
        })?;

        if jwks.keys.is_empty() || jwks.keys.len() > MAX_JWKS_KEYS {
            warn!(key_count = jwks.keys.len(), "OIDC JWKS key count rejected");
            return Err(AuthError::IdentityUnavailable);
        }

        let mut cache = self.cache.write().await;
        cache.keys = jwks.keys;
        cache.fetched_at = Some(Instant::now());
        Ok(())
    }

    #[cfg(test)]
    async fn set_test_jwks(&self, keys: Vec<Jwk>) {
        let mut cache = self.cache.write().await;
        cache.keys = keys;
        cache.fetched_at = Some(Instant::now());
        cache.last_refresh_attempt = Some(Instant::now());
    }
}

#[derive(Default)]
struct CachedJwks {
    keys: Vec<Jwk>,
    fetched_at: Option<Instant>,
    last_refresh_attempt: Option<Instant>,
}

impl CachedJwks {
    fn is_fresh(&self) -> bool {
        !self.keys.is_empty()
            && self.fetched_at.is_some_and(|fetched_at| fetched_at.elapsed() < JWKS_CACHE_TTL)
    }

    fn refresh_was_recent(&self) -> bool {
        self.last_refresh_attempt
            .is_some_and(|attempt| attempt.elapsed() < JWKS_REFRESH_MIN_INTERVAL)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct JwtClaims {
    iss: String,
    sub: String,
    #[allow(dead_code)]
    aud: AudienceClaim,
    #[allow(dead_code)]
    exp: u64,
    #[serde(default)]
    #[allow(dead_code)]
    nbf: Option<u64>,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default, rename = "urn:zitadel:iam:org:project:roles")]
    zitadel_project_roles: Option<ZitadelProjectRolesClaim>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum AudienceClaim {
    Single(String),
    Multiple(Vec<String>),
}

type ZitadelRoleMap = HashMap<String, HashMap<String, String>>;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ZitadelProjectRolesClaim {
    Map(ZitadelRoleMap),
    List(Vec<ZitadelRoleMap>),
}

impl ZitadelProjectRolesClaim {
    fn into_role_map(self) -> ZitadelRoleMap {
        match self {
            Self::Map(roles) => roles,
            Self::List(role_maps) => {
                let mut merged = ZitadelRoleMap::new();
                for role_map in role_maps {
                    for (role, organizations) in role_map {
                        merged.entry(role).or_default().extend(organizations);
                    }
                }
                merged
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kty: String,
    kid: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default, rename = "use")]
    use_: Option<String>,
    n: String,
    e: String,
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::{
        rand::SystemRandom,
        rsa::{KeyPair as RsaKeyPair, KeySize, PublicKeyComponents},
        signature::{KeyPair as _, RSA_PKCS1_SHA256},
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    use super::*;

    struct TestKey {
        private: RsaKeyPair,
        modulus: String,
        exponent: String,
    }

    fn test_key() -> TestKey {
        let private = RsaKeyPair::generate(KeySize::Rsa2048).unwrap();
        let public = PublicKeyComponents::<Vec<u8>>::from(private.public_key());
        TestKey {
            private,
            modulus: URL_SAFE_NO_PAD.encode(public.n),
            exponent: URL_SAFE_NO_PAD.encode(public.e),
        }
    }

    #[derive(Serialize)]
    struct TestClaims {
        iss: &'static str,
        sub: &'static str,
        aud: &'static str,
        exp: u64,
        azp: &'static str,
        #[serde(rename = "urn:zitadel:iam:org:project:roles")]
        roles: serde_json::Value,
    }

    fn test_config() -> OidcConfig {
        OidcConfig {
            issuer: "https://identity.example".to_owned(),
            audience: "robson-api".to_owned(),
            jwks_uri: "https://identity.example/oauth/v2/keys".to_owned(),
            allowed_client_ids: vec!["robson-android".to_owned()],
            allowed_organization_ids: vec!["org-rbx".to_owned()],
        }
    }

    fn sign_test_token(key: &TestKey, audience: &'static str, roles: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "alg": "RS256",
                "kid": "test-key",
                "typ": "JWT"
            }))
            .unwrap(),
        );
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&TestClaims {
                iss: "https://identity.example",
                sub: "user-1",
                aud: audience,
                exp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 300,
                azp: "robson-android",
                roles,
            })
            .unwrap(),
        );
        let signing_input = format!("{header}.{claims}");
        let mut signature = vec![0; key.private.public_modulus_len()];
        key.private
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signing_input.as_bytes(),
                &mut signature,
            )
            .unwrap();

        format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    async fn test_verifier(key: &TestKey) -> OidcVerifier {
        let verifier = OidcVerifier::new(test_config());
        verifier
            .set_test_jwks(vec![Jwk {
                kty: "RSA".to_owned(),
                kid: "test-key".to_owned(),
                alg: Some("RS256".to_owned()),
                use_: Some("sig".to_owned()),
                n: key.modulus.clone(),
                e: key.exponent.clone(),
            }])
            .await;
        verifier
    }

    #[test]
    fn role_permissions_are_least_privilege() {
        let observer = AuthContext {
            method: AuthMethod::Oidc,
            issuer: "https://identity.example".to_owned(),
            subject: "user-1".to_owned(),
            client_id: Some("robson-android".to_owned()),
            roles: BTreeSet::from([ROLE_OBSERVER.to_owned()]),
            grants_all: false,
        };

        assert!(observer.allows(RequiredPermission::Observer));
        assert!(!observer.allows(RequiredPermission::Operator));
        assert!(!observer.allows(RequiredPermission::Funding));
        assert!(!observer.allows(RequiredPermission::Emergency));
    }

    #[test]
    fn parses_documented_zitadel_role_claim_shapes() {
        for value in [
            serde_json::json!({"rbx:robson:observer": {"org-1": "rbx.example"}}),
            serde_json::json!([{"rbx:robson:observer": {"org-1": "rbx.example"}}]),
        ] {
            let claim: ZitadelProjectRolesClaim = serde_json::from_value(value).unwrap();
            assert!(claim.into_role_map().contains_key(ROLE_OBSERVER));
        }
    }

    #[tokio::test]
    async fn legacy_token_remains_a_temporary_all_permissions_fallback() {
        let auth = AuthService::new(Some("migration-token".to_owned()), None);
        let context = auth
            .authenticate(Some("Bearer migration-token"), RequiredPermission::Emergency)
            .await
            .unwrap();

        assert_eq!(context.method, AuthMethod::LegacyToken);
        assert_eq!(context.permissions(), vec!["observer", "operator", "funding", "emergency"]);
        assert!(matches!(
            auth.authenticate(Some("Bearer wrong"), RequiredPermission::Observer).await,
            Err(AuthError::InvalidToken)
        ));
    }

    #[tokio::test]
    async fn verifies_signature_audience_client_and_organization_scoped_role() {
        let key = test_key();
        let verifier = test_verifier(&key).await;
        let token = sign_test_token(
            &key,
            "robson-api",
            serde_json::json!({(ROLE_OBSERVER): {"org-rbx": "rbx.example"}}),
        );

        let context = verifier.verify(&token).await.unwrap();
        assert_eq!(context.subject, "user-1");
        assert!(context.allows(RequiredPermission::Observer));
        assert!(!context.allows(RequiredPermission::Operator));
    }

    #[tokio::test]
    async fn rejects_wrong_audience_and_ignores_roles_from_unapproved_orgs() {
        let key = test_key();
        let verifier = test_verifier(&key).await;
        let wrong_audience = sign_test_token(
            &key,
            "another-api",
            serde_json::json!({(ROLE_OBSERVER): {"org-rbx": "rbx.example"}}),
        );
        assert!(matches!(verifier.verify(&wrong_audience).await, Err(AuthError::InvalidToken)));

        let wrong_org = sign_test_token(
            &key,
            "robson-api",
            serde_json::json!({(ROLE_OPERATOR): {"org-other": "other.example"}}),
        );
        let context = verifier.verify(&wrong_org).await.unwrap();
        assert!(!context.allows(RequiredPermission::Observer));
        assert!(!context.allows(RequiredPermission::Operator));

        let auth = AuthService {
            legacy_token: None,
            oidc: Some(Arc::new(verifier)),
        };
        let authorization = format!("Bearer {wrong_org}");
        assert!(matches!(
            auth.authenticate(Some(&authorization), RequiredPermission::Operator,).await,
            Err(AuthError::Forbidden)
        ));
    }
}
