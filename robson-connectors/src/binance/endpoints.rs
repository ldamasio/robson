//! Binance endpoints: adapter knowledge, not domain (ADR-0056 §4).

use robson_domain::Exchange;

/// Binance endpoints by credential identity.
pub struct Endpoints;

impl Endpoints {
    /// Get the base URL for API calls.
    ///
    /// Moved unchanged from `robson_domain::Exchange`, which had no production
    /// caller and still has none here. `BinanceRestClient` selects its host
    /// from its testnet flag and the request path, so this is not its host
    /// table: `Exchange::Binance` maps to the spot host and
    /// `Exchange::BinanceTestnet` to the USDⓈ-M testnet host.
    pub fn base_url(exchange: Exchange) -> &'static str {
        match exchange {
            Exchange::Binance => "https://api.binance.com",
            Exchange::BinanceTestnet => "https://testnet.binancefuture.com",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_base_url() {
        assert_eq!(Endpoints::base_url(Exchange::Binance), "https://api.binance.com");
        assert_eq!(
            Endpoints::base_url(Exchange::BinanceTestnet),
            "https://testnet.binancefuture.com"
        );
    }
}
