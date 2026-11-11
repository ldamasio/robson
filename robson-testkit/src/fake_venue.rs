//! A local stand-in for the Binance USD-M REST surface (ADR-0056 step 7).
//!
//! The real `robsond`, with the real adapter and REST client, talks to this
//! server over loopback. It fills market orders at once at a fixed mark,
//! rests conditional orders, and counts what it accepted, filled and
//! cancelled. A request it cannot serve is recorded in
//! [`FakeVenue::unserved_requests`], so a test can assert it never received
//! anything it did not understand.

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
        _ => {
            book.unserved.push(format!("{method} {path}"));
            Err((StatusCode::NOT_FOUND, -1, format!("fake venue cannot serve {path}")))
        },
    }
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
    Ok(json!({
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
    }))
}

fn place_algo(book: &mut Book, params: &HashMap<String, String>) -> Result<Value, Refusal> {
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
}
