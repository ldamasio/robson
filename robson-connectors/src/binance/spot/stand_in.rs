//! A local stand-in for Binance's spot REST API, for the wire tests of the
//! reads (8a) and of the adapter (8c): it records each request and answers it
//! as the test says. Nothing reaches the network.
//!
//! Like Binance, it checks every signed request's signature over the query
//! exactly as it arrived on the wire (spot `rest-api.md` L344-L345,
//! L388-L417), with the test client's secret, and answers a mismatch with
//! HTTP 400 and -1022 (`errors.md` L75-L77) instead of the test's answer.

use std::sync::{Arc, Mutex};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use crate::binance_rest::BinanceRestClient;

/// The API secret of `BinanceRestClient::with_local_base_url`: test-only.
const TEST_SECRET: &[u8] = b"secret";

/// Binance's answer to a signature that does not match (`errors.md` L75-L77).
pub(super) const INVALID_SIGNATURE: &str =
    "{\"code\":-1022,\"msg\":\"Signature for this request is not valid.\"}";

/// Whether `query`, as received, ends in a signature of everything before
/// it, as `build_signed_query` makes one; a query with no signature passes.
fn signature_matches(query: &str) -> bool {
    use hmac::{Hmac, Mac};
    let Some((payload, signature)) = query.rsplit_once("&signature=") else {
        return !query.contains("signature=");
    };
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(TEST_SECRET).unwrap();
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes()) == signature
}

/// One request as the stand-in venue saw it.
#[derive(Debug, Clone)]
pub(super) struct Seen {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) params: Vec<(String, String)>,
    pub(super) api_key: bool,
    /// The query string as it arrived, percent-encoding and all.
    pub(super) query: String,
    /// Whether its signature, if any, matched the query as it arrived.
    pub(super) signature_valid: bool,
}

impl Seen {
    pub(super) fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Method, path and parameters without the signature's, sorted.
    pub(super) fn identity(&self) -> String {
        let mut params: Vec<String> = self
            .params
            .iter()
            .filter(|(k, _)| k != "timestamp" && k != "signature")
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        params.sort();
        if params.is_empty() {
            format!("{} {}", self.method, self.path)
        } else {
            format!("{} {}?{}", self.method, self.path, params.join("&"))
        }
    }
}

pub(super) type Seens = Arc<Mutex<Vec<Seen>>>;

/// A stand-in venue answering each request with `reply`'s status and body,
/// and a real REST client pointed at it.
pub(super) async fn serve(
    reply: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static,
) -> (Arc<BinanceRestClient>, Seens) {
    serve_with_headers(move |request| {
        let (status, body) = reply(request);
        (status, String::new(), body)
    })
    .await
}

/// A stand-in answering every request with `status` and a `Location` of
/// `location`: a redirect.
pub(super) async fn redirecting(status: u16, location: String) -> (Arc<BinanceRestClient>, Seens) {
    serve_with_headers(move |_| (status, format!("Location: {location}\r\n"), String::new())).await
}

/// [`serve`] without a client: the stand-in's own URL, for a test that
/// points something else at it.
pub(super) async fn listening(
    reply: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static,
) -> (String, Seens) {
    listen(move |request| {
        let (status, body) = reply(request);
        (status, String::new(), body)
    })
    .await
}

/// [`serve`], with `reply` giving header lines too, each ending in CRLF.
async fn serve_with_headers(
    reply: impl Fn(&Seen) -> (u16, String, String) + Send + Sync + 'static,
) -> (Arc<BinanceRestClient>, Seens) {
    let (url, seen) = listen(reply).await;
    (Arc::new(BinanceRestClient::with_local_base_url(url)), seen)
}

async fn listen(
    reply: impl Fn(&Seen) -> (u16, String, String) + Send + Sync + 'static,
) -> (String, Seens) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let reply = Arc::new(reply);
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut buf = [0u8; 4096];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let mut lines = head.lines();
            let mut first = lines.next().unwrap_or_default().split_whitespace();
            let (method, target) =
                (first.next().unwrap_or_default(), first.next().unwrap_or_default());
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            let request = Seen {
                method: method.to_string(),
                path: path.to_string(),
                params: query
                    .split('&')
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        let (k, v) = part.split_once('=').unwrap_or((part, ""));
                        (k.to_string(), v.to_string())
                    })
                    .collect(),
                api_key: lines.any(|line| line.to_ascii_lowercase().starts_with("x-mbx-apikey:")),
                query: query.to_string(),
                signature_valid: signature_matches(query),
            };
            recorded.lock().unwrap().push(request.clone());

            let (status, headers, body) = if request.signature_valid {
                reply(&request)
            } else {
                (400, String::new(), INVALID_SIGNATURE.to_string())
            };
            let response = format!(
                "HTTP/1.1 {status} Answer\r\nContent-Type: application/json\r\n{headers}\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    (url, seen)
}

/// A venue that takes each connection and closes it unanswered: a request to
/// it fails in transport. It stays bound for the whole test, so no other
/// test's stand-in can be given its port, as a dropped listener's port can
/// be in a parallel test binary.
pub(super) async fn closing() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            drop(stream);
        }
    });
    url
}
