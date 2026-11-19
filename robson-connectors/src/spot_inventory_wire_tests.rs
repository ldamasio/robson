#![cfg(test)]
//! Test-only C8 loopback wire contract for the three signed Spot inventory
//! reads. The eventual owner will supply the real clock, budget and ledger.

use std::{future::Future, time::Duration};

use hmac::{Hmac, Mac};
use reqwest::{Client, StatusCode};
use sha2::Sha256;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};

use super::{single_shot, BinanceRestError};

const ACCOUNT: &str = include_str!("binance/spot/testdata/account.json");
const OPEN_ORDERS: &str = include_str!("binance/spot/testdata/open_orders.json");
const OPEN_ORDER_LIST: &str = include_str!("binance/spot/testdata/open_order_list.json");

const KEY: &str = "c8-synthetic-key";
const SECRET: &str = "c8-synthetic-secret";
const STAMP: i64 = 1_800_000_000_000;
const EXPECTED_SIGNATURE: &str = "5240291be8a1f9092fc18756b920e57afd279112d735c2e1100014969556c238";
const STEP: Duration = Duration::from_secs(5);
const OUTER: Duration = Duration::from_secs(20);
const QUIET: Duration = Duration::from_millis(250);

/// The closed set of inventory reads: fixed paths and weights, no parameter
/// but the caller's timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InventoryRead {
    Account,
    AllOpenOrders,
    OpenOrderLists,
}

impl InventoryRead {
    const ALL: [Self; 3] = [Self::Account, Self::AllOpenOrders, Self::OpenOrderLists];

    fn path(self) -> &'static str {
        match self {
            Self::Account => "/api/v3/account",
            Self::AllOpenOrders => "/api/v3/openOrders",
            Self::OpenOrderLists => "/api/v3/openOrderList",
        }
    }

    fn weight(self) -> u32 {
        match self {
            Self::Account => 20,
            Self::AllOpenOrders => 80,
            Self::OpenOrderLists => 6,
        }
    }
}

/// One read with its own single-shot transport; moved into [`Self::sign`].
struct PreparedRead {
    read: InventoryRead,
    transport: Client,
}

/// One signed request bound to its transport; moved into [`Self::send`].
struct SignedRead {
    transport: Client,
    request: reqwest::Request,
}

fn prepare(read: InventoryRead) -> Result<PreparedRead, BinanceRestError> {
    Ok(PreparedRead { read, transport: single_shot()? })
}

impl PreparedRead {
    /// The canonical HMAC GET of the commission binder, with the immutable
    /// caller timestamp as its only parameter.
    fn sign(self, port: u16, timestamp_ms: i64) -> Result<SignedRead, BinanceRestError> {
        if timestamp_ms <= 0 {
            return Err(BinanceRestError::Refused("invalid inventory timestamp".into()));
        }
        let endpoint = self.read.path();
        let query = format!("timestamp={timestamp_ms}");
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes())
            .map_err(|e| BinanceRestError::SignatureError(format!("HMAC error: {e}")))?;
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        let url = format!("http://127.0.0.1:{port}{endpoint}?{query}&signature={signature}",);
        let (transport, request) =
            self.transport.get(url).header("X-MBX-APIKEY", KEY).build_split();
        let request = request.map_err(|_| {
            BinanceRestError::Refused("inventory request construction failed".into())
        })?;
        Ok(SignedRead { transport, request })
    }
}

impl SignedRead {
    /// One request, sent when first polled; the original head, body unread.
    fn send(self) -> impl Future<Output = Result<reqwest::Response, BinanceRestError>> {
        let SignedRead { transport, request } = self;
        async move {
            transport
                .execute(request)
                .await
                .map_err(|e| BinanceRestError::RequestFailed(e.without_url().to_string()))
        }
    }
}

async fn bounded<F: Future>(test: F) -> F::Output {
    timeout(OUTER, test).await.expect("outer test deadline")
}

async fn local() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

async fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut buf = [0_u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0 && head.len() + n <= 8192, "incomplete test header");
        head.extend_from_slice(&buf[..n]);
    }
    String::from_utf8(head).unwrap()
}

async fn assert_quiet(listener: &TcpListener) {
    assert!(timeout(QUIET, listener.accept()).await.is_err(), "no further connection");
}

/// The expected signature is the known answer for the fixed key and timestamp.
fn expected_line(read: InventoryRead) -> String {
    format!("GET {}?timestamp={STAMP}&signature={EXPECTED_SIGNATURE} HTTP/1.1", read.path())
}

#[test]
fn the_inventory_reads_are_closed_with_fixed_paths_and_weights() {
    let reads = InventoryRead::ALL;
    assert_eq!(reads.map(InventoryRead::path), [
        "/api/v3/account",
        "/api/v3/openOrders",
        "/api/v3/openOrderList"
    ]);
    assert_eq!(reads.map(InventoryRead::weight), [20, 80, 6]);
    assert_eq!(reads.iter().map(|r| r.weight()).sum::<u32>(), 106);
}

#[tokio::test]
async fn each_read_sends_one_exact_signed_get_and_returns_its_raw_head() {
    bounded(async {
        let mut lines = Vec::new();
        for (read, payload) in [
            (InventoryRead::Account, ACCOUNT),
            (InventoryRead::AllOpenOrders, OPEN_ORDERS),
            (InventoryRead::OpenOrderLists, OPEN_ORDER_LIST),
        ] {
            let (listener, port) = local().await;
            let venue = async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let head = read_head(&mut stream).await;
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                stream.write_all(answer.as_bytes()).await.unwrap();
                drop(stream);
                assert_quiet(&listener).await;
                head
            };
            let receive = async {
                let response =
                    prepare(read).unwrap().sign(port, STAMP).unwrap().send().await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()["content-type"], "application/json");
                let body = response.bytes().await.unwrap();
                assert_eq!(&body[..], payload.as_bytes(), "{read:?} receipt is the fixture");
            };
            let (head, ()) = timeout(STEP, async { tokio::join!(venue, receive) }).await.unwrap();

            let line = head.lines().next().unwrap();
            assert_eq!(line, expected_line(read), "exact URI, no filters");
            let lower = head.to_ascii_lowercase();
            assert_eq!(lower.matches("\r\nx-mbx-apikey: ").count(), 1, "{head}");
            assert!(lower.contains(&format!("\r\nx-mbx-apikey: {KEY}\r\n")), "{head}");
            assert!(!lower.contains("content-length"), "a GET carries no body: {head}");
            assert!(!lower.contains(SECRET), "the secret never goes on the wire");
            lines.push(line.to_string());
        }
        assert_eq!(lines.len(), 3, "three GETs, one per read");
    })
    .await;
}

#[tokio::test]
async fn a_non_positive_stamp_is_refused_before_anything_is_sent() {
    bounded(async {
        let (listener, port) = local().await;
        for read in InventoryRead::ALL {
            for stamp in [0, -1, i64::MIN] {
                match prepare(read).unwrap().sign(port, stamp) {
                    Err(BinanceRestError::Refused(detail)) => {
                        assert_eq!(detail, "invalid inventory timestamp")
                    },
                    _ => panic!("{read:?} at {stamp} must be refused"),
                }
            }
        }
        assert_quiet(&listener).await;
    })
    .await;
}

#[tokio::test]
async fn a_dropped_unpolled_send_sends_nothing() {
    bounded(async {
        let (listener, port) = local().await;
        for read in InventoryRead::ALL {
            let sending = prepare(read).unwrap().sign(port, STAMP).unwrap().send();
            drop(sending);
        }
        assert_quiet(&listener).await;
    })
    .await;
}

/// Dropped once the request reached the venue, its answer withheld: the
/// connection closes, and nothing is sent again or left behind.
#[tokio::test]
async fn a_send_dropped_in_flight_closes_its_one_connection_and_never_repeats() {
    bounded(async {
        for read in InventoryRead::ALL {
            let (listener, port) = local().await;
            let (arrived, arrival) = oneshot::channel();
            let venue = async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let head = read_head(&mut stream).await;
                arrived.send(()).unwrap();
                let mut rest = [0_u8; 64];
                let closed = timeout(STEP, stream.read(&mut rest)).await.expect("closed");
                assert!(
                    matches!(&closed, Ok(0)) || matches!(
                        &closed,
                        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset
                    ),
                    "connection did not retire: {closed:?}"
                );
                assert_quiet(&listener).await;
                head
            };
            let cancel = async {
                let mut sending = Box::pin(prepare(read).unwrap().sign(port, STAMP).unwrap().send());
                tokio::select! {
                    biased;
                    answered = sending.as_mut() => panic!("answered before cancellation: {}", answered.is_ok()),
                    arrived = arrival => arrived.unwrap(),
                }
                drop(sending);
            };
            let (head, ()) = timeout(STEP, async { tokio::join!(venue, cancel) }).await.unwrap();
            assert_eq!(head.lines().next().unwrap(), expected_line(read));
        }
    })
    .await;
}

#[tokio::test]
async fn the_response_head_arrives_before_the_body() {
    bounded(async {
        let (listener, port) = local().await;
        let (release, released) = oneshot::channel();
        let venue = async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let head = read_head(&mut stream).await;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                ACCOUNT.len()
            );
            stream.write_all(reply.as_bytes()).await.unwrap();
            released.await.unwrap();
            stream.write_all(ACCOUNT.as_bytes()).await.unwrap();
            drop(stream);
            assert_quiet(&listener).await;
            assert_eq!(head.lines().next().unwrap(), expected_line(InventoryRead::Account));
        };
        let receive = async {
            let response = prepare(InventoryRead::Account)
                .unwrap()
                .sign(port, STAMP)
                .unwrap()
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            release.send(()).unwrap();
            assert_eq!(response.bytes().await.unwrap().as_ref(), ACCOUNT.as_bytes());
        };
        timeout(STEP, async { tokio::join!(venue, receive) }).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_redirect_is_the_answer_and_is_not_followed() {
    bounded(async {
        for status in [301_u16, 302, 303, 307, 308] {
            let (elsewhere, target_port) = local().await;
            let (listener, port) = local().await;
            let location = format!("http://127.0.0.1:{target_port}/api/v3/account?timestamp=1");
            let answered_location = location.clone();
            let venue = async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let head = read_head(&mut stream).await;
                let answer = format!(
                    "HTTP/1.1 {status} Moved\r\nLocation: {location}\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                );
                stream.write_all(answer.as_bytes()).await.unwrap();
                drop(stream);
                assert_quiet(&listener).await;
                head
            };
            let receive = async {
                let response = prepare(InventoryRead::Account)
                    .unwrap()
                    .sign(port, STAMP)
                    .unwrap()
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status().as_u16(), status);
                assert_eq!(response.headers()["location"], answered_location.as_str());
            };
            let (head, ()) = timeout(STEP, async { tokio::join!(venue, receive) }).await.unwrap();
            assert_eq!(head.lines().next().unwrap(), expected_line(InventoryRead::Account));
            assert_quiet(&elsewhere).await;
        }
    })
    .await;
}
