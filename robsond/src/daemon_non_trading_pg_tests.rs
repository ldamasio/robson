#![cfg(all(test, feature = "postgres"))]
//! PS-1: the non-trading Spot start kind on the daemon (PLAN-r4 §6.1, §6.2,
//! §6.9): the constructor's refusals, an ordinary epoch with F seeded, the
//! stop before the TIME owner with no wire I/O, and the unchanged refusal of
//! a Spot daemon built without a start kind.

use std::sync::Arc;

use robson_connectors::{
    binance::{spot::BinanceSpotAdapter, usdm::BinanceExchangeAdapter},
    binance_rest::BinanceRestClient,
};
use robson_exec::{
    gate::{IncidentReason, ReadOnlyReason},
    StubOhlcv, StubTradeFeed,
};
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

use crate::{Config, Daemon, DaemonError};

const NOT_WIRED: &str = "Spot TIME owner not wired";

fn config(tenant: Option<Uuid>) -> Config {
    let mut config = Config::test();
    config.projection.tenant_id = tenant;
    config
}

/// A pool that is never connected: construction must not need one.
fn unconnected() -> Arc<PgPool> {
    Arc::new(PgPoolOptions::new().connect_lazy("postgres://robson@127.0.0.1:9/none").unwrap())
}

fn client() -> Arc<BinanceRestClient> {
    Arc::new(BinanceRestClient::new("fixture-key".into(), "fixture-secret".into()))
}

fn spot(
    tenant: Option<Uuid>,
    pool: Arc<PgPool>,
    client: Arc<BinanceRestClient>,
    venue: Arc<BinanceSpotAdapter>,
) -> crate::DaemonResult<Daemon<BinanceSpotAdapter, robson_store::MemoryStore>> {
    Daemon::non_trading_spot(
        config(tenant),
        venue,
        Arc::new(StubTradeFeed::default()),
        Arc::new(StubOhlcv::default()),
        None,
        pool,
        client,
    )
}

fn refused<T>(built: crate::DaemonResult<T>) -> bool {
    matches!(built, Err(DaemonError::Config(reason)) if reason.starts_with("A non-trading start needs"))
}

/// Only `binance:spot`, with a tenant and a closed client, starts non-trading;
/// its gate refuses every mutation from construction. USD-M, a missing tenant
/// and an open client are refused, nothing built.
#[tokio::test]
async fn only_binance_spot_starts_non_trading() {
    let closed = client();
    let venue = Arc::new(BinanceSpotAdapter::non_trading(Arc::clone(&closed)).unwrap());
    let tenant = Some(Uuid::now_v7());
    let daemon = spot(tenant, unconnected(), Arc::clone(&closed), Arc::clone(&venue)).unwrap();
    assert_eq!(daemon.order_journal.gate().read_only(), Some(ReadOnlyReason::NonTrading));
    assert!(daemon.order_journal.gate().epoch().is_none());

    assert!(refused(spot(None, unconnected(), Arc::clone(&closed), Arc::clone(&venue))));
    assert!(refused(spot(tenant, unconnected(), client(), venue)));
    let usdm = client();
    usdm.close_unbudgeted_sends();
    let futures = Daemon::non_trading_spot(
        config(tenant),
        Arc::new(BinanceExchangeAdapter::new(Arc::clone(&usdm))),
        Arc::new(StubTradeFeed::default()),
        Arc::new(StubOhlcv::default()),
        None,
        unconnected(),
        usdm,
    );
    assert!(refused(futures));
}

/// A Spot daemon built the ordinary way (today's `main`) still refuses before
/// any epoch: no start kind, no start, nothing written.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_spot_daemon_without_a_start_kind_still_refuses(pool: PgPool) {
    let tenant = Uuid::now_v7();
    for client in [client(), {
        let closed = client();
        closed.close_unbudgeted_sends();
        closed
    }] {
        let daemon = Daemon::with_venue_and_recovery(
            config(Some(tenant)),
            Arc::new(BinanceSpotAdapter::new(client).unwrap()),
            Arc::new(StubTradeFeed::default()),
            Arc::new(StubOhlcv::default()),
            None,
            Some(Arc::new(pool.clone())),
        );
        assert!(daemon.order_journal.gate().read_only().is_none());
        let started = Box::pin(daemon.run_until(crate::stop::StopRequest::default())).await;
        assert!(
            matches!(&started, Err(DaemonError::Config(reason))
                if reason == "Spot startup requires a supervised journal"),
            "{started:?}"
        );
    }
    let epochs: i64 = sqlx::query_scalar("SELECT count(*) FROM process_epochs WHERE tenant_id=$1")
        .bind(tenant)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(epochs, 0, "no epoch without a start kind");
}

/// A non-trading start inserts an ordinary epoch, seeds the gate and F from
/// it, then stops before the TIME owner (until PS-6): no venue request, the
/// epoch left unmarked. Each start is unclean, the first for want of a
/// predecessor and the second for its unmarked one: `unclean_start` is open
/// and the fence closed, and every mutation is refused.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_non_trading_start_inserts_an_ordinary_epoch_then_stops(pool: PgPool) {
    #[cfg(feature = "crash-points")]
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    #[cfg(feature = "crash-points")]
    let base = format!("http://{}", listener.local_addr().unwrap());
    let tenant = Uuid::now_v7();
    for (start, why) in [(1_i64, "no_predecessor"), (2, "no_marker")] {
        #[cfg(feature = "crash-points")]
        let closed = Arc::new(BinanceRestClient::with_harness_base_url(
            base.clone(),
            "fixture-key".into(),
            "fixture-secret".into(),
        ));
        #[cfg(not(feature = "crash-points"))]
        let closed = client();
        let venue = Arc::new(BinanceSpotAdapter::non_trading(Arc::clone(&closed)).unwrap());
        let daemon = spot(Some(tenant), Arc::new(pool.clone()), closed, venue).unwrap();
        let journal = Arc::clone(&daemon.order_journal);
        let started = Box::pin(daemon.run_until(crate::stop::StopRequest::default())).await;
        assert!(
            matches!(&started, Err(DaemonError::Config(reason)) if reason == NOT_WIRED),
            "{started:?}"
        );
        let gate = journal.gate();
        let (epochs, marked): (i64, i64) = sqlx::query_as(
            "SELECT count(*), count(clean_stop_at) FROM process_epochs WHERE tenant_id=$1",
        )
        .bind(tenant)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((epochs, marked), (start, 0), "an ordinary epoch, left unmarked");
        assert_eq!(gate.epoch().and_then(|view| view.unclean_reason), Some(why));
        assert_eq!(gate.read_only(), Some(ReadOnlyReason::NonTrading));
        let unclean =
            gate.open_incidents().iter().any(|i| i.reason == IncidentReason::UncleanStart);
        assert!(unclean && gate.fenced(), "start {start}");
        assert!(gate.mutation("POST /api/v3/order").is_err());
    }
    #[cfg(feature = "crash-points")]
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err(),
        "a non-trading start sent nothing"
    );
}
