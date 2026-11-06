//! ADR-0056 step 6: what each order kind sends, observed on the wire.
//!
//! `testdata/adr0056_step6_before.json` was written by this harness on the
//! code before the quantisation change. The requests go through the real
//! adapter and REST client to a local listener standing in for Binance, which
//! accepts every order, so a row shows what Robson itself decided to send, and
//! which requests it made to get there.

use std::sync::{Arc, Mutex};

use robson_domain::{OrderSide, Price, Quantity, Side, Symbol};
use robson_exec::venue::{EntryOrReduce, PositionAccountPort, TradingPort};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use crate::{
    binance::usdm::{normalize_market_quantity, BinanceExchangeAdapter},
    binance_rest::BinanceRestClient,
};

const QUANTITIES: [&str; 9] = [
    "0.0005", "0.001", "0.0015", "0.0219", "0.105", "120", "120.001", "1000", "1000.001",
];
const ALIGNED_TRIGGER: &str = "62873.9";
const UNALIGNED_TRIGGER: &str = "62873.86105";
const STATES: [RulesState; 4] = [
    RulesState::WarmBtc,
    RulesState::ColdBtc,
    RulesState::Unavailable,
    RulesState::WarmStep001,
];

/// What the venue's metadata looks like when the order is made.
#[derive(Debug, Clone, Copy)]
enum RulesState {
    /// Rules cached before the order; `BTCUSDT` USDⓈ-M filters of
    /// `docs/evidence/2026-11-04-binance-exchangeinfo-btcusdt.md`.
    WarmBtc,
    /// Nothing cached; `exchangeInfo` would answer with the same filters.
    ColdBtc,
    /// Nothing cached; `exchangeInfo` fails.
    Unavailable,
    /// Rules cached with a lot step of 0.01, which the old hard-coded table
    /// does not know about.
    WarmStep001,
}

impl RulesState {
    fn name(self) -> &'static str {
        match self {
            Self::WarmBtc => "warm_btc",
            Self::ColdBtc => "cold_btc",
            Self::Unavailable => "unavailable",
            Self::WarmStep001 => "warm_step_0_01",
        }
    }

    fn exchange_info(self) -> Option<String> {
        let (step, min_qty) = match self {
            Self::Unavailable => return None,
            Self::WarmBtc | Self::ColdBtc => ("0.001", "0.001"),
            Self::WarmStep001 => ("0.01", "0.01"),
        };
        Some(format!(
            r#"{{"symbols":[{{"symbol":"BTCUSDT","pricePrecision":2,"quantityPrecision":3,"filters":[
            {{"filterType":"PRICE_FILTER","minPrice":"556.80","maxPrice":"4529764","tickSize":"0.10"}},
            {{"filterType":"LOT_SIZE","minQty":"{min_qty}","maxQty":"1000","stepSize":"{step}"}},
            {{"filterType":"MARKET_LOT_SIZE","minQty":"{min_qty}","maxQty":"120","stepSize":"{step}"}},
            {{"filterType":"MIN_NOTIONAL","notional":"50"}}]}}]}}"#
        ))
    }

    fn warm(self) -> bool {
        matches!(self, Self::WarmBtc | Self::WarmStep001)
    }
}

fn query_param(target: &str, name: &str) -> Option<String> {
    target
        .split(['?', '&'])
        .find_map(|part| part.strip_prefix(&format!("{name}=")))
        .map(str::to_string)
}

/// A local stand-in for Binance that accepts every order, recording requests.
async fn fake_venue(state: RulesState) -> (Arc<BinanceExchangeAdapter>, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let exchange_info = state.exchange_info();

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
            let mut line = head.split_whitespace();
            let (method, target) =
                (line.next().unwrap_or_default(), line.next().unwrap_or_default());
            let path = target.split('?').next().unwrap_or_default();
            recorded.lock().unwrap().push(format!("{method} {path}"));

            let quantity = query_param(target, "quantity").unwrap_or_default();
            let trigger = query_param(target, "triggerPrice").unwrap_or_default();
            let (status, body) = match path {
                "/fapi/v1/exchangeInfo" => match &exchange_info {
                    Some(body) => (200, body.clone()),
                    None => (500, "unavailable".to_string()),
                },
                "/fapi/v1/order" => (
                    200,
                    format!(
                        r#"{{"symbol":"BTCUSDT","orderId":7,"clientOrderId":"c1",
                        "updateTime":1700000000000,"price":"0","origQty":"{quantity}",
                        "executedQty":"{quantity}","cumQuote":"1000","status":"FILLED",
                        "side":"SELL","type":"MARKET","fills":[]}}"#
                    ),
                ),
                "/fapi/v1/algoOrder" => (
                    200,
                    format!(
                        r#"{{"algoId":9,"clientAlgoId":"ins-1","algoType":"CONDITIONAL",
                        "orderType":"STOP_MARKET","symbol":"BTCUSDT","side":"SELL",
                        "quantity":"{quantity}","algoStatus":"NEW","triggerPrice":"{trigger}",
                        "reduceOnly":true,"createTime":1700000000000,"updateTime":1700000000000}}"#
                    ),
                ),
                _ => (404, String::new()),
            };
            recorded.lock().unwrap().extend(
                ["quantity", "triggerPrice"]
                    .into_iter()
                    .filter_map(|name| Some(format!("{name}={}", query_param(target, name)?))),
            );
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });

    let client = BinanceRestClient::with_local_base_url(url);
    (Arc::new(BinanceExchangeAdapter::new(Arc::new(client))), requests)
}

fn symbol() -> Symbol {
    Symbol::from_pair("BTCUSDT").unwrap()
}

fn qty(value: &str) -> Quantity {
    Quantity::new(value.parse().unwrap()).unwrap()
}

fn px(value: &str) -> Price {
    Price::new(value.parse().unwrap()).unwrap()
}

/// Row of the fixture: the input, the requests in order with the quantity and
/// trigger each order carried, and the outcome.
fn row(
    kind: &str,
    state: RulesState,
    quantity: &str,
    extra: Value,
    requests: Vec<String>,
    outcome: Result<(), String>,
) -> Value {
    json!({
        "kind": kind,
        "rules": state.name(),
        "qty": quantity,
        "extra": extra,
        "requests": requests,
        "outcome": match outcome { Ok(()) => "accepted".to_string(), Err(error) => error },
    })
}

async fn observe<F, Fut>(
    kind: &str,
    state: RulesState,
    quantity: &str,
    extra: Value,
    order: F,
) -> Value
where
    F: FnOnce(Arc<BinanceExchangeAdapter>) -> Fut,
    Fut: std::future::Future<Output = Result<(), robson_exec::ExecError>>,
{
    let (venue, requests) = fake_venue(state).await;
    if state.warm() {
        TradingPort::trading_rules(&*venue, &symbol()).await.unwrap();
        requests.lock().unwrap().clear();
    }
    let outcome = order(venue).await.map_err(|error| error.to_string());
    let requests = requests.lock().unwrap().clone();
    row(kind, state, quantity, extra, requests, outcome)
}

async fn observe_all_rows() -> Vec<Value> {
    let mut rows = Vec::new();
    for state in STATES {
        for quantity in QUANTITIES {
            // The position manager's check before any exchange submission. It
            // consults no rules and no price on the old code.
            for reference_price in ["77500", "30000"] {
                rows.push(
                    observe(
                        "entry_precheck",
                        state,
                        quantity,
                        json!({ "reference_price": reference_price }),
                        |_venue| async move {
                            normalize_market_quantity(&symbol(), qty(quantity)).map(|_| ())
                        },
                    )
                    .await,
                );
            }
            rows.push(
                observe("entry", state, quantity, Value::Null, |venue| async move {
                    TradingPort::place_market_order(
                        &*venue,
                        &symbol(),
                        OrderSide::Buy,
                        qty(quantity),
                        "entry-1",
                        EntryOrReduce::Entry,
                    )
                    .await
                    .map(|_| ())
                })
                .await,
            );
            rows.push(
                observe("exit", state, quantity, Value::Null, |venue| async move {
                    TradingPort::place_market_order(
                        &*venue,
                        &symbol(),
                        OrderSide::Sell,
                        qty(quantity),
                        "exit-1",
                        EntryOrReduce::Reduce,
                    )
                    .await
                    .map(|_| ())
                })
                .await,
            );
            rows.push(
                observe("untracked_close", state, quantity, Value::Null, |venue| async move {
                    PositionAccountPort::close_position_market(
                        &*venue,
                        &symbol(),
                        Side::Long,
                        qty(quantity),
                        "close-1",
                    )
                    .await
                    .map(|_| ())
                })
                .await,
            );
            rows.push(
                observe(
                    "protective_stop",
                    state,
                    quantity,
                    json!({ "trigger": ALIGNED_TRIGGER }),
                    |venue| async move {
                        venue
                            .place_protection(
                                &symbol(),
                                OrderSide::Sell,
                                qty(quantity),
                                px(ALIGNED_TRIGGER),
                                "ins-1",
                            )
                            .await
                            .map(|_| ())
                    },
                )
                .await,
            );
        }
        // A legacy-policy trigger off the tick grid.
        rows.push(
            observe(
                "protective_stop",
                state,
                "0.105",
                json!({ "trigger": UNALIGNED_TRIGGER }),
                |venue| async move {
                    venue
                        .place_protection(
                            &symbol(),
                            OrderSide::Sell,
                            qty("0.105"),
                            px(UNALIGNED_TRIGGER),
                            "ins-2",
                        )
                        .await
                        .map(|_| ())
                },
            )
            .await,
        );
    }
    rows
}

fn fixture_rows(fixture: &str) -> Vec<Value> {
    let parsed: Value = serde_json::from_str(fixture).unwrap();
    parsed["rows"].as_array().unwrap().clone()
}

#[tokio::test]
async fn order_kinds_on_the_wire_match_the_before_fixture() {
    let observed = observe_all_rows().await;
    let expected = fixture_rows(include_str!("testdata/adr0056_step6_before.json"));

    assert_eq!(observed.len(), expected.len());
    for (observed, expected) in observed.iter().zip(&expected) {
        assert_eq!(observed, expected);
    }
}
