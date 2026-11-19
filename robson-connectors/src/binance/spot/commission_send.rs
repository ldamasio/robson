//! Fixed prepared commission request for the controlled N5 owner.

use std::{future::Future, sync::Arc};

use reqwest::{Client, RequestBuilder, Response};
use robson_domain::Symbol;

use crate::binance_rest::{single_shot, BinanceRestClient, BinanceRestError};

pub struct PreparedSpotCommission {
    transport: Client,
    signer: Arc<BinanceRestClient>,
    symbol: String,
}

pub struct SignedSpotCommission {
    request: RequestBuilder,
}

impl BinanceRestClient {
    pub fn prepare_spot_commission(
        self: &Arc<Self>,
        symbol: &Symbol,
    ) -> Result<PreparedSpotCommission, BinanceRestError> {
        Ok(PreparedSpotCommission {
            transport: single_shot()?,
            signer: Arc::clone(self),
            symbol: symbol.as_pair(),
        })
    }
}

impl PreparedSpotCommission {
    pub fn sign(self, timestamp_ms: i64) -> Result<SignedSpotCommission, BinanceRestError> {
        let request =
            self.signer.bind_spot_commission(self.transport, &self.symbol, timestamp_ms)?;
        Ok(SignedSpotCommission { request })
    }
}

impl SignedSpotCommission {
    pub fn send(self) -> impl Future<Output = reqwest::Result<Response>> + Send {
        self.request.send()
    }
}
