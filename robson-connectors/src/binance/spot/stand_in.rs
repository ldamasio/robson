//! A local stand-in for Binance's spot REST API, for the wire tests of the
//! reads (8a) and of the adapter (8c): it records each request and answers it
//! as the test says. Nothing reaches the network.

use std::sync::{Arc, Mutex};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use crate::binance_rest::BinanceRestClient;

/// One request as the stand-in venue saw it.
#[derive(Debug, Clone)]
pub(super) struct Seen {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) params: Vec<(String, String)>,
    pub(super) api_key: bool,
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
            };
            recorded.lock().unwrap().push(request.clone());

            let (status, body) = reply(&request);
            let response = format!(
                "HTTP/1.1 {status} Answer\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    (Arc::new(BinanceRestClient::with_local_base_url(url)), seen)
}
