//! Negative pre-I/O tests. These prove allowance ordering only, not durability.
use robson_exec::{clock_records::RawClockCapture, venue_clock::ClockDomain};

use super::*;

fn input(tenant: Uuid, epoch: Uuid) -> RawClockSample {
    let mark = RawClockCapture {
        epoch,
        wall_seconds: 1_794_614_400,
        wall_nanoseconds: 0,
        domain: ClockDomain::new(Uuid::from_u128(1), 0, 0).unwrap(),
        before_ns: 0,
        after_ns: 0,
    };
    RawClockSample {
        id: Uuid::from_u128(2),
        tenant,
        venue: VenueId::new(VenueId::BINANCE_SPOT),
        send: mark,
        receive: mark,
        server_ms: 1_794_614_400_000,
    }
}
fn backend(tenant: Uuid, epoch: Uuid) -> ClockSampleStore {
    ClockSampleStore {
        options: Arc::new(
            PgConnectOptions::new_without_pgpass().host("/clock-tests-do-not-connect"),
        ),
        tenant,
        epoch,
        connection: None,
        last_cycle: None,
        statements: 0,
    }
}

#[tokio::test]
async fn expired_attempt_spends_cycle_before_any_io() {
    let tenant = Uuid::from_u128(3);
    let epoch = Uuid::from_u128(4);
    let mut store = backend(tenant, epoch);
    assert!(matches!(
        store.persist(7, input(tenant, epoch), Some(Instant::now())).await,
        Err(StoreError::Expired)
    ));
    // Invalid identity also prevents I/O if a mutant forgets the cycle check.
    assert!(matches!(
        store.persist(7, input(Uuid::nil(), epoch), None).await,
        Err(StoreError::RepeatedCycle)
    ));
    assert_eq!(store.last_cycle, Some(7));
    assert_eq!(store.statements, 0);
    assert!(store.connection.is_none());
}

#[tokio::test]
async fn invalid_identity_cannot_refund_cycle_or_prove_absence() {
    let tenant = Uuid::from_u128(3);
    let epoch = Uuid::from_u128(4);
    let mut store = backend(tenant, epoch);
    assert!(matches!(
        store.persist(1, input(Uuid::nil(), epoch), None).await,
        Err(StoreError::Invalid)
    ));
    assert!(matches!(
        store.persist(1, input(Uuid::nil(), epoch), None).await,
        Err(StoreError::RepeatedCycle)
    ));
    assert!(matches!(
        store.persist(0, input(Uuid::nil(), epoch), None).await,
        Err(StoreError::RepeatedCycle)
    ));
    assert_eq!(store.statements, 0);
    assert!(store.connection.is_none());
}

#[tokio::test]
async fn store_sessions_start_with_the_server_bounds() {
    let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy_with(
        PgConnectOptions::new_without_pgpass().host("/clock-tests-do-not-connect"),
    );
    let options = bounded_options(&pool);
    let startup = options.get_options().unwrap_or_default();
    for bound in [
        "-c lock_timeout=300ms",
        "-c statement_timeout=1s",
        "-c idle_in_transaction_session_timeout=1s",
    ] {
        assert!(startup.contains(bound), "{bound} missing from {startup:?}");
    }
}
