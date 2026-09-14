//! Google ID token verification (ADR-0054).
//!
//! Every authenticated request presents a Google-issued ID token as its
//! Bearer credential. Verification is stateless and per-request: robsond
//! never issues or stores a session. The token's signature is checked
//! against Google's published JWKS (`google_jwks.rs`), then standard OIDC
//! claims (`iss`, `aud`, `exp`) plus a single-email allowlist are enforced —
//! Robson has exactly one operator (`ROBSON_ALLOWED_EMAIL`).
//!
//! Any verification failure returns a generic, opaque error to the caller.
//! The real reason is logged at `warn` level server-side only — see
//! `verify_google_id_token`'s callers in `api.rs`.

use std::sync::Arc;

use jsonwebtoken::{decode, decode_header, Algorithm, Validation};
use serde::Deserialize;

use crate::google_jwks::GoogleJwksCache;

/// Google's two historically-used `iss` values for ID tokens.
const GOOGLE_ISSUERS: [&str; 2] = ["accounts.google.com", "https://accounts.google.com"];

/// Runtime configuration for Google ID token verification.
#[derive(Clone)]
pub struct GoogleAuthConfig {
    /// Acceptable `aud` values: the frontend's Web client ID and/or the
    /// CLI's Device Authorization Grant (TV/limited-input) client ID.
    /// Either is accepted so both the browser and `robson-cli` can call the
    /// same API.
    pub client_ids: Vec<String>,
    /// The single allowlisted operator email, matched case-insensitively.
    pub allowed_email: String,
    /// Shared JWKS cache, refreshed independently of any single request.
    pub jwks: Arc<GoogleJwksCache>,
}

/// Claims extracted from a verified Google ID token.
#[derive(Debug, Clone, Deserialize)]
pub struct GoogleClaims {
    pub sub: String,
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
    pub iss: String,
    pub aud: String,
    pub exp: usize,
}

/// Verification failure. Deliberately opaque: `Display` is for server-side
/// logs only and must never be rendered into an HTTP response body.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("malformed token: {0}")]
    Malformed(String),
    #[error("unknown signing key: {0}")]
    UnknownKey(String),
    #[error("signature/claim verification failed: {0}")]
    Verification(String),
    #[error("email not verified")]
    EmailNotVerified,
    #[error("email not allowlisted")]
    EmailNotAllowed,
}

/// Verify `token` as a Google-issued ID token under `cfg`.
///
/// Checks (in order): well-formed header with a `kid`, RS256 signature
/// against the matching Google public key, `aud` in `cfg.client_ids`, `iss`
/// in Google's known issuer set, `exp` not passed, `email_verified == true`,
/// and `email` case-insensitively equal to `cfg.allowed_email`.
pub async fn verify_google_id_token(
    token: &str,
    cfg: &GoogleAuthConfig,
) -> Result<GoogleClaims, AuthError> {
    let header = decode_header(token).map_err(|e| AuthError::Malformed(e.to_string()))?;
    let kid = header.kid.ok_or_else(|| AuthError::Malformed("missing kid".to_string()))?;

    let key = cfg.jwks.get_key(&kid).await.map_err(|e| AuthError::UnknownKey(e.to_string()))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&cfg.client_ids);
    validation.set_issuer(&GOOGLE_ISSUERS);
    // `exp` is validated by default by the `jsonwebtoken` crate.

    let data = decode::<GoogleClaims>(token, &key, &validation)
        .map_err(|e| AuthError::Verification(e.to_string()))?;
    let claims = data.claims;

    if !claims.email_verified {
        return Err(AuthError::EmailNotVerified);
    }

    let email = claims.email.as_deref().unwrap_or_default();
    if !email.eq_ignore_ascii_case(&cfg.allowed_email) {
        return Err(AuthError::EmailNotAllowed);
    }

    Ok(claims)
}

/// True when `value` has the three dot-separated, base64url-charset
/// segments characteristic of a JWT, as opposed to an opaque legacy bearer
/// token. Used by the auth middleware's dual-accept rollout bridge to route
/// a presented credential to the right verifier.
///
/// TEMPORARY: dual-accept bridge helper during ADR-0054 rollout, remove
/// after cutover — see ADR-0054 migration plan and `api.rs`'s auth layer.
pub fn looks_like_jwt(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

/// Constant-time equality for the legacy bearer-token rollout bridge.
///
/// A plain `==` short-circuits on the first mismatched byte, which leaks
/// timing information an attacker could use to recover the token
/// byte-by-byte — exactly the class of risk this migration exists to
/// remove, so the temporary bridge must not reintroduce it.
///
/// Note: the length check is a fast-path `return`, so distinct-length
/// inputs are distinguishable by timing (only equal-length inputs compare
/// in constant time). This is the same trade-off standard constant-time
/// primitives make (e.g. `subtle`'s `ConstantTimeEq` on byte slices,
/// `ring`'s `verify_slices_are_equal`) — hiding length as well would mean
/// comparing against a fixed maximum size, which isn't warranted for a
/// bridge already scheduled for removal.
///
/// TEMPORARY: dual-accept bridge helper during ADR-0054 rollout, remove
/// after cutover — see ADR-0054 migration plan and `api.rs`'s auth layer.
pub fn constant_time_eq(expected: &str, actual: &str) -> bool {
    let (expected, actual) = (expected.as_bytes(), actual.as_bytes());
    if expected.len() != actual.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (a, b) in expected.iter().zip(actual.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_jwt_accepts_three_base64url_segments() {
        assert!(looks_like_jwt("eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxMjMifQ.c2ln"));
    }

    #[test]
    fn looks_like_jwt_rejects_opaque_tokens() {
        assert!(!looks_like_jwt("secret-token-123"));
        assert!(!looks_like_jwt(""));
        assert!(!looks_like_jwt("only.two"));
        assert!(!looks_like_jwt("a.b.c.d"));
        assert!(!looks_like_jwt("has spaces.in.it"));
    }

    #[test]
    fn constant_time_eq_matches_equal_strings() {
        assert!(constant_time_eq("secret-token-123", "secret-token-123"));
    }

    #[test]
    fn constant_time_eq_rejects_different_strings() {
        assert!(!constant_time_eq("secret-token-123", "secret-token-124"));
        assert!(!constant_time_eq("short", "much-longer-value"));
        assert!(!constant_time_eq("", "nonempty"));
        assert!(constant_time_eq("", ""));
    }
}
