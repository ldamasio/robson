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

/// The closed set of inventory reads: fixed paths and weights, no parameter
/// but the caller's timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryRead {
    Account,
    AllOpenOrders,
    OpenOrderLists,
}

impl InventoryRead {
    pub const ALL: [Self; 3] = [Self::Account, Self::AllOpenOrders, Self::OpenOrderLists];

    pub fn path(self) -> &'static str {
        match self {
            Self::Account => "/api/v3/account",
            Self::AllOpenOrders => "/api/v3/openOrders",
            Self::OpenOrderLists => "/api/v3/openOrderList",
        }
    }

    pub fn weight(self) -> u32 {
        match self {
            Self::Account => 20,
            Self::AllOpenOrders => 80,
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

    /// The canonical HMAC GET of the commission binder, with the immutable
    /// caller timestamp as its only parameter, to `127.0.0.1:port` only.
    pub fn sign(self, port: u16, timestamp_ms: i64) -> Result<SignedRead, BinanceRestError> {
        if timestamp_ms <= 0 {
            return Err(BinanceRestError::Refused("invalid inventory timestamp".into()));
        }
        let endpoint = self.read.path();
        let query = format!("timestamp={timestamp_ms}");
        let mut mac = Hmac::<Sha256>::new_from_slice(LOOPBACK_SECRET.as_bytes())
            .map_err(|e| BinanceRestError::SignatureError(format!("HMAC error: {e}")))?;
        mac.update(query.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        let url = format!("http://127.0.0.1:{port}{endpoint}?{query}&signature={signature}",);
        let (transport, request) =
            self.transport.get(url).header("X-MBX-APIKEY", LOOPBACK_KEY).build_split();
        let request = request.map_err(|_| {
            BinanceRestError::Refused("inventory request construction failed".into())
        })?;
        Ok(SignedRead { transport, request })
    }
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
    let account: crate::binance::spot::SpotAccount =
        serde_json::from_slice(body).map_err(|_| refuse("invalid typed account"))?;
    Ok(account.uid)
}
