//! Google JWKS (JSON Web Key Set) cache for verifying Google-issued ID
//! tokens (ADR-0054).
//!
//! Fetches and caches Google's public signing keys from
//! `https://www.googleapis.com/oauth2/v3/certs`. Keys rotate infrequently,
//! so this cache refreshes periodically in the background (started once at
//! daemon startup — see `daemon.rs`) and, defensively, on-demand when a
//! request presents a `kid` the cache doesn't recognize (e.g. right after a
//! Google-side rotation, before the next periodic refresh runs). The
//! on-demand path is guarded by a mutex so a burst of requests carrying the
//! same unknown `kid` triggers exactly one refresh, not a thundering herd,
//! and further rate-limited (`MIN_ON_DEMAND_REFRESH_INTERVAL`) so repeated
//! requests carrying *different* unknown/bogus `kid`s can't force a fresh
//! Google fetch on every single one.
//!
//! Per the plan, JWKS is never fetched per-request on the happy path — only
//! read from the in-memory cache.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use jsonwebtoken::DecodingKey;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

const GOOGLE_CERTS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";

/// How often the background loop refreshes the JWKS cache unconditionally.
/// Google rotates these keys on the order of days/weeks; a few hours keeps
/// us well ahead of rotation without hammering the endpoint.
pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Minimum spacing between on-demand (unknown-`kid`-triggered) refresh
/// *attempts*, independent of the periodic background loop. Without this,
/// a caller sending a stream of JWT-shaped tokens with random/unknown
/// `kid`s could force a Google JWKS fetch on every single request — an
/// availability and outbound-dependency risk on the auth hot path.
const MIN_ON_DEMAND_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
struct GoogleJwk {
    kid: String,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct GoogleJwksResponse {
    keys: Vec<GoogleJwk>,
}

/// Verification/refresh failure for the JWKS cache.
#[derive(Debug, thiserror::Error)]
pub enum JwksError {
    #[error("failed to fetch Google JWKS: {0}")]
    Fetch(#[from] reqwest::Error),
    #[error("failed to parse Google JWKS response: {0}")]
    Parse(String),
    #[error("failed to build decoding key: {0}")]
    Key(String),
}

/// Cache of Google's RSA public signing keys, keyed by `kid`.
pub struct GoogleJwksCache {
    keys: RwLock<HashMap<String, DecodingKey>>,
    refresh_lock: Mutex<()>,
    last_refresh_attempt: RwLock<Option<Instant>>,
    http: reqwest::Client,
}

impl GoogleJwksCache {
    /// Empty cache; the first `spawn_refresh_loop` tick (or the first
    /// on-demand lookup) populates it.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            keys: RwLock::new(HashMap::new()),
            refresh_lock: Mutex::new(()),
            last_refresh_attempt: RwLock::new(None),
            http: reqwest::Client::new(),
        })
    }

    /// Test-only constructor seeded directly with known keys so unit tests
    /// never hit the network. See `robsond/src/testdata/test_rsa_key.pem`
    /// for the matching non-secret test keypair.
    #[cfg(test)]
    pub fn from_keys_for_test(keys: HashMap<String, DecodingKey>) -> Arc<Self> {
        Arc::new(Self {
            keys: RwLock::new(keys),
            refresh_lock: Mutex::new(()),
            last_refresh_attempt: RwLock::new(None),
            http: reqwest::Client::new(),
        })
    }

    /// Like [`Self::from_keys_for_test`], but also seeds
    /// `last_refresh_attempt` — lets tests exercise the on-demand
    /// rate-limit branch in `get_key` deterministically, without a real
    /// (or previously-triggered) network refresh.
    #[cfg(test)]
    pub fn from_keys_for_test_with_last_attempt(
        keys: HashMap<String, DecodingKey>,
        last_refresh_attempt: Option<Instant>,
    ) -> Arc<Self> {
        Arc::new(Self {
            keys: RwLock::new(keys),
            refresh_lock: Mutex::new(()),
            last_refresh_attempt: RwLock::new(last_refresh_attempt),
            http: reqwest::Client::new(),
        })
    }

    /// Spawn the periodic background refresh loop. Call once at daemon
    /// startup (`daemon.rs`); the returned task runs for the process
    /// lifetime.
    pub fn spawn_refresh_loop(self: &Arc<Self>) {
        let cache = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if let Err(e) = cache.refresh().await {
                    warn!(error = %e, "Periodic Google JWKS refresh failed; keeping stale cache");
                }
                tokio::time::sleep(DEFAULT_REFRESH_INTERVAL).await;
            }
        });
    }

    /// Look up the decoding key for `kid`, refreshing on-demand once if it
    /// isn't in the cache yet.
    pub async fn get_key(&self, kid: &str) -> Result<DecodingKey, JwksError> {
        if let Some(key) = self.keys.read().await.get(kid) {
            return Ok(key.clone());
        }

        // Unknown kid: refresh, guarded so concurrent lookups for the same
        // rotation only trigger one HTTP fetch.
        let _guard = self.refresh_lock.lock().await;
        // Re-check: another task may have refreshed while we waited on the lock.
        if let Some(key) = self.keys.read().await.get(kid) {
            return Ok(key.clone());
        }

        // Rate-limit on-demand refresh attempts (see
        // `MIN_ON_DEMAND_REFRESH_INTERVAL`) — applies even when the fetch
        // below fails, so a degraded/unreachable Google endpoint can't be
        // hammered by every subsequent request either.
        let now = Instant::now();
        let too_soon = self
            .last_refresh_attempt
            .read()
            .await
            .is_some_and(|attempted_at| now.duration_since(attempted_at) < MIN_ON_DEMAND_REFRESH_INTERVAL);
        if too_soon {
            return Err(JwksError::Key(format!(
                "unknown kid {kid}; last on-demand refresh attempt was under {}s ago",
                MIN_ON_DEMAND_REFRESH_INTERVAL.as_secs()
            )));
        }
        *self.last_refresh_attempt.write().await = Some(now);
        self.refresh().await?;

        self.keys
            .read()
            .await
            .get(kid)
            .cloned()
            .ok_or_else(|| JwksError::Key(format!("unknown kid after refresh: {kid}")))
    }

    async fn refresh(&self) -> Result<(), JwksError> {
        let resp: GoogleJwksResponse = self
            .http
            .get(GOOGLE_CERTS_URL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if resp.keys.is_empty() {
            return Err(JwksError::Parse("Google JWKS response had no keys".into()));
        }

        let mut new_keys = HashMap::with_capacity(resp.keys.len());
        for jwk in resp.keys {
            let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
                .map_err(|e| JwksError::Key(e.to_string()))?;
            new_keys.insert(jwk.kid, key);
        }

        let count = new_keys.len();
        *self.keys.write().await = new_keys;
        info!(key_count = count, "Refreshed Google JWKS cache");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_key_rate_limits_on_demand_refresh_for_unknown_kid() {
        // `last_refresh_attempt` seeded to "now" simulates a refresh attempt
        // having just happened; the next unknown-kid lookup must be
        // rejected by the rate limiter rather than triggering another
        // network fetch (which would hang/fail in this offline test).
        let cache = GoogleJwksCache::from_keys_for_test_with_last_attempt(
            HashMap::new(),
            Some(Instant::now()),
        );

        // `DecodingKey` (the `Ok` type) isn't `Debug`, so `unwrap_err()` — whose
        // panic message formats the `Ok` value if the result isn't actually
        // an `Err` — won't compile here; match explicitly instead.
        let result = cache.get_key("unknown-kid").await;
        let Err(err) = result else {
            panic!("expected the rate-limit error, got Ok");
        };

        assert!(
            err.to_string().contains("refresh attempt was under"),
            "expected the rate-limit error, got: {err}"
        );
    }

    #[tokio::test]
    async fn get_key_returns_cached_key_without_refresh_attempt_check() {
        // A `kid` already in the cache must short-circuit before the
        // rate-limit/refresh path is even considered, regardless of
        // `last_refresh_attempt`.
        let decoding_key =
            DecodingKey::from_rsa_pem(include_bytes!("testdata/test_rsa_key_pub.pem"))
                .expect("valid test RSA public key");
        let mut keys = HashMap::new();
        keys.insert("known-kid".to_string(), decoding_key);
        let cache =
            GoogleJwksCache::from_keys_for_test_with_last_attempt(keys, Some(Instant::now()));

        assert!(cache.get_key("known-kid").await.is_ok());
    }
}
