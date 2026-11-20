//! Real SQLx paths, deliberately ignored until coordinator PostgreSQL
//! acceptance. Supplied raw coordinates test storage only; they are not native
//! provenance.
use robson_domain::MonthlyBudgetModel;
use robson_exec::{
    clock_records::{RawClockCapture, RawClockSample},
    venue::{OrderPhase, VenueId},
    venue_clock::ClockDomain,
};
use sqlx::{Column, ConnectOptions, Connection, PgPool, Type};

use super::*;
use crate::{process_epoch, venue_clock_store::ClockSampleStore};

fn raw_sample(tenant: Uuid, epoch: Uuid) -> RawClockSample {
    let mark = RawClockCapture {
        epoch,
        wall_seconds: 1_794_614_400,
        wall_nanoseconds: 17,
        domain: ClockDomain::new(Uuid::from_u128(1), -1, 7).unwrap(),
        before_ns: 10,
        after_ns: 20,
    };
    RawClockSample {
        id: Uuid::now_v7(),
        tenant,
        venue: VenueId::new(VenueId::BINANCE_SPOT),
        send: mark,
        receive: RawClockCapture {
            before_ns: 30,
            after_ns: 40,
            wall_nanoseconds: 27,
            ..mark
        },
        server_ms: 1_794_614_400_000,
    }
}
fn order_record() -> VenueOrderRecord {
    VenueOrderRecord {
        exchange_order_id: "7".into(),
        client_order_id: "original".into(),
        orig_client_order_id: None,
        symbol: "BTCUSDT".into(),
        side: OrderSide::Sell,
        order_type: "MARKET".into(),
        protection_kind: None,
        status: "NEW".into(),
        phase: OrderPhase::Live,
        orig_qty: Decimal::ONE,
        executed_qty: Decimal::ZERO,
        cumulative_quote_qty: None,
        stop_price: None,
        expiry_reason: None,
        transact_time_ms: None,
        time_ms: None,
        update_time_ms: None,
        working_time_ms: None,
    }
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn actual_sample_backend_and_both_order_contexts_retain_exact_originals(pool: PgPool) {
    let tenant = Uuid::now_v7();
    let start = process_epoch::start_epoch(&pool, tenant, "clock-storage-fixture")
        .await
        .unwrap();
    let mut samples = ClockSampleStore::from_committed_epoch(&pool, tenant, &start);
    let raw = raw_sample(tenant, start.epoch.epoch_id);
    samples.persist(1, raw.clone(), None).await.unwrap();
    let ledger = PgVenueLedger::new(&pool, tenant);
    ledger
        .ensure_accounting_row(&raw.venue, MonthlyBudgetModel::NetFromStartNonExpandingV1)
        .await
        .unwrap();
    let record = order_record();
    let link = OrderLink {
        venue: raw.venue.clone(),
        account_scope: "fixture-scope".into(),
        owner: OrderOwner::Discovered,
        client_order_id: "original".into(),
    };
    let seen = SeenAt {
        at: DateTime::from_timestamp(
            raw.receive.wall_seconds,
            raw.receive.wall_nanoseconds / 1_000 * 1_000,
        )
        .unwrap(),
        clock: Some(ClockEvidence {
            sample_id: raw.id,
            wall_delta_ms: 0,
            mono_delta_ms: 0,
            verdict: ClockVerdict::Invalid,
        }),
        capture: Some(raw.receive),
    };
    assert!(ledger.record_order(&record, &link, seen.clone()).await.unwrap().changed);
    // Fresh, warm, explicitly cleared, replaced: check OIDs even if retained
    // metadata suppresses discovery. Wire/catalog counting remains pending B2
    // PostgreSQL acceptance.
    let sql = format!("SELECT {ORDER_COLUMNS} FROM venue_orders o WHERE o.tenant_id=$1");
    let allowed = [
        <Uuid as Type<Postgres>>::type_info(),
        <String as Type<Postgres>>::type_info(),
        <i64 as Type<Postgres>>::type_info(),
        <i32 as Type<Postgres>>::type_info(),
        <Decimal as Type<Postgres>>::type_info(),
        <DateTime<Utc> as Type<Postgres>>::type_info(),
        <bool as Type<Postgres>>::type_info(),
    ];
    let mut raw_conn = pool
        .connect_options()
        .as_ref()
        .clone()
        .statement_cache_capacity(2)
        .connect()
        .await
        .unwrap();
    for phase in 0..5 {
        if phase == 2 {
            // Three distinct prepared builtin statements evict the main query
            // from this test connection's actual two-entry statement LRU.
            for slot in 0..3 {
                sqlx::query(&format!("SELECT $1::int4 AS eviction_slot_{slot}"))
                    .bind(1i32)
                    .execute(&mut raw_conn)
                    .await
                    .unwrap();
            }
        }
        if phase == 3 {
            raw_conn.clear_cached_statements().await.unwrap();
        }
        if phase == 4 {
            drop(raw_conn);
            raw_conn = pool.connect_options().connect().await.unwrap();
        }
        let row = sqlx::query(&sql).bind(tenant).fetch_one(&mut raw_conn).await.unwrap();
        assert!(
            row.columns()
                .iter()
                .all(|c| allowed.iter().any(|a| a.oid() == c.type_info().oid())),
            "phase{phase}"
        );
        assert_eq!(crate::venue_clock_store::codec::capture(&row, "live").unwrap(), seen.capture);
    }
    let key = OrderKey::from_record(&record, &link).unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let before = ledger.statements_total();
    let stored = ledger
        .read_order_context(&mut conn, &key, None, false)
        .await
        .unwrap()
        .order
        .unwrap();
    assert_eq!(stored.live_seen, seen);
    assert_eq!(ledger.statements_total() - before, 1);
    let before = ledger.statements_total();
    let (traded, _) = ledger.read_trade_context(&mut conn, &key, &[]).await.unwrap();
    assert_eq!(traded.live_seen, seen);
    assert_eq!(ledger.statements_total() - before, 1);
    let later = SeenAt {
        capture: Some(RawClockCapture { wall_nanoseconds: 28, ..raw.receive }),
        ..seen.clone()
    };
    assert!(!ledger.record_order(&record, &link, later).await.unwrap().changed);
    let stored = ledger
        .read_order_context(&mut conn, &key, None, false)
        .await
        .unwrap()
        .order
        .unwrap();
    assert_eq!(stored.live_seen, seen);
    // B1's whole-tuple CHECK permits a complete raw terminal without its
    // legacy instant. Both real readers must refuse it, not drop the raw tuple.
    sqlx::query(
        "UPDATE venue_orders SET terminal_capture_epoch=live_capture_epoch,
        terminal_wall_seconds=live_wall_seconds,terminal_wall_nanoseconds=live_wall_nanoseconds,
        terminal_tick_before=live_tick_before,terminal_tick_after=live_tick_after,
        terminal_capture_domain=live_capture_domain WHERE tenant_id=$1",
    )
    .bind(tenant)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        ledger.read_order_context(&mut conn, &key, None, false).await,
        Err(LedgerError::InvalidEvidence(_))
    ));
    assert!(matches!(
        ledger.read_trade_context(&mut conn, &key, &[]).await,
        Err(LedgerError::InvalidEvidence(_))
    ));
}
