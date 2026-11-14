//! A local stand-in for Binance's spot REST API over a [`SpotBook`] (ADR-0056
//! step 8, 8d plan §2.3, §7.4): the real spot adapter, with the real REST
//! client, talks to it over loopback in the conformance run S.
//!
//! - **Routes**, Binance spot's shapes at `995f6eb3`: `ping`, `exchangeInfo`,
//!   `ticker/price` (public); `account`, `openOrders`, `openOrderList`, `GET
//!   order`, `myTrades`, `POST order` and `DELETE order` (signed). Any other
//!   route is answered HTTP 400 and recorded in
//!   [`FakeSpotVenue::unserved_requests`] before anything else happens to it;
//!   holds and scripts are refused for it.
//! - **Placements** are validated before anything acts: `side`, `type`,
//!   `quantity` and a STOP_LOSS's `stopPrice` are mandatory (`-1102`); an
//!   invalid side is `-1117`, an unknown type `-1116`, a `stopPrice` on a
//!   MARKET order `-1106`, a response type or STP mode that is no such value
//!   `-1100`, an STP mode the symbol does not allow `-1013` (stp_faq.md
//!   L170-L180; the other pairings with HTTP 400 are synthetic). What the fake
//!   does not model (other order types, a BUY stop, `quoteOrderQty`,
//!   `trailingDelta`, no `newClientOrderId`) is unserved. The answer follows
//!   `newOrderRespType` (MARKET defaults to `FULL`, STOP_LOSS to `ACK`,
//!   rest-api.md L2166) and echoes the STP mode requested (the symbol's default
//!   `EXPIRE_MAKER` when none was), which later records of the order carry too.
//! - **Authentication** on signed routes: no `X-MBX-APIKEY` is `-2014`
//!   (synthetic: the docs give no answer for it), a value that is not a key's
//!   format `-2014` (errors.md L439-L441), a well-formed foreign key `-2015`
//!   (L443-L445), each at HTTP 401 (the status is synthetic); no `signature` is
//!   `-1102` (L107), and a signature that is not the HMAC-SHA256, in hex, of
//!   the encoded query exactly as received before `&signature=`, compared
//!   case-insensitively (rest-api.md L292-L294, L344-L345), is `-1022`
//!   (errors.md L75-L77), each at HTTP 400.
//! - **Answers:** the book's refusals are Binance's JSON at HTTP 400; a lost
//!   placement and a delayed cancel are HTTP 503 `-1007` after the book acted
//!   or queued (errors.md L39-L41; the pairing is synthetic).
//! - **Holds:** [`FakeSpotVenue::hold_next`] reads, authenticates and records a
//!   request, then answers it only at shutdown, HTTP 503: a real timeout for
//!   the client's own bound.
//! - **Shutdown** ([`FakeSpotVenue::shutdown`]): graceful, waiting for every
//!   connection task (axum 0.7.9 spawns them detached); a held handler answers
//!   on the stop signal; a drain past 5 s is an error, never a teardown.
//!   [`FakeSpotVenue::live`] counts open connections and running handlers.
//!
//! Test support: no production crate depends on this crate.

use std::{
    collections::{BTreeMap, VecDeque},
    convert::Infallible,
    future::{ready, Ready},
    io,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard,
    },
    task::{Context, Poll},
    time::Duration,
};

use anyhow::{bail, Result};
use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    serve::IncomingStream,
    Extension, Router,
};
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::{net::TcpListener, sync::watch, task::JoinHandle};
use tower_service::Service;

use crate::{
    balance_stub::PlacementHook,
    spot_book::{
        BookError, BookOrder, BookRules, BookTrade, OrderKind, OrderRef, Side, SpotBook,
        MAX_RETAINED_ORDERS,
    },
};

/// How long a shutdown waits for the drain.
pub const SHUTDOWN_BOUND: Duration = Duration::from_secs(5);
/// Requests kept in the log; older ones are dropped, the total counts all.
pub const REQUEST_LOG: usize = 4_096;
/// Wire scripts queued at once.
pub const WIRE_SCRIPTS: usize = 64;
/// Holds queued at once.
pub const HOLDS: usize = 64;
/// Unserved requests kept; [`FakeSpotVenue::unserved_total`] counts all.
pub const UNSERVED_KEPT: usize = 64;
/// The symbols' self-trade prevention modes: BTCUSDT's at `995f6eb3`, the
/// first its default (`exchange_info_btcusdt.json`).
pub const STP_MODES: [&str; 5] = [
    "EXPIRE_MAKER",
    "EXPIRE_TAKER",
    "EXPIRE_BOTH",
    "DECREMENT",
    "TRANSFER",
];
/// Every STP mode Binance knows (enums.md L174-L184); one a symbol does not
/// allow is `-1013` (stp_faq.md L170-L180).
pub const STP_ENUM: [&str; 6] = [
    "NONE",
    "EXPIRE_MAKER",
    "EXPIRE_TAKER",
    "EXPIRE_BOTH",
    "DECREMENT",
    "TRANSFER",
];

/// The venue's API key and secret: test values, 64 alphanumerics each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestCredentials {
    pub api_key: &'static str,
    pub api_secret: &'static str,
}

impl TestCredentials {
    pub const FIXED: Self = Self {
        api_key: "TestKey000000000000000000000000000000000000000000000000000000000",
        api_secret: "TestSecret000000000000000000000000000000000000000000000000000000",
    };
}

/// A signature's verdict on one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureVerdict {
    /// A public route: no signature asked.
    Unsigned,
    /// The key was refused first: the signature was not checked.
    NotChecked,
    Valid,
    Missing,
    Invalid,
}

/// One request as the fake saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireRequest {
    pub method: String,
    pub path: String,
    /// The query exactly as received, percent-encoding and all.
    pub query: String,
    pub api_key: bool,
    pub signature: SignatureVerdict,
}

#[derive(Default)]
struct Live {
    connections: AtomicUsize,
    handlers: AtomicUsize,
    held: AtomicUsize,
}

/// The venue's live counts, readable after its shutdown.
#[derive(Clone)]
pub struct Counters(Arc<Live>);

impl Counters {
    /// Open connections and running handlers.
    pub fn live(&self) -> (usize, usize) {
        (
            self.0.connections.load(Ordering::SeqCst),
            self.0.handlers.load(Ordering::SeqCst),
        )
    }

    /// Handlers waiting on a hold.
    pub fn held(&self) -> usize {
        self.0.held.load(Ordering::SeqCst)
    }
}

/// Counted while alive.
struct Counted(Arc<Live>, fn(&Live) -> &AtomicUsize);

impl Counted {
    fn new(live: &Arc<Live>, which: fn(&Live) -> &AtomicUsize) -> Self {
        which(live).fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(live), which)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        (self.1)(&self.0).fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Wire {
    requests: VecDeque<WireRequest>,
    total: u64,
    weight: u32,
    unserved: Vec<String>,
    unserved_total: u64,
    /// The STP mode each order was placed with, by order id, kept while
    /// the book retains the order.
    stp: BTreeMap<u64, String>,
    holds: VecDeque<(Method, String)>,
    scripts: VecDeque<(Method, String, u16, String)>,
    next_placement: Option<PlacementHook>,
    cancels: u64,
}

struct Shared {
    book: Arc<SpotBook>,
    credentials: TestCredentials,
    wire: Mutex<Wire>,
    live: Arc<Live>,
    stop: watch::Receiver<bool>,
}

impl Shared {
    fn wire(&self) -> MutexGuard<'_, Wire> {
        self.wire.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The fake spot venue; see the module documentation.
pub struct FakeSpotVenue {
    addr: SocketAddr,
    shared: Arc<Shared>,
    server: Option<JoinHandle<io::Result<()>>>,
    stop: watch::Sender<bool>,
}

/// The per-connection service: the router, with a guard counting the
/// connection until its task drops it.
#[derive(Clone)]
struct PerConnection {
    router: Router,
    live: Arc<Live>,
}

impl<'a> Service<IncomingStream<'a>> for PerConnection {
    type Response = Router;
    type Error = Infallible;
    type Future = Ready<Result<Router, Infallible>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: IncomingStream<'a>) -> Self::Future {
        let guard = Arc::new(Counted::new(&self.live, |live| &live.connections));
        ready(Ok(self.router.clone().layer(Extension(guard))))
    }
}

impl FakeSpotVenue {
    /// Serve `book` on a loopback port, signed with `credentials`.
    pub async fn start(book: Arc<SpotBook>, credentials: TestCredentials) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (stop, stopped) = watch::channel(false);
        let live = Arc::new(Live::default());
        let shared = Arc::new(Shared {
            book,
            credentials,
            wire: Mutex::new(Wire::default()),
            live: Arc::clone(&live),
            stop: stopped.clone(),
        });
        let router = Router::new().fallback(handle).with_state(Arc::clone(&shared));
        let mut until = stopped;
        let server = tokio::spawn(async move {
            axum::serve(listener, PerConnection { router, live })
                .with_graceful_shutdown(async move {
                    let _ = until.wait_for(|stopped| *stopped).await;
                })
                .await
        });
        Ok(Self { addr, shared, server: Some(server), stop })
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn book(&self) -> &SpotBook {
        &self.shared.book
    }

    /// The last [`REQUEST_LOG`] requests, oldest first.
    pub fn requests(&self) -> Vec<WireRequest> {
        self.shared.wire().requests.iter().cloned().collect()
    }

    /// Every request received.
    pub fn requests_total(&self) -> u64 {
        self.shared.wire().total
    }

    /// The documented weight of every request served (rest-api.md).
    pub fn weight(&self) -> u32 {
        self.shared.wire().weight
    }

    /// The first [`UNSERVED_KEPT`] requests the fake did not serve.
    pub fn unserved_requests(&self) -> Vec<String> {
        self.shared.wire().unserved.clone()
    }

    /// Every request the fake did not serve: zero means none ever was.
    pub fn unserved_total(&self) -> u64 {
        self.shared.wire().unserved_total
    }

    /// Read the next `method` `path` request and answer it only at shutdown.
    ///
    /// # Panics
    /// For a route the fake does not serve, or past [`HOLDS`] queued.
    pub fn hold_next(&self, method: Method, path: &str) {
        assert!(
            served(&method, path),
            "fake spot venue: {method} {path} is not a route it serves"
        );
        let mut wire = self.shared.wire();
        assert!(wire.holds.len() < HOLDS, "fake spot venue: {HOLDS} holds queued");
        wire.holds.push_back((method, path.to_string()));
    }

    /// Answer the next `method` `path` request with `status` and `body`.
    ///
    /// # Panics
    /// For a route the fake does not serve, or past [`WIRE_SCRIPTS`] queued.
    pub fn script_wire_next(&self, method: Method, path: &str, status: u16, body: &str) {
        assert!(
            served(&method, path),
            "fake spot venue: {method} {path} is not a route it serves"
        );
        let mut wire = self.shared.wire();
        assert!(
            wire.scripts.len() < WIRE_SCRIPTS,
            "fake spot venue: {WIRE_SCRIPTS} scripts queued"
        );
        wire.scripts.push_back((method, path.to_string(), status, body.to_string()));
    }

    /// Run `hook` once, as the next placement passes authentication and
    /// before the book acts.
    pub fn on_next_placement(&self, hook: PlacementHook) {
        self.shared.wire().next_placement = Some(hook);
    }

    /// Open connections and running handlers.
    pub fn live(&self) -> (usize, usize) {
        self.counters().live()
    }

    /// Handlers waiting on a hold.
    pub fn held(&self) -> usize {
        self.counters().held()
    }

    /// The live counts, for a check after [`Self::shutdown`].
    pub fn counters(&self) -> Counters {
        Counters(Arc::clone(&self.shared.live))
    }

    /// Stop: every connection drains (a held handler answers 503 on the
    /// signal), then the server ends. Past [`SHUTDOWN_BOUND`] it is an
    /// error: the bound limits the wait and closes nothing.
    ///
    /// # Errors
    /// The drain outlasted the bound, or the server failed.
    pub async fn shutdown(mut self) -> Result<()> {
        let _ = self.stop.send(true);
        let server = self.server.take().expect("started");
        match tokio::time::timeout(SHUTDOWN_BOUND, server).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(error.into()),
            Ok(Err(join)) => Err(join.into()),
            Err(_) => bail!(
                "fake spot venue: the drain outlasted {SHUTDOWN_BOUND:?}, live {:?}",
                self.live()
            ),
        }
    }
}

impl Drop for FakeSpotVenue {
    /// Without `shutdown`, the drain completes in the background, and the
    /// omission is reported.
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if self.server.is_some() {
            eprintln!("fake spot venue {}: dropped without shutdown", self.addr);
        }
    }
}

type Answer = (StatusCode, String);

fn error(status: u16, code: i64, msg: &str) -> Answer {
    let status = StatusCode::from_u16(status).expect("a status");
    (status, json!({ "code": code, "msg": msg }).to_string())
}

/// Binance spot's documented request weights (rest-api.md, 995f6eb3).
fn weight(method: &Method, path: &str, params: &[(String, String)]) -> Option<(u32, bool)> {
    let has = |name: &str| params.iter().any(|(k, _)| k == name);
    let route = match (method.as_str(), path) {
        ("GET", "/api/v3/ping") => (1, false),
        ("GET", "/api/v3/exchangeInfo") => (20, false),
        ("GET", "/api/v3/ticker/price") => (2, false),
        ("GET", "/api/v3/account") => (20, true),
        ("GET", "/api/v3/openOrders") => (if has("symbol") { 6 } else { 80 }, true),
        ("GET", "/api/v3/openOrderList") => (6, true),
        ("GET", "/api/v3/order") => (4, true),
        ("GET", "/api/v3/myTrades") => (if has("orderId") { 5 } else { 20 }, true),
        ("POST", "/api/v3/order") | ("DELETE", "/api/v3/order") => (1, true),
        _ => return None,
    };
    Some(route)
}

/// Whether the fake serves `method` `path` at all.
fn served(method: &Method, path: &str) -> bool {
    weight(method, path, &[]).is_some()
}

/// Record an unserved request, keeping the first [`UNSERVED_KEPT`].
fn unserved(shared: &Shared, what: String) -> Answer {
    let mut wire = shared.wire();
    wire.unserved_total += 1;
    if wire.unserved.len() < UNSERVED_KEPT {
        wire.unserved.push(what);
    }
    error(400, -1100, "unserved by the fake spot venue")
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                        continue;
                    },
                    Err(_) => out.push(b'%'),
                }
            },
            b'+' => out.push(b' '),
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Authentication of a signed request (module documentation).
fn authenticate(
    shared: &Shared,
    query: &str,
    key: Option<&str>,
) -> Result<(), (Answer, SignatureVerdict)> {
    let format = |k: &str| k.len() == 64 && k.bytes().all(|b| b.is_ascii_alphanumeric());
    let refused = |code, msg| Err((error(401, code, msg), SignatureVerdict::NotChecked));
    match key {
        // Synthetic: the docs give no answer for a missing header.
        None => return refused(-2014, "API-key format invalid."),
        Some(k) if !format(k) => return refused(-2014, "API-key format invalid."),
        Some(k) if k != shared.credentials.api_key => {
            return refused(-2015, "Invalid API-key, IP, or permissions for action.");
        },
        Some(_) => {},
    }
    let Some((payload, signature)) = query.rsplit_once("&signature=") else {
        let msg = "Mandatory parameter 'signature' was not sent, was empty/null, or malformed.";
        return Err((error(400, -1102, msg), SignatureVerdict::Missing));
    };
    let mut mac = Hmac::<Sha256>::new_from_slice(shared.credentials.api_secret.as_bytes())
        .expect("any key length");
    mac.update(payload.as_bytes());
    let expected: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
    if !expected.eq_ignore_ascii_case(signature) {
        let msg = "Signature for this request is not valid.";
        return Err((error(400, -1022, msg), SignatureVerdict::Invalid));
    }
    Ok(())
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let _handler = Counted::new(&shared.live, |live| &live.handlers);
    let (status, body) = serve(&shared, method, uri, headers).await;
    (status, [(CONTENT_TYPE, "application/json")], body).into_response()
}

async fn serve(shared: &Shared, method: Method, uri: Uri, headers: HeaderMap) -> Answer {
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or_default().to_string();
    let key = headers.get("X-MBX-APIKEY").and_then(|v| v.to_str().ok()).map(str::to_string);
    let params = params(&query);
    let Some((weight, signed)) = weight(&method, &path, &params) else {
        record(shared, &method, &path, &query, key.is_some(), SignatureVerdict::Unsigned, 0);
        return unserved(shared, format!("{method} {path}?{query}"));
    };
    let signature = if signed {
        match authenticate(shared, &query, key.as_deref()) {
            Ok(()) => SignatureVerdict::Valid,
            Err((answer, verdict)) => {
                record(shared, &method, &path, &query, key.is_some(), verdict, 0);
                return answer;
            },
        }
    } else {
        SignatureVerdict::Unsigned
    };
    record(shared, &method, &path, &query, key.is_some(), signature, weight);
    // A hold reads, authenticates and records the request, then answers
    // only on the stop signal.
    let held = {
        let mut wire = shared.wire();
        let at = wire.holds.iter().position(|(m, p)| *m == method && *p == path);
        at.map(|at| wire.holds.remove(at)).is_some()
    };
    if held {
        let _waiting = Counted::new(&shared.live, |live| &live.held);
        let mut stop = shared.stop.clone();
        let _ = stop.wait_for(|stopped| *stopped).await;
        return error(503, -1007, "Timeout waiting for response from backend server.");
    }
    let scripted = {
        let mut wire = shared.wire();
        let at = wire.scripts.iter().position(|(m, p, ..)| *m == method && *p == path);
        at.and_then(|at| wire.scripts.remove(at))
    };
    if let Some((_, _, status, body)) = scripted {
        return (StatusCode::from_u16(status).unwrap_or(StatusCode::OK), body);
    }
    respond(shared, &method, &path, &params)
}

fn record(
    shared: &Shared,
    method: &Method,
    path: &str,
    query: &str,
    api_key: bool,
    signature: SignatureVerdict,
    weight: u32,
) {
    let mut wire = shared.wire();
    if wire.requests.len() == REQUEST_LOG {
        wire.requests.pop_front();
    }
    wire.requests.push_back(WireRequest {
        method: method.to_string(),
        path: path.to_string(),
        query: query.to_string(),
        api_key,
        signature,
    });
    wire.total += 1;
    wire.weight += weight;
}

fn ok(body: Value) -> Answer {
    (StatusCode::OK, body.to_string())
}

fn dec(value: Decimal) -> String {
    value.normalize().to_string()
}

fn side(side: Side) -> &'static str {
    match side {
        Side::Buy => "BUY",
        Side::Sell => "SELL",
    }
}

/// The STP mode `order_id` was placed with: the symbol's default for an
/// order that never came over the wire (a hook's).
fn stp_of(shared: &Shared, order_id: u64) -> String {
    let wire = shared.wire();
    wire.stp.get(&order_id).cloned().unwrap_or_else(|| STP_MODES[0].to_string())
}

/// An order's record as a `RESULT` answer gives it (rest-api.md
/// L2222-L2242), `stopPrice` only for a stop (conditional fields, L186).
fn record_json(order: &BookOrder, stp: &str) -> Value {
    let order_type = match order.kind {
        OrderKind::Market => "MARKET",
        OrderKind::StopLoss { .. } => "STOP_LOSS",
    };
    let mut record = json!({
        "symbol": order.symbol, "orderId": order.id, "orderListId": -1,
        "clientOrderId": order.client_id, "price": "0", "origQty": dec(order.orig_qty),
        "executedQty": dec(order.executed_qty), "origQuoteOrderQty": "0",
        "cummulativeQuoteQty": dec(order.quote_qty), "status": order.status.as_str(),
        "timeInForce": "GTC", "type": order_type, "side": side(order.side),
        "workingTime": order.time_ms, "selfTradePreventionMode": stp,
    });
    if let OrderKind::StopLoss { trigger } = order.kind {
        record["stopPrice"] = json!(dec(trigger));
    }
    record
}

/// An order as `GET order` and `openOrders` give it: its record with its
/// times.
fn order_json(shared: &Shared, order: &BookOrder) -> Value {
    let mut record = record_json(order, &stp_of(shared, order.id));
    record["icebergQty"] = json!("0");
    record["time"] = json!(order.time_ms);
    record["updateTime"] = json!(order.update_ms);
    record["isWorking"] = json!(!order.status.is_terminal());
    record
}

fn trade_json(trade: &BookTrade) -> Value {
    json!({
        "symbol": trade.symbol, "id": trade.id, "orderId": trade.order_id, "orderListId": -1,
        "price": dec(trade.price), "qty": dec(trade.qty), "quoteQty": dec(trade.quote_qty),
        "commission": dec(trade.commission), "commissionAsset": trade.commission_asset,
        "time": trade.time_ms, "isBuyer": trade.side == Side::Buy, "isMaker": false,
        "isBestMatch": true,
    })
}

fn book_error(error_: BookError) -> Answer {
    match error_ {
        BookError::Rejected { code, msg } => error(400, code, &msg),
        BookError::NotOpen => error(400, -2011, "Unknown order sent."),
        BookError::NoSuchOrder => error(400, -2013, "Order does not exist."),
        BookError::Archived => error(
            400,
            -2026,
            "Order was canceled or expired with no executed qty over 90 days ago and has been archived.",
        ),
        BookError::Lost => error(503, -1007, "Timeout waiting for response from backend server."),
    }
}

/// `exchangeInfo` for one symbol: the spot adapter's
/// `exchange_info_btcusdt.json` fixture with the symbol, its assets and its
/// order limits from the rules; a limit the book does not apply is not
/// advertised.
fn exchange_info(rules: &BookRules) -> Value {
    let filters = json!([
        {"filterType": "PRICE_FILTER", "minPrice": "0.01000000", "maxPrice": "1000000.00000000", "tickSize": "0.01000000"},
        {"filterType": "LOT_SIZE", "minQty": "0.00001000", "maxQty": "9000.00000000", "stepSize": "0.00001000"},
        {"filterType": "ICEBERG_PARTS", "limit": 100},
        {"filterType": "MARKET_LOT_SIZE", "minQty": "0.00000000", "maxQty": "108.95042220", "stepSize": "0.00000000"},
        {"filterType": "TRAILING_DELTA", "minTrailingAboveDelta": 10, "maxTrailingAboveDelta": 2000, "minTrailingBelowDelta": 10, "maxTrailingBelowDelta": 2000},
        {"filterType": "PERCENT_PRICE_BY_SIDE", "bidMultiplierUp": "1.2", "bidMultiplierDown": "0.5", "askMultiplierUp": "2", "askMultiplierDown": "0.8", "avgPriceMins": 5},
        {"filterType": "NOTIONAL", "minNotional": "5.00000000", "applyMinToMarket": true, "maxNotional": "9000000.00000000", "applyMaxToMarket": false, "avgPriceMins": 5},
        {"filterType": "MAX_NUM_ORDER_LISTS", "maxNumOrderLists": 20},
        {"filterType": "MAX_NUM_ORDER_AMENDS", "maxNumOrderAmends": 10},
    ]);
    let mut filters = filters.as_array().cloned().unwrap_or_default();
    if let Some(max) = rules.max_num_orders {
        filters.push(json!({"filterType": "MAX_NUM_ORDERS", "maxNumOrders": max}));
    }
    if let Some(max) = rules.max_num_algo_orders {
        filters.push(json!({"filterType": "MAX_NUM_ALGO_ORDERS", "maxNumAlgoOrders": max}));
    }
    json!({
        "timezone": "UTC", "serverTime": 1_767_225_600_000_i64, "rateLimits": [], "exchangeFilters": [],
        "symbols": [{
            "symbol": rules.symbol, "status": "TRADING", "baseAsset": rules.base, "baseAssetPrecision": 8,
            "quoteAsset": rules.quote, "quotePrecision": 8, "quoteAssetPrecision": 8,
            "baseCommissionPrecision": 8, "quoteCommissionPrecision": 8,
            "orderTypes": ["LIMIT", "LIMIT_MAKER", "MARKET", "STOP_LOSS", "STOP_LOSS_LIMIT", "TAKE_PROFIT", "TAKE_PROFIT_LIMIT"],
            "icebergAllowed": true, "ocoAllowed": true, "otoAllowed": true, "opoAllowed": true,
            "quoteOrderQtyMarketAllowed": true, "allowTrailingStop": true, "cancelReplaceAllowed": true,
            "amendAllowed": true, "pegInstructionsAllowed": true, "isSpotTradingAllowed": true,
            "isMarginTradingAllowed": true, "filters": filters, "permissions": [], "permissionSets": [["SPOT"]],
            "defaultSelfTradePreventionMode": STP_MODES[0],
            "allowedSelfTradePreventionModes": STP_MODES,
        }],
    })
}

fn respond(shared: &Shared, method: &Method, path: &str, params: &[(String, String)]) -> Answer {
    let book = &shared.book;
    let symbol = param(params, "symbol").unwrap_or_default().to_string();
    match (method.as_str(), path) {
        ("GET", "/api/v3/ping") => ok(json!({})),
        ("GET", "/api/v3/exchangeInfo") => match book.rules(&symbol) {
            Some(rules) => ok(exchange_info(&rules)),
            None => error(400, -1121, "Invalid symbol."),
        },
        ("GET", "/api/v3/ticker/price") => match book.price(&symbol) {
            Some(price) => ok(json!({ "symbol": symbol, "price": dec(price) })),
            None => error(400, -1121, "Invalid symbol."),
        },
        ("GET", "/api/v3/account") => {
            let balances: Vec<Value> = book
                .balances()
                .into_iter()
                .map(|(asset, b)| json!({ "asset": asset, "free": dec(b.free), "locked": dec(b.locked) }))
                .collect();
            ok(json!({
                "makerCommission": 10, "takerCommission": 10, "buyerCommission": 0, "sellerCommission": 0,
                "commissionRates": { "maker": "0.001", "taker": "0.001", "buyer": "0", "seller": "0" },
                "canTrade": true, "canWithdraw": false, "canDeposit": false, "brokered": false,
                "requireSelfTradePrevention": false, "preventSor": false, "updateTime": book.now_ms(),
                "accountType": "SPOT", "balances": balances, "permissions": ["SPOT"], "uid": 1,
            }))
        },
        ("GET", "/api/v3/openOrders") => {
            let on = param(params, "symbol");
            let orders = book.open_orders(on);
            ok(Value::Array(orders.iter().map(|o| order_json(shared, o)).collect()))
        },
        ("GET", "/api/v3/openOrderList") => ok(json!([])),
        ("GET", "/api/v3/order") => {
            let by = match (param(params, "orderId"), param(params, "origClientOrderId")) {
                (Some(id), _) => id.parse().map(OrderRef::Id).ok(),
                (None, Some(client)) => Some(OrderRef::Client(client)),
                (None, None) => None,
            };
            match by.map(|by| book.order(&symbol, by)) {
                Some(Ok(order)) => ok(order_json(shared, &order)),
                Some(Err(e)) => book_error(e),
                None => error(400, -1102, "Mandatory parameter 'orderId' was not sent."),
            }
        },
        ("GET", "/api/v3/myTrades") => {
            let order = param(params, "orderId").and_then(|id| id.parse().ok());
            let from = param(params, "fromId").and_then(|id| id.parse().ok());
            let limit = param(params, "limit").and_then(|l| l.parse().ok()).unwrap_or(500);
            ok(Value::Array(
                book.trades(&symbol, order, from, limit).iter().map(trade_json).collect(),
            ))
        },
        ("POST", "/api/v3/order") => place(shared, &symbol, params),
        ("DELETE", "/api/v3/order") => cancel(shared, &symbol, params),
        _ => error(400, -1100, "unserved by the fake spot venue"),
    }
}

/// The answer a placement asked for (`newOrderRespType`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Ack,
    Result,
    Full,
}

/// A placement the fake supports, validated.
struct NewOrder<'a> {
    side: Side,
    kind: OrderKind,
    qty: Decimal,
    client_id: &'a str,
    shape: Shape,
    stp: &'static str,
}

/// Why a placement is not acted on.
enum Refused {
    /// Binance's answer to it.
    Answer(Answer),
    /// Something the fake does not model: unserved.
    Unsupported(&'static str),
}

fn mandatory(name: &str) -> Refused {
    let msg = format!("Mandatory parameter '{name}' was not sent, was empty/null, or malformed.");
    Refused::Answer(error(400, -1102, &msg))
}

/// The placement's request, validated before anything acts (module
/// documentation).
fn new_order(params: &[(String, String)]) -> Result<NewOrder<'_>, Refused> {
    for name in [
        "quoteOrderQty",
        "trailingDelta",
        "price",
        "icebergQty",
        "timeInForce",
    ] {
        if param(params, name).is_some() {
            return Err(Refused::Unsupported("a parameter the fake does not model"));
        }
    }
    let side = match param(params, "side") {
        None | Some("") => return Err(mandatory("side")),
        Some("BUY") => Side::Buy,
        Some("SELL") => Side::Sell,
        Some(_) => return Err(Refused::Answer(error(400, -1117, "Invalid side."))),
    };
    let stop_price = param(params, "stopPrice");
    let kind = match param(params, "type") {
        None | Some("") => return Err(mandatory("type")),
        Some("MARKET") if stop_price.is_some() => {
            let msg = "Parameter 'stopPrice' sent when not required.";
            return Err(Refused::Answer(error(400, -1106, msg)));
        },
        Some("MARKET") => OrderKind::Market,
        Some("STOP_LOSS") if side == Side::Buy => {
            return Err(Refused::Unsupported("a BUY STOP_LOSS"));
        },
        Some("STOP_LOSS") => match stop_price.and_then(|p| p.parse().ok()) {
            Some(trigger) => OrderKind::StopLoss { trigger },
            None => return Err(mandatory("stopPrice")),
        },
        Some("LIMIT" | "LIMIT_MAKER" | "STOP_LOSS_LIMIT" | "TAKE_PROFIT" | "TAKE_PROFIT_LIMIT") => {
            return Err(Refused::Unsupported("an order type the fake does not model"));
        },
        Some(_) => return Err(Refused::Answer(error(400, -1116, "Invalid orderType."))),
    };
    let Some(qty) = param(params, "quantity").and_then(|q| q.parse().ok()) else {
        return Err(mandatory("quantity"));
    };
    let Some(client_id) = param(params, "newClientOrderId").filter(|id| !id.is_empty()) else {
        return Err(Refused::Unsupported("no newClientOrderId"));
    };
    let shape = match (param(params, "newOrderRespType"), kind) {
        (None, OrderKind::Market) | (Some("FULL"), _) => Shape::Full,
        (None, OrderKind::StopLoss { .. }) | (Some("ACK"), _) => Shape::Ack,
        (Some("RESULT"), _) => Shape::Result,
        (Some(_), _) => {
            let msg = "Illegal characters found in parameter 'newOrderRespType'; legal range is \
                       'ACK, RESULT, FULL'.";
            return Err(Refused::Answer(error(400, -1100, msg)));
        },
    };
    let stp = match param(params, "selfTradePreventionMode") {
        None => STP_MODES[0],
        Some(mode) => match STP_MODES.iter().find(|allowed| **allowed == mode) {
            Some(allowed) => allowed,
            None if STP_ENUM.contains(&mode) => {
                let msg = "This symbol does not allow the specified self-trade prevention mode.";
                return Err(Refused::Answer(error(400, -1013, msg)));
            },
            None => {
                let msg = "Illegal characters found in parameter 'selfTradePreventionMode'.";
                return Err(Refused::Answer(error(400, -1100, msg)));
            },
        },
    };
    Ok(NewOrder { side, kind, qty, client_id, shape, stp })
}

/// Keep `order_id`'s STP mode while the book retains the order: past the
/// book's retention, the entries of orders it evicted go, oldest first, and
/// every retained order keeps its mode.
fn keep_stp(shared: &Shared, order_id: u64, stp: &str) {
    let mut wire = shared.wire();
    wire.stp.insert(order_id, stp.to_string());
    if wire.stp.len() <= MAX_RETAINED_ORDERS {
        return;
    }
    let excess = wire.stp.len() - MAX_RETAINED_ORDERS;
    let evicted: Vec<u64> = wire
        .stp
        .keys()
        .copied()
        .filter(|id| !shared.book.retains(*id))
        .take(excess)
        .collect();
    for id in evicted {
        wire.stp.remove(&id);
    }
}

/// `POST /api/v3/order`: a MARKET order or a SELL `STOP_LOSS`, validated
/// first, answered in the shape it asked for, its STP mode echoed.
fn place(shared: &Shared, symbol: &str, params: &[(String, String)]) -> Answer {
    let order = match new_order(params) {
        Ok(order) => order,
        Err(Refused::Answer(answer)) => return answer,
        Err(Refused::Unsupported(what)) => {
            let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
            return unserved(shared, format!("POST /api/v3/order ({what}) {}", query.join("&")));
        },
    };
    let hook = shared.wire().next_placement.take();
    if let Some(hook) = hook {
        hook(&shared.book);
    }
    let book = &shared.book;
    let latest = |book: &SpotBook| book.order(symbol, OrderRef::Client(order.client_id)).ok();
    let before = latest(book).map(|o| o.id);
    let placed = match order.kind {
        OrderKind::Market => book.place_market(symbol, order.side, order.qty, order.client_id),
        OrderKind::StopLoss { trigger } => {
            book.place_stop(symbol, order.qty, trigger, order.client_id)
        },
    };
    let placement = match placed {
        Ok(placement) => placement,
        Err(BookError::Lost) => {
            // The book may have accepted it: its records carry its mode.
            if let Some(accepted) = latest(book).filter(|o| Some(o.id) != before) {
                keep_stp(shared, accepted.id, order.stp);
            }
            return book_error(BookError::Lost);
        },
        Err(e) => return book_error(e),
    };
    let placed = &placement.order;
    keep_stp(shared, placed.id, order.stp);
    if order.shape == Shape::Ack {
        return ok(json!({
            "symbol": placed.symbol, "orderId": placed.id, "orderListId": -1,
            "clientOrderId": placed.client_id, "transactTime": placed.update_ms,
        }));
    }
    let mut answer = record_json(placed, order.stp);
    answer["transactTime"] = json!(placed.update_ms);
    if order.shape == Shape::Full {
        let fills: Vec<Value> = placement
            .fills
            .iter()
            .map(|t| {
                json!({
                    "price": dec(t.price), "qty": dec(t.qty), "commission": dec(t.commission),
                    "commissionAsset": t.commission_asset, "tradeId": t.id,
                })
            })
            .collect();
        answer["fills"] = Value::Array(fills);
    }
    ok(answer)
}

/// `DELETE /api/v3/order` by `orderId`: the record after the cancel, under
/// a new client id (F12).
fn cancel(shared: &Shared, symbol: &str, params: &[(String, String)]) -> Answer {
    let Some(id) = param(params, "orderId").and_then(|id| id.parse().ok()) else {
        return error(400, -1102, "Mandatory parameter 'orderId' was not sent.");
    };
    match shared.book.cancel(symbol, id) {
        Ok(order) => {
            let sequence = {
                let mut wire = shared.wire();
                wire.cancels += 1;
                wire.cancels
            };
            let mut answer = record_json(&order, &stp_of(shared, order.id));
            answer["origClientOrderId"] = json!(order.client_id);
            answer["clientOrderId"] = json!(format!("cancel{sequence}"));
            answer["transactTime"] = json!(order.update_ms);
            ok(answer)
        },
        Err(e) => book_error(e),
    }
}
