//! The one-shot transport of the closed Spot reads (PS-4): one fresh,
//! never-used `single_shot` client per read (no redirect followed, nothing
//! retried), prepared by the owner's bounded job; a signed read bound to the
//! owner's immutable timestamp in the commission binder's canonical form (the
//! read's fixed parameters, then `timestamp`, signed exactly as sent); one
//! GET, sent when first polled, its response returned at its head.

use std::{future::Future, sync::Arc};

use reqwest::{Client, Request, Response};

use super::SpotPreparedRead;
use crate::binance_rest::{single_shot, BinanceRestClient, BinanceRestError};

/// One closed read with its own transport; consumed by `sign` or `unsigned`.
pub struct PreparedSpotRead {
    read: SpotPreparedRead,
    transport: Client,
    signer: Arc<BinanceRestClient>,
}

/// One bound GET on its own transport; consumed by `send`.
pub struct SpotReadRequest {
    transport: Client,
    request: Request,
}

fn refused(detail: &str) -> BinanceRestError {
    BinanceRestError::Refused(format!("{detail}, nothing sent"))
}

impl BinanceRestClient {
    /// Prepare one read of the closed set on a fresh transport, with this
    /// client's endpoint and key. Building the transport may block (TLS
    /// roots): the owner runs this on its bounded worker. Nothing is signed
    /// or sent.
    pub fn prepare_spot_read(
        self: &Arc<Self>,
        read: SpotPreparedRead,
    ) -> Result<PreparedSpotRead, BinanceRestError> {
        let (transport, signer) = (single_shot()?, Arc::clone(self));
        Ok(PreparedSpotRead { read, transport, signer })
    }
}

impl PreparedSpotRead {
    /// A signed read at the owner's timestamp, with the key header. A
    /// public read, or a timestamp that is not positive, is refused.
    pub fn sign(self, timestamp_ms: i64) -> Result<SpotReadRequest, BinanceRestError> {
        let Self { read, transport, signer } = self;
        if !read.request().signed || timestamp_ms <= 0 {
            return Err(refused("a signed read at a positive owner timestamp only"));
        }
        let stamp = format!("timestamp={timestamp_ms}");
        let query = read.query().map_or(stamp.clone(), |fixed| format!("{fixed}&{stamp}"));
        let (transport, request) = signer.bind_spot_read(transport, read.request().path, &query)?;
        Ok(SpotReadRequest { transport, request })
    }

    /// A public read: its fixed parameters only, no key, no timestamp.
    pub fn unsigned(self) -> Result<SpotReadRequest, BinanceRestError> {
        let (false, Some(query)) = (self.read.request().signed, self.read.query()) else {
            return Err(refused("a signed read needs the owner's timestamp"));
        };
        let path = self.read.request().path;
        let url = format!("{}{path}?{query}", self.signer.endpoint_base_url(path));
        let (transport, request) = self.transport.get(url).build_split();
        let request = request.map_err(|_| refused("read request construction failed"))?;
        Ok(SpotReadRequest { transport, request })
    }
}

impl SpotReadRequest {
    /// Send once, when first polled. Return the original response at its
    /// head; the owner reads its body under the same deadline and cap.
    pub fn send(self) -> impl Future<Output = reqwest::Result<Response>> + Send {
        self.transport.execute(self.request)
    }
}
