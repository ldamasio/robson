//! Google OAuth 2.0 Device Authorization Grant for `robson-cli` (ADR-0054).
//!
//! `robson-cli` authenticates as the same single operator as the frontend
//! (Google Identity Services), but through the Device Authorization Grant
//! (RFC 8628) since a CLI has no browser-embeddable redirect URI. The
//! resulting Google ID token is sent as the `Authorization: Bearer` header
//! on every robsond API call — robsond verifies it statelessly, exactly as
//! it does for the frontend's token (see `robsond/src/auth.rs`).
//!
//! Credentials are cached at
//! `$XDG_CONFIG_HOME/robson/credentials.json` (falling back to
//! `$HOME/.config/robson/credentials.json`) with `0600` permissions on
//! Unix, and transparently refreshed via the stored refresh token when
//! within `REFRESH_SKEW_SECS` of expiry.

use std::{
    fs, io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use oauth2::{
    basic::{BasicErrorResponseType, BasicTokenType},
    devicecode::StandardDeviceAuthorizationResponse,
    reqwest::async_http_client,
    AuthUrl, Client, ClientId, DeviceAuthorizationUrl, EmptyExtraTokenFields,
    RefreshToken, RevocationErrorResponseType, Scope, StandardErrorResponse,
    StandardRevocableToken, StandardTokenIntrospectionResponse, StandardTokenResponse,
    TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};

/// Google's OAuth 2.0 "TVs and Limited-Input devices" client ID registered
/// for `robson-cli`'s Device Authorization Grant flow.
///
/// TODO(owner): fill in the real Client ID after registering it in Google
/// Cloud Console (APIs & Services -> Credentials -> Create Credentials ->
/// OAuth client ID -> "TVs and Limited Input devices"). This is
/// deliberately compiled into the binary rather than operator-configurable
/// — see ADR-0054's "amends ADR-0025" section: the CLI's audience is a
/// fixed, pre-registered OAuth client, not a per-deployment setting.
pub const GOOGLE_CLI_CLIENT_ID: &str =
    "TODO-REPLACE-WITH-REAL-GOOGLE-CLI-CLIENT-ID.apps.googleusercontent.com";

const GOOGLE_DEVICE_AUTH_URL: &str = "https://oauth2.googleapis.com/device/code";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
// Unused by the device-code flow itself, but `oauth2::Client` requires an
// authorization endpoint to be configured.
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Refresh proactively once within this many seconds of the cached
/// `id_token`'s expiry, mirroring the frontend's silent-refresh window.
const REFRESH_SKEW_SECS: i64 = 300;

/// Cached credential set, persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub id_token: String,
    pub refresh_token: Option<String>,
    /// Unix timestamp (seconds, UTC) the `id_token` expires at.
    pub expiry: i64,
}

/// Google puts the ID token in a non-standard `id_token` field on the token
/// response; this is how `oauth2-rs` recommends capturing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleExtraTokenFields {
    pub id_token: Option<String>,
}
impl oauth2::ExtraTokenFields for GoogleExtraTokenFields {}

type GoogleTokenResponse = StandardTokenResponse<GoogleExtraTokenFields, BasicTokenType>;

type GoogleClient = Client<
    StandardErrorResponse<BasicErrorResponseType>,
    GoogleTokenResponse,
    BasicTokenType,
    StandardTokenIntrospectionResponse<EmptyExtraTokenFields, BasicTokenType>,
    StandardRevocableToken,
    StandardErrorResponse<RevocationErrorResponseType>,
>;

/// Minimal, unverified decode of an ID token's payload claims we care
/// about — client-side convenience only (email + expiry for `auth status`
/// and the refresh-skew check). robsond re-verifies the signature and every
/// claim server-side on every request (`robsond/src/auth.rs`); the CLI
/// never needs to and does not attempt to.
#[derive(Debug, Clone, Deserialize)]
struct IdTokenClaims {
    email: Option<String>,
    exp: i64,
}

fn credentials_path() -> Result<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .context("neither XDG_CONFIG_HOME nor HOME is set; cannot locate credentials")?;
    Ok(config_home.join("robson").join("credentials.json"))
}

fn decode_id_token_claims(id_token: &str) -> Result<IdTokenClaims> {
    let payload = id_token
        .split('.')
        .nth(1)
        .context("malformed id_token: expected a three-part JWT")?;
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload)
        .context("malformed id_token: invalid base64url payload")?;
    serde_json::from_slice(&decoded).context("malformed id_token: payload is not valid JSON")
}

fn build_client() -> Result<GoogleClient> {
    Ok(GoogleClient::new(
        ClientId::new(GOOGLE_CLI_CLIENT_ID.to_string()),
        None,
        AuthUrl::new(GOOGLE_AUTH_URL.to_string()).context("invalid Google auth URL")?,
        Some(TokenUrl::new(GOOGLE_TOKEN_URL.to_string()).context("invalid Google token URL")?),
    )
    .set_device_authorization_url(
        DeviceAuthorizationUrl::new(GOOGLE_DEVICE_AUTH_URL.to_string())
            .context("invalid Google device authorization URL")?,
    ))
}

/// Run the Device Authorization Grant end to end: request a device code,
/// print the user code and verification URL, poll until the operator
/// approves in a browser, then persist the resulting credentials.
///
/// Returns the signed-in email on success.
pub async fn login() -> Result<String> {
    let client = build_client()?;

    let details: StandardDeviceAuthorizationResponse = client
        .exchange_device_code()
        .context("failed to build device authorization request")?
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .request_async(async_http_client)
        .await
        .context("failed to start Google device authorization")?;

    println!(
        "To sign in, open {} and enter this code: {}",
        details.verification_uri().as_str(),
        details.user_code().secret()
    );
    if let Some(complete_uri) = details.verification_uri_complete() {
        println!("Or open directly: {}", complete_uri.secret());
    }
    println!("Waiting for approval...");

    let token = client
        .exchange_device_access_token(&details)
        .request_async(async_http_client, tokio::time::sleep, None)
        .await
        .map_err(|e| anyhow::anyhow!("Google device authorization failed: {e}"))?;

    let id_token = token
        .extra_fields()
        .id_token
        .clone()
        .context("Google did not return an id_token — was the `openid` scope granted?")?;
    let claims = decode_id_token_claims(&id_token)?;
    let refresh_token = token.refresh_token().map(|t| t.secret().clone());

    let creds = Credentials { id_token, refresh_token, expiry: claims.exp };
    save_credentials(&creds)?;

    Ok(claims.email.unwrap_or_else(|| "<unknown email>".to_string()))
}

async fn refresh(refresh_token: &str) -> Result<Credentials> {
    let client = build_client()?;
    let token = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
        .request_async(async_http_client)
        .await
        .map_err(|e| anyhow::anyhow!("failed to refresh Google credentials: {e}"))?;

    let id_token = token
        .extra_fields()
        .id_token
        .clone()
        .context("Google refresh response did not include an id_token")?;
    let claims = decode_id_token_claims(&id_token)?;
    // Google does not always return a new refresh_token on refresh; keep
    // reusing the existing one when it doesn't.
    let refresh_token =
        token.refresh_token().map(|t| t.secret().clone()).or_else(|| Some(refresh_token.to_string()));

    Ok(Credentials { id_token, refresh_token, expiry: claims.exp })
}

/// Load the cached credentials without any network call or refresh — used
/// by `robson auth status`.
///
/// `save_credentials` sets `0600` permissions, but only at write time; a
/// file that predates that fix, was restored from a backup/tarball that
/// doesn't preserve modes, or was tampered with could sit at looser
/// permissions indefinitely between logins. Re-check (and, if needed,
/// re-tighten) the mode on every load so a credential this sensitive is
/// never trusted world/group-readable, however it got that way.
pub fn load_credentials() -> Result<Credentials> {
    let path = credentials_path()?;

    // Check (and, if needed, re-tighten) permissions *before* reading the
    // file's contents — narrows the window in which a permissively-mode'd
    // credential file's bytes could be read by another local process to
    // "before we've corrected it," rather than also spanning the read.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&path) {
            let mode = metadata.permissions().mode() & 0o777;
            if mode != 0o600 {
                eprintln!(
                    "warning: {} had permissions {mode:o} (expected 0600); tightening now",
                    path.display()
                );
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
            }
        }
    }

    let raw = fs::read_to_string(&path)
        .map_err(|_| anyhow::anyhow!("no cached Google credentials found; run `robson auth login` first"))?;

    serde_json::from_str(&raw).context("cached credentials file is corrupt; run `robson auth login` again")
}

fn save_credentials(creds: &Credentials) -> Result<()> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(creds)?;
    fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
    }

    Ok(())
}

/// Delete the cached credentials file. Returns `true` if a file was
/// actually removed, `false` if there was nothing cached.
pub fn delete_credentials() -> Result<bool> {
    let path = credentials_path()?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("failed to delete {}", path.display())),
    }
}

/// Return the cached email/expiry pair without any network call — used by
/// `robson auth status`.
pub fn cached_identity() -> Result<(Option<String>, i64)> {
    let creds = load_credentials()?;
    let claims = decode_id_token_claims(&creds.id_token)?;
    Ok((claims.email, claims.exp))
}

/// Return a currently-valid Google ID token, transparently refreshing via
/// the cached refresh token when within `REFRESH_SKEW_SECS` of expiry.
/// This is what `api_client::ApiClient` calls before every request.
pub async fn current_id_token() -> Result<String> {
    let creds = load_credentials()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;

    if creds.expiry - now > REFRESH_SKEW_SECS {
        return Ok(creds.id_token);
    }

    let Some(refresh_token) = creds.refresh_token.as_deref() else {
        bail!(
            "cached Google credentials are expired and no refresh_token is available; \
             run `robson auth login` again"
        );
    };

    let refreshed = refresh(refresh_token)
        .await
        .context("failed to refresh cached Google credentials; run `robson auth login` again")?;
    save_credentials(&refreshed)?;
    Ok(refreshed.id_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_id_token(payload_json: &str) -> String {
        use base64::Engine;
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"alg\":\"RS256\"}");
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        format!("{header}.{payload}.signature-not-checked-client-side")
    }

    #[test]
    fn decodes_email_and_exp_from_id_token_payload() {
        let token = make_id_token(r#"{"email":"ldamasio@gmail.com","exp":1999999999}"#);
        let claims = decode_id_token_claims(&token).unwrap();
        assert_eq!(claims.email.as_deref(), Some("ldamasio@gmail.com"));
        assert_eq!(claims.exp, 1999999999);
    }

    #[test]
    fn rejects_malformed_id_token() {
        assert!(decode_id_token_claims("not-a-jwt").is_err());
        assert!(decode_id_token_claims("only.two").is_err());
    }

    #[test]
    fn credentials_path_prefers_xdg_config_home() {
        // SAFETY: single-threaded test, no other test mutates these vars
        // concurrently within this process invocation of this test binary
        // section (cargo test runs each test in its own thread, so this
        // could race with other env-mutating tests; kept narrowly scoped
        // and immediately restored to minimize the window).
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/robson-cli-test-xdg");
        let path = credentials_path().unwrap();
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        assert_eq!(path, PathBuf::from("/tmp/robson-cli-test-xdg/robson/credentials.json"));
    }

    #[cfg(unix)]
    #[test]
    fn load_credentials_tightens_overly_permissive_file() {
        use std::os::unix::fs::PermissionsExt;

        // SAFETY: see `credentials_path_prefers_xdg_config_home` above —
        // same narrowly-scoped env mutation, immediately restored.
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let dir = std::env::temp_dir().join(format!(
            "robson-cli-test-perms-{}",
            std::process::id()
        ));
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        let path = credentials_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let creds = Credentials {
            id_token: make_id_token(r#"{"email":"ldamasio@gmail.com","exp":1999999999}"#),
            refresh_token: None,
            expiry: 1999999999,
        };
        fs::write(&path, serde_json::to_string(&creds).unwrap()).unwrap();
        // Simulate a file that predates the 0600-on-write fix (or was
        // restored without preserving mode bits): world-readable.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let loaded = load_credentials();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = fs::remove_dir_all(&dir);
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }

        assert!(loaded.is_ok(), "load_credentials should still succeed: {loaded:?}");
        assert_eq!(mode, 0o600, "load_credentials should have tightened the mode");
    }
}
