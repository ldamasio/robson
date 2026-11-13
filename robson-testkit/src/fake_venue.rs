//! A local stand-in for the Binance USD-M REST surface (ADR-0056 step 7).
//!
//! The real `robsond`, with the real adapter and REST client, talks to this
//! server over loopback. It fills market orders at once at a fixed mark,
//! rests conditional orders, and counts what it accepted, filled and
//! cancelled. A request it cannot serve is recorded in
//! [`FakeVenue::unserved_requests`], so a test can assert it never received
//! anything it did not understand.
//!
//! For the conformance run U (ADR-0056 step 8, 8d plan §4 notes ¹ and ⁴) it
//! also answers `GET /fapi/v1/order` and `GET /fapi/v1/algoOrder` (`-2013`
//! for an id it never accepted), and takes four controls:
//! [`FakeVenue::reject_next_algo`], [`FakeVenue::trigger_algo`],
//! [`FakeVenue::on_next_algo_accepted`] and the reads
//! [`FakeVenue::open_algo_ids`] and [`FakeVenue::placement_requests`]. The
//! routes it answered before answer as they did.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::{Method, StatusCode, Uri},
    Router,
};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use tokio::{net::TcpListener, task::JoinHandle};

/// Symbol the venue lists, and the only one it serves rules and candles for.
pub const FAKE_SYMBOL: &str = "BTCUSDT";
/// Mark price, also the last close of the served candles.
pub const FAKE_MARK: Decimal = Decimal::from_parts(100, 0, 0, false, 0);
const WALLET_BALANCE: Decimal = Decimal::from_parts(10_000, 0, 0, false, 0);
const CANDLES_MS: i64 = 15 * 60 * 1_000;
/// How often a held order looks for its release.
const HOLD_POLL: Duration = Duration::from_millis(10);

/// A hook run as the next conditional order is accepted, with the open
/// algo ids, the new one included.
pub type AlgoAcceptedHook = Box<dyn FnOnce(Vec<String>) + Send>;

/// Everything the venue did, as counted by the venue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VenueCounts {
    /// Orders accepted, market and conditional alike.
    pub accepted_orders: u64,
    /// Market orders filled.
    pub fills: u64,
    /// Orders cancelled.
    pub cancels: u64,
}

/// One order the venue accepted.
#[derive(Clone, Debug, PartialEq)]
pub struct AcceptedOrder {
    pub client_id: String,
    pub side: String,
    pub quantity: Decimal,
    pub reduce_only: bool,
    pub conditional: bool,
}

#[derive(Default)]
struct Book {
    counts: VenueCounts,
    orders: Vec<AcceptedOrder>,
    /// Signed position per symbol: positive long, negative short.
    positions: HashMap<String, Decimal>,
    /// Open conditional orders, by algo id.
    open_algos: HashMap<i64, Value>,
    next_id: i64,
    unserved: Vec<String>,
    /// Every request received, served or not.
    requests: u64,
    /// Market orders are read and held unanswered until released.
    hold_orders: bool,
    /// Market orders read and held so far.
    held: u64,
    /// `POST /fapi/v1/order` and `POST /fapi/v1/algoOrder` received.
    placements: u64,
    /// Every market order filled, as `GET /fapi/v1/order` gives it.
    market_records: Vec<Value>,
    /// Every conditional order accepted, as `GET /fapi/v1/algoOrder` gives
    /// it, by algo id.
    algo_records: HashMap<i64, Value>,
    /// The next conditional order is refused (`-2010` at HTTP 400).
    reject_next_algo: bool,
    algo_accepted: Option<AlgoAcceptedHook>,
}

type Shared = Arc<Mutex<Book>>;

/// A running fake venue. Dropping it stops the server.
pub struct FakeVenue {
    addr: SocketAddr,
    book: Shared,
    server: JoinHandle<()>,
}

impl Drop for FakeVenue {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl FakeVenue {
    /// Bind a loopback port and start serving.
    pub async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let book = Shared::default();
        let app = Router::new().fallback(serve).with_state(book.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { addr, book, server })
    }

    /// Base URL to hand to the daemon.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Counts so far.
    pub fn counts(&self) -> VenueCounts {
        self.book().counts
    }

    /// Every accepted order, in arrival order.
    pub fn orders(&self) -> Vec<AcceptedOrder> {
        self.book().orders.clone()
    }

    /// Signed position in `symbol`; zero when flat.
    pub fn position(&self, symbol: &str) -> Decimal {
        self.book().positions.get(symbol).copied().unwrap_or_default()
    }

    /// Requests no route could serve, as `"METHOD /path"`.
    pub fn unserved_requests(&self) -> Vec<String> {
        self.book().unserved.clone()
    }

    /// Every request received so far, served or not.
    pub fn requests(&self) -> u64 {
        self.book().requests
    }

    /// From now on, read market orders and hold them unanswered until
    /// [`FakeVenue::release_orders`]; the client's own bound may end one first,
    /// as an outcome it cannot know.
    pub fn hold_orders(&self) {
        self.book().hold_orders = true;
    }

    /// Answer the held market orders, and every later one at once.
    pub fn release_orders(&self) {
        self.book().hold_orders = false;
    }

    /// Market orders read and held so far.
    pub fn held_orders(&self) -> u64 {
        self.book().held
    }

    /// Order placements received (`POST /fapi/v1/order` and `POST
    /// /fapi/v1/algoOrder`), accepted, refused or held.
    pub fn placement_requests(&self) -> u64 {
        self.book().placements
    }

    /// The open conditional orders' algo ids, ascending.
    pub fn open_algo_ids(&self) -> Vec<String> {
        let mut ids: Vec<i64> = self.book().open_algos.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter().map(|id| id.to_string()).collect()
    }

    /// Refuse the next conditional order with HTTP 400 `-2010`
    /// (`NEW_ORDER_REJECTED`, an authoritative rejection on USD-M), nothing
    /// accepted.
    pub fn reject_next_algo(&self) {
        self.book().reject_next_algo = true;
    }

    /// Run `hook` as the next conditional order is accepted, with the open
    /// algo ids, the new one included. It runs under the venue's lock: it
    /// must not call the venue.
    pub fn on_next_algo_accepted(&self, hook: AlgoAcceptedHook) {
        self.book().algo_accepted = Some(hook);
    }

    /// The open conditional order `algo_id` triggers and its market order
    /// executes `executed` of its quantity: the algo leaves the open list,
    /// its record names the triggered order, whose `GET /fapi/v1/order`
    /// record carries `executedQty = executed` (`FILLED` when whole,
    /// `EXPIRED` otherwise), and the position moves by what executed.
    /// `false`, with nothing changed, for an algo that is not open or an
    /// execution outside `0..=quantity`.
    pub fn trigger_algo(&self, algo_id: &str, executed: Decimal) -> bool {
        let mut book = self.book();
        let Some(id) = algo_id.parse::<i64>().ok().filter(|id| book.open_algos.contains_key(id))
        else {
            return false;
        };
        let algo = book.open_algos[&id].clone();
        let quantity: Decimal =
            algo["quantity"].as_str().and_then(|q| q.parse().ok()).unwrap_or_default();
        if executed < Decimal::ZERO || executed > quantity {
            return false;
        }
        book.open_algos.remove(&id);
        book.next_id += 1;
        let order_id = book.next_id;
        let side = algo["side"].as_str().unwrap_or_default().to_string();
        let symbol = algo["symbol"].as_str().unwrap_or_default().to_string();
        let now = Utc::now().timestamp_millis();
        let status = if executed == quantity {
            "FILLED"
        } else {
            "EXPIRED"
        };
        book.market_records.push(json!({
            "symbol": symbol,
            "orderId": order_id,
            "clientOrderId": format!("triggered-{id}"),
            "updateTime": now,
            "price": "0",
            "avgPrice": FAKE_MARK.to_string(),
            "origQty": quantity.to_string(),
            "executedQty": executed.to_string(),
            "cumQuote": (executed * FAKE_MARK).to_string(),
            "status": status,
            "side": side,
            "type": "MARKET",
        }));
        let signed = if side == "BUY" { executed } else { -executed };
        *book.positions.entry(symbol).or_default() += signed;
        if let Some(record) = book.algo_records.get_mut(&id) {
            record["algoStatus"] = json!("FINISHED");
            record["actualOrderId"] = json!(order_id.to_string());
            record["actualQty"] = json!(executed.to_string());
            record["triggerTime"] = json!(now);
            record["updateTime"] = json!(now);
        }
        true
    }
}

async fn serve(
    State(book): State<Shared>,
    method: Method,
    uri: Uri,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, String) {
    let lock = || book.lock().unwrap_or_else(|e| e.into_inner());
    let held = {
        let mut book = lock();
        book.requests += 1;
        let placement =
            method == Method::POST && matches!(uri.path(), "/fapi/v1/order" | "/fapi/v1/algoOrder");
        book.placements += u64::from(placement);
        let held = book.hold_orders && method == Method::POST && uri.path() == "/fapi/v1/order";
        book.held += u64::from(held);
        held
    };
    if held {
        while lock().hold_orders {
            tokio::time::sleep(HOLD_POLL).await;
        }
    }
    let mut book = lock();
    match respond(&mut book, &method, uri.path(), &params) {
        Ok(body) => (StatusCode::OK, body.to_string()),
        Err((status, code, msg)) => (status, json!({ "code": code, "msg": msg }).to_string()),
    }
}

type Refusal = (StatusCode, i64, String);

fn respond(
    book: &mut Book,
    method: &Method,
    path: &str,
    params: &HashMap<String, String>,
) -> Result<Value, Refusal> {
    match (method.as_str(), path) {
        ("GET", "/fapi/v1/ping") => Ok(json!({})),
        ("GET", "/fapi/v1/exchangeInfo") => Ok(exchange_info()),
        ("GET", "/fapi/v1/ticker/price") => {
            Ok(json!({ "symbol": FAKE_SYMBOL, "price": FAKE_MARK.to_string() }))
        },
        ("GET", "/fapi/v1/klines") => Ok(klines(params)),
        ("GET", "/fapi/v2/balance") => Ok(json!([{
            "asset": "USDT",
            "balance": WALLET_BALANCE.to_string(),
            "availableBalance": WALLET_BALANCE.to_string(),
            "crossUnPnl": "0",
        }])),
        ("GET", "/fapi/v2/positionRisk") => Ok(position_risk(book)),
        ("GET", "/fapi/v1/positionSide/dual") => Ok(json!({ "dualSidePosition": false })),
        ("POST", "/fapi/v1/leverage") => Ok(json!({ "leverage": 1 })),
        ("GET", "/fapi/v1/userTrades" | "/fapi/v1/income") => Ok(json!([])),
        ("POST", "/fapi/v1/order") => place_market(book, params),
        ("POST", "/fapi/v1/algoOrder") => place_algo(book, params),
        ("DELETE", "/fapi/v1/algoOrder") => cancel_algo(book, params),
        ("GET", "/fapi/v1/openAlgoOrders") => {
            Ok(Value::Array(book.open_algos.values().cloned().collect()))
        },
        ("GET", "/fapi/v1/order") => order_record(book, params),
        ("GET", "/fapi/v1/algoOrder") => algo_record(book, params),
        _ => {
            book.unserved.push(format!("{method} {path}"));
            Err((StatusCode::NOT_FOUND, -1, format!("fake venue cannot serve {path}")))
        },
    }
}

/// `-2013` "Order does not exist." for an id the venue never accepted.
fn no_such_order() -> Refusal {
    (StatusCode::BAD_REQUEST, -2013, "Order does not exist.".to_string())
}

/// `GET /fapi/v1/order` by `orderId` or `origClientOrderId`.
fn order_record(book: &Book, params: &HashMap<String, String>) -> Result<Value, Refusal> {
    let found = match (params.get("orderId"), params.get("origClientOrderId")) {
        (Some(id), _) => book.market_records.iter().find(|r| r["orderId"].to_string() == *id),
        (None, Some(client)) => book.market_records.iter().find(|r| r["clientOrderId"] == **client),
        (None, None) => None,
    };
    found.cloned().ok_or_else(no_such_order)
}

/// `GET /fapi/v1/algoOrder` by `algoId` or `clientAlgoId`.
fn algo_record(book: &Book, params: &HashMap<String, String>) -> Result<Value, Refusal> {
    let found = match (params.get("algoId"), params.get("clientAlgoId")) {
        (Some(id), _) => id.parse().ok().and_then(|id: i64| book.algo_records.get(&id)),
        (None, Some(client)) => book.algo_records.values().find(|r| r["clientAlgoId"] == **client),
        (None, None) => None,
    };
    found.cloned().ok_or_else(no_such_order)
}

fn exchange_info() -> Value {
    json!({ "symbols": [{
        "symbol": FAKE_SYMBOL,
        "status": "TRADING",
        "pricePrecision": 2,
        "quantityPrecision": 3,
        "filters": [
            { "filterType": "PRICE_FILTER", "tickSize": "0.01", "minPrice": "0.01" },
            { "filterType": "LOT_SIZE", "stepSize": "0.001", "minQty": "0.001", "maxQty": "1000000" },
            { "filterType": "MIN_NOTIONAL", "notional": "5" },
        ],
    }]})
}

/// Flat candles at the mark with two swing lows and two swing highs, so the
/// real analyzer finds a second support 4% from the close.
fn klines(params: &HashMap<String, String>) -> Value {
    let limit: i64 = params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100);
    let first_open = Utc::now().timestamp_millis() - limit * CANDLES_MS;
    let rows = (0..limit).map(|i| {
        let (low, high) = match limit - i {
            30 => ("98", "102"),
            60 => ("96", "104"),
            _ => ("100", "100"),
        };
        let open_time = first_open + i * CANDLES_MS;
        json!([
            open_time,
            "100",
            high,
            low,
            "100",
            "10",
            open_time + CANDLES_MS - 1,
            "1000",
            10,
            "5",
            "500",
            "0"
        ])
    });
    Value::Array(rows.collect())
}

fn position_risk(book: &Book) -> Value {
    let positions = book.positions.iter().filter(|(_, amount)| !amount.is_zero());
    Value::Array(
        positions
            .map(|(symbol, amount)| {
                json!({
                    "symbol": symbol,
                    "positionAmt": amount.to_string(),
                    "entryPrice": FAKE_MARK.to_string(),
                    "unRealizedProfit": "0",
                    "leverage": "1",
                })
            })
            .collect(),
    )
}

fn required<'a>(params: &'a HashMap<String, String>, name: &str) -> Result<&'a str, Refusal> {
    params
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, -1102, format!("missing {name}")))
}

fn decimal(params: &HashMap<String, String>, name: &str) -> Result<Decimal, Refusal> {
    required(params, name)?
        .parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, -1102, format!("invalid {name}")))
}

fn accept(
    book: &mut Book,
    params: &HashMap<String, String>,
    client_id: &str,
    conditional: bool,
) -> Result<(String, Decimal, bool), Refusal> {
    if book.orders.iter().any(|order| order.client_id == client_id) {
        return Err((StatusCode::BAD_REQUEST, -4116, "ClientOrderId is duplicated".to_string()));
    }
    let side = required(params, "side")?.to_string();
    let quantity = decimal(params, "quantity")?;
    let reduce_only = params.get("reduceOnly").is_some_and(|v| v == "true");
    book.orders.push(AcceptedOrder {
        client_id: client_id.to_string(),
        side: side.clone(),
        quantity,
        reduce_only,
        conditional,
    });
    book.counts.accepted_orders += 1;
    Ok((side, quantity, reduce_only))
}

fn place_market(book: &mut Book, params: &HashMap<String, String>) -> Result<Value, Refusal> {
    let symbol = required(params, "symbol")?.to_string();
    let client_id = required(params, "newClientOrderId")?.to_string();
    let open = book.positions.get(&symbol).copied().unwrap_or_default();
    let requested_reduce = params.get("reduceOnly").is_some_and(|v| v == "true");
    if requested_reduce && open.is_zero() {
        return Err((StatusCode::BAD_REQUEST, -2022, "ReduceOnly Order is rejected".to_string()));
    }
    let (side, quantity, _) = accept(book, params, &client_id, false)?;
    let signed = if side == "BUY" { quantity } else { -quantity };
    *book.positions.entry(symbol.clone()).or_default() += signed;
    book.counts.fills += 1;
    book.next_id += 1;
    let notional = quantity * FAKE_MARK;
    let answer = json!({
        "symbol": symbol,
        "orderId": book.next_id,
        "clientOrderId": client_id,
        "updateTime": Utc::now().timestamp_millis(),
        "price": "0",
        "avgPrice": FAKE_MARK.to_string(),
        "origQty": quantity.to_string(),
        "executedQty": quantity.to_string(),
        "cumQuote": notional.to_string(),
        "status": "FILLED",
        "side": side,
        "type": "MARKET",
    });
    book.market_records.push(answer.clone());
    Ok(answer)
}

fn place_algo(book: &mut Book, params: &HashMap<String, String>) -> Result<Value, Refusal> {
    if std::mem::take(&mut book.reject_next_algo) {
        return Err((StatusCode::BAD_REQUEST, -2010, "Order was rejected.".to_string()));
    }
    let client_id = required(params, "clientAlgoId")?.to_string();
    let (side, quantity, reduce_only) = accept(book, params, &client_id, true)?;
    book.next_id += 1;
    let now = Utc::now().timestamp_millis();
    let order = json!({
        "algoId": book.next_id,
        "clientAlgoId": client_id,
        "algoType": "CONDITIONAL",
        "orderType": "STOP_MARKET",
        "symbol": required(params, "symbol")?,
        "side": side,
        "quantity": quantity.to_string(),
        "algoStatus": "NEW",
        "triggerPrice": required(params, "triggerPrice")?,
        "reduceOnly": reduce_only,
        "createTime": now,
        "updateTime": now,
    });
    book.open_algos.insert(book.next_id, order.clone());
    book.algo_records.insert(book.next_id, order.clone());
    if let Some(hook) = book.algo_accepted.take() {
        let mut open: Vec<i64> = book.open_algos.keys().copied().collect();
        open.sort_unstable();
        hook(open.into_iter().map(|id| id.to_string()).collect());
    }
    Ok(order)
}

fn cancel_algo(book: &mut Book, params: &HashMap<String, String>) -> Result<Value, Refusal> {
    let algo_id: i64 = required(params, "algoId")?
        .parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, -1102, "invalid algoId".to_string()))?;
    let Some(order) = book.open_algos.remove(&algo_id) else {
        return Err((StatusCode::BAD_REQUEST, -2011, "Unknown order sent".to_string()));
    };
    book.counts.cancels += 1;
    if let Some(record) = book.algo_records.get_mut(&algo_id) {
        record["algoStatus"] = json!("CANCELED");
    }
    Ok(json!({
        "algoId": algo_id,
        "clientAlgoId": order["clientAlgoId"],
        "code": "200",
        "msg": "success",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn post(url: String) -> reqwest::Response {
        reqwest::Client::new().post(url).send().await.unwrap()
    }

    #[tokio::test]
    async fn counts_follow_what_the_venue_did() {
        let exchange = async {
            let venue = FakeVenue::start().await.unwrap();
            let base = venue.url();

            let order = |id: &str, side: &str, reduce: &str| {
                format!(
                    "{base}/fapi/v1/order?symbol={FAKE_SYMBOL}&side={side}&type=MARKET&quantity=2\
                 &newClientOrderId={id}&reduceOnly={reduce}"
                )
            };
            assert!(post(order("entry-1", "BUY", "false")).await.status().is_success());
            assert_eq!(venue.position(FAKE_SYMBOL), Decimal::from(2));

            // The same client id is refused and changes nothing.
            let duplicate = post(order("entry-1", "BUY", "false")).await;
            assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
            assert_eq!(venue.counts(), VenueCounts { accepted_orders: 1, fills: 1, cancels: 0 });

            let stop = post(format!(
            "{base}/fapi/v1/algoOrder?symbol={FAKE_SYMBOL}&side=SELL&quantity=2&triggerPrice=96\
             &reduceOnly=true&clientAlgoId=ins-1"
        ))
            .await
            .json::<Value>()
            .await
            .unwrap();
            let algo_id = stop["algoId"].as_i64().unwrap();
            let cancel = |id: i64| format!("{base}/fapi/v1/algoOrder?algoId={id}");
            let client = reqwest::Client::new();
            assert!(client.delete(cancel(algo_id)).send().await.unwrap().status().is_success());
            // Already gone: refused, not counted twice.
            assert_eq!(
                client.delete(cancel(algo_id)).send().await.unwrap().status(),
                StatusCode::BAD_REQUEST
            );
            assert_eq!(venue.counts(), VenueCounts { accepted_orders: 2, fills: 1, cancels: 1 });

            assert!(post(order("close-1", "SELL", "true")).await.status().is_success());
            assert!(venue.position(FAKE_SYMBOL).is_zero());
            assert_eq!(venue.counts(), VenueCounts { accepted_orders: 3, fills: 2, cancels: 1 });
            assert!(venue.unserved_requests().is_empty());

            assert_eq!(post(format!("{base}/nope")).await.status(), StatusCode::NOT_FOUND);
            assert_eq!(venue.unserved_requests(), ["POST /nope"]);
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), exchange)
            .await
            .expect("the venue self-test hung");
    }

    async fn get(url: String) -> (StatusCode, Value) {
        let answer = reqwest::Client::new().get(url).send().await.unwrap();
        (answer.status(), answer.json().await.unwrap())
    }

    /// Run U's lookups and controls (8d plan §4 notes ¹ and ⁴).
    #[tokio::test]
    async fn lookups_and_controls_for_run_u() {
        let venue = FakeVenue::start().await.unwrap();
        let base = venue.url();
        let algo = |id: &str| {
            format!(
                "{base}/fapi/v1/algoOrder?symbol={FAKE_SYMBOL}&side=SELL&quantity=1&triggerPrice=90\
                 &reduceOnly=true&clientAlgoId={id}"
            )
        };
        let market = format!(
            "{base}/fapi/v1/order?symbol={FAKE_SYMBOL}&side=BUY&type=MARKET&quantity=1\
             &newClientOrderId=m1&reduceOnly=false"
        );
        let filled: Value = post(market).await.json().await.unwrap();
        let (status, record) = get(format!("{base}/fapi/v1/order?origClientOrderId=m1")).await;
        assert_eq!((status, record), (StatusCode::OK, filled.clone()));
        let id = filled["orderId"].as_i64().unwrap();
        let (status, record) = get(format!("{base}/fapi/v1/order?orderId={id}")).await;
        assert_eq!((status, record["clientOrderId"].as_str()), (StatusCode::OK, Some("m1")));
        for unknown in [
            "/fapi/v1/order?origClientOrderId=never",
            "/fapi/v1/algoOrder?clientAlgoId=never",
        ] {
            let (status, body) = get(format!("{base}{unknown}")).await;
            assert_eq!((status, body["code"].as_i64()), (StatusCode::BAD_REQUEST, Some(-2013)));
        }

        // A refused conditional order: counted as a placement, nothing accepted.
        venue.reject_next_algo();
        let refused = post(algo("rej")).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert_eq!(refused.json::<Value>().await.unwrap()["code"], -2010);
        assert_eq!(venue.placement_requests(), 2);
        assert_eq!(venue.counts().accepted_orders, 1);
        let (status, _) = get(format!("{base}/fapi/v1/algoOrder?clientAlgoId=rej")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // The hook sees the open algos as the next one is accepted.
        let seen = Arc::new(Mutex::new(None));
        let saw = Arc::clone(&seen);
        venue.on_next_algo_accepted(Box::new(move |open| *saw.lock().unwrap() = Some(open)));
        let first: Value = post(algo("a1")).await.json().await.unwrap();
        let first_id = first["algoId"].to_string();
        assert_eq!(seen.lock().unwrap().clone(), Some(vec![first_id.clone()]));
        let second: Value = post(algo("a2")).await.json().await.unwrap();
        let second_id = second["algoId"].to_string();
        assert_eq!(venue.open_algo_ids(), [first_id.clone(), second_id.clone()]);

        // A partial trigger: the algo leaves the open list, its record names
        // the triggered order, which carries the execution.
        assert!(!venue.trigger_algo(&first_id, Decimal::from(2)), "more than its quantity");
        assert!(venue.trigger_algo(&first_id, Decimal::new(3, 1)));
        assert!(!venue.trigger_algo(&first_id, Decimal::ZERO), "no longer open");
        assert_eq!(venue.open_algo_ids(), [second_id]);
        let (_, record) = get(format!("{base}/fapi/v1/algoOrder?clientAlgoId=a1")).await;
        assert_eq!(record["algoStatus"], "FINISHED");
        let triggered = record["actualOrderId"].as_str().unwrap().to_string();
        let (status, order) = get(format!("{base}/fapi/v1/order?orderId={triggered}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (order["executedQty"].as_str(), order["status"].as_str()),
            (Some("0.3"), Some("EXPIRED"))
        );
        assert_eq!(venue.position(FAKE_SYMBOL), Decimal::new(7, 1));
        // The old algo's cancel is refused as before (-2011).
        let cancel = reqwest::Client::new()
            .delete(format!("{base}/fapi/v1/algoOrder?algoId={first_id}"))
            .send()
            .await
            .unwrap();
        assert_eq!(cancel.json::<Value>().await.unwrap()["code"], -2011);
        let (_, by_id) = get(format!("{base}/fapi/v1/algoOrder?algoId={first_id}")).await;
        assert_eq!(by_id["clientAlgoId"], "a1");
        assert!(venue.unserved_requests().is_empty());
    }
}
