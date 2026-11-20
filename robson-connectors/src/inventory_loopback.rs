#![cfg(any(test, feature = "crash-points"))]
//! Test and crash-points only: the C8 signed inventory reads over literal
//! loopback with fixed synthetic credentials. No caller URL, key, secret or
//! signer is accepted, and nothing here can reach a venue. The normal
//! `single_shot` transport and the production signers are untouched.

use std::future::Future;

use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::Sha256;

use crate::BinanceRestError;

pub const LOOPBACK_KEY: &str = "c8-synthetic-key";
pub(crate) const LOOPBACK_SECRET: &str = "c8-synthetic-secret";
/// Synthetic identity answered by loopback fixtures; never a real account.
pub const LOOPBACK_UID: u64 = 4_200_000_008;
/// Explicit test bound for one loopback ACCOUNT body; not a production cap.
pub const LOOPBACK_BODY_LIMIT: usize = 64 * 1024;

/// The one pair whose open orders the inventory reads (8k-1 option B): a
/// fixed constant, never a caller's symbol.
pub const OPERATED_SYMBOL: &str = "BTCUSDT";

/// The closed set of inventory reads: fixed paths, fixed parameters and
/// weights. The balances and the account-wide order lists are read whole;
/// open orders only for [`OPERATED_SYMBOL`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryRead {
    Account,
    OperatedOpenOrders,
    OpenOrderLists,
}

impl InventoryRead {
    pub const ALL: [Self; 3] = [
        Self::Account,
        Self::OperatedOpenOrders,
        Self::OpenOrderLists,
    ];

    pub fn path(self) -> &'static str {
        match self {
            Self::Account => "/api/v3/account",
            Self::OperatedOpenOrders => "/api/v3/openOrders",
            Self::OpenOrderLists => "/api/v3/openOrderList",
        }
    }

    /// The fixed parameters before the caller's timestamp, if any.
    pub fn query(self) -> Option<&'static str> {
        match self {
            Self::OperatedOpenOrders => Some("symbol=BTCUSDT"),
            Self::Account | Self::OpenOrderLists => None,
        }
    }

    /// The documented weight: 6 for a single symbol's open orders.
    pub fn weight(self) -> u32 {
        match self {
            Self::Account => 20,
            Self::OperatedOpenOrders => 6,
            Self::OpenOrderLists => 6,
        }
    }
}

/// One client per request: no proxy (a literal loopback URL cannot leave the
/// host through a configured proxy), no redirect, no retry.
fn loopback_transport() -> Result<Client, BinanceRestError> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|error| {
            BinanceRestError::Refused(format!("no loopback transport, nothing sent: {error}"))
        })
}

/// One read with its own loopback transport; moved into [`Self::sign`].
pub struct PreparedRead {
    read: InventoryRead,
    transport: Client,
}

/// One signed request bound to its transport; moved into [`Self::send`].
pub struct SignedRead {
    transport: Client,
    request: reqwest::Request,
}

pub fn prepare(read: InventoryRead) -> Result<PreparedRead, BinanceRestError> {
    Ok(PreparedRead { read, transport: loopback_transport()? })
}

impl PreparedRead {
    pub fn read(&self) -> InventoryRead {
        self.read
    }

    /// The canonical HMAC GET of the commission binder: the read's fixed
    /// parameters, then the immutable caller timestamp, to `127.0.0.1:port`
    /// only.
    pub fn sign(self, port: u16, timestamp_ms: i64) -> Result<SignedRead, BinanceRestError> {
        let query = match self.read.query() {
            Some(fixed) => format!("{fixed}&timestamp={timestamp_ms}"),
            None => format!("timestamp={timestamp_ms}"),
        };
        sign_loopback(self.transport, port, self.read.path(), &query, timestamp_ms)
    }
}

/// One closed native-order request for the held fixture.
pub struct PreparedOwnedOrder(Client);

pub const OWNED_ORDER_SYMBOL: &str = "BTCUSDT";
pub const OWNED_ORDER_ID: u64 = 417013;
pub const OWNED_ORDER_WEIGHT: u32 = crate::binance::spot::SpotRead::ORDER.weight;

pub fn prepare_owned_order() -> Result<PreparedOwnedOrder, BinanceRestError> {
    loopback_transport().map(PreparedOwnedOrder)
}

impl PreparedOwnedOrder {
    pub fn sign(self, port: u16, timestamp_ms: i64) -> Result<SignedRead, BinanceRestError> {
        sign_loopback(
            self.0,
            port,
            crate::binance::spot::SpotRead::ORDER.path,
            &format!(
                "symbol={OWNED_ORDER_SYMBOL}&orderId={OWNED_ORDER_ID}&timestamp={timestamp_ms}"
            ),
            timestamp_ms,
        )
    }
}

fn sign_loopback(
    transport: Client,
    port: u16,
    endpoint: &str,
    query: &str,
    timestamp_ms: i64,
) -> Result<SignedRead, BinanceRestError> {
    if timestamp_ms <= 0 {
        return Err(BinanceRestError::Refused("invalid inventory timestamp".into()));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(LOOPBACK_SECRET.as_bytes())
        .map_err(|e| BinanceRestError::SignatureError(format!("HMAC error: {e}")))?;
    mac.update(query.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());
    let url = format!("http://127.0.0.1:{port}{endpoint}?{query}&signature={signature}");
    let (transport, request) =
        transport.get(url).header("X-MBX-APIKEY", LOOPBACK_KEY).build_split();
    let request = request
        .map_err(|_| BinanceRestError::Refused("inventory request construction failed".into()))?;
    Ok(SignedRead { transport, request })
}

pub fn decode_owned_order(
    status: u16,
    body: &[u8],
) -> Result<robson_exec::venue::OrderObservation, BinanceRestError> {
    let refuse = || BinanceRestError::Refused("invalid fixture order response".into());
    if body.len() > LOOPBACK_BODY_LIMIT {
        return Err(refuse());
    }
    let body = std::str::from_utf8(body).map_err(|_| refuse())?;
    crate::binance::spot::decode_fixture_order(status, body, OWNED_ORDER_SYMBOL, OWNED_ORDER_ID)
        .map_err(|_| refuse())
}

impl SignedRead {
    /// One request, sent when first polled; the original head, body unread.
    pub fn send(self) -> impl Future<Output = Result<reqwest::Response, BinanceRestError>> {
        let execute = self.execute();
        async move {
            execute
                .await
                .map_err(|e| BinanceRestError::RequestFailed(e.without_url().to_string()))
        }
    }

    /// The same single request with reqwest's own result, for a caller whose
    /// first poll is admitted by its owner.
    pub fn execute(self) -> impl Future<Output = reqwest::Result<reqwest::Response>> {
        let SignedRead { transport, request } = self;
        async move { transport.execute(request).await }
    }
}

/// Strict decode of one complete, bounded ACCOUNT body to its `uid`: one JSON
/// object, no trailing bytes, every documented field present with its type.
/// Nothing but the identity leaves this function.
pub fn account_uid(body: &[u8]) -> Result<u64, BinanceRestError> {
    let refuse = |detail: &str| BinanceRestError::Refused(format!("loopback account: {detail}"));
    if body.len() > LOOPBACK_BODY_LIMIT {
        return Err(refuse("body over the test bound"));
    }
    let text = std::str::from_utf8(body).map_err(|_| refuse("not UTF-8"))?;
    let account = crate::binance::spot::decode_account(text, LOOPBACK_BODY_LIMIT)
        .map_err(|_| refuse("invalid typed account"))?;
    Ok(account.uid)
}

/// One strictly decoded inventory body, typed by the read that fetched it.
/// Descriptive only: no balance, order or list here carries authority.
#[derive(Debug)]
pub enum InventoryBody {
    Account(crate::binance::spot::SpotAccount),
    OpenOrders(Vec<crate::binance::spot::SpotOrderRecord>),
    OpenOrderLists(Vec<crate::binance::spot::SpotOpenOrderList>),
}

impl InventoryBody {
    pub fn read(&self) -> InventoryRead {
        match self {
            Self::Account(_) => InventoryRead::Account,
            Self::OpenOrders(_) => InventoryRead::OperatedOpenOrders,
            Self::OpenOrderLists(_) => InventoryRead::OpenOrderLists,
        }
    }
}

/// Strict decode of one complete, bounded body by the read that fetched it,
/// all or nothing. Refusals carry no input text. A symbol-scoped answer with
/// an order on any other symbol is outside its own request and is refused
/// whole, as the production reader does.
pub fn decode(read: InventoryRead, body: &[u8]) -> Result<InventoryBody, BinanceRestError> {
    use crate::binance::spot;
    let refuse = |detail: &str| BinanceRestError::Refused(format!("loopback inventory: {detail}"));
    if body.len() > LOOPBACK_BODY_LIMIT {
        return Err(refuse("body over the test bound"));
    }
    let text = std::str::from_utf8(body).map_err(|_| refuse("not UTF-8"))?;
    match read {
        InventoryRead::Account => spot::decode_account(text, LOOPBACK_BODY_LIMIT)
            .map(InventoryBody::Account)
            .map_err(|_| refuse("invalid typed account")),
        InventoryRead::OperatedOpenOrders => {
            let orders = spot::decode_open_orders(text, LOOPBACK_BODY_LIMIT)
                .map_err(|_| refuse("invalid typed open orders"))?;
            if orders.iter().any(|order| order.symbol != OPERATED_SYMBOL) {
                return Err(refuse("open orders outside the operated symbol"));
            }
            Ok(InventoryBody::OpenOrders(orders))
        },
        InventoryRead::OpenOrderLists => spot::decode_open_order_lists(text, LOOPBACK_BODY_LIMIT)
            .map(InventoryBody::OpenOrderLists)
            .map_err(|_| refuse("invalid typed open order lists")),
    }
}

#[cfg(test)]
mod tests {
    use robson_exec::venue::OrderObservation;

    use super::*;

    #[tokio::test]
    async fn owned_order_signature_binds_the_fixed_symbol_and_id() {
        let signed = prepare_owned_order().unwrap().sign(1234, 1500).unwrap();
        assert_eq!(signed.request.method(), reqwest::Method::GET);
        assert_eq!(signed.request.url().host_str(), Some("127.0.0.1"));
        assert_eq!(signed.request.url().port(), Some(1234));
        assert_eq!(signed.request.url().path(), "/api/v3/order");
        assert_eq!(signed.request.headers()["X-MBX-APIKEY"], LOOPBACK_KEY);
        let query = "symbol=BTCUSDT&orderId=417013&timestamp=1500";
        let mut mac = Hmac::<Sha256>::new_from_slice(LOOPBACK_SECRET.as_bytes()).unwrap();
        mac.update(query.as_bytes());
        assert_eq!(
            signed.request.url().query(),
            Some(
                format!("{query}&signature={}", hex::encode(mac.finalize().into_bytes())).as_str()
            )
        );
        assert_eq!(OWNED_ORDER_WEIGHT, 4);
    }

    #[test]
    fn owned_order_body_requires_exact_native_id_and_unresolved_status_pairs() {
        let positive = r#"{"symbol":"BTCUSDT","orderId":417013,"orderListId":-1,"clientOrderId":"fixture","price":"0","origQty":"1","executedQty":"0.25","cummulativeQuoteQty":"25","status":"PARTIALLY_FILLED","timeInForce":"GTC","type":"MARKET","side":"BUY","stopPrice":"0","time":1,"updateTime":2,"workingTime":-1,"selfTradePreventionMode":"EXPIRE_MAKER"}"#;
        let found = decode_owned_order(200, positive.as_bytes()).unwrap();
        assert!(
            matches!(found, OrderObservation::Found(record) if record.exchange_order_id == "417013")
        );
        for bad in [
            positive.replace("417013", "417014"),
            positive.replace("BTCUSDT", "AAABBB"),
            positive.replace("0.25", "2"),
        ] {
            assert!(decode_owned_order(200, bad.as_bytes()).is_err());
        }
        let missing = br#"{"code":-2013,"msg":"fixture"}"#;
        let archived = br#"{"code":-2026,"msg":"fixture"}"#;
        assert!(matches!(decode_owned_order(400, missing), Ok(OrderObservation::NotFound)));
        assert!(matches!(decode_owned_order(400, archived), Ok(OrderObservation::Archived)));
        assert!(decode_owned_order(500, missing).is_err());
        assert!(decode_owned_order(404, archived).is_err());
        assert!(decode_owned_order(400, b"").is_err());
    }
}
