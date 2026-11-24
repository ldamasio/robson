use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::StreamExt;
use rand::RngCore;
use reqwest::{
    header::{HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, COOKIE, HOST},
    Method, StatusCode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;
use url::Url;
use zeroize::Zeroizing;

const CALLBACK_SCHEME: &str = "br.ia.rbx.robson";
const CALLBACK_HOST: &str = "oauth";
const CALLBACK_PATH: &str = "/callback";
const TRANSACTION_TTL: Duration = Duration::from_secs(10 * 60);
const NETWORK_TIMEOUT: Duration = Duration::from_secs(15);
const SSE_IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_IDENTITY_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_API_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_API_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_SSE_BUFFER_BYTES: usize = 256 * 1024;
const MAX_ACTIVE_STREAMS: usize = 4;

struct RuntimeState {
    auth: Arc<AuthState>,
    streams: StreamState,
}

impl RuntimeState {
    fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(NETWORK_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("RBX-Robson-Tauri/0.1")
            .build()
            .map_err(|_| "native HTTP client initialization failed".to_string())?;
        Ok(Self {
            auth: Arc::new(AuthState {
                client,
                inner: Mutex::new(AuthInner::default()),
            }),
            streams: StreamState::default(),
        })
    }
}

struct AuthState {
    client: reqwest::Client,
    inner: Mutex<AuthInner>,
}

#[derive(Default)]
struct AuthInner {
    transaction: Option<OidcTransaction>,
    session: Option<NativeSession>,
}

struct OidcTransaction {
    state: String,
    code_verifier: Zeroizing<String>,
    issuer: Url,
    token_endpoint: Url,
    redirect_uri: Url,
    client_id: String,
    api_base: Url,
    return_path: String,
    created_at: SystemTime,
}

struct NativeSession {
    access_token: Zeroizing<String>,
    expires_at_ms: u64,
    api_base: Url,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OidcConfigInput {
    issuer: String,
    client_id: String,
    redirect_uri: String,
    scopes: Vec<String>,
    org_id: String,
    project_id: String,
    google_idp_id: Option<String>,
    api_base: String,
}

struct ValidatedOidcConfig {
    issuer: Url,
    client_id: String,
    redirect_uri: Url,
    scopes: Vec<String>,
    api_base: Url,
}

#[derive(Deserialize)]
struct OidcDiscovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthSessionSnapshot {
    authenticated: bool,
    expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_path: Option<String>,
}

#[derive(Clone, Serialize)]
struct AuthErrorPayload {
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeHttpRequest {
    method: String,
    path: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeHttpResponse {
    status: u16,
    status_text: String,
    body: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamEventPayload {
    stream_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stale_secs: Option<u64>,
}

#[derive(Clone, Default)]
struct StreamState {
    active: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl AuthState {
    fn snapshot(&self) -> AuthSessionSnapshot {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner
            .session
            .as_ref()
            .is_some_and(|session| session.expires_at_ms <= now_epoch_ms())
        {
            inner.session = None;
        }
        AuthSessionSnapshot {
            authenticated: inner.session.is_some(),
            expires_at: inner.session.as_ref().map(|session| session.expires_at_ms),
            return_path: None,
        }
    }

    fn credentials(&self) -> Option<(Zeroizing<String>, Url)> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner
            .session
            .as_ref()
            .is_some_and(|session| session.expires_at_ms <= now_epoch_ms())
        {
            inner.session = None;
            return None;
        }
        inner.session.as_ref().map(|session| {
            (Zeroizing::new(session.access_token.to_string()), session.api_base.clone())
        })
    }

    fn clear_session(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.session = None;
        inner.transaction = None;
    }
}

#[tauri::command]
async fn begin_oidc_login(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    config: OidcConfigInput,
    return_path: String,
) -> Result<(), String> {
    let config = validate_oidc_config(config)?;
    let discovery = discover(&runtime.auth.client, &config).await?;
    let authorization_endpoint = validate_identity_endpoint(
        &config.issuer,
        &discovery.authorization_endpoint,
        "authorization endpoint",
    )?;
    let token_endpoint =
        validate_identity_endpoint(&config.issuer, &discovery.token_endpoint, "token endpoint")?;

    let state = random_base64_url(32);
    let code_verifier = Zeroizing::new(random_base64_url(64));
    let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
    let authorization_url =
        build_authorization_url(authorization_endpoint, &config, &state, &code_challenge);

    {
        let mut inner = runtime.auth.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.transaction = Some(OidcTransaction {
            state,
            code_verifier,
            issuer: config.issuer,
            token_endpoint,
            redirect_uri: config.redirect_uri,
            client_id: config.client_id,
            api_base: config.api_base,
            return_path: sanitize_return_path(&return_path),
            created_at: SystemTime::now(),
        });
    }

    if app.opener().open_url(authorization_url.as_str(), None::<&str>).is_err() {
        let mut inner = runtime.auth.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.transaction = None;
        return Err("system browser could not be opened".to_string());
    }
    Ok(())
}

#[tauri::command]
fn native_auth_session(runtime: State<'_, RuntimeState>) -> AuthSessionSnapshot {
    runtime.auth.snapshot()
}

#[tauri::command]
fn native_logout(runtime: State<'_, RuntimeState>) {
    runtime.auth.clear_session();
}

#[tauri::command]
async fn native_api_request(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    request: NativeHttpRequest,
) -> Result<NativeHttpResponse, String> {
    let Some((access_token, api_base)) = runtime.auth.credentials() else {
        emit_auth_expired(&app);
        return Ok(unauthorized_response());
    };
    let method = Method::from_bytes(request.method.to_uppercase().as_bytes())
        .map_err(|_| "unsupported HTTP method".to_string())?;
    if !matches!(
        method,
        Method::GET | Method::POST | Method::PUT | Method::PATCH | Method::DELETE | Method::HEAD
    ) {
        return Err("unsupported HTTP method".to_string());
    }
    let url = api_url(&api_base, &request.path)?;
    if request.body.as_ref().is_some_and(|body| body.len() > MAX_API_REQUEST_BYTES) {
        return Err("API request body is too large".to_string());
    }

    let mut builder = runtime.auth.client.request(method, url).bearer_auth(access_token.as_str());
    let mut has_content_type = false;
    for (name, value) in request.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| "API request contains an invalid header".to_string())?;
        if matches!(name, AUTHORIZATION | COOKIE | HOST) {
            return Err("API request contains a forbidden header".to_string());
        }
        if name == CONTENT_TYPE {
            has_content_type = true;
        }
        let value = HeaderValue::from_str(&value)
            .map_err(|_| "API request contains an invalid header".to_string())?;
        builder = builder.header(name, value);
    }
    if let Some(body) = request.body {
        if !has_content_type {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }
        builder = builder.body(body);
    }

    let response = builder.send().await.map_err(|_| "Robson API request failed".to_string())?;
    let status = response.status();
    let body = read_limited(response, MAX_API_RESPONSE_BYTES).await?;
    if status == StatusCode::UNAUTHORIZED {
        runtime.auth.clear_session();
        emit_auth_expired(&app);
    }
    Ok(NativeHttpResponse {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("Unknown").to_string(),
        body: String::from_utf8(body)
            .map_err(|_| "Robson API returned non-UTF-8 data".to_string())?,
    })
}

#[tauri::command]
fn native_start_event_stream(
    app: AppHandle,
    runtime: State<'_, RuntimeState>,
    stream_id: String,
) -> Result<(), String> {
    if stream_id.is_empty() || stream_id.len() > 128 {
        return Err("invalid stream id".to_string());
    }
    let cancellation = CancellationToken::new();
    {
        let mut active = runtime.streams.active.lock().unwrap_or_else(|error| error.into_inner());
        if active.contains_key(&stream_id) {
            return Err("event stream already exists".to_string());
        }
        if active.len() >= MAX_ACTIVE_STREAMS {
            return Err("too many active event streams".to_string());
        }
        active.insert(stream_id.clone(), cancellation.clone());
    }

    let auth = runtime.auth.clone();
    let active = runtime.streams.active.clone();
    tauri::async_runtime::spawn(async move {
        run_event_stream(&app, &auth, &stream_id, &cancellation).await;
        let mut streams = active.lock().unwrap_or_else(|error| error.into_inner());
        streams.remove(&stream_id);
    });
    Ok(())
}

#[tauri::command]
fn native_stop_event_stream(runtime: State<'_, RuntimeState>, stream_id: String) {
    let cancellation = runtime
        .streams
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&stream_id);
    if let Some(cancellation) = cancellation {
        cancellation.cancel();
    }
}

async fn handle_oidc_callback(
    app: AppHandle,
    auth: Arc<AuthState>,
    callback: Url,
) -> Result<(), String> {
    validate_callback_target(&callback)?;
    let received_state = callback
        .query_pairs()
        .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
        .ok_or_else(|| "RBX Identity callback is missing state".to_string())?;

    let transaction = {
        let mut inner = auth.inner.lock().unwrap_or_else(|error| error.into_inner());
        let transaction = inner
            .transaction
            .as_ref()
            .ok_or_else(|| "RBX Identity login transaction was not found".to_string())?;
        if transaction.state != received_state {
            return Err("RBX Identity state validation failed".to_string());
        }
        inner.transaction.take().expect("transaction checked above")
    };

    if SystemTime::now().duration_since(transaction.created_at).unwrap_or_default()
        > TRANSACTION_TTL
    {
        return Err("RBX Identity login expired; start again".to_string());
    }
    if callback.scheme() != transaction.redirect_uri.scheme()
        || callback.host_str() != transaction.redirect_uri.host_str()
        || callback.path() != transaction.redirect_uri.path()
    {
        return Err("RBX Identity callback does not match this application".to_string());
    }
    if let Some(response_issuer) = callback
        .query_pairs()
        .find_map(|(key, value)| (key == "iss").then(|| value.into_owned()))
    {
        if response_issuer.trim_end_matches('/')
            != transaction.issuer.as_str().trim_end_matches('/')
        {
            return Err("RBX Identity issuer validation failed".to_string());
        }
    }
    if let Some(provider_error) = callback
        .query_pairs()
        .find_map(|(key, value)| (key == "error").then(|| value.into_owned()))
    {
        return Err(format!(
            "RBX Identity rejected the login ({})",
            sanitize_provider_error(&provider_error)
        ));
    }
    let code = callback
        .query_pairs()
        .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
        .ok_or_else(|| "RBX Identity callback is missing the code".to_string())?;

    let response = auth
        .client
        .post(transaction.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", transaction.client_id.as_str()),
            ("code", code.as_str()),
            ("code_verifier", transaction.code_verifier.as_str()),
            ("redirect_uri", transaction.redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|_| "RBX Identity token exchange failed".to_string())?;
    if !response.status().is_success() {
        return Err("RBX Identity token exchange failed".to_string());
    }
    let bytes = read_limited(response, MAX_IDENTITY_RESPONSE_BYTES).await?;
    let token: TokenResponse = serde_json::from_slice(&bytes)
        .map_err(|_| "RBX Identity returned an invalid token response".to_string())?;
    if token.access_token.is_empty()
        || !token.token_type.eq_ignore_ascii_case("bearer")
        || token.expires_in == 0
    {
        return Err("RBX Identity returned an invalid token response".to_string());
    }
    let expires_at_ms = now_epoch_ms()
        .checked_add(
            token
                .expires_in
                .checked_mul(1_000)
                .ok_or_else(|| "RBX Identity token lifetime is invalid".to_string())?,
        )
        .ok_or_else(|| "RBX Identity token lifetime is invalid".to_string())?;

    {
        let mut inner = auth.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.session = Some(NativeSession {
            access_token: Zeroizing::new(token.access_token),
            expires_at_ms,
            api_base: transaction.api_base,
        });
    }

    let _ = app.emit(
        "rbx://auth-session",
        AuthSessionSnapshot {
            authenticated: true,
            expires_at: Some(expires_at_ms),
            return_path: Some(transaction.return_path),
        },
    );
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(())
}

async fn run_event_stream(
    app: &AppHandle,
    auth: &AuthState,
    stream_id: &str,
    cancellation: &CancellationToken,
) {
    let mut retries = 0_u32;
    loop {
        let Some((access_token, api_base)) = auth.credentials() else {
            emit_auth_expired(app);
            return;
        };
        let Ok(url) = api_url(&api_base, "/events") else {
            emit_stream(app, "rbx://sse-error", stream_id, None, None);
            return;
        };
        let request = auth
            .client
            .get(url)
            .header(ACCEPT, "text/event-stream")
            .bearer_auth(access_token.as_str());
        let response = tokio::select! {
            _ = cancellation.cancelled() => return,
            result = request.send() => result,
        };
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                emit_stream(app, "rbx://sse-error", stream_id, None, None);
                if !wait_for_retry(cancellation, &mut retries).await {
                    return;
                }
                continue;
            },
        };
        if response.status() == StatusCode::UNAUTHORIZED {
            auth.clear_session();
            emit_auth_expired(app);
            emit_stream(app, "rbx://sse-error", stream_id, None, None);
            return;
        }
        if !response.status().is_success() {
            emit_stream(app, "rbx://sse-error", stream_id, None, None);
            if !wait_for_retry(cancellation, &mut retries).await {
                return;
            }
            continue;
        }

        if retries > 0 {
            emit_stream(app, "rbx://sse-reconnect", stream_id, None, None);
        }
        retries = 0;
        let mut body = response.bytes_stream();
        let mut buffer = Vec::new();
        let reconnect;
        loop {
            let next = tokio::select! {
                _ = cancellation.cancelled() => return,
                result = tokio::time::timeout(SSE_IDLE_TIMEOUT, body.next()) => result,
            };
            match next {
                Err(_) => {
                    emit_stream(
                        app,
                        "rbx://sse-stale",
                        stream_id,
                        None,
                        Some(SSE_IDLE_TIMEOUT.as_secs()),
                    );
                    emit_stream(app, "rbx://sse-error", stream_id, None, None);
                    reconnect = true;
                    break;
                },
                Ok(Some(Ok(chunk))) => {
                    emit_stream(app, "rbx://sse-activity", stream_id, None, None);
                    if buffer.len().saturating_add(chunk.len()) > MAX_SSE_BUFFER_BYTES {
                        emit_stream(app, "rbx://sse-error", stream_id, None, None);
                        reconnect = true;
                        break;
                    }
                    buffer.extend_from_slice(&chunk);
                    dispatch_sse_blocks(app, stream_id, &mut buffer);
                },
                Ok(Some(Err(_))) | Ok(None) => {
                    emit_stream(app, "rbx://sse-error", stream_id, None, None);
                    reconnect = true;
                    break;
                },
            }
        }
        if reconnect && !wait_for_retry(cancellation, &mut retries).await {
            return;
        }
    }
}

fn dispatch_sse_blocks(app: &AppHandle, stream_id: &str, buffer: &mut Vec<u8>) {
    while let Some((boundary, delimiter_len)) = find_sse_boundary(buffer) {
        let block = buffer[..boundary].to_vec();
        buffer.drain(..boundary + delimiter_len);
        let text = String::from_utf8_lossy(&block);
        let mut data = String::new();
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        if !data.is_empty() {
            emit_stream(app, "rbx://sse-message", stream_id, Some(data), None);
        }
    }
}

fn find_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len().saturating_sub(1) {
        if buffer[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
        if buffer[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
    }
    None
}

async fn wait_for_retry(cancellation: &CancellationToken, retries: &mut u32) -> bool {
    let exponent = (*retries).min(5);
    let delay = Duration::from_secs((1_u64 << exponent).min(30));
    *retries = retries.saturating_add(1);
    tokio::select! {
        _ = cancellation.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

fn emit_stream(
    app: &AppHandle,
    event: &str,
    stream_id: &str,
    data: Option<String>,
    stale_secs: Option<u64>,
) {
    let _ = app.emit(
        event,
        StreamEventPayload {
            stream_id: stream_id.to_string(),
            data,
            stale_secs,
        },
    );
}

async fn discover(
    client: &reqwest::Client,
    config: &ValidatedOidcConfig,
) -> Result<OidcDiscovery, String> {
    let discovery_url = config
        .issuer
        .join(".well-known/openid-configuration")
        .map_err(|_| "RBX Identity issuer is invalid".to_string())?;
    let response = client
        .get(discovery_url)
        .send()
        .await
        .map_err(|_| "RBX Identity discovery failed".to_string())?;
    if !response.status().is_success() {
        return Err("RBX Identity discovery failed".to_string());
    }
    let bytes = read_limited(response, MAX_IDENTITY_RESPONSE_BYTES).await?;
    let discovery: OidcDiscovery = serde_json::from_slice(&bytes)
        .map_err(|_| "RBX Identity discovery response is invalid".to_string())?;
    if discovery.issuer.trim_end_matches('/') != config.issuer.as_str().trim_end_matches('/') {
        return Err("RBX Identity discovery issuer does not match".to_string());
    }
    Ok(discovery)
}

async fn read_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|size| size > limit as u64) {
        return Err("remote response is too large".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "remote response could not be read".to_string())?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err("remote response is too large".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_oidc_config(input: OidcConfigInput) -> Result<ValidatedOidcConfig, String> {
    let issuer = parse_https_url(&input.issuer, "RBX Identity issuer")?;
    if input.client_id.trim().is_empty() || input.client_id.len() > 256 {
        return Err("RBX Identity client id is invalid".to_string());
    }
    let redirect_uri = Url::parse(&input.redirect_uri)
        .map_err(|_| "RBX Identity redirect URI is invalid".to_string())?;
    validate_callback_target(&redirect_uri)?;
    if input.scopes.is_empty()
        || input.scopes.len() > 32
        || !input.scopes.iter().any(|scope| scope == "openid")
        || input
            .scopes
            .iter()
            .any(|scope| scope.is_empty() || scope.chars().any(char::is_whitespace))
    {
        return Err("RBX Identity scopes are invalid".to_string());
    }
    validate_resource_id(&input.org_id, "organization id")?;
    validate_resource_id(&input.project_id, "project id")?;
    if let Some(google_idp_id) = input.google_idp_id.as_deref() {
        validate_resource_id(google_idp_id, "Google IdP id")?;
    }
    let mut scopes = input.scopes;
    scopes.push(format!("urn:zitadel:iam:org:id:{}", input.org_id));
    scopes.push(format!("urn:zitadel:iam:org:project:id:{}:aud", input.project_id));
    scopes.push(format!("urn:zitadel:iam:org:roles:id:{}", input.org_id));
    if let Some(google_idp_id) = input.google_idp_id {
        scopes.push(format!("urn:zitadel:iam:org:idp:id:{google_idp_id}"));
    }
    scopes.sort();
    scopes.dedup();
    let api_base = parse_api_base(&input.api_base)?;
    Ok(ValidatedOidcConfig {
        issuer,
        client_id: input.client_id,
        redirect_uri,
        scopes,
        api_base,
    })
}

fn validate_resource_id(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 32
        || !value.bytes().all(|character| character.is_ascii_digit())
    {
        return Err(format!("RBX Identity {label} is invalid"));
    }
    Ok(())
}

fn parse_https_url(value: &str, label: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| format!("{label} is invalid"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!("{label} must be a credential-free HTTPS URL"));
    }
    Ok(url)
}

fn parse_api_base(value: &str) -> Result<Url, String> {
    let mut url = Url::parse(value).map_err(|_| "Robson API base URL is invalid".to_string())?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Robson API base must be a credential-free HTTPS URL".to_string());
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn validate_identity_endpoint(issuer: &Url, value: &str, label: &str) -> Result<Url, String> {
    let endpoint = parse_https_url(value, label)?;
    if endpoint.origin() != issuer.origin() {
        return Err(format!("RBX Identity {label} must use the issuer origin"));
    }
    Ok(endpoint)
}

fn validate_callback_target(url: &Url) -> Result<(), String> {
    if url.scheme() != CALLBACK_SCHEME
        || url.host_str() != Some(CALLBACK_HOST)
        || url.path() != CALLBACK_PATH
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("RBX Identity callback does not match this application".to_string());
    }
    Ok(())
}

fn build_authorization_url(
    mut endpoint: Url,
    config: &ValidatedOidcConfig,
    state: &str,
    code_challenge: &str,
) -> Url {
    endpoint
        .query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", config.redirect_uri.as_str())
        .append_pair("response_type", "code")
        .append_pair("scope", &config.scopes.join(" "))
        .append_pair("state", state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256");
    endpoint
}

fn api_url(base: &Url, path: &str) -> Result<Url, String> {
    if !path.starts_with('/') || path.starts_with("//") {
        return Err("Robson API path is invalid".to_string());
    }
    let url = base
        .join(path.trim_start_matches('/'))
        .map_err(|_| "Robson API path is invalid".to_string())?;
    if url.origin() != base.origin() {
        return Err("Robson API path changed origin".to_string());
    }
    Ok(url)
}

fn random_base64_url(size: usize) -> String {
    let mut bytes = vec![0_u8; size];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sanitize_return_path(value: &str) -> String {
    if value.starts_with('/') && !value.starts_with("//") {
        value.to_string()
    } else {
        "/dashboard".to_string()
    }
}

fn sanitize_provider_error(value: &str) -> &str {
    if value.len() <= 64
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
    {
        value
    } else {
        "provider_error"
    }
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn unauthorized_response() -> NativeHttpResponse {
    NativeHttpResponse {
        status: 401,
        status_text: "Unauthorized".to_string(),
        body: "authentication required".to_string(),
    }
}

fn emit_auth_expired(app: &AppHandle) {
    let _ = app.emit("rbx://auth-expired", ());
}

fn dispatch_deep_links(app: &AppHandle, auth: Arc<AuthState>, urls: Vec<Url>) {
    for url in urls {
        if validate_callback_target(&url).is_err() {
            continue;
        }
        let app = app.clone();
        let auth = auth.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(message) = handle_oidc_callback(app.clone(), auth, url).await {
                let _ = app.emit("rbx://auth-error", AuthErrorPayload { message });
            }
        });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let runtime = RuntimeState::new().expect("native runtime state must initialize");
    tauri::Builder::default()
        .manage(runtime)
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init())
        .invoke_handler(tauri::generate_handler![
            begin_oidc_login,
            native_auth_session,
            native_logout,
            native_api_request,
            native_start_event_stream,
            native_stop_event_stream,
        ])
        .setup(|app| {
            #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
            app.deep_link().register_all()?;

            let auth = app.state::<RuntimeState>().auth.clone();
            if let Some(urls) = app.deep_link().get_current()? {
                dispatch_deep_links(app.handle(), auth.clone(), urls);
            }
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                dispatch_deep_links(&handle, auth.clone(), event.urls());
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Robson");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> OidcConfigInput {
        OidcConfigInput {
            issuer: "https://auth.example.com".to_string(),
            client_id: "robson-native".to_string(),
            redirect_uri: "br.ia.rbx.robson://oauth/callback".to_string(),
            scopes: vec!["openid".to_string(), "profile".to_string()],
            org_id: "375992756025229511".to_string(),
            project_id: "123456789012345678".to_string(),
            google_idp_id: None,
            api_base: "https://robson.example.com".to_string(),
        }
    }

    #[test]
    fn builds_public_client_authorization_without_secret() {
        let config = validate_oidc_config(valid_config()).unwrap();
        let url = build_authorization_url(
            Url::parse("https://auth.example.com/oauth/v2/authorize").unwrap(),
            &config,
            "state",
            "challenge",
        );
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(query.get("code_challenge_method").map(String::as_str), Some("S256"));
        assert_eq!(query.get("code_challenge").map(String::as_str), Some("challenge"));
        assert!(!query.contains_key("client_secret"));
        let scopes = query.get("scope").unwrap();
        assert!(scopes.contains("urn:zitadel:iam:org:id:375992756025229511"));
        assert!(scopes.contains("urn:zitadel:iam:org:project:id:123456789012345678:aud"));
    }

    #[test]
    fn rejects_insecure_identity_issuers() {
        let mut insecure = valid_config();
        insecure.issuer = "http://auth.example.com".to_string();
        assert!(validate_oidc_config(insecure).is_err());
    }

    #[test]
    fn accepts_only_the_registered_callback_target() {
        assert!(validate_callback_target(
            &Url::parse("br.ia.rbx.robson://oauth/callback?code=x&state=y").unwrap()
        )
        .is_ok());
        assert!(validate_callback_target(
            &Url::parse("br.ia.rbx.robson://oauth/other?code=x&state=y").unwrap()
        )
        .is_err());
        assert!(validate_callback_target(
            &Url::parse("https://attacker.example/callback?code=x&state=y").unwrap()
        )
        .is_err());
    }

    #[test]
    fn keeps_api_requests_on_the_configured_origin() {
        let base = parse_api_base("https://robson.example.com/api").unwrap();
        assert_eq!(
            api_url(&base, "/status").unwrap().as_str(),
            "https://robson.example.com/api/status"
        );
        assert!(api_url(&base, "//attacker.example/status").is_err());
        assert!(api_url(&base, "https://attacker.example/status").is_err());
    }

    #[test]
    fn parses_lf_and_crlf_sse_boundaries() {
        assert_eq!(find_sse_boundary(b"data: one\n\nnext"), Some((9, 2)));
        assert_eq!(find_sse_boundary(b"data: one\r\n\r\nnext"), Some((9, 4)));
        assert_eq!(find_sse_boundary(b"data: one"), None);
    }

    #[test]
    fn rejects_external_return_paths() {
        assert_eq!(sanitize_return_path("/dashboard"), "/dashboard");
        assert_eq!(sanitize_return_path("//attacker.example"), "/dashboard");
        assert_eq!(sanitize_return_path("https://attacker.example"), "/dashboard");
    }
}
