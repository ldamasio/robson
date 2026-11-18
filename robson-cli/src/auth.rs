//! RBX Identity Device Authorization Grant for `robson-cli` (ADR-0057).
//!
//! Google is an upstream provider of ZITADEL; the CLI talks only to the RBX
//! issuer and sends its access token to `robsond`. The public client ID and
//! scopes are deployment configuration and no client secret is used.
//!
//! Credentials are cached at
//! `$XDG_CONFIG_HOME/robson/credentials.json` (falling back to
//! `$HOME/.config/robson/credentials.json`) with `0600` permissions on
//! Unix, and transparently refreshed via the stored refresh token when
//! within `REFRESH_SKEW_SECS` of expiry.

use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use oauth2::{
    basic::{
        BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse,
        BasicTokenType,
    },
    AuthUrl, Client, ClientId, DeviceAuthorizationUrl, EndpointNotSet, EndpointSet, RefreshToken,
    Scope, StandardDeviceAuthorizationResponse, StandardRevocableToken, StandardTokenResponse,
    TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};

/// A plain `reqwest::Client` implements `oauth2`'s `AsyncHttpClient` trait
/// directly (via the crate's `reqwest` feature) — no glue code needed, and
/// critically, no *second* HTTP/TLS stack: `oauth2 = "5"`'s own `reqwest`
/// dependency is already `reqwest = "0.12"` (same major this workspace
/// uses everywhere else), unlike `oauth2 = "4"`'s bundled `reqwest 0.11`
/// (which pulled an old, RUSTSEC-flagged `rustls-webpki` — see
/// RUSTSEC-2026-0098/0099/0104/0258 — alongside the one this repo already
/// fixed once in the `fix(deps): resolve RUSTSEC-2026-0258` commit).
fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        // Following redirects opens the client up to SSRF vulnerabilities,
        // same rationale as oauth2's own bundled client.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build HTTP client for RBX Identity requests")
}

const OIDC_ISSUER_ENV: &str = "ROBSON_CLI_OIDC_ISSUER";
const OIDC_CLIENT_ID_ENV: &str = "ROBSON_CLI_OIDC_CLIENT_ID";
const OIDC_SCOPES_ENV: &str = "ROBSON_CLI_OIDC_SCOPES";
const ORGANIZATION_SCOPE_PREFIX: &str = "urn:zitadel:iam:org:id:";
const AUDIENCE_SCOPE_PREFIX: &str = "urn:zitadel:iam:org:project:id:";
const AUDIENCE_SCOPE_SUFFIX: &str = ":aud";

/// Refresh proactively once within this many seconds of the cached access
/// token's expiry.
const REFRESH_SKEW_SECS: i64 = 300;

/// Cached credential set, persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub issuer: String,
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub email: Option<String>,
    /// Unix timestamp (seconds, UTC) the access token expires at.
    pub expiry: i64,
}

/// Capture the optional OIDC ID token for human-facing identity display. It is
/// never sent to `robsond`; API authentication always uses the access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcExtraTokenFields {
    pub id_token: Option<String>,
}
impl oauth2::ExtraTokenFields for OidcExtraTokenFields {}

type OidcTokenResponse = StandardTokenResponse<OidcExtraTokenFields, BasicTokenType>;

/// Like `oauth2::basic::BasicClient`, but with `OidcTokenResponse` (which
/// carries the non-standard `id_token` extra field) in place of
/// `BasicClient`'s hardcoded `EmptyExtraTokenFields` token response — the
/// one piece `BasicClient` can't be used for as-is. Endpoint-state type
/// parameters mirror `build_client()`: auth/device-auth/token URLs are set,
/// introspection/revocation are not.
type OidcClient = Client<
    BasicErrorResponse,
    OidcTokenResponse,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
    EndpointSet,    // HasAuthUrl
    EndpointSet,    // HasDeviceAuthUrl
    EndpointNotSet, // HasIntrospectionUrl
    EndpointNotSet, // HasRevocationUrl
    EndpointSet,    // HasTokenUrl
>;

/// `Client::new()` only exists on the type with every endpoint left
/// unset (`OidcClient`'s type alias defaults) — `build_client()` starts
/// from this and transitions to `OidcClient`'s state via the `set_*_uri`
/// builder calls, which is how `oauth2`'s type-state builder is meant to
/// be used (see the crate's own `google_devicecode.rs` example).
type OidcClientInitial = Client<
    BasicErrorResponse,
    OidcTokenResponse,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
>;

/// Minimal, unverified decode of an ID token's payload claims we care
/// about — client-side convenience only (email for `auth status`). The ID
/// token is never sent to robsond. The API independently verifies the access
/// token's signature and authorization claims on every request; the CLI does
/// not use these unverified display claims for access control.
#[derive(Debug, Clone, Deserialize)]
struct IdTokenClaims {
    email: Option<String>,
}

struct OidcClientConfig {
    issuer: String,
    client_id: String,
    scopes: Vec<String>,
    authorization_url: String,
    device_authorization_url: String,
    token_url: String,
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
    let decoded =
        base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload)
            .context("malformed id_token: invalid base64url payload")?;
    serde_json::from_slice(&decoded).context("malformed id_token: payload is not valid JSON")
}

fn client_config() -> Result<OidcClientConfig> {
    let raw_issuer =
        std::env::var(OIDC_ISSUER_ENV).with_context(|| format!("{OIDC_ISSUER_ENV} is required"))?;
    let raw_client_id = std::env::var(OIDC_CLIENT_ID_ENV)
        .with_context(|| format!("{OIDC_CLIENT_ID_ENV} is required"))?;
    let raw_scopes =
        std::env::var(OIDC_SCOPES_ENV).with_context(|| format!("{OIDC_SCOPES_ENV} is required"))?;
    parse_client_config(&raw_issuer, &raw_client_id, &raw_scopes)
}

fn parse_client_config(
    raw_issuer: &str,
    raw_client_id: &str,
    raw_scopes: &str,
) -> Result<OidcClientConfig> {
    let issuer_url = reqwest::Url::parse(raw_issuer.trim())
        .with_context(|| format!("{OIDC_ISSUER_ENV} must be a valid URL"))?;
    if issuer_url.scheme() != "https"
        || !issuer_url.username().is_empty()
        || issuer_url.password().is_some()
        || issuer_url.host_str().is_none()
        || issuer_url.query().is_some()
        || issuer_url.fragment().is_some()
    {
        bail!("{OIDC_ISSUER_ENV} must be a credential-free HTTPS URL");
    }
    let issuer = issuer_url.as_str().trim_end_matches('/').to_owned();
    let client_id = raw_client_id.trim().to_owned();
    if client_id.is_empty() {
        bail!("{OIDC_CLIENT_ID_ENV} must not be empty");
    }
    let scopes = raw_scopes.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    if !scopes.iter().any(|scope| scope == "openid")
        || !scopes.iter().any(|scope| scope == "offline_access")
    {
        bail!("{OIDC_SCOPES_ENV} must include openid and offline_access");
    }
    if !scopes
        .iter()
        .any(|scope| parameterized_scope_value(scope, ORGANIZATION_SCOPE_PREFIX, "").is_some())
    {
        bail!("{OIDC_SCOPES_ENV} must include a ZITADEL organization-id scope");
    }
    if !scopes.iter().any(|scope| {
        parameterized_scope_value(scope, AUDIENCE_SCOPE_PREFIX, AUDIENCE_SCOPE_SUFFIX).is_some()
    }) {
        bail!("{OIDC_SCOPES_ENV} must include a ZITADEL API audience scope");
    }

    Ok(OidcClientConfig {
        authorization_url: format!("{issuer}/oauth/v2/authorize"),
        device_authorization_url: format!("{issuer}/oauth/v2/device_authorization"),
        token_url: format!("{issuer}/oauth/v2/token"),
        issuer,
        client_id,
        scopes,
    })
}

fn parameterized_scope_value<'a>(scope: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    scope
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
        .filter(|value| !value.is_empty())
}

fn build_client(config: &OidcClientConfig) -> Result<OidcClient> {
    Ok(OidcClientInitial::new(ClientId::new(config.client_id.clone()))
        .set_auth_uri(
            AuthUrl::new(config.authorization_url.clone())
                .context("invalid RBX Identity authorization URL")?,
        )
        .set_token_uri(
            TokenUrl::new(config.token_url.clone()).context("invalid RBX Identity token URL")?,
        )
        .set_device_authorization_url(
            DeviceAuthorizationUrl::new(config.device_authorization_url.clone())
                .context("invalid RBX Identity device authorization URL")?,
        ))
}

/// Run the Device Authorization Grant end to end: request a device code,
/// print the user code and verification URL, poll until the operator
/// approves in a browser, then persist the resulting credentials.
///
/// Returns the signed-in email on success.
pub async fn login() -> Result<String> {
    let config = client_config()?;
    let client = build_client(&config)?;
    let http = http_client()?;

    let mut request = client.exchange_device_code();
    for scope in &config.scopes {
        request = request.add_scope(Scope::new(scope.clone()));
    }
    let details: StandardDeviceAuthorizationResponse = request
        .request_async(&http)
        .await
        .context("failed to start RBX Identity device authorization")?;

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
        .request_async(&http, tokio::time::sleep, None)
        .await
        .map_err(|e| anyhow::anyhow!("RBX Identity device authorization failed: {e}"))?;

    let email = token
        .extra_fields()
        .id_token
        .as_deref()
        .map(decode_id_token_claims)
        .transpose()?
        .and_then(|claims| claims.email);
    let refresh_token = token.refresh_token().map(|t| t.secret().clone());
    let expires_in = token
        .expires_in()
        .context("RBX Identity token response did not include expires_in")?;

    let creds = Credentials {
        issuer: config.issuer,
        client_id: config.client_id,
        access_token: token.access_token().secret().clone(),
        refresh_token,
        email: email.clone(),
        expiry: chrono::Utc::now().timestamp() + expires_in.as_secs() as i64,
    };
    save_credentials(&creds)?;

    Ok(email.unwrap_or_else(|| "<canonical RBX identity>".to_string()))
}

async fn refresh(existing: &Credentials, refresh_token: &str) -> Result<Credentials> {
    let config = client_config()?;
    if config.issuer != existing.issuer || config.client_id != existing.client_id {
        bail!(
            "configured RBX Identity issuer/client differs from cached credentials; run \
             `robson-cli auth logout` and `robson-cli auth login`"
        );
    }
    let client = build_client(&config)?;
    let http = http_client()?;
    let token = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
        .request_async(&http)
        .await
        .map_err(|e| anyhow::anyhow!("failed to refresh RBX Identity credentials: {e}"))?;

    let refresh_token = token
        .refresh_token()
        .map(|t| t.secret().clone())
        .or_else(|| Some(refresh_token.to_string()));
    let email = token
        .extra_fields()
        .id_token
        .as_deref()
        .map(decode_id_token_claims)
        .transpose()?
        .and_then(|claims| claims.email)
        .or_else(|| existing.email.clone());
    let expires_in = token
        .expires_in()
        .context("RBX Identity refresh response did not include expires_in")?;

    Ok(Credentials {
        issuer: config.issuer,
        client_id: config.client_id,
        access_token: token.access_token().secret().clone(),
        refresh_token,
        email,
        expiry: chrono::Utc::now().timestamp() + expires_in.as_secs() as i64,
    })
}

/// Load the cached credentials without any network call or refresh — used
/// by `robson-cli auth status`.
///
/// `save_credentials` creates its temporary file with `0600` permissions; a
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
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            anyhow::anyhow!(
                "no cached RBX Identity credentials found; run `robson-cli auth login` first"
            )
        })?;
        if !metadata.file_type().is_file() {
            anyhow::bail!("refusing to read a non-regular RBX Identity credential file");
        }
        let mode = metadata.permissions().mode() & 0o777;
        if mode != 0o600 {
            eprintln!(
                "warning: {} had permissions {mode:o} (expected 0600); tightening now",
                path.display()
            );
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .context("failed to secure the RBX Identity credential file")?;
        }
        let secured = fs::symlink_metadata(&path)
            .context("failed to verify the RBX Identity credential file permissions")?;
        if !secured.file_type().is_file() || secured.permissions().mode() & 0o777 != 0o600 {
            anyhow::bail!("RBX Identity credential file permissions are not 0600");
        }
    }

    let raw = fs::read_to_string(&path).map_err(|_| {
        anyhow::anyhow!(
            "no cached RBX Identity credentials found; run `robson-cli auth login` first"
        )
    })?;

    serde_json::from_str(&raw)
        .context("cached credentials file is corrupt; run `robson-cli auth login` again")
}

fn save_credentials(creds: &Credentials) -> Result<()> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(creds)?;
    let parent = path.parent().context("credentials path has no parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create a credential file in {}", parent.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("failed to secure temporary credential file")?;
    }

    temporary.write_all(json.as_bytes()).context("failed to write credentials")?;
    temporary.flush().context("failed to flush credentials")?;
    temporary.as_file().sync_all().context("failed to sync credentials")?;
    temporary
        .persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to atomically replace {}", path.display()))?;

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
/// `robson-cli auth status`.
pub fn cached_identity() -> Result<(Option<String>, i64)> {
    let creds = load_credentials()?;
    Ok((creds.email, creds.expiry))
}

/// Return a currently-valid RBX Identity access token, transparently refreshing
/// via the cached refresh token when within `REFRESH_SKEW_SECS` of expiry.
/// This is what `api_client::ApiClient` calls before every request.
pub async fn current_access_token() -> Result<String> {
    let creds = load_credentials()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;

    if creds.expiry - now > REFRESH_SKEW_SECS {
        return Ok(creds.access_token);
    }

    let Some(refresh_token) = creds.refresh_token.as_deref() else {
        bail!(
            "cached RBX Identity credentials are expired and no refresh_token is available; \
             run `robson-cli auth login` again"
        );
    };

    let refreshed = refresh(&creds, refresh_token).await.context(
        "failed to refresh cached RBX Identity credentials; run `robson-cli auth login` again",
    )?;
    save_credentials(&refreshed)?;
    Ok(refreshed.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    // XDG_CONFIG_HOME is process-wide. Hold this lock until the original
    // value is restored in every test that changes the credentials path.
    static CREDENTIALS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn make_id_token(payload_json: &str) -> String {
        use base64::Engine;
        let header =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{\"alg\":\"RS256\"}");
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        format!("{header}.{payload}.signature-not-checked-client-side")
    }

    #[test]
    fn decodes_email_from_optional_id_token_payload() {
        let token = make_id_token(r#"{"email":"owner@example.com"}"#);
        let claims = decode_id_token_claims(&token).unwrap();
        assert_eq!(claims.email.as_deref(), Some("owner@example.com"));
    }

    #[test]
    fn rejects_malformed_id_token() {
        assert!(decode_id_token_claims("not-a-jwt").is_err());
        assert!(decode_id_token_claims("only.two").is_err());
    }

    #[test]
    fn parses_public_client_configuration_without_a_secret() {
        let config = parse_client_config(
            "https://auth.merovelis.com/",
            " robson-cli ",
            "openid profile email offline_access urn:zitadel:iam:org:id:456 \
             urn:zitadel:iam:org:project:id:123:aud",
        )
        .unwrap();

        assert_eq!(config.issuer, "https://auth.merovelis.com");
        assert_eq!(config.client_id, "robson-cli");
        assert_eq!(config.token_url, "https://auth.merovelis.com/oauth/v2/token");
    }

    #[test]
    fn rejects_insecure_issuer_and_incomplete_scopes() {
        let complete_scopes = "openid offline_access urn:zitadel:iam:org:id:456 \
                               urn:zitadel:iam:org:project:id:123:aud";
        assert!(parse_client_config("http://auth.example", "robson-cli", complete_scopes).is_err());
        assert!(parse_client_config(
            "https://auth.example",
            "robson-cli",
            "profile offline_access urn:zitadel:iam:org:id:456 \
             urn:zitadel:iam:org:project:id:123:aud"
        )
        .is_err());
        assert!(parse_client_config(
            "https://auth.example",
            "robson-cli",
            "openid profile urn:zitadel:iam:org:id:456 \
             urn:zitadel:iam:org:project:id:123:aud"
        )
        .is_err());
        assert!(parse_client_config(
            "https://auth.example",
            "robson-cli",
            "openid offline_access urn:zitadel:iam:org:project:id:123:aud"
        )
        .is_err());
        assert!(parse_client_config(
            "https://auth.example",
            "robson-cli",
            "openid offline_access urn:zitadel:iam:org:id:456"
        )
        .is_err());
    }

    #[test]
    fn credentials_path_prefers_xdg_config_home() {
        let _env_guard = CREDENTIALS_ENV_LOCK.lock().unwrap();
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

        let _env_guard = CREDENTIALS_ENV_LOCK.lock().unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let dir =
            std::env::temp_dir().join(format!("robson-cli-test-perms-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        let path = credentials_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let creds = Credentials {
            issuer: "https://identity.example".to_owned(),
            client_id: "robson-cli".to_owned(),
            access_token: "test-access-token".to_owned(),
            refresh_token: None,
            email: Some("owner@example.com".to_owned()),
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

    #[cfg(unix)]
    #[test]
    fn load_credentials_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let _env_guard = CREDENTIALS_ENV_LOCK.lock().unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        let path = credentials_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let target = dir.path().join("redirected-credentials.json");
        fs::write(&target, "{}").unwrap();
        symlink(&target, &path).unwrap();

        let error = load_credentials().unwrap_err().to_string();

        match prev_xdg {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        assert!(error.contains("non-regular"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn save_credentials_is_atomic_and_private() {
        use std::os::unix::fs::PermissionsExt;

        let _env_guard = CREDENTIALS_ENV_LOCK.lock().unwrap();
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        let creds = Credentials {
            issuer: "https://identity.example".to_owned(),
            client_id: "robson-cli".to_owned(),
            access_token: "test-access-token".to_owned(),
            refresh_token: Some("test-refresh-token".to_owned()),
            email: Some("owner@example.com".to_owned()),
            expiry: 1_999_999_999,
        };

        save_credentials(&creds).unwrap();
        let path = credentials_path().unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let loaded: Credentials = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();

        match prev_xdg {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        assert_eq!(mode, 0o600);
        assert_eq!(loaded.refresh_token.as_deref(), Some("test-refresh-token"));
    }
}
