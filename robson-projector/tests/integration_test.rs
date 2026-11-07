//! Integration tests for robson-projector
//!
//! Tests event handlers against a test database using sqlx-test fixtures.
//!
//! # Running these tests
//!
//! These tests require a PostgreSQL database with migration 002 applied:
//!
//! ```bash
//! # 1. Start PostgreSQL (example with docker)
//! docker run --rm -p 5432:5432 -e POSTGRES_PASSWORD=test postgres:16
//!
//! # 2. Apply migration 002
//! psql -h localhost -U postgres -d postgres -f v2/migrations/002_event_log_phase9.sql
//!
//! # 3. Run tests
//! DATABASE_URL="postgresql://postgres:test@localhost/postgres" \
//!   cargo test -p robson-projector --test integration_test -- --ignored
//! ```

use chrono::{DateTime, Datelike, TimeZone, Utc};
use proptest::{
    prelude::{prop, Just, Strategy},
    prop_oneof,
    strategy::ValueTree,
    test_runner::TestRunner,
};
use robson_eventlog::{ActorType, EventEnvelope};
use robson_projector::apply_event_to_projections;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use uuid::Uuid;

type ExecutableSpanProjectionRow = (
    Option<Decimal>,
    Option<Decimal>,
    Option<Decimal>,
    Option<Decimal>,
    Option<Decimal>,
    Option<Decimal>,
    Option<Decimal>,
);

/// Test helper: Create an event envelope from an event
fn make_envelope(
    stream_key: &str,
    event_type: &str,
    payload: serde_json::Value,
    seq: i64,
) -> EventEnvelope {
    EventEnvelope {
        event_id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        stream_key: stream_key.to_string(),
        seq,
        event_type: event_type.to_string(),
        payload,
        payload_schema_version: 1,
        occurred_at: Utc::now(),
        ingested_at: Utc::now(),
        idempotency_key: format!("{}:{}:{}", stream_key, seq, event_type),
        trace_id: None,
        causation_id: None,
        command_id: None,
        workflow_id: None,
        actor_type: Some(ActorType::CLI),
        actor_id: Some("test-user".to_string()),
        prev_hash: None,
        hash: None,
    }
}

// =============================================================================
// SQLX-TEST: Integration tests require DATABASE_URL
// =============================================================================

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn test_entry_signal_projects_executable_span_admission_payload(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    let position_id = Uuid::new_v4();
    let stream_key = format!("position:{position_id}");
    let armed = make_envelope(
        &stream_key,
        "position_armed",
        serde_json::json!({
            "position_id": position_id,
            "account_id": Uuid::new_v4(),
            "symbol": {"base": "BTC", "quote": "USDT"},
            "side": "Long",
            "tech_stop_distance": null,
            "stop_policy": "executable_span",
            "stop_buffer_bps_at_arm": "10",
            "timestamp": Utc::now(),
        }),
        1,
    );
    apply_event_to_projections(&pool, &armed).await.unwrap();

    let admitted = make_envelope(
        &stream_key,
        "entry_signal_received",
        serde_json::json!({
            "position_id": position_id,
            "signal_id": Uuid::new_v4(),
            "entry_price": "62000",
            "stop_loss": "61000",
            "quantity": "0.1",
            "initial_executable_stop": "60939",
            "executable_span": "1061",
            "cap_basis_distance": "1000",
            "tick_size": "0.1",
            "timestamp": Utc::now(),
        }),
        2,
    );
    apply_event_to_projections(&pool, &admitted).await.unwrap();

    // Admission evidence is first-write-wins. A later corrupt/replayed
    // signal must not redefine the entry reference, S, its cap basis, tick,
    // or original trigger.
    let conflicting = make_envelope(
        &stream_key,
        "entry_signal_received",
        serde_json::json!({
            "position_id": position_id,
            "signal_id": Uuid::new_v4(),
            "entry_price": "63000",
            "stop_loss": "60000",
            "quantity": "0.2",
            "initial_executable_stop": "59900",
            "executable_span": "3100",
            "cap_basis_distance": "3000",
            "tick_size": "1",
            "timestamp": Utc::now(),
        }),
        3,
    );
    apply_event_to_projections(&pool, &conflicting).await.unwrap();

    let projected: ExecutableSpanProjectionRow = sqlx::query_as(
        r#"
            SELECT stop_plan_entry_reference,
                   initial_executable_stop, executable_span,
                   cap_basis_distance, tick_size_at_admission,
                   technical_stop_price, technical_stop_distance
              FROM positions_current
             WHERE position_id = $1
            "#,
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        projected,
        (
            Some(Decimal::from(62000)),
            Some(Decimal::from(60939)),
            Some(Decimal::from(1061)),
            Some(Decimal::from(1000)),
            Some(Decimal::new(1, 1)),
            Some(Decimal::from(61000)),
            Some(Decimal::from(1000)),
        )
    );

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_order_submitted_creates_projection(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let order_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "order_id": order_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "position_id": null,
        "client_order_id": "client-123",
        "symbol": "BTCUSDT",
        "side": "buy",
        "order_type": "limit",
        "quantity": "0.1",
        "price": "50000",
        "stop_price": null
    });

    let envelope = make_envelope("order:test", "ORDER_SUBMITTED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;
    assert!(result.is_ok());

    // Assert
    let (status, last_seq): (String, i64) =
        sqlx::query_as("SELECT status, last_seq FROM orders_current WHERE order_id = $1")
            .bind(order_id)
            .fetch_one(&pool)
            .await?;

    assert_eq!(status, "pending");
    assert_eq!(last_seq, 1);

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_order_acked_updates_projection(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let order_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    // First submit
    let submit_payload = serde_json::json!({
        "order_id": order_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "position_id": null,
        "client_order_id": "client-123",
        "symbol": "BTCUSDT",
        "side": "buy",
        "order_type": "limit",
        "quantity": "0.1",
        "price": "50000",
        "stop_price": null
    });
    let submit_env = make_envelope("order:test", "ORDER_SUBMITTED", submit_payload, 1);
    apply_event_to_projections(&pool, &submit_env).await.unwrap();

    // Act: Ack the order
    let ack_payload = serde_json::json!({
        "order_id": order_id,
        "exchange_order_id": "exchange-456"
    });
    let ack_env = make_envelope("order:test", "ORDER_ACKED", ack_payload, 2);
    apply_event_to_projections(&pool, &ack_env).await.unwrap();

    // Assert
    let (status, exchange_order_id, last_seq): (String, String, i64) = sqlx::query_as(
        "SELECT status, COALESCE(exchange_order_id, ''), last_seq FROM orders_current WHERE order_id = $1"
    )
    .bind(order_id)
    .fetch_one(&pool)
    .await?;

    assert_eq!(status, "acknowledged");
    assert_eq!(exchange_order_id, "exchange-456");
    assert_eq!(last_seq, 2);

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_month_boundary_reset_upserts_monthly_state(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let payload = serde_json::json!({
        "capital_base": "9750",
        "carried_positions_risk": "250",
        "month": 5,
        "year": 2026,
        "timestamp": Utc::now(),
    });
    let envelope =
        make_envelope("system:month_boundary:2026-05", "month_boundary_reset", payload, 1);

    apply_event_to_projections(&pool, &envelope).await.unwrap();

    let (capital_base, carried_risk): (Decimal, Decimal) = sqlx::query_as(
        "SELECT capital_base, carried_risk FROM monthly_state WHERE year = $1 AND month = $2",
    )
    .bind(2026_i16)
    .bind(5_i16)
    .fetch_one(&pool)
    .await?;

    assert_eq!(capital_base, Decimal::from(9750));
    assert_eq!(carried_risk, Decimal::from(250));

    let update_payload = serde_json::json!({
        "capital_base": "9800",
        "carried_positions_risk": "200",
        "month": 5,
        "year": 2026,
        "timestamp": Utc::now(),
    });
    let update_envelope =
        make_envelope("system:month_boundary:2026-05", "month_boundary_reset", update_payload, 2);

    apply_event_to_projections(&pool, &update_envelope).await.unwrap();

    let (capital_base, carried_risk): (Decimal, Decimal) = sqlx::query_as(
        "SELECT capital_base, carried_risk FROM monthly_state WHERE year = $1 AND month = $2",
    )
    .bind(2026_i16)
    .bind(5_i16)
    .fetch_one(&pool)
    .await?;

    assert_eq!(capital_base, Decimal::from(9800));
    assert_eq!(carried_risk, Decimal::from(200));

    Ok(())
}

// =============================================================================
// monthly_state: a capital_base needs account evidence
// =============================================================================

type MonthRow = (Decimal, Decimal, i32, Option<DateTime<Utc>>);

const CURRENT_MONTH_BACKFILL: &str =
    include_str!("../../migrations/20240101000011_backfill_current_month_state.sql");
const ZERO_BASE_CLEANUP: &str =
    include_str!("../../migrations/20240101000030_remove_unevidenced_zero_base_month_row.sql");

/// Envelope dated `at`, so a test picks the month the handlers write to.
fn month_event(
    event_type: &str,
    payload: serde_json::Value,
    at: DateTime<Utc>,
    seq: i64,
) -> EventEnvelope {
    let mut envelope =
        make_envelope(&format!("test:monthly-state:{seq}"), event_type, payload, seq);
    envelope.occurred_at = at;
    envelope.ingested_at = at;
    envelope
}

fn entry_filled_event(at: DateTime<Utc>, seq: i64) -> EventEnvelope {
    let payload = serde_json::json!({
        "position_id": Uuid::new_v4(),
        "order_id": Uuid::new_v4(),
        "fill_price": "100",
        "filled_quantity": "1",
        "fee": "0.1",
        "initial_stop": "90",
        "invalidation_guard_level": null,
        "timestamp": at,
    });
    month_event("entry_filled", payload, at, seq)
}

/// Close with a net loss of 11.
fn losing_close_event(at: DateTime<Utc>, seq: i64) -> EventEnvelope {
    let payload = serde_json::json!({
        "position_id": Uuid::new_v4(),
        "exit_reason": "TrailingStop",
        "entry_price": "100",
        "exit_price": "90",
        "realized_pnl": "-10",
        "total_fees": "1",
        "timestamp": at,
    });
    month_event("position_closed", payload, at, seq)
}

fn boundary_reset_event(at: DateTime<Utc>, capital_base: Decimal, seq: i64) -> EventEnvelope {
    let payload = serde_json::json!({
        "capital_base": capital_base,
        "carried_positions_risk": "0",
        "month": at.month(),
        "year": at.year(),
        "timestamp": at,
    });
    month_event("month_boundary_reset", payload, at, seq)
}

async fn month_row(pool: &sqlx::PgPool, at: DateTime<Utc>) -> sqlx::Result<Option<MonthRow>> {
    sqlx::query_as(
        "SELECT capital_base, realized_loss, trades_opened, boundary_reset_at \
         FROM monthly_state WHERE year = $1 AND month = $2",
    )
    .bind(at.year() as i16)
    .bind(at.month() as i16)
    .fetch_optional(pool)
    .await
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_fill_and_losing_close_create_no_month_row(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Domain events project inline, and the first boundary poll of the month
    // has not run yet: nothing in the account has supplied a base.
    let at = Utc.with_ymd_and_hms(2027, 4, 1, 0, 0, 10).unwrap();

    apply_event_to_projections(&pool, &entry_filled_event(at, 1)).await.unwrap();
    apply_event_to_projections(&pool, &losing_close_event(at, 2)).await.unwrap();

    assert_eq!(month_row(&pool, at).await?, None);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_no_event_order_shows_a_zero_capital_base(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let at = Utc.with_ymd_and_hms(2027, 4, 1, 0, 0, 10).unwrap();
    let orders = [
        ["fill", "close", "reset"],
        ["fill", "reset", "close"],
        ["close", "fill", "reset"],
        ["close", "reset", "fill"],
        ["reset", "fill", "close"],
        ["reset", "close", "fill"],
    ];

    for order in orders {
        sqlx::query("DELETE FROM monthly_state").execute(&pool).await?;
        let mut after_reset = None;

        for (index, step) in order.into_iter().enumerate() {
            let seq = index as i64 + 1;
            let envelope = match step {
                "fill" => entry_filled_event(at, seq),
                "close" => losing_close_event(at, seq),
                _ => boundary_reset_event(at, dec!(9750), seq),
            };
            apply_event_to_projections(&pool, &envelope).await.unwrap();

            match (month_row(&pool, at).await?, step) {
                (Some(row), "reset") => {
                    assert_eq!(row, (dec!(9750), Decimal::ZERO, 0, Some(at)), "{order:?}");
                    after_reset = Some((Decimal::ZERO, 0));
                },
                (Some((base, ..)), _) => assert_eq!(base, dec!(9750), "{order:?} after {step}"),
                (None, _) => assert!(after_reset.is_none(), "{order:?} lost the row at {step}"),
            }
            after_reset = after_reset.map(|(loss, trades)| match step {
                "fill" => (loss, trades + 1),
                "close" => (loss + dec!(11), trades),
                _ => (loss, trades),
            });
        }

        // The reset discards what came before it; what follows it is counted.
        let (loss, trades) = after_reset.unwrap();
        assert_eq!(month_row(&pool, at).await?, Some((dec!(9750), loss, trades, Some(at))));
    }
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_replay_through_the_reset_converges_on_the_same_month_row(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    let window = Utc.with_ymd_and_hms(2027, 4, 1, 0, 0, 10).unwrap();
    let reset = Utc.with_ymd_and_hms(2027, 4, 1, 0, 0, 49).unwrap();
    let later = Utc.with_ymd_and_hms(2027, 4, 2, 9, 0, 0).unwrap();
    let events = [
        entry_filled_event(window, 1),
        losing_close_event(window, 2),
        boundary_reset_event(reset, dec!(9750), 3),
        entry_filled_event(later, 4),
        losing_close_event(later, 5),
    ];

    for event in &events {
        apply_event_to_projections(&pool, event).await.unwrap();
    }
    let first = month_row(&pool, reset).await?;
    assert_eq!(first, Some((dec!(9750), dec!(11), 1, Some(reset))));

    for event in &events {
        apply_event_to_projections(&pool, event).await.unwrap();
    }
    assert_eq!(month_row(&pool, reset).await?, first);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_close_during_an_uncommitted_reset_is_counted(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let at = Utc.with_ymd_and_hms(2027, 4, 1, 0, 0, 49).unwrap();

    let mut reset = pool.begin().await?;
    sqlx::query(
        "INSERT INTO monthly_state (year, month, capital_base, boundary_reset_at, created_at) \
         VALUES ($1, $2, 9750, $3, $3)",
    )
    .bind(at.year() as i16)
    .bind(at.month() as i16)
    .bind(at)
    .execute(&mut *reset)
    .await?;

    let close = {
        let pool = pool.clone();
        tokio::spawn(
            async move { apply_event_to_projections(&pool, &losing_close_event(at, 1)).await },
        )
    };
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!close.is_finished(), "the close must wait for the row being inserted");

    reset.commit().await?;
    close.await.unwrap().unwrap();

    assert_eq!(month_row(&pool, at).await?, Some((dec!(9750), dec!(11), 0, Some(at))));
    Ok(())
}

/// Every writer of a month's row.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MonthWriter {
    Fill,
    Close,
    Reset,
    Recalibrate,
    PeakRefresh,
}

fn month_writer() -> impl Strategy<Value = MonthWriter> {
    prop_oneof![
        Just(MonthWriter::Fill),
        Just(MonthWriter::Close),
        Just(MonthWriter::Reset),
        Just(MonthWriter::Recalibrate),
        Just(MonthWriter::PeakRefresh),
    ]
}

async fn run_month_writer(pool: sqlx::PgPool, writer: MonthWriter, at: DateTime<Utc>, seq: i64) {
    let event = match writer {
        MonthWriter::Fill => entry_filled_event(at, seq),
        MonthWriter::Close => losing_close_event(at, seq),
        MonthWriter::Reset => boundary_reset_event(at, dec!(9750), seq),
        MonthWriter::Recalibrate => month_event(
            "capital_base_recalibrated",
            serde_json::json!({
                "previous_capital_base": "9750",
                "new_capital_base": "9900",
                "wallet_balance": "9900",
                "carried_risk": "0",
                "reason": "test",
                "evidence": "test",
                "month": at.month(),
                "year": at.year(),
                "timestamp": at,
            }),
            at,
            seq,
        ),
        MonthWriter::PeakRefresh => {
            // Mirrors refresh_month_peak_net_from_snapshot in robsond, which
            // passes the base load_monthly_state resolved.
            sqlx::query(
                "INSERT INTO monthly_state (year, month, capital_base, month_peak_net, created_at) \
                 VALUES ($1, $2, 10000, 10, $3) \
                 ON CONFLICT (year, month) DO UPDATE SET month_peak_net = \
                 GREATEST(monthly_state.month_peak_net, EXCLUDED.month_peak_net)",
            )
            .bind(at.year() as i16)
            .bind(at.month() as i16)
            .bind(at)
            .execute(&pool)
            .await
            .unwrap();
            return;
        },
    };
    apply_event_to_projections(&pool, &event).await.unwrap();
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_generated_schedules_keep_the_month_row_invariants(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    // Writers in one batch race; batches run one after the other. The proptest
    // macro cannot await, so this samples its strategy without shrinking. The
    // failure message prints the schedule.
    let at = Utc.with_ymd_and_hms(2027, 4, 1, 0, 0, 10).unwrap();
    let schedules = prop::collection::vec(prop::collection::vec(month_writer(), 1..6), 1..6);
    let mut runner = TestRunner::deterministic();

    for _ in 0..64 {
        let schedule = schedules.new_tree(&mut runner).unwrap().current();
        sqlx::query("DELETE FROM monthly_state").execute(&pool).await?;

        let mut seq = 0;
        for batch in &schedule {
            let writers: Vec<_> = batch
                .iter()
                .map(|writer| {
                    seq += 1;
                    tokio::spawn(run_month_writer(pool.clone(), *writer, at, seq))
                })
                .collect();
            while !writers.iter().all(|writer| writer.is_finished()) {
                let zero_bases: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM monthly_state WHERE capital_base = 0")
                        .fetch_one(&pool)
                        .await?;
                assert_eq!(zero_bases, 0, "{schedule:?}");
            }
            for writer in writers {
                writer.await.unwrap();
            }
        }

        // From this batch on, nothing zeroes the counters: the last reset, or
        // without a reset the first writer that creates the row. A counter in a
        // later batch must survive. One racing in this batch may land on either
        // side of it, and one in an earlier batch must not count.
        let anchor = schedule
            .iter()
            .rposition(|batch| batch.contains(&MonthWriter::Reset))
            .or_else(|| {
                schedule.iter().position(|batch| {
                    batch.contains(&MonthWriter::Recalibrate)
                        || batch.contains(&MonthWriter::PeakRefresh)
                })
            });
        let row = month_row(&pool, at).await?;
        let Some(anchor) = anchor else {
            assert_eq!(row, None, "{schedule:?}");
            continue;
        };
        let (base, loss, trades, _) = row.unwrap_or_else(|| panic!("no row: {schedule:?}"));
        assert!(base > Decimal::ZERO, "{schedule:?}");

        let count = |batches: &[Vec<MonthWriter>], writer| {
            batches.iter().flatten().filter(|candidate| **candidate == writer).count()
        };
        let fills = count(&schedule[anchor + 1..], MonthWriter::Fill)
            ..=count(&schedule[anchor..], MonthWriter::Fill);
        assert!(fills.contains(&(trades as usize)), "trades {trades} of {fills:?}: {schedule:?}");

        let closes = count(&schedule[anchor + 1..], MonthWriter::Close)
            ..=count(&schedule[anchor..], MonthWriter::Close);
        assert_eq!(loss % dec!(11), Decimal::ZERO, "{schedule:?}");
        assert!(
            closes.contains(&((loss / dec!(11)).trunc().to_string().parse::<usize>().unwrap())),
            "loss {loss} of {closes:?}: {schedule:?}",
        );
    }
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_fresh_database_has_no_month_row(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM monthly_state")
        .fetch_one(&pool)
        .await?;
    assert_eq!(rows, 0);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_zero_base_cleanup_removes_only_rows_that_record_nothing(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    struct Case {
        name: &'static str,
        months_from_now: i32,
        capital_base: Decimal,
        carried_risk: Decimal,
        realized_loss: Decimal,
        trades_opened: i32,
        month_peak_net: Decimal,
        model: &'static str,
        boundary_event: bool,
        removed: bool,
    }
    let empty = Case {
        name: "",
        months_from_now: 0,
        capital_base: Decimal::ZERO,
        carried_risk: Decimal::ZERO,
        realized_loss: Decimal::ZERO,
        trades_opened: 0,
        month_peak_net: Decimal::ZERO,
        model: "hwm_v1",
        boundary_event: false,
        removed: false,
    };
    let cases = [
        Case {
            name: "current month",
            removed: true,
            ..empty
        },
        Case {
            name: "later month",
            months_from_now: 1,
            removed: true,
            ..empty
        },
        Case {
            name: "history",
            months_from_now: -1,
            ..empty
        },
        Case {
            name: "boundary event",
            months_from_now: 2,
            boundary_event: true,
            ..empty
        },
        Case {
            name: "real base",
            months_from_now: 3,
            capital_base: dec!(100),
            ..empty
        },
        Case {
            name: "carried risk",
            months_from_now: 4,
            carried_risk: dec!(5),
            ..empty
        },
        Case {
            name: "realized loss",
            months_from_now: 5,
            realized_loss: dec!(5),
            ..empty
        },
        Case {
            name: "trades opened",
            months_from_now: 6,
            trades_opened: 1,
            ..empty
        },
        Case {
            name: "peak",
            months_from_now: 7,
            month_peak_net: dec!(5),
            ..empty
        },
        Case {
            name: "budget model",
            months_from_now: 8,
            model: "net_from_start_non_expanding_v1",
            ..empty
        },
    ];

    let now = Utc::now();
    let year_month = |months_from_now: i32| {
        let index = now.year() * 12 + now.month0() as i32 + months_from_now;
        (index.div_euclid(12) as i16, index.rem_euclid(12) as i16 + 1)
    };

    for case in &cases {
        let (year, month) = year_month(case.months_from_now);
        sqlx::query(
            "INSERT INTO monthly_state (year, month, capital_base, carried_risk, realized_loss, \
             trades_opened, month_peak_net, monthly_budget_model, boundary_reset_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)",
        )
        .bind(year)
        .bind(month)
        .bind(case.capital_base)
        .bind(case.carried_risk)
        .bind(case.realized_loss)
        .bind(case.trades_opened)
        .bind(case.month_peak_net)
        .bind(case.model)
        .bind(now)
        .execute(&pool)
        .await?;

        if case.boundary_event {
            let stream_key = format!("system:month_boundary:{year:04}-{month:02}");
            let event = robson_eventlog::Event::new(
                Uuid::new_v4(),
                stream_key.clone(),
                "month_boundary_reset",
                serde_json::json!({}),
            );
            robson_eventlog::append_event(&pool, &stream_key, None, event).await.unwrap();
        }
    }

    sqlx::query(ZERO_BASE_CLEANUP).execute(&pool).await?;

    for case in &cases {
        let (year, month) = year_month(case.months_from_now);
        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM monthly_state WHERE year = $1 AND month = $2")
                .bind(year)
                .bind(month)
                .fetch_one(&pool)
                .await?;
        assert_eq!(rows == 0, case.removed, "{}", case.name);
    }
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_zero_base_cleanup_covers_a_bootstrap_west_of_utc(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    // At 00:30 UTC on December 1 a session at UTC-3 is still in November, so the
    // backfill writes November's row. NOW() is unqualified in both migrations,
    // so a function earlier on the search path sets their clock.
    let mut conn = pool.acquire().await?;
    for statement in [
        "CREATE SCHEMA fake_clock",
        "CREATE FUNCTION fake_clock.now() RETURNS TIMESTAMPTZ LANGUAGE SQL \
         AS $$ SELECT TIMESTAMPTZ '2026-12-01 00:30:00+00' $$",
        "SET search_path = fake_clock, pg_catalog, public",
        "SET TIME ZONE INTERVAL '-03:00' HOUR TO MINUTE",
        CURRENT_MONTH_BACKFILL,
        "UPDATE monthly_state SET boundary_reset_at = created_at",
    ] {
        sqlx::query(statement).execute(&mut *conn).await?;
    }
    let bootstrapped: (i16, i16) = sqlx::query_as("SELECT year, month FROM monthly_state")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(bootstrapped, (2026, 11));

    sqlx::query(ZERO_BASE_CLEANUP).execute(&mut *conn).await?;

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM monthly_state")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(rows, 0);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_position_opened_enforces_invariants(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let position_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "position_id": position_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "strategy_id": null,
        "symbol": "BTCUSDT",
        "side": "long",
        "entry_price": "50000",
        "entry_quantity": "0.1",
        "entry_filled_at": null,
        "technical_stop_price": "49000",
        "technical_stop_distance": "1000",
        "entry_order_id": null,
        "stop_loss_order_id": null
    });

    let envelope = make_envelope("position:test", "POSITION_OPENED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert - should succeed
    assert!(result.is_ok());

    let (state, trailing_stop): (String, Option<Decimal>) = sqlx::query_as(
        "SELECT state, trailing_stop_price FROM positions_current WHERE position_id = $1",
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await?;

    assert_eq!(state, "armed");
    // INVARIANT: trailing_stop should be anchored to technical_stop_price
    assert_eq!(trailing_stop, Some(Decimal::from(49000)));

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_position_opened_rejects_zero_stop_distance(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange - invalid payload with zero stop distance
    let position_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "position_id": position_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "strategy_id": null,
        "symbol": "BTCUSDT",
        "side": "long",
        "entry_price": "50000",
        "entry_quantity": "0.1",
        "entry_filled_at": null,
        "technical_stop_price": "50000",
        "technical_stop_distance": "0",
        "entry_order_id": null,
        "stop_loss_order_id": null
    });

    let envelope = make_envelope("position:test", "POSITION_OPENED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert - should fail invariant check
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("Invariant violated"),
        "Expected 'Invariant violated', got: {err}"
    );

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_legacy_entry_order_placed_does_not_transition_to_entering(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    let position_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();
    let signal_id = Uuid::new_v4();
    let order_id = Uuid::new_v4();

    let opened = make_envelope(
        "position:test",
        "POSITION_OPENED",
        serde_json::json!({
            "position_id": position_id,
            "tenant_id": tenant_id,
            "account_id": account_id,
            "strategy_id": null,
            "symbol": "BTCUSDT",
            "side": "long",
            "entry_price": null,
            "entry_quantity": null,
            "entry_filled_at": null,
            "technical_stop_price": "49000",
            "technical_stop_distance": "1000",
            "entry_order_id": null,
            "stop_loss_order_id": null
        }),
        1,
    );
    apply_event_to_projections(&pool, &opened).await.unwrap();

    let legacy = make_envelope(
        "position:test",
        "entry_order_placed",
        serde_json::json!({
            "position_id": position_id,
            "order_id": order_id,
            "expected_price": "50000",
            "quantity": "0.1",
            "signal_id": signal_id,
            "timestamp": Utc::now(),
        }),
        2,
    );
    apply_event_to_projections(&pool, &legacy).await.unwrap();

    let (state, entry_price, entry_quantity, current_quantity, stored_order_id, stored_signal_id): (
        String,
        Option<Decimal>,
        Option<Decimal>,
        Decimal,
        Option<Uuid>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT state, entry_price, entry_quantity, current_quantity, entry_order_id, entry_signal_id FROM positions_current WHERE position_id = $1",
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await?;

    assert_eq!(state, "armed");
    assert_eq!(entry_price, Some(Decimal::from(50000)));
    assert_eq!(entry_quantity, Some(Decimal::from(1) / Decimal::from(10)));
    assert_eq!(current_quantity, Decimal::from(1) / Decimal::from(10));
    assert_eq!(stored_order_id, None);
    assert_eq!(stored_signal_id, Some(signal_id));

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_entry_requested_accepted_creates_valid_order_and_entering_projection(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    let position_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();
    let signal_id = Uuid::new_v4();
    let cycle_id = Uuid::new_v4();
    let order_id = Uuid::new_v4();
    let client_order_id = signal_id.to_string();

    let opened = make_envelope(
        "position:test",
        "POSITION_OPENED",
        serde_json::json!({
            "position_id": position_id,
            "tenant_id": tenant_id,
            "account_id": account_id,
            "strategy_id": null,
            "symbol": "BTCUSDT",
            "side": "long",
            "entry_price": null,
            "entry_quantity": null,
            "entry_filled_at": null,
            "technical_stop_price": "49000",
            "technical_stop_distance": "1000",
            "entry_order_id": null,
            "stop_loss_order_id": null
        }),
        1,
    );
    apply_event_to_projections(&pool, &opened).await.unwrap();

    let requested = make_envelope(
        "position:test",
        "entry_order_requested",
        serde_json::json!({
            "position_id": position_id,
            "cycle_id": cycle_id,
            "order_id": order_id,
            "client_order_id": client_order_id.clone(),
            "expected_price": "50000",
            "quantity": "0.1",
            "signal_id": signal_id,
            "timestamp": Utc::now(),
        }),
        2,
    );
    apply_event_to_projections(&pool, &requested).await.unwrap();

    let (requested_state, requested_order_id, order_status): (String, Option<Uuid>, String) =
        sqlx::query_as(
            r#"
            SELECT p.state, p.entry_order_id, o.status
            FROM positions_current p
            JOIN orders_current o ON o.order_id = p.entry_order_id
            WHERE p.position_id = $1
            "#,
        )
        .bind(position_id)
        .fetch_one(&pool)
        .await?;

    assert_eq!(requested_state, "armed");
    assert_eq!(requested_order_id, Some(order_id));
    assert_eq!(order_status, "pending");

    let accepted = make_envelope(
        "position:test",
        "entry_order_accepted",
        serde_json::json!({
            "position_id": position_id,
            "cycle_id": cycle_id,
            "order_id": order_id,
            "client_order_id": client_order_id.clone(),
            "exchange_order_id": "exchange-456",
            "expected_price": "50000",
            "quantity": "0.1",
            "signal_id": signal_id,
            "timestamp": Utc::now(),
        }),
        3,
    );
    apply_event_to_projections(&pool, &accepted).await.unwrap();

    let (state, stored_order_id, stored_signal_id, status, exchange_order_id): (
        String,
        Option<Uuid>,
        Option<Uuid>,
        String,
        Option<String>,
    ) = sqlx::query_as(
        r#"
        SELECT p.state, p.entry_order_id, p.entry_signal_id, o.status, o.exchange_order_id
        FROM positions_current p
        JOIN orders_current o ON o.order_id = p.entry_order_id
        WHERE p.position_id = $1
        "#,
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await?;

    assert_eq!(state, "entering");
    assert_eq!(stored_order_id, Some(order_id));
    assert_eq!(stored_signal_id, Some(signal_id));
    assert_eq!(status, "acknowledged");
    assert_eq!(exchange_order_id.as_deref(), Some("exchange-456"));

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_exit_order_placed_updates_projection_with_exit_reason(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    let position_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();
    let entry_order_id = Uuid::new_v4();
    let exit_order_id = Uuid::new_v4();

    let opened = make_envelope(
        "position:test",
        "POSITION_OPENED",
        serde_json::json!({
            "position_id": position_id,
            "tenant_id": tenant_id,
            "account_id": account_id,
            "strategy_id": null,
            "symbol": "BTCUSDT",
            "side": "long",
            "entry_price": null,
            "entry_quantity": null,
            "entry_filled_at": null,
            "technical_stop_price": "49000",
            "technical_stop_distance": "1000",
            "entry_order_id": null,
            "stop_loss_order_id": null
        }),
        1,
    );
    apply_event_to_projections(&pool, &opened).await.unwrap();

    let entry_filled = make_envelope(
        "position:test",
        "entry_filled",
        serde_json::json!({
            "position_id": position_id,
            "order_id": entry_order_id,
            "fill_price": "50000",
            "filled_quantity": "0.1",
            "fee": "0.001",
            "initial_stop": "49000",
            "timestamp": Utc::now(),
        }),
        2,
    );
    apply_event_to_projections(&pool, &entry_filled).await.unwrap();

    let exit_order = make_envelope(
        "position:test",
        "exit_order_placed",
        serde_json::json!({
            "position_id": position_id,
            "order_id": exit_order_id,
            "expected_price": "48900",
            "quantity": "0.1",
            "exit_reason": "TrailingStop",
            "timestamp": Utc::now(),
        }),
        3,
    );
    apply_event_to_projections(&pool, &exit_order).await.unwrap();

    let (state, stored_order_id, exit_reason): (String, Option<Uuid>, Option<String>) =
        sqlx::query_as(
            "SELECT state, exit_order_id, exit_reason FROM positions_current WHERE position_id = $1",
        )
        .bind(position_id)
        .fetch_one(&pool)
        .await?;

    assert_eq!(state, "exiting");
    assert_eq!(stored_order_id, Some(exit_order_id));
    assert_eq!(exit_reason, Some("TrailingStop".to_string()));

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_entry_filled_overwrites_quantity_with_real_fill(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    // Regression (2026-07-07 incident): entry_filled must overwrite
    // entry_quantity/current_quantity with the REAL, exchange-confirmed
    // filled_quantity. Before this fix, handle_entry_filled left these
    // columns at whatever theoretical (pre-lot-size) value was written at
    // request time — POSITION_OPENED here carries entry_quantity=0.00737028,
    // simulating the risk-sizing formula's raw output before the exchange
    // truncates it to its LOT_SIZE step. The real fill (0.007, a routine
    // Binance BTCUSDT truncation) must win. A restart rehydrates the
    // position straight from this row, so a stale, larger quantity here
    // would size a future reduce-only exit order above the real exchange
    // position -- rejected, reproducing the same "wrong local size, live
    // position" failure class as the reconciliation-anchor bug (ADR-0045
    // amendment).
    let position_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();
    let entry_order_id = Uuid::new_v4();

    let opened = make_envelope(
        "position:test",
        "POSITION_OPENED",
        serde_json::json!({
            "position_id": position_id,
            "tenant_id": tenant_id,
            "account_id": account_id,
            "strategy_id": null,
            "symbol": "BTCUSDT",
            "side": "long",
            "entry_price": null,
            "entry_quantity": "0.00737028",
            "entry_filled_at": null,
            "technical_stop_price": "61297.00",
            "technical_stop_distance": "2023.10",
            "entry_order_id": null,
            "stop_loss_order_id": null
        }),
        1,
    );
    apply_event_to_projections(&pool, &opened).await.unwrap();

    let entry_filled = make_envelope(
        "position:test",
        "entry_filled",
        serde_json::json!({
            "position_id": position_id,
            "order_id": entry_order_id,
            "fill_price": "63299.70",
            "filled_quantity": "0.007",
            "fee": "0.44309790",
            "initial_stop": "61297.00",
            "timestamp": Utc::now(),
        }),
        2,
    );
    apply_event_to_projections(&pool, &entry_filled).await.unwrap();

    let (entry_quantity, current_quantity): (Decimal, Decimal) = sqlx::query_as(
        "SELECT entry_quantity, current_quantity FROM positions_current WHERE position_id = $1",
    )
    .bind(position_id)
    .fetch_one(&pool)
    .await?;

    assert_eq!(
        entry_quantity,
        Decimal::new(7, 3),
        "entry_quantity must be the real fill, not the pre-lot-size request"
    );
    assert_eq!(
        current_quantity,
        Decimal::new(7, 3),
        "current_quantity must be the real fill, not the pre-lot-size request"
    );

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_fill_received_idempotent(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let order_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    // Submit order first
    let submit_payload = serde_json::json!({
        "order_id": order_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "position_id": null,
        "client_order_id": "client-123",
        "symbol": "BTCUSDT",
        "side": "buy",
        "order_type": "limit",
        "quantity": "0.1",
        "price": "50000",
        "stop_price": null
    });
    let submit_env = make_envelope("order:test", "ORDER_SUBMITTED", submit_payload, 1);
    apply_event_to_projections(&pool, &submit_env).await.unwrap();

    let fill_payload = serde_json::json!({
        "fill_id": Uuid::new_v4(),
        "tenant_id": tenant_id,
        "account_id": account_id,
        "order_id": order_id,
        "exchange_order_id": "exchange-456",
        "exchange_trade_id": "trade-789",
        "symbol": "BTCUSDT",
        "side": "buy",
        "fill_price": "50000",
        "fill_quantity": "0.1",
        "fee": "0.001",
        "fee_asset": "BTC",
        "is_maker": true,
        "filled_at": Utc::now()
    });

    let fill_env = make_envelope("order:test", "FILL_RECEIVED", fill_payload.clone(), 2);

    // Act: Apply twice
    apply_event_to_projections(&pool, &fill_env).await.unwrap();
    apply_event_to_projections(&pool, &fill_env).await.unwrap();

    // Assert: filled_quantity should only be applied once
    let (filled_quantity,): (Decimal,) =
        sqlx::query_as("SELECT filled_quantity FROM orders_current WHERE order_id = $1")
            .bind(order_id)
            .fetch_one(&pool)
            .await?;

    assert_eq!(filled_quantity, Decimal::from(1) / Decimal::from(10)); // 0.1, not applied twice (not 0.2)

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_strategy_enabled(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let strategy_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "strategy_id": strategy_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "strategy_name": "Mean Reversion MA99",
        "strategy_type": "trend_following",
        "detector_config": null,
        "risk_config": {"max_risk_percent": "1.0"}
    });

    let envelope = make_envelope("strategy:test", "STRATEGY_ENABLED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert
    assert!(result.is_ok());

    let (is_enabled, strategy_name): (bool, String) = sqlx::query_as(
        "SELECT is_enabled, strategy_name FROM strategy_state_current WHERE strategy_id = $1",
    )
    .bind(strategy_id)
    .fetch_one(&pool)
    .await?;

    assert!(is_enabled);
    assert_eq!(strategy_name, "Mean Reversion MA99");

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_balance_sampled(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let balance_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "balance_id": balance_id,
        "tenant_id": tenant_id,
        "account_id": account_id,
        "asset": "BTC",
        "free": "1.5",
        "locked": "0.5",
        "sampled_at": Utc::now()
    });

    let envelope = make_envelope("account:test", "BALANCE_SAMPLED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert
    assert!(result.is_ok());

    let (free, locked, total): (Decimal, Decimal, Decimal) =
        sqlx::query_as("SELECT free, locked, total FROM balances_current WHERE balance_id = $1")
            .bind(balance_id)
            .fetch_one(&pool)
            .await?;

    assert_eq!(free, Decimal::from(15) / Decimal::from(10)); // 1.5
    assert_eq!(locked, Decimal::from(5) / Decimal::from(10)); // 0.5
    assert_eq!(total, Decimal::from(2)); // 1.5 + 0.5

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_risk_check_failed(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Arrange
    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();

    let payload = serde_json::json!({
        "tenant_id": tenant_id,
        "account_id": account_id,
        "strategy_id": null,
        "violation_reason": "Daily loss limit exceeded"
    });

    let envelope = make_envelope("account:test", "RISK_CHECK_FAILED", payload, 1);

    // Act
    let result = apply_event_to_projections(&pool, &envelope).await;

    // Assert
    assert!(result.is_ok());

    let (is_violated, reason): (bool, Option<String>) = sqlx::query_as(
        "SELECT is_violated, violation_reason FROM risk_state_current WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await?;

    assert!(is_violated);
    assert_eq!(reason, Some("Daily loss limit exceeded".to_string()));

    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL (see file header for setup)"]
async fn test_global_idempotency(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Regression test: inserting same event twice should not duplicate
    // Uses event_idempotency table for global idempotency by (tenant_id,
    // idempotency_key)

    let tenant_id = Uuid::new_v4();
    let account_id = Uuid::new_v4();
    let idempotency_key = "global-idempotency-test";
    let stream_key = "account:global-test";

    // Helper to insert via event_idempotency
    async fn insert_via_idempotency(
        stream_key: &str,
        seq: i64,
        pool: &sqlx::PgPool,
        tenant_id: Uuid,
        idempotency_key: &str,
        account_id: Uuid,
    ) -> sqlx::Result<Option<Uuid>> {
        let event_id = Uuid::new_v4();

        // First, insert into event_idempotency
        sqlx::query(
            r#"
            INSERT INTO event_idempotency (tenant_id, idempotency_key, event_id)
            VALUES ($1, $2, $3)
            ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
            "#,
        )
        .bind(tenant_id)
        .bind(idempotency_key)
        .bind(event_id)
        .execute(pool)
        .await?;

        // Fetch which event_id was registered (first writer wins)
        let existing = sqlx::query_scalar::<_, Uuid>(
            "SELECT event_id FROM event_idempotency WHERE tenant_id = $1 AND idempotency_key = $2",
        )
        .bind(tenant_id)
        .bind(idempotency_key)
        .fetch_one(pool)
        .await?;

        // Only insert into event_log when this call registered a NEW idempotency key.
        // If existing != event_id, another writer got there first — skip the insert.
        if existing != event_id {
            return Ok(Some(existing));
        }

        let payload = serde_json::json!({
            "balance_id": Uuid::new_v4(),
            "tenant_id": tenant_id,
            "account_id": account_id,
            "asset": "USDT",
            "free": "2500.00",
            "locked": "0.00",
            "sampled_at": chrono::Utc::now()
        });

        sqlx::query(
            r#"
            INSERT INTO event_log (
                tenant_id, stream_key, seq, event_type, payload, payload_schema_version,
                occurred_at, ingested_at, idempotency_key, actor_type, actor_id, event_id
            ) VALUES ($1, $2, $3, 'BALANCE_SAMPLED', $4, 1,
                       NOW(), NOW(), $5, 'CLI', 'test-user', $6)
            "#,
        )
        .bind(tenant_id)
        .bind(stream_key)
        .bind(seq)
        .bind(payload)
        .bind(idempotency_key)
        .bind(event_id)
        .execute(pool)
        .await?;

        Ok(Some(existing))
    }

    // First insert - should succeed and create new row
    let event_id_1 =
        insert_via_idempotency(stream_key, 1, &pool, tenant_id, idempotency_key, account_id)
            .await?;
    assert!(event_id_1.is_some());

    // Verify event_log has 1 row
    let count_1: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_log WHERE tenant_id = $1 AND idempotency_key = $2",
    )
    .bind(tenant_id)
    .bind(idempotency_key)
    .fetch_one(&pool)
    .await?;
    assert_eq!(count_1, 1, "First insert should create 1 row");

    // Second insert - should be blocked by event_idempotency (ON CONFLICT DO
    // NOTHING)
    let event_id_2 =
        insert_via_idempotency(stream_key, 2, &pool, tenant_id, idempotency_key, account_id)
            .await?;
    assert!(event_id_2.is_some());

    // Verify still only 1 row in event_log (second insert blocked by idempotency)
    let count_2: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_log WHERE tenant_id = $1 AND idempotency_key = $2",
    )
    .bind(tenant_id)
    .bind(idempotency_key)
    .fetch_one(&pool)
    .await?;
    assert_eq!(count_2, 1, "Second insert should NOT create duplicate; should still be 1 row");

    // Verify the event_id returned is the same (first insert)
    assert_eq!(event_id_1, event_id_2, "Both inserts should return same event_id");

    Ok(())
}
