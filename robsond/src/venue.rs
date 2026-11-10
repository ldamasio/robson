//! Venue construction by configuration (ADR-0056 §3).
//!
//! [`VenueFactory`] is the only place that names a concrete adapter: from
//! `ROBSON_VENUE` it builds the venue, its feeds and its credential scope. A
//! runtime-selected venue is the sized dispatch enum [`VenueRuntime`] (§2),
//! which forwards to the adapter it holds; everything above it stays generic
//! over `Venue`.
//!
//! `ROBSON_VENUE` unset keeps the historical selection, including the fallback
//! to the stub exchange when credentials are absent or refused. An explicit
//! value never falls through to another venue.
//!
//! The stub built here exposes no funding surface. Funding sagas are durable
//! and shared through the database, and a simulator must not create or execute
//! one: a real saga resumed on simulated balances would skip its transfer and
//! persist a false `REFRESHED`, and a simulated saga could later be resumed by
//! a real venue.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use robson_connectors::{
    binance::usdm::{BinanceExchangeAdapter, BinanceOhlcvAdapter, BinanceUsdmTradeFeed},
    BinanceRestClient,
};
use robson_domain::{Exchange, OrderSide, Price, Quantity, Symbol, SymbolTradingRules};
use robson_exec::{
    gate::OperationGate,
    order_journal::OrderJournal,
    ports::{OpenOrderRecord, OrderResult, UserTradeRecord},
    venue::{
        AccountPort, Cash, EntryOrReduce, ExecutionCostEnvelope, FundingPort, InventoryPort, Mark,
        OrderLookup, PlacedStop, PositionAccountPort, ReplaceOutcome, SettlementPort,
        StopCancelOutcome, StopParams, StopRef, TradingPort, Venue, VenueCapabilities, VenueId,
    },
    ExecError, OhlcvPort, StubExchange, StubOhlcv, StubTradeFeed, TradeFeedPort,
};
use rust_decimal_macros::dec;
use tracing::info;

use crate::{
    config::{Config, Environment, VenueSelection},
    error::{DaemonError, DaemonResult},
};

// =============================================================================
// VenueRuntime
// =============================================================================

/// A venue selected at runtime: one sized type for the daemon to be generic
/// over, with one variant per venue that exists.
pub enum VenueRuntime {
    /// `binance:usdm`
    Usdm(BinanceExchangeAdapter),
    /// `stub`
    Stub(Box<StubExchange>),
}

macro_rules! forward {
    ($self:ident, $venue:ident => $call:expr) => {
        match $self {
            VenueRuntime::Usdm($venue) => $call,
            VenueRuntime::Stub($venue) => $call,
        }
    };
}

#[async_trait]
impl TradingPort for VenueRuntime {
    async fn trading_rules(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        forward!(self, venue => venue.trading_rules(symbol).await)
    }

    async fn trading_rules_preview(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        forward!(self, venue => venue.trading_rules_preview(symbol).await)
    }

    async fn trading_rules_fresh(&self, symbol: &Symbol) -> Result<SymbolTradingRules, ExecError> {
        forward!(self, venue => venue.trading_rules_fresh(symbol).await)
    }

    fn last_validated_rules(&self, symbol: &Symbol) -> Option<SymbolTradingRules> {
        forward!(self, venue => venue.last_validated_rules(symbol))
    }

    fn rules_refresh_due(&self, symbol: &Symbol) -> bool {
        forward!(self, venue => venue.rules_refresh_due(symbol))
    }

    async fn refresh_trading_rules(
        &self,
        symbol: &Symbol,
    ) -> Result<SymbolTradingRules, ExecError> {
        forward!(self, venue => venue.refresh_trading_rules(symbol).await)
    }

    async fn place_market_order(
        &self,
        symbol: &Symbol,
        side: OrderSide,
        quantity: Quantity,
        client_order_id: &str,
        intent: EntryOrReduce,
    ) -> Result<OrderResult, ExecError> {
        forward!(self, venue => {
            venue.place_market_order(symbol, side, quantity, client_order_id, intent).await
        })
    }

    async fn place_protection(
        &self,
        symbol: &Symbol,
        close_side: OrderSide,
        quantity: Quantity,
        trigger: Price,
        client_order_id: &str,
    ) -> Result<PlacedStop, ExecError> {
        forward!(self, venue => {
            venue.place_protection(symbol, close_side, quantity, trigger, client_order_id).await
        })
    }

    async fn cancel_protection(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<StopCancelOutcome, ExecError> {
        forward!(self, venue => venue.cancel_protection(symbol, stop).await)
    }

    async fn replace_protection(
        &self,
        symbol: &Symbol,
        old: &StopRef,
        new: StopParams,
    ) -> Result<ReplaceOutcome, ExecError> {
        forward!(self, venue => venue.replace_protection(symbol, old, new).await)
    }

    async fn cancel_order(&self, symbol: &Symbol, order_id: &str) -> Result<(), ExecError> {
        forward!(self, venue => venue.cancel_order(symbol, order_id).await)
    }

    async fn order_by_client_id(
        &self,
        symbol: &Symbol,
        client_order_id: &str,
    ) -> Result<OrderLookup, ExecError> {
        forward!(self, venue => venue.order_by_client_id(symbol, client_order_id).await)
    }

    async fn order_by_exchange_id(
        &self,
        symbol: &Symbol,
        order_id: &str,
    ) -> Result<Option<OrderResult>, ExecError> {
        forward!(self, venue => venue.order_by_exchange_id(symbol, order_id).await)
    }

    async fn protection_fill(
        &self,
        symbol: &Symbol,
        stop: &StopRef,
    ) -> Result<Option<OrderResult>, ExecError> {
        forward!(self, venue => venue.protection_fill(symbol, stop).await)
    }

    async fn open_protective_orders(
        &self,
        symbol: &Symbol,
    ) -> Result<Vec<OpenOrderRecord>, ExecError> {
        forward!(self, venue => venue.open_protective_orders(symbol).await)
    }

    async fn user_trades_since(
        &self,
        symbol: &Symbol,
        since: DateTime<Utc>,
        limit: u16,
    ) -> Result<Vec<UserTradeRecord>, ExecError> {
        forward!(self, venue => venue.user_trades_since(symbol, since, limit).await)
    }

    async fn price(&self, symbol: &Symbol) -> Result<Price, ExecError> {
        forward!(self, venue => venue.price(symbol).await)
    }

    async fn health(&self) -> Result<(), ExecError> {
        forward!(self, venue => venue.health().await)
    }
}

#[async_trait]
impl AccountPort for VenueRuntime {
    async fn cash(&self, asset: &str) -> Result<Cash, ExecError> {
        forward!(self, venue => venue.cash(asset).await)
    }

    async fn valuation(&self, symbol: &Symbol) -> Result<Mark, ExecError> {
        forward!(self, venue => venue.valuation(symbol).await)
    }

    async fn execution_costs(&self, symbol: &Symbol) -> Result<ExecutionCostEnvelope, ExecError> {
        forward!(self, venue => venue.execution_costs(symbol).await)
    }

    fn capabilities(&self) -> VenueCapabilities {
        forward!(self, venue => venue.capabilities())
    }
}

impl Venue for VenueRuntime {
    fn id(&self) -> &VenueId {
        forward!(self, venue => venue.id())
    }

    fn attach_order_journal(&self, journal: Arc<OrderJournal>) {
        forward!(self, venue => venue.attach_order_journal(journal))
    }

    fn attach_gate(&self, gate: Arc<OperationGate>) {
        forward!(self, venue => venue.attach_gate(gate))
    }

    fn position_account(&self) -> Option<&dyn PositionAccountPort> {
        forward!(self, venue => venue.position_account())
    }

    fn inventory(&self) -> Option<&dyn InventoryPort> {
        forward!(self, venue => venue.inventory())
    }

    fn settlement(&self) -> &dyn SettlementPort {
        forward!(self, venue => venue.settlement())
    }

    fn funding(&self) -> Option<&dyn FundingPort> {
        forward!(self, venue => venue.funding())
    }
}

// =============================================================================
// VenueFactory
// =============================================================================

/// Everything built for the selected venue.
pub struct VenueParts {
    pub venue: Arc<VenueRuntime>,
    /// The venue's live trade feed.
    pub market_data: Arc<dyn TradeFeedPort>,
    /// The venue's chart history, for its technical analysis.
    pub ohlcv: Arc<dyn OhlcvPort>,
    /// The credential identity the venue authenticates with (ADR-0056 §4);
    /// `None` when it uses no credentials.
    pub credential: Option<Exchange>,
    /// Where an older unfinished funding saga is recovered when the venue has
    /// no funding surface; `None` leaves it blocked (see
    /// [`funding_recovery_route`]).
    pub funding_recovery: Option<Arc<VenueRuntime>>,
}

/// Builds the selected venue. The only place that names a concrete adapter.
pub struct VenueFactory;

impl VenueFactory {
    /// Build from `config.venue`. Fails closed: an explicit selection that
    /// cannot be honoured is a startup error, never another venue.
    pub fn build(config: &Config) -> DaemonResult<VenueParts> {
        let credential = usdm_credential(config);
        let mut parts = match config.venue {
            VenueSelection::Unset => match credential {
                Some(credential) => build_usdm(config, credential),
                None => {
                    let has_credentials =
                        config.binance.api_key.is_some() && config.binance.api_secret.is_some();
                    if has_credentials && !config.binance.use_testnet {
                        tracing::error!(
                            "Binance credentials present but neither ROBSON_BINANCE_USE_TESTNET=true \
                             nor ROBSON_ENV=production is set. Refusing to connect. \
                             Falling back to StubExchange."
                        );
                    } else {
                        info!("Exchange: Stub (no Binance credentials)");
                    }
                    build_stub(usdm_trade_feed(config))
                },
            },
            VenueSelection::BinanceUsdm => match credential {
                Some(credential) => build_usdm(config, credential),
                None => {
                    return Err(DaemonError::Config(
                        "ROBSON_VENUE=binance_usdm requires Binance credentials and either \
                         ROBSON_BINANCE_USE_TESTNET=true or ROBSON_ENV=production. \
                         Refusing to start on any other venue."
                            .to_string(),
                    ));
                },
            },
            VenueSelection::Stub => {
                if config.environment == Environment::Production {
                    return Err(DaemonError::Config(
                        "ROBSON_VENUE=stub is not allowed with ROBSON_ENV=production".to_string(),
                    ));
                }
                info!("Exchange: Stub (ROBSON_VENUE=stub)");
                build_stub(Arc::new(StubTradeFeed::new()))
            },
        };
        parts.funding_recovery = funding_recovery_route(&parts.venue, config);
        Ok(parts)
    }
}

/// The runtime stub. No funding surface: see the module docs.
fn stub_venue() -> VenueRuntime {
    VenueRuntime::Stub(Box::new(StubExchange::new(dec!(95000)).without_funding()))
}

/// The credential identity USDⓈ-M may run on: credentials plus an environment
/// marker. `ROBSON_BINANCE_USE_TESTNET` wins, as it always has; without it
/// only `ROBSON_ENV=production` connects.
fn usdm_credential(config: &Config) -> Option<Exchange> {
    let has_credentials = config.binance.api_key.is_some() && config.binance.api_secret.is_some();
    if !has_credentials {
        None
    } else if config.binance.use_testnet {
        Some(Exchange::BinanceTestnet)
    } else if config.environment == Environment::Production {
        Some(Exchange::Binance)
    } else {
        None
    }
}

fn usdm_client(config: &Config, credential: Exchange) -> Option<Arc<BinanceRestClient>> {
    let api_key = config.binance.api_key.clone()?;
    let api_secret = config.binance.api_secret.clone()?;
    #[cfg(feature = "crash-points")]
    if let Ok(base_url) = std::env::var(robson_exec::crash_points::VENUE_URL_ENV) {
        return Some(Arc::new(BinanceRestClient::with_harness_base_url(
            base_url, api_key, api_secret,
        )));
    }
    Some(Arc::new(match credential {
        Exchange::Binance => BinanceRestClient::new(api_key, api_secret),
        Exchange::BinanceTestnet => BinanceRestClient::testnet(api_key, api_secret),
    }))
}

fn usdm_trade_feed(config: &Config) -> Arc<dyn TradeFeedPort> {
    Arc::new(BinanceUsdmTradeFeed::new(
        config.binance.use_testnet,
        config.market_data.ws_endpoints.clone(),
    ))
}

fn build_usdm(config: &Config, credential: Exchange) -> VenueParts {
    match credential {
        Exchange::Binance => info!("Exchange: Binance (production)"),
        Exchange::BinanceTestnet => info!("Exchange: Binance (testnet)"),
    }
    let client = usdm_client(config, credential)
        .expect("usdm_credential only returns an identity when credentials are present");
    VenueParts {
        venue: Arc::new(VenueRuntime::Usdm(BinanceExchangeAdapter::new(Arc::clone(&client)))),
        market_data: usdm_trade_feed(config),
        ohlcv: Arc::new(BinanceOhlcvAdapter::new(client)),
        credential: Some(credential),
        funding_recovery: None,
    }
}

/// The stub venue. Unset `ROBSON_VENUE` keeps the historical no-credentials
/// mode, the stub over the real public feed; explicit `stub` is fully in
/// memory.
fn build_stub(market_data: Arc<dyn TradeFeedPort>) -> VenueParts {
    VenueParts {
        venue: Arc::new(stub_venue()),
        market_data,
        ohlcv: Arc::new(StubOhlcv::default()),
        credential: None,
        funding_recovery: None,
    }
}

/// Where an older unfinished funding saga is recovered when `venue` has no
/// funding surface (ADR-0056 §3).
///
/// Sagas do not persist their route yet, so the route is implied:
/// `binance:usdm` is the only funding route that has ever existed, on the
/// credentials this process is configured with. `None` when `venue` has its own
/// funding surface, and when those credentials are missing, in which case the
/// saga stays visibly blocked with its liabilities retained. Building the
/// route sends nothing to the venue.
pub(crate) fn funding_recovery_route(
    venue: &VenueRuntime,
    config: &Config,
) -> Option<Arc<VenueRuntime>> {
    if venue.funding().is_some() {
        return None;
    }
    let client = usdm_client(config, usdm_credential(config)?)?;
    Some(Arc::new(VenueRuntime::Usdm(BinanceExchangeAdapter::new(client))))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use robson_connectors::binance::usdm::{
        MAINNET_MARKET_WS_ENDPOINT, TESTNET_MARKET_WS_ENDPOINT,
    };
    use robson_exec::venue::capability_mismatches;

    use super::*;

    fn config(
        venue: VenueSelection,
        credentials: bool,
        use_testnet: bool,
        environment: Environment,
    ) -> Config {
        let mut config = Config::test();
        config.venue = venue;
        config.binance.use_testnet = use_testnet;
        config.environment = environment;
        if credentials {
            config.binance.api_key = Some("key-fixture".to_string());
            config.binance.api_secret = Some("secret-fixture".to_string());
        }
        config
    }

    fn built(config: &Config) -> VenueParts {
        match VenueFactory::build(config) {
            Ok(parts) => parts,
            Err(error) => panic!("expected a venue: {error}"),
        }
    }

    fn refused(config: &Config) -> String {
        match VenueFactory::build(config) {
            Ok(parts) => panic!("expected a startup error, built {}", parts.venue.id()),
            Err(error) => error.to_string(),
        }
    }

    /// The table `main.rs` applied before `ROBSON_VENUE` existed: venue,
    /// credential identity and feed endpoints for every combination.
    #[test]
    fn unset_matches_the_historical_selection() {
        use Environment::{Development, Production};
        let cases = [
            // credentials + testnet marker: testnet, whatever the environment
            (true, true, Production, "binance:usdm", Some(Exchange::BinanceTestnet)),
            (true, true, Development, "binance:usdm", Some(Exchange::BinanceTestnet)),
            // credentials + production
            (true, false, Production, "binance:usdm", Some(Exchange::Binance)),
            // credentials in neither environment: refused, falls back to stub
            (true, false, Development, "stub", None),
            // no credentials: stub, in production as well
            (false, false, Production, "stub", None),
            (false, false, Development, "stub", None),
            (false, true, Development, "stub", None),
        ];
        for (credentials, use_testnet, environment, venue, credential) in cases {
            let parts =
                built(&config(VenueSelection::Unset, credentials, use_testnet, environment));
            let case = format!("{credentials} {use_testnet} {environment:?}");
            assert_eq!(parts.venue.id().as_str(), venue, "{case}");
            assert_eq!(parts.credential, credential, "{case}");
            // The stub fallback keeps the real public feed.
            let endpoint = if use_testnet {
                TESTNET_MARKET_WS_ENDPOINT
            } else {
                MAINNET_MARKET_WS_ENDPOINT
            };
            assert_eq!(parts.market_data.endpoints(), [endpoint], "{case}");
            assert!(parts.funding_recovery.is_none(), "{case}");
        }
    }

    #[test]
    fn configured_stream_bases_reach_the_feed() {
        let endpoints = vec!["wss://primary.example/market".to_string()];
        for credentials in [true, false] {
            let mut config =
                config(VenueSelection::Unset, credentials, false, Environment::Production);
            config.market_data.ws_endpoints = endpoints.clone();
            assert_eq!(built(&config).market_data.endpoints(), endpoints.as_slice());
        }
    }

    #[test]
    fn explicit_binance_usdm_runs_only_on_binance_usdm() {
        let testnet =
            built(&config(VenueSelection::BinanceUsdm, true, true, Environment::Development));
        assert_eq!(testnet.venue.id().as_str(), "binance:usdm");
        assert_eq!(testnet.credential, Some(Exchange::BinanceTestnet));

        let production =
            built(&config(VenueSelection::BinanceUsdm, true, false, Environment::Production));
        assert_eq!(production.venue.id().as_str(), "binance:usdm");
        assert_eq!(production.credential, Some(Exchange::Binance));

        // Where unset falls back to the stub, the explicit selection refuses.
        for (credentials, environment) in [
            (false, Environment::Production),
            (true, Environment::Development),
        ] {
            let error =
                refused(&config(VenueSelection::BinanceUsdm, credentials, false, environment));
            assert!(error.contains("ROBSON_VENUE=binance_usdm"), "{error}");
        }
    }

    #[test]
    fn explicit_stub_is_in_memory_and_never_production() {
        // Credentials present: the explicit selection still ignores them.
        let parts = built(&config(VenueSelection::Stub, true, false, Environment::Development));
        assert_eq!(parts.venue.id().as_str(), "stub");
        assert_eq!(parts.credential, None);
        assert_eq!(parts.market_data.endpoints(), [robson_exec::stub::STUB_FEED_ENDPOINT]);

        let error = refused(&config(VenueSelection::Stub, false, false, Environment::Production));
        assert!(error.contains("ROBSON_VENUE=stub"), "{error}");
    }

    #[test]
    fn every_factory_stub_has_no_funding_surface() {
        for (selection, credentials) in [
            (VenueSelection::Stub, false),
            (VenueSelection::Stub, true),
            (VenueSelection::Unset, false),
            // Credentials refused by the environment markers: the same stub.
            (VenueSelection::Unset, true),
        ] {
            let venue =
                built(&config(selection, credentials, false, Environment::Development)).venue;
            assert_eq!(venue.id().as_str(), "stub");
            assert!(venue.funding().is_none(), "{selection:?} {credentials}");
            assert!(!venue.capabilities().has_funding, "{selection:?} {credentials}");
            assert!(capability_mismatches(venue.as_ref()).is_empty());
        }
    }

    #[tokio::test]
    async fn the_runtime_forwards_to_the_venue_it_holds() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let stub = StubExchange::new(dec!(95000));
        // The port's default says rules are unavailable, so a missing forward
        // would surface here.
        stub.set_trading_rules(
            SymbolTradingRules::new(
                symbol.clone(),
                dec!(0.01),
                dec!(0),
                dec!(0.001),
                dec!(0.001),
                dec!(1000),
                dec!(0),
                2,
                3,
            )
            .unwrap(),
        );
        stub.set_rules_refresh_due(true);
        let runtime = VenueRuntime::Stub(Box::new(stub));
        assert_eq!(runtime.id().as_str(), "stub");
        assert_eq!(runtime.price(&symbol).await.unwrap().as_decimal(), dec!(95000));
        assert!(runtime.trading_rules(&symbol).await.is_ok());
        assert!(runtime.trading_rules_preview(&symbol).await.is_ok());
        // ADR-0056 §5: each default differs from what the stub answers here.
        assert!(runtime.last_validated_rules(&symbol).is_some());
        assert!(runtime.rules_refresh_due(&symbol));
        assert!(runtime.refresh_trading_rules(&symbol).await.is_ok());
        let VenueRuntime::Stub(held) = &runtime else {
            panic!("the runtime holds the stub");
        };
        assert_eq!(held.rules_refresh_call_count(), 1);
        held.set_fresh_rules_unavailable(true);
        assert!(runtime.trading_rules_fresh(&symbol).await.is_err());
        assert!(capability_mismatches(&runtime).is_empty());

        let without_funding =
            VenueRuntime::Stub(Box::new(StubExchange::new(dec!(95000)).without_funding()));
        assert!(without_funding.funding().is_none());
        assert!(!without_funding.capabilities().has_funding);
        assert!(capability_mismatches(&without_funding).is_empty());

        let usdm = built(&config(VenueSelection::BinanceUsdm, true, true, Environment::Test)).venue;
        assert_eq!(usdm.id().as_str(), VenueId::BINANCE_USDM);
        assert!(usdm.funding().is_some());
        assert!(capability_mismatches(usdm.as_ref()).is_empty());
    }

    #[test]
    fn funding_recovery_route_is_the_implied_usdm_route_when_credentials_allow() {
        let no_funding = stub_venue();

        // A venue with funding recovers its own sagas.
        let usdm_config = config(VenueSelection::Unset, true, false, Environment::Production);
        let funded = VenueRuntime::Stub(Box::new(StubExchange::new(dec!(95000))));
        assert!(funding_recovery_route(&funded, &usdm_config).is_none());

        for use_testnet in [false, true] {
            let config = config(VenueSelection::Unset, true, use_testnet, Environment::Production);
            let route = funding_recovery_route(&no_funding, &config).expect("route");
            assert_eq!(route.id().as_str(), VenueId::BINANCE_USDM);
            assert!(route.funding().is_some());
        }

        // Original credentials missing or refused: no route, blocked.
        for (credentials, environment) in [
            (false, Environment::Production),
            (true, Environment::Development),
        ] {
            let config = config(VenueSelection::Unset, credentials, false, environment);
            assert!(funding_recovery_route(&no_funding, &config).is_none());
        }
    }
}
