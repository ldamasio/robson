//! RBX Identity OIDC access-token verification for Robson.
//!
//! Google remains an upstream identity provider owned by ZITADEL. Robson
//! trusts only the configured RBX issuer, API audience, public client IDs,
//! canonical subjects, and organization-scoped project roles. Direct Google
//! tokens and static API tokens are not accepted.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    time::{Duration, Instant},
};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;
use uuid::Uuid;

use crate::config::OidcConfig;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const JWKS_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(30);
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_JWKS_KEYS: usize = 32;
const MAX_JWKS_RESPONSE_BYTES: usize = 256 * 1024;
const CLOCK_SKEW_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub struct RbxOidcIdentity {
    pub issuer: String,
    pub subject: String,
    pub client_id: String,
    pub tenant_id: Uuid,
    pub roles: BTreeSet<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RbxOidcError {
    #[error("RBX Identity token is invalid")]
    InvalidToken,
    #[error("RBX Identity is temporarily unavailable")]
    IdentityUnavailable,
}

pub struct RbxOidcVerifier {
    issuer: String,
    audience: String,
    jwks_uri: String,
    allowed_client_ids: HashSet<String>,
    organization_id: String,
    tenant_id: Uuid,
    allowed_subjects: HashSet<String>,
    client: reqwest::Client,
    cache: RwLock<CachedJwks>,
    refresh_lock: Mutex<()>,
}

impl RbxOidcVerifier {
    pub fn new(config: OidcConfig) -> Self {
        Self {
            issuer: config.issuer,
            audience: config.audience,
            jwks_uri: config.jwks_uri,
            allowed_client_ids: config.allowed_client_ids.into_iter().collect(),
            organization_id: config.organization_id,
            tenant_id: config.tenant_id,
            allowed_subjects: config.allowed_subjects.into_iter().collect(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client construction should not fail with static settings"),
            cache: RwLock::new(CachedJwks::default()),
            refresh_lock: Mutex::new(()),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub async fn verify(&self, bearer_token: &str) -> Result<RbxOidcIdentity, RbxOidcError> {
        let header = decode_header(bearer_token).map_err(|_| RbxOidcError::InvalidToken)?;
        if header.alg != Algorithm::RS256 {
            return Err(RbxOidcError::InvalidToken);
        }
        let kid = header.kid.as_deref().ok_or(RbxOidcError::InvalidToken)?;
        let decoding_key = self.key_for(kid).await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.leeway = CLOCK_SKEW_SECS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.required_spec_claims =
            ["aud", "exp", "iss", "sub"].into_iter().map(str::to_owned).collect();

        let claims = decode::<JwtClaims>(bearer_token, &decoding_key, &validation)
            .map_err(|_| RbxOidcError::InvalidToken)?
            .claims;

        let client_id = claims.azp.or(claims.client_id).ok_or(RbxOidcError::InvalidToken)?;
        if !self.allowed_client_ids.contains(&client_id) {
            return Err(RbxOidcError::InvalidToken);
        }
        if !self.allowed_subjects.contains(&claims.sub) {
            return Err(RbxOidcError::InvalidToken);
        }

        let roles = claims
            .zitadel_project_roles
            .map(ZitadelProjectRolesClaim::into_role_map)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(role, organizations)| {
                organizations.contains_key(&self.organization_id).then_some(role)
            })
            .collect();

        Ok(RbxOidcIdentity {
            issuer: claims.iss,
            subject: claims.sub,
            client_id,
            tenant_id: self.tenant_id,
            roles,
        })
    }

    async fn key_for(&self, kid: &str) -> Result<DecodingKey, RbxOidcError> {
        self.refresh_jwks(false).await?;
        if let Some(key) = self.cache.read().await.keys.get(kid).cloned() {
            return Ok(key);
        }

        // A missing kid can mean key rotation. The refresh path is bounded so
        // arbitrary kid values cannot turn into one IdP request each.
        self.refresh_jwks(true).await?;
        self.cache.read().await.keys.get(kid).cloned().ok_or(RbxOidcError::InvalidToken)
    }

    async fn refresh_jwks(&self, force: bool) -> Result<(), RbxOidcError> {
        {
            let cache = self.cache.read().await;
            if !force && cache.is_fresh() {
                return Ok(());
            }
        }

        // Always join an in-flight refresh before applying the retry bound.
        // This coalesces cold-cache bursts instead of returning 503 to every
        // request except the first one.
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
                return Err(RbxOidcError::IdentityUnavailable);
            }
        }

        self.cache.write().await.last_refresh_attempt = Some(Instant::now());
        let mut response = self
            .client
            .get(&self.jwks_uri)
            .timeout(JWKS_FETCH_TIMEOUT)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| {
                warn!(error = %error, "RBX Identity JWKS refresh failed");
                RbxOidcError::IdentityUnavailable
            })?;

        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            warn!(error = %error, "RBX Identity JWKS response could not be read");
            RbxOidcError::IdentityUnavailable
        })? {
            if body.len().saturating_add(chunk.len()) > MAX_JWKS_RESPONSE_BYTES {
                warn!("RBX Identity JWKS response exceeded the size limit");
                return Err(RbxOidcError::IdentityUnavailable);
            }
            body.extend_from_slice(&chunk);
        }

        let jwks = serde_json::from_slice::<JwkSet>(&body).map_err(|error| {
            warn!(error = %error, "RBX Identity JWKS response was invalid");
            RbxOidcError::IdentityUnavailable
        })?;
        if jwks.keys.is_empty() || jwks.keys.len() > MAX_JWKS_KEYS {
            warn!(key_count = jwks.keys.len(), "RBX Identity JWKS key count rejected");
            return Err(RbxOidcError::IdentityUnavailable);
        }

        let mut keys = HashMap::with_capacity(jwks.keys.len());
        for jwk in jwks.keys {
            if jwk.kty != "RSA"
                || jwk.alg.as_deref().is_some_and(|algorithm| algorithm != "RS256")
                || jwk.use_.as_deref().is_some_and(|usage| usage != "sig")
            {
                continue;
            }
            let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e).map_err(|error| {
                warn!(error = %error, kid = %jwk.kid, "RBX Identity JWKS key was invalid");
                RbxOidcError::IdentityUnavailable
            })?;
            keys.insert(jwk.kid, key);
        }
        if keys.is_empty() {
            warn!("RBX Identity JWKS contained no acceptable signing keys");
            return Err(RbxOidcError::IdentityUnavailable);
        }

        let mut cache = self.cache.write().await;
        cache.keys = keys;
        cache.fetched_at = Some(Instant::now());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn set_test_cache(
        &self,
        keys: HashMap<String, DecodingKey>,
        fetched_at: Option<Instant>,
        last_refresh_attempt: Option<Instant>,
    ) {
        let mut cache = self.cache.write().await;
        cache.keys = keys;
        cache.fetched_at = fetched_at;
        cache.last_refresh_attempt = last_refresh_attempt;
    }
}

#[derive(Default)]
struct CachedJwks {
    keys: HashMap<String, DecodingKey>,
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

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
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
    use std::sync::Arc;

    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde::Serialize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const TEST_KID: &str = "rbx-identity-test-key";
    const TEST_MODULUS: &str = "vHCN6w1b_xQa7LJLh5RoCjll3v47GgvT_fnohSFdV7URV_50YTO-tFq640UnFS-wmt4ZPKMMhsfaifSEmZrFeyvT845xJANI6bNS7ZezQmTLxVq01cPNCv4v7pBNJsyz4uMcDaAnPCJM8XyYjia_d06noFmCj_6hB_6E6tHWt6Ikl9nqc-jC4sWMa4XyWvMGRssxG1-aoze-197wM-LSctIOeGSkQS-AjVdjWkRhTSbrP44dMriSX6cDBZ2dVPCjh6FmFrVnGGlKMhh2f7bwb7e1oRqQ-DGXyHE8gQwJGqSExb3y35tsLI6i7ABp6LutlB6R3jdKRfzYxYrTMhOu_w";

    #[derive(Serialize)]
    struct TestClaims<'a> {
        iss: &'a str,
        sub: &'a str,
        aud: &'a str,
        exp: u64,
        azp: &'a str,
        #[serde(rename = "urn:zitadel:iam:org:project:roles")]
        roles: serde_json::Value,
    }

    fn test_config() -> OidcConfig {
        OidcConfig {
            issuer: "https://identity.example".to_owned(),
            audience: "robson-api".to_owned(),
            jwks_uri: "https://identity.example/oauth/v2/keys".to_owned(),
            allowed_client_ids: vec!["robson-android".to_owned()],
            organization_id: "org-rbx".to_owned(),
            tenant_id: Uuid::parse_str("018f0000-0000-7000-8000-000000000001").unwrap(),
            allowed_subjects: vec!["user-1".to_owned()],
        }
    }

    fn sign_test_token(
        issuer: &str,
        audience: &str,
        client_id: &str,
        roles: serde_json::Value,
    ) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_owned());
        encode(
            &header,
            &TestClaims {
                iss: issuer,
                sub: "user-1",
                aud: audience,
                exp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 300,
                azp: client_id,
                roles,
            },
            &EncodingKey::from_rsa_pem(include_bytes!("testdata/test_rsa_key.pem")).unwrap(),
        )
        .unwrap()
    }

    async fn test_verifier() -> RbxOidcVerifier {
        let verifier = RbxOidcVerifier::new(test_config());
        let mut keys = HashMap::new();
        keys.insert(
            TEST_KID.to_owned(),
            DecodingKey::from_rsa_pem(include_bytes!("testdata/test_rsa_key_pub.pem")).unwrap(),
        );
        verifier.set_test_cache(keys, Some(Instant::now()), Some(Instant::now())).await;
        verifier
    }

    #[test]
    fn parses_documented_zitadel_role_claim_shapes() {
        for value in [
            serde_json::json!({"rbx:robson:observer": {"org-1": "rbx.example"}}),
            serde_json::json!([{"rbx:robson:observer": {"org-1": "rbx.example"}}]),
        ] {
            let claim: ZitadelProjectRolesClaim = serde_json::from_value(value).unwrap();
            assert!(claim.into_role_map().contains_key("rbx:robson:observer"));
        }
    }

    #[tokio::test]
    async fn verifies_signature_audience_client_and_organization_scoped_role() {
        let verifier = test_verifier().await;
        let token = sign_test_token(
            "https://identity.example",
            "robson-api",
            "robson-android",
            serde_json::json!({"rbx:robson:observer": {"org-rbx": "rbx.example"}}),
        );

        let identity = verifier.verify(&token).await.unwrap();
        assert_eq!(identity.subject, "user-1");
        assert!(identity.roles.contains("rbx:robson:observer"));
    }

    #[tokio::test]
    async fn rejects_wrong_issuer_audience_and_client() {
        let verifier = test_verifier().await;
        for token in [
            sign_test_token(
                "https://wrong.example",
                "robson-api",
                "robson-android",
                serde_json::json!({}),
            ),
            sign_test_token(
                "https://identity.example",
                "wrong-audience",
                "robson-android",
                serde_json::json!({}),
            ),
            sign_test_token(
                "https://identity.example",
                "robson-api",
                "wrong-client",
                serde_json::json!({}),
            ),
        ] {
            assert!(matches!(verifier.verify(&token).await, Err(RbxOidcError::InvalidToken)));
        }
    }

    #[tokio::test]
    async fn rejects_a_valid_token_for_another_subject() {
        let verifier = test_verifier().await;
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_owned());
        let token = encode(
            &header,
            &TestClaims {
                iss: "https://identity.example",
                sub: "another-user",
                aud: "robson-api",
                exp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 300,
                azp: "robson-android",
                roles: serde_json::json!({
                    "rbx:robson:operator": {"org-rbx": "rbx.example"}
                }),
            },
            &EncodingKey::from_rsa_pem(include_bytes!("testdata/test_rsa_key.pem")).unwrap(),
        )
        .unwrap();

        assert!(matches!(verifier.verify(&token).await, Err(RbxOidcError::InvalidToken)));
    }

    #[tokio::test]
    async fn ignores_roles_from_unapproved_organizations() {
        let verifier = test_verifier().await;
        let token = sign_test_token(
            "https://identity.example",
            "robson-api",
            "robson-android",
            serde_json::json!({"rbx:robson:operator": {"org-other": "other.example"}}),
        );

        let identity = verifier.verify(&token).await.unwrap();
        assert!(identity.roles.is_empty());
    }

    #[tokio::test]
    async fn reports_recent_failed_refresh_as_identity_unavailable() {
        let verifier = RbxOidcVerifier::new(test_config());
        verifier.set_test_cache(HashMap::new(), None, Some(Instant::now())).await;
        let token = sign_test_token(
            "https://identity.example",
            "robson-api",
            "robson-android",
            serde_json::json!({}),
        );

        assert!(matches!(verifier.verify(&token).await, Err(RbxOidcError::IdentityUnavailable)));
    }

    #[tokio::test]
    async fn concurrent_cold_cache_requests_share_one_refresh() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "kid": TEST_KID,
                "alg": "RS256",
                "use": "sig",
                "n": TEST_MODULUS,
                "e": "AQAB"
            }]
        })
        .to_string();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        let mut config = test_config();
        config.jwks_uri = format!("http://{address}/keys");
        let verifier = Arc::new(RbxOidcVerifier::new(config));
        let token = sign_test_token(
            "https://identity.example",
            "robson-api",
            "robson-android",
            serde_json::json!({"rbx:robson:observer": {"org-rbx": "rbx.example"}}),
        );

        let (first, second) = tokio::join!(verifier.verify(&token), verifier.verify(&token));
        assert!(first.is_ok(), "first request should authenticate: {first:?}");
        assert!(second.is_ok(), "concurrent request should share refresh: {second:?}");
        server.await.unwrap();
    }
}
