//! ADR-0056 step 8m-2b proofs (8m PLAN-r4 §6.7, §8): the projector's
//! `positions_current` and `monthly_state` writes, on both account kinds, take
//! the cutover certificate's shared key before any row lock and wait for a
//! completion holding it exclusive, with nothing written meanwhile; the
//! three-epoch case; and the source guard over the production module trees
//! (robsond, robson-projector, robson-eventlog, and robson-db, whose seed
//! `robsond db init` runs), which reads past inline test modules.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, ensure, Context as _, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use robson_eventlog::{append_event, query_events, ActorType, Event, EventEnvelope, QueryOptions};
use robson_exec::{
    order_journal::{JournalBackend, ReleaseRecord},
    venue::{AccountKind, VenueId},
};
use robson_projector::apply_event_to_projections_with;
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    certificate_tests::{until_key_waiters, usdm_batch, Completion},
    projector_step, BalanceAccounting, GenerationCell,
};
use crate::{order_journal_store::PgJournalBackend, process_epoch};

fn envelope(event_type: &str, payload: serde_json::Value, at: DateTime<Utc>) -> EventEnvelope {
    EventEnvelope {
        event_id: Uuid::now_v7(),
        tenant_id: Uuid::now_v7(),
        stream_key: "test:8m-2b".to_string(),
        seq: 2,
        event_type: event_type.to_string(),
        payload,
        payload_schema_version: 1,
        occurred_at: at,
        ingested_at: at,
        idempotency_key: Uuid::now_v7().to_string(),
        trace_id: None,
        causation_id: None,
        command_id: None,
        workflow_id: None,
        actor_type: Some(ActorType::Daemon),
        actor_id: None,
        prev_hash: None,
        hash: None,
    }
}

fn april() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2027, 4, 1, 0, 0, 10).unwrap()
}

fn reset(at: DateTime<Utc>) -> EventEnvelope {
    let payload = serde_json::json!({
        "capital_base": "10000", "carried_positions_risk": "0",
        "month": at.month(), "year": at.year(), "timestamp": at,
    });
    envelope("month_boundary_reset", payload, at)
}

/// A position row of `tenant_id` at `last_seq` 1, state `entering`.
async fn position_of(pool: &PgPool, tenant_id: Uuid) -> Result<Uuid> {
    let position_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO positions_current (position_id, tenant_id, account_id, symbol, side, state, \
         current_quantity, last_event_id, last_seq, created_at, updated_at) \
         VALUES ($1, $2, $3, 'BTCUSDT', 'long', 'entering', 1, $4, 1, NOW(), NOW())",
    )
    .bind(position_id)
    .bind(tenant_id)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .execute(pool)
    .await?;
    Ok(position_id)
}

async fn position(pool: &PgPool) -> Result<Uuid> {
    position_of(pool, Uuid::now_v7()).await
}

fn entry_filled_payload(position_id: Uuid, at: DateTime<Utc>) -> serde_json::Value {
    serde_json::json!({
        "position_id": position_id, "order_id": Uuid::now_v7(), "fill_price": "100",
        "filled_quantity": "1", "fee": "0.1", "initial_stop": "90",
        "invalidation_guard_level": null, "timestamp": at,
    })
}

/// A formerly autocommit `positions_current` write.
fn entry_filled(position_id: Uuid, at: DateTime<Utc>) -> EventEnvelope {
    envelope("entry_filled", entry_filled_payload(position_id, at), at)
}

/// A transactional `positions_current` write (its `orders_current` update
/// finds no row; the position's `last_seq` moves).
fn entry_order_failed(position_id: Uuid, at: DateTime<Utc>) -> EventEnvelope {
    let payload = serde_json::json!({
        "position_id": position_id, "cycle_id": Uuid::now_v7(), "order_id": Uuid::now_v7(),
        "client_order_id": "m2b", "signal_id": Uuid::now_v7(), "reason": "8m-2b",
        "timestamp": at,
    });
    envelope("entry_order_failed", payload, at)
}

async fn state_and_seq(pool: &PgPool, position_id: Uuid) -> Result<(String, i64)> {
    Ok(
        sqlx::query_as("SELECT state, last_seq FROM positions_current WHERE position_id = $1")
            .bind(position_id)
            .fetch_one(pool)
            .await?,
    )
}

async fn months(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM monthly_state").fetch_one(pool).await?)
}

/// A balance daemon's context on a Spot accounting row at version 0.
async fn balance(pool: &PgPool) -> Result<BalanceAccounting> {
    let tenant_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO accounting_state (tenant_id, venue_id, budget_model) \
         VALUES ($1, $2, 'net_from_start_non_expanding_v1')",
    )
    .bind(tenant_id)
    .bind(VenueId::BINANCE_SPOT)
    .execute(pool)
    .await?;
    Ok(BalanceAccounting::for_daemon(
        AccountKind::BalanceAccount,
        tenant_id,
        &VenueId::new(VenueId::BINANCE_SPOT),
        &GenerationCell::default(),
    )
    .expect("a balance account builds a context"))
}

/// Killer of "a positions handler left autocommit" and "a transactional
/// handler without its key": on both account kinds (`positions_current` takes
/// the key alone on a balance account too), a formerly autocommit write
/// (`entry_filled`) and a transactional one (`entry_order_failed`) wait for a
/// completion holding the key, write nothing while it does, and land after
/// its `COMMIT`.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn every_positions_write_takes_the_key(pool: PgPool) -> Result<()> {
    let spot = balance(&pool).await?;
    for accounting in [None, Some(&spot)] {
        for event in [
            entry_filled as fn(Uuid, DateTime<Utc>) -> EventEnvelope,
            entry_order_failed,
        ] {
            let position_id = position(&pool).await?;
            let completion = Completion::hold(&pool).await?;
            let apply = {
                let (pool, event, accounting) =
                    (pool.clone(), event(position_id, april()), accounting.cloned());
                tokio::spawn(async move {
                    let step = projector_step(accounting.as_ref());
                    apply_event_to_projections_with(&pool, &event, step).await
                })
            };
            until_key_waiters(&pool, 1).await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
            ensure!(state_and_seq(&pool, position_id).await? == ("entering".into(), 1));
            ensure!(!apply.is_finished(), "the write did not wait for the key");
            completion.release().await?;
            apply.await??;
            ensure!(state_and_seq(&pool, position_id).await?.1 == 2);
        }
    }
    Ok(())
}

/// Killer of "USD-M `monthly_state` write left autocommit": a position
/// account's reset waits for completion, writes no month while it holds the
/// key, then lands; repeated progress with no holder stays healthy.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn a_usdm_monthly_state_write_waits_for_completion(pool: PgPool) -> Result<()> {
    let completion = Completion::hold(&pool).await?;
    let apply = {
        let pool = pool.clone();
        tokio::spawn(
            async move { apply_event_to_projections_with(&pool, &reset(april()), None).await },
        )
    };
    until_key_waiters(&pool, 1).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    ensure!(months(&pool).await? == 0);
    ensure!(!apply.is_finished());
    completion.release().await?;
    apply.await??;
    ensure!(months(&pool).await? == 1);
    for _ in 0..3 {
        apply_event_to_projections_with(&pool, &reset(april()), None).await?;
    }
    ensure!(months(&pool).await? == 1);
    Ok(())
}

/// What completion's own session sees: the tenant's latest epoch, its G4
/// (`system:%` events ingested in `month`'s calendar month), its intents, and
/// whether `month`'s `monthly_state` row exists.
async fn window(
    completion: &mut Completion,
    tenant_id: Uuid,
    month: DateTime<Utc>,
) -> Result<(Option<i64>, i64, i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT (SELECT max(epoch_seq) FROM process_epochs WHERE tenant_id = $1), \
                (SELECT count(*) FROM event_log WHERE tenant_id = $1 \
                 AND stream_key LIKE 'system:%' \
                 AND date_trunc('month', ingested_at) = date_trunc('month', $2::timestamptz)), \
                (SELECT count(*) FROM order_intents WHERE tenant_id = $1), \
                (SELECT count(*) FROM monthly_state WHERE year = $3 AND month = $4)",
    )
    .bind(tenant_id)
    .bind(month)
    .bind(month.year())
    .bind(month.month() as i32)
    .fetch_one(completion.connection())
    .await?)
}

/// The three-epoch case (8m PLAN-r4 §0.1 P1, §6.3, §8 8m-2b; killer of
/// "apply without the step"). Three epochs of one tenant: the first never
/// marked a clean stop, so the second, the older process still alive,
/// started unclean and its `unclean_start` was released; the third, the
/// certifying process, started while the second was alive and is unclean
/// too, released. The older process's append of this month's boundary reset,
/// a `system:` event and so in tx1's G4 (the tenant's `system:%` events
/// ingested this month), landed before completion; its apply of that stored
/// event (a USD-M `monthly_state` write) and an intent commit arrive between
/// completion's read and its `COMMIT`. Both wait for the key; completion's
/// re-read sees the certifying epoch, G4, the intents and the month
/// unchanged; both land after its `COMMIT`, ordered after the certified
/// state.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn older_live_process_apply_or_intent_is_ordered_against_completion(
    pool: PgPool,
) -> Result<()> {
    let tenant_id = Uuid::now_v7();
    let journal = PgJournalBackend::new(&pool, tenant_id);
    journal.check_ready().await.map_err(|error| anyhow!(error))?;
    let mut epochs = Vec::new();
    for _ in 0..3 {
        let start = process_epoch::start_epoch(&pool, tenant_id, "8m-2b").await?;
        let generation = start.unclean_generation.context("every start here is unclean")?;
        let release = ReleaseRecord {
            decision_id: Uuid::now_v7(),
            generation,
            actor: "operator@example.test".to_string(),
            auth_kind: "verified",
            note: "8m-2b".to_string(),
            treatment: "unbooked_accepted",
            decided_at: Utc::now(),
        };
        journal.release(&release).await.map_err(|error| anyhow!(error))?;
        epochs.push(start.epoch.epoch_seq);
    }
    // The older process's append, in G4 before tx1: this month's reset.
    let month = Utc::now();
    let stream_key = format!("system:month_boundary:{:04}-{:02}", month.year(), month.month());
    let payload = serde_json::json!({
        "capital_base": "10000", "carried_positions_risk": "0",
        "month": month.month(), "year": month.year(), "timestamp": month,
    });
    let event = Event::new(tenant_id, &stream_key, "month_boundary_reset", payload);
    let event_id = append_event(&pool, &stream_key, None, event).await?;
    let stored = query_events(&pool, QueryOptions::new(tenant_id).stream(stream_key.clone()))
        .await?
        .into_iter()
        .find(|envelope| envelope.event_id == event_id)
        .context("the stored append")?;

    let mut completion = Completion::hold(&pool).await?;
    let before = window(&mut completion, tenant_id, month).await?;
    ensure!(before == (Some(epochs[2]), 1, 0, 0), "{before:?}");
    let apply = {
        let pool = pool.clone();
        tokio::spawn(async move { apply_event_to_projections_with(&pool, &stored, None).await })
    };
    let commit = {
        let batch = usdm_batch();
        tokio::spawn(async move { journal.commit(&batch).await })
    };
    until_key_waiters(&pool, 2).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let inside = window(&mut completion, tenant_id, month).await?;
    ensure!(inside == before, "a write landed inside completion's window: {inside:?}");
    ensure!(!apply.is_finished() && !commit.is_finished());
    completion.release().await?;
    apply.await??;
    commit.await?.map_err(|failure| anyhow!("{failure:?}"))?;
    let (rows, intents): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM monthly_state WHERE year = $2 AND month = $3), \
                (SELECT count(*) FROM order_intents WHERE tenant_id = $1)",
    )
    .bind(tenant_id)
    .bind(month.year())
    .bind(month.month() as i32)
    .fetch_one(&pool)
    .await?;
    ensure!(
        (rows, intents) == (1, 1),
        "{rows} month rows, {intents} intents after completion"
    );
    Ok(())
}

async fn seeded(pool: &PgPool, tenant_id: Uuid) -> Result<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM event_log WHERE tenant_id = $1), \
                (SELECT count(*) FROM strategy_state_current WHERE tenant_id = $1)",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await?)
}

/// Killer of "the `db init` seed without the key": robson-db's seed of a new
/// tenant with an explicit account (reachable from `robsond db init`) writes
/// `event_log`, so it waits for completion, writes nothing while it holds the
/// key and lands after it; a run that finds its strategy writes nothing,
/// takes no key and does not wait.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn the_db_init_seed_waits_for_completion(pool: PgPool) -> Result<()> {
    let (tenant_id, account_id) = (Uuid::now_v7(), Uuid::now_v7());
    let completion = Completion::hold(&pool).await?;
    let seed = {
        let pool = pool.clone();
        tokio::spawn(async move {
            robson_db::init_minimal_data(&pool, Some(tenant_id), Some(account_id)).await
        })
    };
    until_key_waiters(&pool, 1).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    ensure!(seeded(&pool, tenant_id).await? == (0, 0), "the seed wrote inside the window");
    ensure!(!seed.is_finished());
    completion.release().await?;
    ensure!(seed.await?.map_err(|error| anyhow!("{error:?}"))? == (tenant_id, account_id));
    ensure!(seeded(&pool, tenant_id).await? == (1, 1));
    let completion = Completion::hold(&pool).await?;
    let again = tokio::time::timeout(
        Duration::from_secs(5),
        robson_db::init_minimal_data(&pool, Some(tenant_id), Some(account_id)),
    )
    .await
    .map_err(|_| anyhow!("a run that writes nothing waited for the key"))?;
    ensure!(again.map_err(|error| anyhow!("{error:?}"))? == (tenant_id, account_id));
    completion.release().await?;
    ensure!(seeded(&pool, tenant_id).await? == (1, 1));
    Ok(())
}

/// Append and apply latency with the key, measured under §7.2's workload
/// (8m PLAN-r4 §7.2, §8 8m-2b [AE]): with 10,000 events appended in the
/// month's partitions, 200 positions and 2,000 intents preloaded outside the
/// measured intervals, 200 appends (the try-lock first), 200 formerly
/// autocommit `positions_current` applies on those positions (`BEGIN`, key,
/// statement, `COMMIT`) and 200 USD-M `monthly_state` applies, on PG16 with no
/// holder. Printed as p50, p99 and max; each call stays under a statement's
/// 1 s bound.
#[sqlx::test(migrations = "../migrations")]
#[ignore = "requires DATABASE_URL"]
async fn append_and_apply_latency_with_the_key(pool: PgPool) -> Result<()> {
    const N: usize = 200;
    let tenant_id = Uuid::now_v7();
    for stream in 0..100 {
        let stream_key = format!("position:{}", Uuid::now_v7());
        for i in 0..100 {
            let payload = serde_json::json!({ "stream": stream, "i": i });
            append_event(
                &pool,
                &stream_key,
                None,
                Event::new(tenant_id, &stream_key, "probe", payload),
            )
            .await?;
        }
    }
    let mut positions = Vec::new();
    for _ in 0..N {
        positions.push(position_of(&pool, tenant_id).await?);
    }
    let journal = PgJournalBackend::new(&pool, tenant_id);
    journal.check_ready().await.map_err(|error| anyhow!(error))?;
    for _ in 0..2_000 {
        journal.commit(&usdm_batch()).await.map_err(|failure| anyhow!("{failure:?}"))?;
    }
    let (events, intents): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM event_log WHERE tenant_id = $1), \
                (SELECT count(*) FROM order_intents WHERE tenant_id = $1)",
    )
    .bind(tenant_id)
    .fetch_one(&pool)
    .await?;
    ensure!(
        events >= 10_000 && intents == 2_000,
        "workload {events} events, {intents} intents"
    );

    let (mut appends, mut applies, mut months) = (Vec::new(), Vec::new(), Vec::new());
    for (i, position_id) in positions.into_iter().enumerate() {
        let stream_key = format!("position:{}", Uuid::now_v7());
        let event = Event::new(tenant_id, &stream_key, "probe", serde_json::json!({ "i": i }));
        let started = std::time::Instant::now();
        append_event(&pool, &stream_key, None, event).await?;
        appends.push(started.elapsed());
        let event = entry_filled(position_id, april());
        let started = std::time::Instant::now();
        apply_event_to_projections_with(&pool, &event, None).await?;
        applies.push(started.elapsed());
        let started = std::time::Instant::now();
        apply_event_to_projections_with(&pool, &reset(april()), None).await?;
        months.push(started.elapsed());
    }
    for (what, mut samples) in [
        ("append", appends),
        ("positions apply", applies),
        ("monthly apply", months),
    ] {
        samples.sort();
        let (p50, p99, max) = (samples[N / 2], samples[N * 99 / 100], samples[N - 1]);
        println!(
            "LATENCY_8M2B {what}: n={N} p50={p50:?} p99={p99:?} max={max:?} \
             (workload: {events} events, {N} positions, {intents} intents)"
        );
        ensure!(max < Duration::from_secs(1), "{what} max {max:?}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The source guard (8m PLAN-r4 §6.7).
// ---------------------------------------------------------------------------

/// Certification inputs (8m PLAN-r4 §6.7), including 8l's and 8p's tables
/// that completion reads, whose migrations have landed (M46-M54, M36).
const INPUTS: [&str; 13] = [
    "accounting_state",
    "monthly_state",
    "event_log",
    "positions_current",
    "order_intents",
    "recovery_incidents",
    "stream_suspensions",
    "process_epochs",
    "funding_sagas",
    "cash_discrepancies",
    "cash_anchors",
    "activation_bases",
    "venue_accounts",
];

/// The calls that open or close a transaction: `begin`, `begin_with` and
/// `try_begin` open one, `commit` and `rollback` close it.
const BOUNDARY_CALLS: [&str; 5] = ["begin", "begin_with", "try_begin", "commit", "rollback"];

/// The sqlx macros that read their SQL from a file at compile time.
const QUERY_FILE_MACROS: [&str; 6] = [
    "query_file",
    "query_file_as",
    "query_file_scalar",
    "query_file_unchecked",
    "query_file_as_unchecked",
    "query_file_scalar_unchecked",
];

/// Where a table entry's calls count.
#[derive(Clone, Copy, PartialEq)]
enum Reach {
    /// Every production file (a public function).
    Anywhere,
    /// Its own file and the files of its module's descendants, where a
    /// private item is visible (a journal store method used from
    /// `order_journal_balance.rs` and `order_journal_clocked.rs`).
    Module,
    /// That one file only (a trait's sole implementation, called through the
    /// trait object where the trait is used).
    In(&'static str),
}

/// Which call forms are calls of a table entry.
#[derive(Clone, Copy, PartialEq)]
enum Form {
    /// `name(`, never `.name(` (`boundary::begin` is not `pool.begin()`).
    Free,
    /// `name(` or `.name(` on any receiver.
    Method,
    /// A method of the defining type, by any receiver or path: `.name(`
    /// (`self.name(`, `this.name(`, `(*self).name(`: no receiver identity is
    /// read) or `…::name(` (`Self::name(self, …)`, `Type::name(…)`,
    /// `<T as Tr>::name(…)`), never a bare `name(`, which no method is.
    Receiver,
}

/// One function the guard trusts, reviewed here, verified against its own
/// body ([`Guard::problems`]) and required to cover at least one site: a
/// stale entry fails the guard.
#[derive(Clone, Copy)]
struct Entry {
    file: &'static str,
    name: &'static str,
    reach: Reach,
    form: Form,
}

const fn entry(file: &'static str, name: &'static str, reach: Reach, form: Form) -> Entry {
    Entry { file, name, reach, form }
}

/// Self-contained transactions around a statement passed to them: `BEGIN`,
/// the key, the statement, `COMMIT`, in that order, or a call of another
/// wrapper. They cover the statement that hands them its SQL, never later
/// code of their caller.
const WRAPPERS: [Entry; 4] = [
    entry(
        "robson-eventlog/src/certificate.rs",
        "execute_participating",
        Reach::Anywhere,
        Form::Free,
    ),
    entry("robson-eventlog/src/append.rs", "append_event", Reach::Anywhere, Form::Free),
    entry(
        "robson-projector/src/handlers/monthly_state.rs",
        "write",
        Reach::Module,
        Form::Free,
    ),
    entry(
        "robson-projector/src/handlers/positions.rs",
        "execute_participating",
        Reach::Module,
        Form::Method,
    ),
];

/// Functions that take the key on the transaction they are given (a
/// [`MARKERS`] entry in their body): a later write of their caller, in the
/// same function, is covered.
const KEYING: [Entry; 4] = [
    entry("robsond/src/boundary.rs", "begin", Reach::Module, Form::Free),
    entry(
        "robsond/src/order_journal_balance.rs",
        "prepare_accounting_write",
        Reach::Anywhere,
        Form::Method,
    ),
    entry(
        "robson-projector/src/handlers/monthly_state.rs",
        "step",
        Reach::Module,
        Form::Free,
    ),
    // `AccountingStep::run`'s sole implementation (the balance step: the
    // bounds, then `advance`), called as `step.run(…)` by the month `step`.
    entry(
        "robsond/src/balance_accounting.rs",
        "run",
        Reach::In("robson-projector/src/handlers/monthly_state.rs"),
        Form::Method,
    ),
];

/// Writers on their caller's transaction: covered only when every call of
/// them is.
const DELEGATED: [Entry; 5] = [
    entry("robson-eventlog/src/append.rs", "append_event_tx", Reach::Anywhere, Form::Free),
    entry("robsond/src/balance_accounting.rs", "bump", Reach::Module, Form::Receiver),
    entry(
        "robsond/src/balance_accounting.rs",
        "bump_and_advance",
        Reach::Module,
        Form::Receiver,
    ),
    entry(
        "robsond/src/order_journal_store.rs",
        "write_routed",
        Reach::Module,
        Form::Receiver,
    ),
    entry("robsond/src/order_journal_store.rs", "write", Reach::Module, Form::Receiver),
];

/// What makes later statements of the same function participants: the key
/// statement (`participate`, `try_participate`, robson-db's
/// `CERTIFICATE_PARTICIPATE`), the balance context's `advance` (which issues
/// it), `advance_participating` and `advance_from` (which take the typed
/// proof of an earlier one), and that proof as a parameter; matched in code
/// (comments and literals blanked) as token sequences ([`seq`]).
const MARKERS: [&str; 7] = [
    "participate(",
    "try_participate(",
    "CERTIFICATE_PARTICIPATE",
    ".advance(",
    ".advance_participating(",
    ".advance_from(",
    "&Participating",
];

/// Calls of `name` in `text[from..to]` in the given form: word-bounded and
/// followed by `(` ([`open_paren`]: past whitespace, masked comments and a
/// turbofish), never its definition; a method's `.` (and `self`) read across
/// whitespace too; a raw identifier (`r#name`) is `name`.
fn calls(text: &str, name: &str, from: usize, to: usize, form: Form) -> Vec<usize> {
    uses(text, name, from, to)
        .into_iter()
        .filter(|&i| {
            // A raw identifier's `r#` belongs to the name.
            let raw = text[..i].ends_with("r#") && !text[..i - 2].ends_with(|c: char| is_ident(c));
            let before = text[..if raw { i - 2 } else { i }].trim_end();
            let definition =
                before.strip_suffix("fn").is_some_and(|prefix| !prefix.ends_with(is_ident));
            text[open_paren(text, i + name.len())..].starts_with('(')
                && !definition
                && match form {
                    Form::Free => !before.ends_with('.'),
                    Form::Method => true,
                    Form::Receiver => before.ends_with('.') || before.ends_with("::"),
                }
        })
        .collect()
}

/// Where the `(` opening a call of a name ending at `at` is: past whitespace
/// (and masked comments) and a turbofish (`::<…>`); `at` itself's next
/// non-space offset when none follows.
fn open_paren(text: &str, at: usize) -> usize {
    let skip = |i: usize| i + text[i..].len() - text[i..].trim_start().len();
    let i = skip(at);
    let Some(rest) = text[i..].strip_prefix("::") else {
        return i;
    };
    let lt = text.len() - rest.trim_start().len();
    if !text[lt..].starts_with('<') {
        return i;
    }
    let mut depth = 0usize;
    for (o, b) in text[lt..].bytes().enumerate() {
        match b {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return skip(lt + o + 1);
                }
            },
            _ => {},
        }
    }
    i
}

/// Each start of `pattern` in `text[from..to]`: its tokens (identifier runs
/// and single punctuation characters) in order, with any whitespace (or
/// masked comment) between them, each identifier word-bounded.
fn seq(text: &str, pattern: &str, from: usize, to: usize) -> Vec<usize> {
    let mut tokens = Vec::new();
    let mut rest = pattern;
    while let Some(c) = rest.chars().next() {
        let len = if is_ident(c) {
            rest.find(|c: char| !is_ident(c)).unwrap_or(rest.len())
        } else {
            c.len_utf8()
        };
        tokens.push(&rest[..len]);
        rest = rest[len..].trim_start();
    }
    let ident = |t: &str| t.starts_with(|c: char| is_ident(c));
    let Some((first, others)) = tokens.split_first() else {
        return Vec::new();
    };
    let starts: Vec<usize> = if ident(first) {
        uses(text, first, from, to)
    } else {
        text[from..to].match_indices(first).map(|(i, _)| from + i).collect()
    };
    starts
        .into_iter()
        .filter(|&start| {
            let mut i = start + first.len();
            for t in others {
                i += text[i..].len() - text[i..].trim_start().len();
                if !text[i..].starts_with(t)
                    || (ident(t) && text[i + t.len()..].starts_with(|c: char| is_ident(c)))
                {
                    return false;
                }
                i += t.len();
            }
            i <= to
        })
        .collect()
}

/// Where the [`MARKERS`] evidence `m` at `pos` has taken the key, if it does
/// where it stands (`body` being where the function's body opens): the end of
/// a key call awaited there (`participate(…).await`; a dropped or stored
/// future runs nothing), the end of the executor that runs the key's SQL
/// ([`key_statement`]), or the proof as a parameter (in the signature, before
/// `body`); never where it may be skipped ([`conditional`]). A site counts as
/// keyed only after that end: what a key call's own arguments do runs before
/// the key.
fn credited(text: &str, m: &str, pos: usize, body: usize) -> Option<usize> {
    if conditional(text, pos) {
        return None;
    }
    match m {
        "&Participating" => (pos < body && direct_proof(text, pos)).then_some(pos),
        "CERTIFICATE_PARTICIPATE" => key_statement(text, pos, pos + m.len()),
        "try_participate(" => {
            let open = pos + text[pos..].find('(')?;
            if !awaited_paren(text, open) {
                return None;
            }
            try_lock_held(text, pos, closing(text, open))
        },
        _ => {
            let open = pos + text[pos..].find('(')?;
            awaited_paren(text, open).then(|| closing(text, open))
        },
    }
}

/// Whether the proof marker at `pos` is a parameter's whole type (`key:
/// &Participating,` or `…)`) in the function's own parameter list
/// ([`own_parameters`]): the proof itself, which only a caller holding the
/// key can pass; never an optional or nested one (`Option<&Participating>`,
/// a tuple's part, a function-pointer type's parameter) nor a return type.
fn direct_proof(text: &str, pos: usize) -> bool {
    let name = "Participating";
    let after = pos + text[pos..].find(name).map_or(0, |i| i + name.len());
    let before = text[..pos].trim_end();
    before.ends_with(':')
        && !before.ends_with("::")
        && text[after..].trim_start().starts_with([',', ')'])
        && unclosed(text, pos).is_some_and(|(o, b)| b == b'(' && own_parameters(text, o))
}

/// Whether the `(` at `open` opens a function's own parameter list: `fn
/// name(` or `fn name<…>(`, standing in no other bracket. A function-pointer
/// type's parameters (`fn(key: &Participating)`) are none: its caller passes
/// no proof.
fn own_parameters(text: &str, open: usize) -> bool {
    let mut end = text[..open].trim_end().len();
    if text[..end].ends_with('>') {
        let mut depth = 0usize;
        let mut cut = None;
        for (i, b) in text[..end].bytes().enumerate().rev() {
            match b {
                b'>' => depth += 1,
                b'<' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        cut = Some(i);
                        break;
                    }
                },
                _ => {},
            }
        }
        let Some(c) = cut else {
            return false;
        };
        end = text[..c].trim_end().len();
    }
    let name = end - text[..end].trim_end_matches(is_ident).len();
    name > 0
        && &text[end - name..end] != "fn"
        && word_before(text, end - name, "fn")
        && unclosed(text, open).is_none_or(|(_, b)| b == b'{')
}

/// Whether the [`MARKERS`] evidence `m` is no call whose failure must be
/// ruled out: the proof as a parameter, or a try-lock (whose refusal
/// [`try_lock_held`] reads, its `?` part of the form).
fn proof(m: &str) -> bool {
    m == "&Participating" || m == "try_participate("
}

/// Where the key or keying call ending at `close` (through a pin holding it,
/// `Box::pin(…)`, `pin!(…)`) is known to have succeeded, if it is: just after
/// a `?` in its chain (`.await?`, `.await.map_err(…)?`), so a failure leaves;
/// the key is credited only from there, never from the call's own end (an
/// adapter's argument runs before that `?`, `map_err({ write; … })?`); or,
/// where `tail` is given (a keying function's checked
/// block or arm), its value is that block's own (`None =>
/// participate(c).await.map(drop).map_err(…),`), so a failure is the
/// function's. Only adapters that keep the error may stand between
/// ([`KEEPS_ERROR`]), and none whose argument (unless a closure) leaves by
/// `return`, `break` or `continue`: a recovery (`.or(…)`, `.or_else(…)`,
/// `.ok()`, `.unwrap_or(…)`) or any other call ends the chain unproved. A
/// result dropped, bound and ignored or matched away (`let _ = step(…).await;`)
/// is no success.
fn established(text: &str, close: usize, tail: Option<usize>) -> Option<usize> {
    let mut p = close;
    while let Some(rest) = Some(text[p..].trim_start()).filter(|r| r.starts_with(')')) {
        let at = text.len() - rest.len();
        let pin = opening(text, at);
        let mut from = pin.saturating_sub(64);
        while !text.is_char_boundary(from) {
            from -= 1;
        }
        let head: String = text[from..pin].split_whitespace().collect();
        if !(head.ends_with("Box::pin") || head.ends_with("pin!")) {
            break;
        }
        p = at + 1;
    }
    loop {
        let rest = text[p..].trim_start();
        let q = text.len() - rest.len();
        if rest.starts_with('?') {
            return Some(q + 1);
        }
        let Some(r) = rest.strip_prefix('.').map(str::trim_start) else {
            return tail
                .filter(|&t| {
                    q <= t && text[q..t].trim().trim_end_matches(['}', ',']).trim().is_empty()
                })
                .map(|_| q);
        };
        let n = r.find(|c: char| !is_ident(c)).unwrap_or(r.len());
        let after = text.len() - r[n..].len();
        if n == 0 {
            return None;
        }
        if &r[..n] == "await" {
            p = after;
            continue;
        }
        let open = open_paren(text, after);
        if !text[open..].starts_with('(') || !KEEPS_ERROR.contains(&&r[..n]) {
            return None;
        }
        // Its argument runs before it handles the result: one that leaves
        // (`map_err({ … return Ok(()); … })`) may turn a failure into a
        // success. A closure's `return` leaves only the closure.
        let close = closing(text, open);
        let args = &text[open + 1..close - 1];
        let lead = args.trim_start();
        let closure = lead.strip_prefix("move").map_or(lead, str::trim_start).starts_with('|');
        if !closure
            && ["return", "break", "continue"]
                .iter()
                .any(|w| !uses(args, w, 0, args.len()).is_empty())
        {
            return None;
        }
        p = close;
    }
}

/// The adapters a key's result may pass through on its way to `?` (or out
/// as a keying function's value) and keep its error an error: they map the
/// value or the error, annotate it, or instrument the future.
const KEEPS_ERROR: [&str; 7] = [
    "map_err",
    "map",
    "context",
    "with_context",
    "inspect",
    "inspect_err",
    "instrument",
];

/// Where a try-lock (`try_participate` at `pos`, its call ending at `close`)
/// is known to hold the key: only once its no-key path has left, in the one
/// form `if …try_participate(…).await?.is_none() { … return …; }`, after that
/// block. The condition is the refusal itself (a bare path to the name before
/// it, [`path_prefix`], and `{` right after `.is_none()`): never negated
/// (`if !…`), joined to another condition or read any other way; the block
/// [`leaves`] (a `return` statement at its own level, not a word inside
/// another). `None` is a refused try, no key.
fn try_lock_held(text: &str, pos: usize, close: usize) -> Option<usize> {
    let (start, _) = statement(text, pos);
    let head = text[start..pos].trim_start();
    if !word(head, "if") || !path_prefix(&head[2..]) {
        return None;
    }
    fn after<'a>(r: &'a str, t: &str) -> Option<&'a str> {
        r.trim_start().strip_prefix(t)
    }
    let rest = after(&text[close..], ".")?;
    let rest = after(rest, "await")?;
    let rest = after(rest, "?").unwrap_or(rest);
    let rest = after(rest, ".")?;
    let rest = after(rest, "is_none")?;
    let rest = after(after(rest, "(")?, ")")?;
    let block = text.len() - rest.trim_start().len();
    if !text[block..].starts_with('{') {
        return None;
    }
    let end = closing(text, block);
    leaves(text, block, block, end).then_some(end)
}

/// Whether `p`, all that precedes a name in its expression, is only a path to
/// it: nothing, or `a::b::` (a leading `::`, raw identifiers and whitespace
/// between tokens included); never an operator (`!`), another operand or a
/// call.
fn path_prefix(p: &str) -> bool {
    let mut rest = p.strip_suffix("r#").unwrap_or(p).trim();
    if let Some(r) = rest.strip_prefix("::") {
        rest = r.trim_start();
    }
    while !rest.is_empty() {
        let r = rest.strip_prefix("r#").unwrap_or(rest);
        let n = r.find(|c: char| !is_ident(c)).unwrap_or(r.len());
        let Some(r) = r[n..].trim_start().strip_prefix("::").filter(|_| n > 0) else {
            return false;
        };
        rest = r.trim_start();
    }
    true
}

/// Where the key's SQL (the constant between `pos` and `after`) has run: it is
/// the sole argument of a call (`query(CERTIFICATE_PARTICIPATE)`) on which an
/// executor is called and awaited (`.execute(…).await`); the end of that
/// executor call. An executor run on anything else credits nothing.
fn key_statement(text: &str, pos: usize, after: usize) -> Option<usize> {
    if !text[..pos].trim_end().ends_with('(') {
        return None;
    }
    let chained = text[after..].trim_start().strip_prefix(')')?.trim_start().strip_prefix('.')?;
    let at = text.len() - chained.trim_start().len();
    let name = EXECUTORS.iter().find(|e| {
        text[at..].starts_with(**e) && !text[at + e.len()..].starts_with(|c: char| is_ident(c))
    })?;
    let open = open_paren(text, at + name.len());
    (text[open..].starts_with('(') && awaited_paren(text, open)).then(|| closing(text, open))
}

/// Whether evidence at `at` may never run within its statement: inside a
/// closure (a `|` before it there; its body runs only if it is called),
/// after a short-circuit operator (`&&`, `||`), in an `else if` condition
/// (the statement there starts with `else`: an earlier branch taken skips
/// it), in a statement a `cfg` attribute may compile out (`#[cfg(…)]`,
/// `#[cfg_attr(…)]`), or inside any macro's arguments (`debug_assert_eq!(…)`,
/// a `tracing` field: the macro may never evaluate them).
fn conditional(text: &str, at: usize) -> bool {
    let (start, _) = statement(text, at);
    let before = &text[start..at];
    let mut head = before.trim_start();
    let mut gated = false;
    while let Some(bracket) = attribute(head) {
        let end = closing(head, bracket);
        gated |= ["cfg", "cfg_attr"]
            .iter()
            .any(|w| !uses(&head[bracket..end], w, 0, end - bracket).is_empty());
        head = head[end..].trim_start();
    }
    // An item's own attributes (before a proof parameter in its signature)
    // gate the item with its body, not a statement before the evidence.
    gated &= item_keyword(head).is_none();
    let mut open = unclosed(text, at);
    let mut in_macro = false;
    while let Some((o, _)) = open {
        in_macro |= text[..o].trim_end().ends_with('!');
        open = unclosed(text, o);
    }
    before.contains('|')
        || before.contains("&&")
        || word(before.trim_start(), "else")
        || gated
        || in_macro
}

/// Where the call of the name at `at` ends, if it is awaited where it is
/// made.
fn awaited(text: &str, at: usize) -> Option<usize> {
    let end = at + text[at..].find(|c: char| !is_ident(c)).unwrap_or(0);
    let open = open_paren(text, end);
    (text[open..].starts_with('(') && awaited_paren(text, open)).then(|| closing(text, open))
}

/// Whether the call whose `(` is at `open` is awaited: `.await` follows its
/// `)` (across whitespace), or the call is pinned (`Box::pin(…)`, `pin!(…)`)
/// and the pin is awaited.
fn awaited_paren(text: &str, open: usize) -> bool {
    let close = closing(text, open);
    let rest = text[close..].trim_start();
    let awaits = rest.strip_prefix('.').map(str::trim_start).is_some_and(|r| {
        r.strip_prefix("await")
            .is_some_and(|after| !after.starts_with(|c: char| is_ident(c)))
    });
    if awaits {
        return true;
    }
    if !rest.starts_with(')') {
        return false;
    }
    let pin = opening(text, text.len() - rest.len());
    let mut from = pin.saturating_sub(64);
    while !text.is_char_boundary(from) {
        from -= 1;
    }
    let head: String = text[from..pin].split_whitespace().collect();
    (head.ends_with("Box::pin") || head.ends_with("pin!")) && awaited_paren(text, pin)
}

/// Each offset of `pat` in `text[from..to]` outside any bracket opened in
/// the range.
fn top_level(text: &str, from: usize, to: usize, pat: &str) -> Vec<usize> {
    let mut depth = 0usize;
    let mut found = Vec::new();
    for (i, b) in text.bytes().enumerate().take(to).skip(from) {
        if depth == 0 && text[i..to].starts_with(pat) {
            found.push(i);
        }
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {},
        }
    }
    found
}

/// Whether `text` starts with the word `w`.
fn word(text: &str, w: &str) -> bool {
    text.strip_prefix(w)
        .is_some_and(|rest| !rest.starts_with(|c: char| is_ident(c)))
}

/// The arms of the `match` whose `{` is at `open`: `(head, from, to, braced)`,
/// `text[head..from]` its pattern and guard, a braced arm's body by its
/// block's `{` (`from`), any other (a braced body a chain continues,
/// `{ … }.or(Ok(()))`, included) by its expression `text[from..to]`: to its
/// `,`, or, block-like ([`block_like`]: `if … else …`, `const { … }`, …,
/// which needs no `,`), to its block's end ([`item_end`]) unless a chain
/// (`.`, `?`) continues it there.
fn match_arms(text: &str, open: usize) -> Vec<(usize, usize, usize, bool)> {
    let close = closing(text, open) - 1;
    let skip = |i: usize| i + text[i..close].len() - text[i..close].trim_start().len();
    let mut arms = Vec::new();
    let mut at = open + 1;
    while let Some(arrow) = top_level(text, at, close, "=>").first().copied() {
        let start = skip(arrow + 2);
        // A braced body a chain continues (`{ … }.or(Ok(()))`) is an
        // expression arm, to its `,`, like any other.
        let continued = text[start..].starts_with('{')
            && text[closing(text, start).min(close)..close]
                .trim_start()
                .starts_with(['.', '?']);
        if text[start..].starts_with('{') && !continued {
            arms.push((at, start, start, true));
            let after = skip(closing(text, start));
            at = if text[after..close].starts_with(',') {
                after + 1
            } else {
                after
            };
        } else {
            let block = item_end(text, start).min(close);
            let chained = text[block..close].trim_start().starts_with(['.', '?']);
            if block_like(&text[start..close]) && !chained {
                arms.push((at, start, block, false));
                let after = skip(block);
                at = if text[after..close].starts_with(',') {
                    after + 1
                } else {
                    after
                };
                continue;
            }
            let end = top_level(text, start, close, ",").first().copied().unwrap_or(close);
            arms.push((at, start, end, false));
            at = (end + 1).min(close);
        }
    }
    arms
}

/// Whether the block at `open` is a `match`'s arms and `e` and `at` lie in
/// different arms of it: mutually exclusive, so evidence in one reaches no
/// statement of another.
fn other_arm(text: &str, open: usize, e: usize, at: usize) -> bool {
    let close = closing(text, open);
    if at >= close || top_level(text, open + 1, close - 1, "=>").is_empty() {
        return false;
    }
    let arm = |x: usize| {
        match_arms(text, open).into_iter().position(|(head, from, to, braced)| {
            let end = if braced { closing(text, from) } else { to };
            head <= x && x < end
        })
    };
    arm(e) != arm(at)
}

/// The offset of the `(` matching the `)` at `close`.
fn opening(text: &str, close: usize) -> usize {
    let mut depth = 0usize;
    for (i, b) in text[..close].bytes().enumerate().rev() {
        match b {
            b')' => depth += 1,
            b'(' if depth == 0 => return i,
            b'(' => depth -= 1,
            _ => {},
        }
    }
    0
}

/// The sqlx calls that run a statement (raw execution), however written
/// (`q.execute(…)`, `Executor::execute(…)`, `q . fetch_one (…)`).
const EXECUTORS: [&str; 9] = [
    "execute",
    "execute_many",
    "fetch",
    "fetch_one",
    "fetch_all",
    "fetch_optional",
    "fetch_many",
    "copy_in_raw",
    "copy_out_raw",
];

/// Each raw execution in `text[from..to]`: a call of an [`EXECUTORS`] name.
fn executes(text: &str, from: usize, to: usize) -> Vec<usize> {
    EXECUTORS.iter().flat_map(|e| calls(text, e, from, to, Form::Method)).collect()
}

/// The statement around `at`: after the previous `;`, `{` or `}`, to the
/// next `;`.
fn statement(text: &str, at: usize) -> (usize, usize) {
    let start = text[..at].rfind([';', '{', '}']).map_or(0, |s| s + 1);
    let end = text[at..].find(';').map_or(text.len(), |e| at + e);
    (start, end)
}

/// Where the statement holding `at` ends, by matched delimiters: the first
/// `;` outside every bracket opened after `at`, or the `}` closing the block
/// whose tail it is when that block is a statement's own (a function, branch,
/// arm, closure or `async` body, a `let` initializer). Brackets opened before
/// `at` and closed after it are passed through: the arguments holding it, and
/// a block whose value flows on in the statement ([`flows_on`]:
/// `sqlx::query({ "…" })`, `query(if c { "…" } else { "…" })`,
/// `{ sqlx::query("…") }.bind(…)`). A `;` inside a block in a later argument
/// ends nothing.
fn matched_statement_end(text: &str, at: usize) -> usize {
    let mut depth = 0usize;
    for (i, b) in text[at..].bytes().enumerate() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b'}' if depth == 0 && flows_on(text, at + i) => {},
            b';' | b'}' if depth == 0 => return at + i,
            _ => {},
        }
    }
    text.len()
}

/// Where the value of the SQL at `at` is held: the start of the statement
/// that may bind it with `let`, read out through each block of which it is
/// the tail ([`tail`]: `let q = { sqlx::query(…) };`); and the innermost
/// `async` block or closure holding it at any depth ([`deferred`]: inside a
/// branch of it too), if any, which runs later, where it is awaited or
/// called. Such a block is returned (its
/// opening) with whether a `let` binds it directly (`let f = async { … };`,
/// the holder's statement then returned) or it is passed on (`spawn(async {
/// … })`, `Box::pin(async { … })`). The function's own body (opening at
/// `body`) ends the search.
fn held_by(text: &str, at: usize, body: usize) -> (usize, Option<(usize, bool)>) {
    let (mut start, mut from, mut inner, mut tails) = (statement(text, at).0, at, at, true);
    loop {
        // The innermost block around `inner`, through the calls holding it.
        let mut open = unclosed(text, inner);
        while let Some((o, _)) = open.filter(|&(_, b)| b != b'{') {
            open = unclosed(text, o);
        }
        let Some((open, _)) = open.filter(|&(o, _)| o > body) else {
            break;
        };
        if deferred(text, open) {
            let passed = unclosed(text, open).is_some_and(|(_, b)| b != b'{');
            return if passed {
                (start, Some((open, false)))
            } else {
                (statement(text, open).0, Some((open, true)))
            };
        }
        // The value goes on outward only from a block's tail; the search for
        // an enclosing `async` block or closure goes on regardless.
        let close = closing(text, open) - 1;
        if tails && tail(text, from, close) {
            // An `else` branch's value is the whole `if … else`'s.
            start = if word_before(text, open, "else") {
                open - statement_head(text, open).len()
            } else {
                statement(text, open).0
            };
            from = close + 1;
        } else {
            tails = false;
        }
        inner = open;
    }
    (start, None)
}

/// The body of a braceless closure holding the SQL at `at`, in the statement
/// starting at `start` (`|x| sqlx::query(…).bind(x)`, `async || sqlx::query(…)
/// .execute(pool).await`), if any: a `|…|` parameter list where an expression
/// starts (after `=`, `(`, `[`, `,`, `=>`, `move`, `async` or `return`; never
/// a `||` operator), at one of the bracket levels from `at` outward, with no
/// `,` or `;` between it and `at` at that level. Returned with whether a
/// `let` binds the closure directly (`let run = async move || …;`).
fn braceless_closure(text: &str, start: usize, at: usize) -> Option<(usize, bool)> {
    let bytes = text.as_bytes();
    let mut inner = at;
    loop {
        let level = unclosed(text, inner).filter(|&(o, b)| o >= start && b != b'{');
        let from = level.map_or(start, |(o, _)| o + 1);
        let (mut depth, mut head, mut i) = (0usize, None, from);
        while i < inner {
            match bytes[i] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                b',' | b';' if depth == 0 => head = None,
                b'|' if depth == 0 => {
                    let before = text[from..i].trim_end();
                    let opens = before.is_empty()
                        || before.ends_with(['=', '(', '[', ',', '>'])
                        || ["move", "async", "return"].iter().any(|w| word_before(text, i, w));
                    if let Some(close) = text[i + 1..inner].find('|').filter(|_| opens) {
                        head = Some((i, i + 1 + close + 1));
                        i += close + 2;
                        continue;
                    }
                },
                _ => {},
            }
            i += 1;
        }
        if let Some((h, body)) = head {
            let bound = level.is_none()
                && text[start..h].trim().strip_prefix("let ").is_some_and(|r| {
                    let r = r.trim_start();
                    let r = r.strip_prefix("mut ").unwrap_or(r).trim_start_matches(is_ident);
                    r.trim_start()
                        .strip_prefix('=')
                        .is_some_and(|r| r.split_whitespace().all(|w| w == "async" || w == "move"))
                });
            return Some((body, bound));
        }
        inner = level?.0;
    }
}

/// Whether the word `w` (a whole identifier) ends the code before `at`.
fn word_before(text: &str, at: usize, w: &str) -> bool {
    text[..at].trim_end().strip_suffix(w).is_some_and(|b| !b.ends_with(is_ident))
}

/// Whether the block opening at `open` runs later, when its value is awaited
/// or called: an `async` block (`async {`, `async move {`) or a closure's
/// body (`|…| {`, `move || {`, `|…| -> T {`); never a branch whose condition
/// holds `||`.
fn deferred(text: &str, open: usize) -> bool {
    let before = text[..open].trim_end();
    let unmoved = before.strip_suffix("move").map_or(before, str::trim_end);
    let async_block = unmoved.strip_suffix("async").is_some_and(|b| !b.ends_with(is_ident));
    let params = before.rfind('|').is_some_and(|bar| {
        let after = before[bar + 1..].trim_start();
        after.is_empty() || after.starts_with("->")
    });
    async_block || params
}

/// Each use of the `let` binding `name` after `end` (its statement's end),
/// in the function ending at `fn_end`, within the block holding the binding.
/// A shadowing `let` of the same name takes over only within its own block
/// (the binding's, or a nested one, after which the binding is seen again),
/// from the end of its initializer: a use in that initializer
/// (`let q = q.bind(1);`) is still this binding's. A binding in a condition
/// (`if let q @ Some(_) = … { … }`, `while let`) shadows only within its
/// branch; the pattern's own name is no use. Any other pattern binding the
/// name keeps every use, conservatively.
fn later_uses(text: &str, name: &str, end: usize, fn_end: usize) -> Vec<usize> {
    // The innermost block around `at`, through the calls holding it.
    let block_end = |at: usize, limit: usize| {
        let mut open = unclosed(text, at);
        while let Some((o, _)) = open.filter(|&(_, b)| b != b'{') {
            open = unclosed(text, o);
        }
        open.map_or(limit, |(o, _)| closing(text, o).min(limit))
    };
    let limit = block_end(end, fn_end);
    let mut found = uses(text, name, end, limit);
    for l in uses(text, "let", end, limit) {
        let rest = text[l + 3..].trim_start();
        let rest = rest.strip_prefix("mut ").unwrap_or(rest);
        if !rest.starts_with(name) || rest[name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let shadow = text.len() - rest.len();
        found.retain(|&u| u != shadow);
        if text[..l].trim_end().ends_with([';', '{', '}']) {
            // A `let` statement: from the end of its initializer to the end
            // of its block.
            let from = matched_statement_end(text, l);
            let to = block_end(l, limit);
            found.retain(|&u| !(from <= u && u < to));
            continue;
        }
        // A condition's binding: within the branch its condition opens.
        let mut depth = 0usize;
        for (i, b) in text[l..limit].bytes().enumerate() {
            match b {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b'{' if depth == 0 => {
                    let (open, close) = (l + i, closing(text, l + i));
                    found.retain(|&u| !(open < u && u < close));
                    break;
                },
                b';' if depth == 0 => break,
                _ => {},
            }
        }
    }
    found
}

/// Whether what stands at `from` is the tail of the block closing at
/// `close`: no `;` follows it at its own level (outside the brackets opened
/// after it).
fn tail(text: &str, from: usize, close: usize) -> bool {
    let mut depth = 0usize;
    for b in text[from..close].bytes() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b';' if depth == 0 => return false,
            _ => {},
        }
    }
    true
}

/// The innermost bracket still open at `at`: its offset and byte.
fn unclosed(text: &str, at: usize) -> Option<(usize, u8)> {
    let mut depth = 0usize;
    for (i, b) in text[..at].bytes().enumerate().rev() {
        match b {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' if depth == 0 => return Some((i, b)),
            b'(' | b'[' | b'{' => depth -= 1,
            _ => {},
        }
    }
    None
}

/// Whether the block closed at `close` is an expression whose value flows on
/// in its statement: it stands inside a call's or an array's brackets, or the
/// construct it ends (with an `if`'s further `else` branches, or the `match`
/// whose arm it is) is followed by a chain (`.`, `?`). A block that is a
/// statement's own (a function, loop, closure or `async` body, a branch whose
/// value nothing chains on) ends its statement.
fn flows_on(text: &str, close: usize) -> bool {
    let Some((open, _)) = unclosed(text, close) else {
        return false;
    };
    let outer = unclosed(text, open);
    if outer.is_some_and(|(_, b)| b == b'(' || b == b'[') {
        return true;
    }
    let mut end = close + 1;
    if text[..open].trim_end().ends_with("=>") {
        if let Some((m, _)) = outer {
            end = closing(text, m);
        }
    }
    loop {
        let rest = text[end..].trim_start();
        if !word(rest, "else") {
            break;
        }
        let at = text.len() - rest.len() + 4;
        let Some(o) = text[at..].find('{') else {
            break;
        };
        end = closing(text, at + o);
    }
    let rest = text[end..].trim_start();
    rest.starts_with('.') || rest.starts_with('?')
}

/// Whether the SQL at `at`, in the statement `text[start..end]`, runs in that
/// statement (a site that is itself a call, `append_event_tx(…)`, runs where
/// it is awaited): the query built around it (the name or literal at `at`, then
/// each call holding it, innermost first, each with its method chain:
/// `sqlx::query(…).bind(…)`) reaches an [`EXECUTORS`] call (a method of the
/// query, or a call taking it: `tx.execute(query)`), then an `.await` of that
/// chain or of a call holding it, through adapters that poll it only
/// (`.instrument(…).await`, `Box::pin(…).await`, `timeout(d, …).await`;
/// [`POLLS_METHOD`], [`polls`]): any other call or method may return the
/// future unpolled (`std::future::ready(q.execute(pool)).await`). A query
/// only built there (`let q = sqlx::query(…).bind(x.await);`, run later
/// through `q`) does not run there; nor does one inside a block or an array
/// (a macro's brackets aside), conservatively.
fn runs_in_statement(text: &str, start: usize, at: usize, end: usize) -> bool {
    // The chain from `p`: `?`, `.await`, `.name(…)` (a turbofish included);
    // anything else ends it.
    fn chain(text: &str, mut p: usize, end: usize, executed: &mut bool) -> bool {
        loop {
            // A chain that leaves the statement (a `;` in a closure in it)
            // ends there.
            let rest = text.get(p..end).unwrap_or("").trim_start();
            let q = end - rest.len();
            if rest.starts_with('?') {
                p = q + 1;
                continue;
            }
            let Some(r) = rest.strip_prefix('.') else {
                return false;
            };
            let r = r.trim_start();
            let r = r.strip_prefix("r#").unwrap_or(r);
            let n = r.find(|c: char| !is_ident(c)).unwrap_or(r.len());
            let after = end - r[n..].len();
            if &r[..n] == "await" {
                if *executed {
                    return true;
                }
                p = after;
                continue;
            }
            let open = open_paren(text, after);
            if n == 0 || !text[open..].starts_with('(') {
                return false;
            }
            // An executor's future survives only a method that polls it when
            // awaited ([`POLLS_METHOD`]: `.instrument(…)`); any other may
            // return it unpolled.
            let name = &r[..n];
            *executed = (*executed && POLLS_METHOD.contains(&name)) || EXECUTORS.contains(&name);
            p = closing(text, open);
        }
    }
    let mut executed = false;
    let mut token = at + text[at..end].find(|c: char| !is_ident(c)).unwrap_or(0);
    // A binding awaited itself (`f.await`, `fut.await`: a future holding the
    // SQL, an `async` block's or an executor's) runs it there; a query is no
    // future, so only a future can be.
    let rest = text[token..end].trim_start();
    if token > at && rest.strip_prefix('.').map(str::trim_start).is_some_and(|r| word(r, "await")) {
        return true;
    }
    // A site that is itself a call (`append_event_tx(…)`) runs where it is
    // awaited; a macro's (`query_file!(…)`) only builds the query.
    let bang = rest.starts_with('!');
    let open = open_paren(text, if bang { end - rest.len() + 1 } else { token });
    if open < end && text[open..].starts_with('(') {
        executed = !bang;
        token = closing(text, open);
    }
    if chain(text, token, end, &mut executed) {
        return true;
    }
    let mut inner = at;
    loop {
        // The innermost bracket still open at `inner`: a call, else stop.
        let mut depth = 0usize;
        let mut open = None;
        for (i, b) in text[start..inner].bytes().enumerate().rev() {
            match b {
                b')' | b']' | b'}' => depth += 1,
                b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
                b'(' | b'[' | b'{' => {
                    open = Some((start + i, b));
                    break;
                },
                _ => {},
            }
        }
        // A macro's brackets (`concat! { … }`, `concat![…]`) as a call's.
        let Some((open, _)) =
            open.filter(|&(o, b)| b == b'(' || text[..o].trim_end().ends_with('!'))
        else {
            return false;
        };
        // An executor's future inside survives only a call that polls it when
        // awaited ([`polls`]); any other may return it unpolled
        // (`std::future::ready(q.execute(pool)).await`).
        executed &= polls(text, open);
        executed |= EXECUTORS.iter().any(|e| {
            calls(text, e, start, open, Form::Method)
                .into_iter()
                .any(|c| open_paren(text, c + e.len()) == open)
        });
        if chain(text, closing(text, open), end, &mut executed) {
            return true;
        }
        inner = open;
    }
}

/// Methods on a future that poll it when awaited (reviewed: tracing's
/// `instrument`, production's statement and I/O spans).
const POLLS_METHOD: [&str; 1] = ["instrument"];

/// Whether the call whose bracket opens at `open` polls the future handed to
/// it when awaited: a reviewed deadline ([`RUNS_FUTURE`]) or a pin
/// (`Box::pin(…)`, `pin!(…)`).
fn polls(text: &str, open: usize) -> bool {
    let head = text[..open].trim_end();
    let named = |name: &str| head.strip_suffix(name).is_some_and(|b| !b.ends_with(is_ident));
    RUNS_FUTURE.iter().any(|name| named(name)) || named("Box::pin") || named("pin!")
}

/// The offset of the `[` of an attribute starting `text` (`#`, an optional
/// `!`, `[`, read across whitespace), if one does.
fn attribute(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix('!').map_or(rest, str::trim_start);
    rest.starts_with('[').then(|| text.len() - rest.len())
}

/// Whether the use at `at` is in a `use` declaration (an import, not a use).
fn imported(text: &str, at: usize) -> bool {
    let mut item = text[text[..at].rfind(';').map_or(0, |s| s + 1)..at].trim_start();
    while let Some(bracket) = attribute(item) {
        item = item[closing(item, bracket)..].trim_start();
    }
    let item = item[visibility(item)..].trim_start();
    item.strip_prefix("use")
        .is_some_and(|rest| !rest.starts_with(|c: char| is_ident(c)))
}

struct Source {
    rel: String,
    dir: Option<PathBuf>,
    /// The production source (test items removed): its SQL and paths.
    raw: String,
    /// [`code_only`] of `raw`: structure, names, markers and keywords are
    /// read here, so no string literal can supply them.
    text: String,
    /// [`sql_view`] of `raw`, lowercased: SQL is detected here.
    lower: String,
    /// The offset in `raw` (and `text`) of each byte of `lower`.
    sql_at: Vec<usize>,
    /// Where each (joined) literal's contents start in `lower`.
    starts: Vec<usize>,
    /// Each statement separator in `lower`: a `;` outside every SQL quote.
    semicolons: Vec<usize>,
    /// Where its SQL starts or ends a transaction ([`sql_boundaries`]).
    bounds: Vec<usize>,
    /// Those that only open one ([`opens_transaction`]).
    opens: Vec<usize>,
    /// Each transaction boundary (a [`BOUNDARY_CALLS`] call or boundary SQL)
    /// a closure's or an `async` block's body holds, with where that runs
    /// ([`deferred_boundary`]).
    deferred: Vec<(usize, Deferred)>,
    functions: Vec<(String, usize, usize, bool)>,
}

impl Source {
    fn line(&self, at: usize) -> usize {
        self.text[..at].matches('\n').count() + 1
    }

    fn enclosing(&self, at: usize) -> Option<&(String, usize, usize, bool)> {
        self.functions
            .iter()
            .filter(|(_, start, end, _)| *start < at && at < *end)
            .max_by_key(|(_, start, ..)| *start)
    }
}

/// The production sources and the reviewed tables the guard trusts.
struct Guard {
    files: Vec<Source>,
    /// Each file's declaring file, by index.
    parents: Vec<Option<usize>>,
    wrappers: Vec<Entry>,
    keying: Vec<Entry>,
    delegated: Vec<Entry>,
    /// Reviewed functions that end a transaction their caller opened
    /// ([`RELEASERS`], or a fixture's): each call of one is a boundary
    /// ([`Guard::releasing_calls`]).
    releasers: Vec<Entry>,
    /// `(file, name)` of every table entry that covered a site.
    used: std::cell::RefCell<BTreeSet<(&'static str, &'static str)>>,
}

impl Guard {
    fn new(
        sources: Vec<(String, Option<PathBuf>, String, Option<String>)>,
        wrappers: &[Entry],
        keying: &[Entry],
        delegated: &[Entry],
    ) -> Self {
        let parents = sources
            .iter()
            .map(|(.., parent)| {
                parent.as_ref().map(|p| sources.iter().position(|(rel, ..)| rel == p).expect(p))
            })
            .collect();
        let files = sources
            .into_iter()
            .map(|(rel, dir, text, _)| {
                let raw = without_test_modules(&text);
                let text = code_only(&raw);
                let view = sql_view(&raw);
                let sql = String::from_utf8(view.sql).expect("decoded literals are UTF-8");
                let mut source = Source {
                    rel,
                    dir,
                    lower: sql.to_ascii_lowercase(),
                    sql_at: view.sql_at,
                    starts: view.starts,
                    semicolons: view.semicolons,
                    bounds: Vec::new(),
                    opens: Vec::new(),
                    deferred: Vec::new(),
                    functions: functions(&text),
                    raw,
                    text,
                };
                source.bounds = sql_boundaries(&source);
                source.opens = source
                    .bounds
                    .iter()
                    .copied()
                    .filter(|&b| opens_transaction(&source, b))
                    .collect();
                let calls_at: Vec<usize> = BOUNDARY_CALLS
                    .iter()
                    .flat_map(|b| calls(&source.text, b, 0, source.text.len(), Form::Method))
                    .collect();
                source.deferred = calls_at
                    .into_iter()
                    .chain(source.bounds.iter().copied())
                    .filter_map(|at| deferred_boundary(&source, at).map(|d| (at, d)))
                    .collect();
                source
            })
            .collect();
        let mut guard = Guard {
            files,
            parents,
            wrappers: wrappers.to_vec(),
            keying: keying.to_vec(),
            delegated: delegated.to_vec(),
            releasers: Vec::new(),
            used: Default::default(),
        };
        // A use of a binding holding boundary SQL in a closure's or an
        // `async` block's body runs where that does, as boundary SQL there.
        for f in 0..guard.files.len() {
            let file = &guard.files[f];
            let held: Vec<(usize, Deferred)> = guard
                .held_uses(f, 0, file.text.len())
                .into_iter()
                .filter_map(|u| deferred_boundary(file, u).map(|d| (u, d)))
                .collect();
            guard.files[f].deferred.extend(held);
        }
        guard.review_releasers(&RELEASERS);
        guard
    }

    /// The guard with these reviewed releasers too (a fixture's).
    fn with_releasers(mut self, entries: &[Entry]) -> Self {
        self.review_releasers(entries);
        self
    }

    /// Adds reviewed releasers: each call of one, where it reaches, is a
    /// transaction boundary, followed out of closures like the others
    /// ([`Source::deferred`]).
    fn review_releasers(&mut self, entries: &[Entry]) {
        let added = self.releasers.len();
        self.releasers.extend_from_slice(entries);
        for f in 0..self.files.len() {
            let file = &self.files[f];
            let text = &file.text;
            let calls_there: Vec<(usize, Deferred)> = self.releasers[added..]
                .iter()
                .filter(|e| self.reaches(e, f))
                .flat_map(|e| calls(text, e.name, 0, text.len(), e.form))
                .filter_map(|u| deferred_boundary(file, u).map(|d| (u, d)))
                .collect();
            self.files[f].deferred.extend(calls_there);
        }
    }

    /// Where file `f` may end a transaction: a `commit` or `rollback` call,
    /// closing boundary SQL run there (no holder; `BEGIN`/`START` open one),
    /// a use of a binding holding closing SQL run there, or a call of a
    /// reviewed releaser.
    fn release_points(&self, f: usize) -> Vec<usize> {
        let file = &self.files[f];
        let text = &file.text;
        // sqlx's `commit`/`rollback` take no argument (or one by path,
        // `Transaction::commit(tx)`); a call with arguments is of a function of
        // these sources, judged where it is defined.
        let mut found: Vec<usize> = ["commit", "rollback"]
            .iter()
            .flat_map(|b| {
                calls(text, b, 0, text.len(), Form::Method).into_iter().filter(move |&c| {
                    let open = open_paren(text, c + b.len());
                    text[..c].trim_end().ends_with("::")
                        || text[open + 1..].trim_start().starts_with(')')
                })
            })
            .collect();
        // Closing SQL that runs (unrun, it fails anyway), reviewed inert text
        // aside.
        found.extend(file.bounds.iter().copied().filter(|&b| {
            holder(text, b).is_none()
                && !opener_sql(file, b)
                && runs_here(text, b)
                && inert_entry(file, b, None).is_none()
        }));
        found.extend(
            self.held_uses_where(f, 0, text.len(), |other, at| !opener_sql(other, at))
                .into_iter()
                .filter(|&u| {
                    let name: String = text[u..].chars().take_while(|c| is_ident(*c)).collect();
                    runs_here(text, u) && inert_entry(file, u, Some(&name)).is_none()
                }),
        );
        found.extend(self.releasing_calls(f, 0, text.len()));
        found
    }

    /// Whether the scope whose body opens at `open` in file `f` provably
    /// opened a transaction of its own before `at` ([`opens`]), so a boundary
    /// there ends only that one: an sqlx opener method (`pool.begin()`,
    /// `conn.begin()`: on a connection already in a transaction, a savepoint,
    /// whose end keeps the outer key), a reviewed owning function's call
    /// ([`OWNING_FNS`]), or a raw opener (an [`OPENERS`] or [`OPENER_FNS`]
    /// call, opening SQL) on a connection this function obtained itself
    /// ([`fresh`]). A raw `BEGIN` on a connection it was handed proves
    /// nothing: nested in the caller's transaction, it leaves that one open,
    /// and the `COMMIT` after it ends it.
    fn owned_before(&self, f: usize, open: usize, at: usize, points: &[usize], raw: bool) -> bool {
        let file = &self.files[f];
        let text = &file.text;
        let (fn_start, body) = file
            .enclosing(at)
            .and_then(|(_, start, ..)| item_open(text, *start).map(|body| (*start, body)))
            .unwrap_or((0, 0));
        let fresh_arg = |c: usize, name: &str| {
            plain_name(first_arg(text, open_paren(text, c + name.len())))
                .is_some_and(|conn| fresh(text, body, c, conn))
        };
        // A raw closing statement (`COMMIT` SQL, `raw`) ends the outer
        // transaction whatever savepoint is open: an sqlx opener method proves
        // it owns that one only on a receiver that cannot be in a transaction
        // already, a connection obtained here ([`fresh`]) or a pool parameter
        // ([`pool_param`]).
        let methods = OPENERS.iter().flat_map(move |b| {
            calls(text, b, open, at, Form::Method).into_iter().filter(move |&c| {
                text[..c].trim_end().ends_with('.')
                    && (!raw
                        || receiver(text, c).is_some_and(|name| {
                            fresh(text, body, c, name) || pool_param(text, fn_start, body, name)
                        }))
            })
        });
        let raw_calls = OPENERS
            .iter()
            .map(|b| (*b, Form::Free))
            .chain(OPENER_FNS.iter().map(|e| (e.name, e.form)))
            .flat_map(|(name, form)| {
                calls(text, name, open, at, form)
                    .into_iter()
                    .filter(move |&c| fresh_arg(c, name))
            });
        let owning = OWNING_FNS
            .iter()
            .filter(|e| self.reaches(e, f))
            .flat_map(|e| calls(text, e.name, open, at, e.form));
        let sql = file.bounds.iter().copied().filter(|&b| {
            (open..at).contains(&b)
                && opener_sql(file, b)
                && sql_connection(text, b).is_some_and(|conn| fresh(text, body, b, conn))
        });
        // An opener closed again before `at` owns nothing there (`let sp =
        // c.begin().await?; sp.commit().await?;`, then `COMMIT` on `c`): no
        // point closing a transaction ([`Guard::closing_points`]) may stand
        // between it and `at`, unless in a branch that leaves the function
        // first (`if refused { tx.rollback().await?; return … }`, [`leaves`])
        // or in one exclusive with `at` (the other side of an `if … else`,
        // [`else_holds`], another `match` arm, [`other_arm`]).
        let start = file.enclosing(at).map_or(0, |(_, s, ..)| *s);
        let outstanding = |p: usize| {
            !points.iter().any(|&r| {
                p < r
                    && r < at
                    && !text[p..r].match_indices('{').any(|(o, _)| {
                        let (o, close) = (p + o, closing(text, p + o));
                        r < close
                            && (close <= at
                                && leaves(text, o, r, close)
                                && returns_to(text, start, o, at)
                                || close <= at && branch(text, o) && else_holds(text, close, at)
                                || other_arm(text, o, r, at))
                    })
            })
        };
        methods
            .chain(raw_calls)
            .chain(owning)
            .chain(sql)
            .any(|p| opens(file, open, p) && outstanding(p))
    }

    /// [`Guard::release_points`] of file `f` that may close a transaction: a
    /// rollback to a savepoint (`ROLLBACK TO s`, literal or held) leaves the
    /// transaction open.
    fn closing_points(&self, f: usize) -> Vec<usize> {
        let file = &self.files[f];
        let text = &file.text;
        let savepoints: BTreeSet<usize> = file
            .bounds
            .iter()
            .copied()
            .filter(|&b| rolls_back_to(file, b))
            .chain(self.held_uses_where(f, 0, text.len(), rolls_back_to))
            .collect();
        self.release_points(f).into_iter().filter(|r| !savepoints.contains(r)).collect()
    }

    /// The raw closing statements of file `f`: closing boundary SQL (no
    /// holder, no rollback to a savepoint) or a use of a binding holding it,
    /// which end the outer transaction whatever savepoint is open.
    fn raw_closing(&self, f: usize) -> BTreeSet<usize> {
        let file = &self.files[f];
        let text = &file.text;
        let closes = |file: &Source, b: usize| !opener_sql(file, b) && !rolls_back_to(file, b);
        file.bounds
            .iter()
            .copied()
            .filter(|&b| holder(text, b).is_none() && closes(file, b))
            .chain(self.held_uses_where(f, 0, text.len(), closes))
            .collect()
    }

    /// Calls in `text[from..to]` of file `f` of each reviewed releaser that
    /// reaches it, in its form.
    fn releasing_calls(&self, f: usize, from: usize, to: usize) -> Vec<usize> {
        let text = &self.files[f].text;
        self.releasers
            .iter()
            .filter(|e| self.reaches(e, f))
            .flat_map(|e| calls(text, e.name, from, to, e.form))
            .collect()
    }

    /// The body of a table entry's function, without its signature.
    fn body(&self, e: &Entry) -> Option<&str> {
        let file = self.files.iter().find(|f| f.rel == e.file)?;
        let (_, start, end, _) = file.functions.iter().find(|(name, ..)| name == e.name)?;
        let open = start + file.text[*start..*end].find('{')?;
        Some(&file.text[open..*end])
    }

    /// Whether a call of `e` in file `f` reaches it: anywhere, or in its
    /// file and its module's descendants.
    fn reaches(&self, e: &Entry, f: usize) -> bool {
        if let Reach::In(file) = e.reach {
            return self.files[f].rel == file;
        }
        let mut at = Some(f);
        while let Some(g) = at {
            if e.reach == Reach::Anywhere || self.files[g].rel == e.file {
                return true;
            }
            at = self.parents[g];
        }
        false
    }

    /// Whether file `f` is file `g` or in its module's descendants, where an
    /// item `g` declares without a visibility is visible (`super::NAME`).
    fn descends(&self, f: usize, g: usize) -> bool {
        let mut at = Some(f);
        while let Some(h) = at {
            if h == g {
                return true;
            }
            at = self.parents[h];
        }
        false
    }

    /// Whether a call (at `w`) of one of `entries` in `text[from..to]` of file
    /// `f` satisfies `holds(e, w)`; each such entry is recorded as used.
    fn called(
        &self,
        entries: &[Entry],
        f: usize,
        from: usize,
        to: usize,
        holds: impl Fn(&Entry, usize) -> bool,
    ) -> bool {
        let file = &self.files[f];
        let mut any = false;
        for e in entries {
            if self.reaches(e, f)
                && calls(&file.text, e.name, from, to, e.form).into_iter().any(|w| holds(e, w))
            {
                self.used.borrow_mut().insert((e.file, e.name));
                any = true;
            }
        }
        any
    }

    /// Every transaction boundary in `text[from..to]` of file `f`, at its
    /// source offset: a call of [`BOUNDARY_CALLS`], or boundary SQL
    /// ([`sql_boundaries`]) where it stands or where a binding holding it is
    /// used, a `const` or `static` in another file included.
    fn boundaries(&self, f: usize, from: usize, to: usize) -> Vec<usize> {
        let file = &self.files[f];
        let text = &file.text;
        let mut found: Vec<usize> = BOUNDARY_CALLS
            .iter()
            .flat_map(|b| calls(text, b, from, to, Form::Method))
            .collect();
        // Boundary SQL (a literal statement, an included file) is a boundary
        // where it stands, and at each use of the `const`, `static` or `let`
        // holding it.
        for &at in &file.bounds {
            if (from..to).contains(&at) {
                found.push(at);
            }
        }
        found.extend(self.held_uses(f, from, to));
        found.extend(self.releasing_calls(f, from, to));
        // A boundary call or boundary SQL in a closure or `async` block a
        // `let` binds (or a use there of a binding holding it) is one where
        // that is called or awaited, too.
        for (at, run) in &file.deferred {
            if let Deferred::Bound(name) = run {
                found.extend(uses(text, name, from, to).into_iter().filter(|u| u > at));
            }
        }
        found
    }

    /// Each use in `text[from..to]` of file `f` of a binding holding
    /// boundary SQL ([`holder`]): a `const`, `static` or `let` of this file
    /// (its declaration aside), or a `const` or `static` of another file Rust
    /// can see here.
    fn held_uses(&self, f: usize, from: usize, to: usize) -> Vec<usize> {
        self.held_uses_where(f, from, to, |_, _| true)
    }

    /// [`Guard::held_uses`] of the bindings whose boundary SQL (at its offset
    /// in its file) satisfies `keep`.
    fn held_uses_where(
        &self,
        f: usize,
        from: usize,
        to: usize,
        keep: impl Fn(&Source, usize) -> bool,
    ) -> Vec<usize> {
        let file = &self.files[f];
        let text = &file.text;
        let mut found = Vec::new();
        for &at in file.bounds.iter().filter(|&&at| keep(file, at)) {
            if let Some(held) = holder(text, at) {
                found.extend(
                    uses(text, &held.name, from, to)
                        .into_iter()
                        .filter(|&u| !imported(text, u) && !(held.start..at).contains(&u)),
                );
            }
        }
        // A `const` or `static` holding boundary SQL in another file is one
        // at each use here, by name, where Rust can see it: from its module's
        // descendants, or anywhere when it has a visibility
        // (`crate::append::DONE`, `super::DONE`).
        for (g, other) in self.files.iter().enumerate() {
            if g == f {
                continue;
            }
            for &at in other.bounds.iter().filter(|&&at| keep(other, at)) {
                let Some(held) = holder(&other.text, at).filter(|h| h.item) else {
                    continue;
                };
                if held.public || self.descends(f, g) {
                    found.extend(
                        uses(text, &held.name, from, to)
                            .into_iter()
                            .filter(|&u| !imported(text, u)),
                    );
                }
            }
        }
        found
    }

    /// Where the transaction that runs the statement at `at` (in the
    /// function starting at `start`) begins: the function's last transaction
    /// boundary (`begin(`, `commit(`, `rollback(`, boundary SQL) before `at`,
    /// or its start when it opens none (its caller's transaction). A boundary
    /// in an `if`/`else`/`match`-arm block closed before `at` that then
    /// leaves the function unconditionally (`if refused {
    /// tx.rollback().await?; return … }`, [`leaves`]) does not count; any
    /// other does.
    fn transaction_start(&self, f: usize, start: usize, at: usize) -> usize {
        let text = &self.files[f].text;
        let live = |q: usize| {
            !text[start..q].match_indices('{').any(|(o, _)| {
                let (open, close) = (start + o, closing(text, start + o));
                q < close
                    && close <= at
                    && leaves(text, open, q, close)
                    && returns_to(text, start, open, at)
            })
        };
        self.boundaries(f, start, at)
            .into_iter()
            .filter(|&q| live(q))
            .max()
            .unwrap_or(start)
    }

    /// The key taken earlier in the transaction that runs the statement at
    /// `at`, in the function starting at `start`: after
    /// [`Guard::transaction_start`].
    fn keyed_before(&self, f: usize, start: usize, at: usize) -> bool {
        let text = &self.files[f].text;
        let from = self.transaction_start(f, start, at);
        // Evidence in a block closed before `at` (a scoped transaction, a
        // conditional key) does not reach the statement, nor does evidence in
        // another arm of a `match` around both.
        let reaches_at = |e: usize| {
            !text[start..e].match_indices('{').any(|(o, _)| {
                let close = closing(text, start + o);
                e < close && close <= at
            }) && !text[start..e]
                .match_indices('{')
                .any(|(o, _)| other_arm(text, start + o, e, at))
        };
        // Only evidence that runs: the function's body opens at its first `{`.
        let body = start + text[start..].find('{').unwrap_or(0);
        MARKERS
            .iter()
            .flat_map(|m| seq(text, m, from, at).into_iter().map(move |p| (m, p)))
            .any(|(m, p)| {
                reaches_at(p)
                    && credited(text, m, p, body)
                        .and_then(|end| {
                            if proof(m) {
                                Some(end)
                            } else {
                                established(text, end, None)
                            }
                        })
                        .is_some_and(|end| end <= at)
            })
            || self.called(&self.keying, f, from, at, |_, w| {
                reaches_at(w)
                    && awaited(text, w)
                        .and_then(|end| established(text, end, None))
                        .is_some_and(|end| end <= at)
                    && !conditional(text, w)
            })
    }

    /// Whether the wrapper call of `e` at `w` takes the statement at `at` (in
    /// a statement starting at `start`): as an argument (`at` within the
    /// call, nothing executed among the call's arguments), or, for a method,
    /// as its receiver (`at` before the call, no bracket opened from the
    /// statement's start, around the site or after it in a later argument,
    /// still open at the call, and no raw execution in the statement before
    /// the call).
    fn hands(&self, f: usize, e: &Entry, w: usize, start: usize, at: usize) -> bool {
        let text = &self.files[f].text;
        let open = open_paren(text, w + e.name.len());
        let args = closing(text, open);
        if w <= at && at < args {
            // Handed, unless something runs while the arguments are evaluated
            // (a method on the site's chain, or `Executor::execute(…)` around
            // it).
            return executes(text, open + 1, args).is_empty();
        }
        e.form != Form::Free
            && at < w
            && text[..w].trim_end().ends_with('.')
            && !text[start..w]
                .match_indices(['(', '[', '{'])
                .any(|(o, _)| closing(text, start + o) > w)
            && executes(text, start, w).is_empty()
    }

    /// Input writes, `accounting_state` row locks, included SQL files writing
    /// an input, and event appends, in file `f`: `(offset, what)`.
    fn sites(&self, f: usize) -> Vec<(usize, String)> {
        let file = &self.files[f];
        let lower = &file.lower;
        let mut sites = Vec::new();
        // Writes, as PostgreSQL's grammar names their target: `INSERT INTO`,
        // `UPDATE`, `DELETE FROM`, `MERGE INTO`, each target optionally
        // `ONLY` or `ONLY (…)`; `TRUNCATE [TABLE]` and each name of its list;
        // `COPY` (any direction, conservatively).
        for verb in ["insert", "update", "delete", "merge", "truncate", "copy"] {
            for at in uses(lower, verb, 0, lower.len()) {
                let (mut next, mut end) = sql_name(lower, at + verb.len());
                match verb {
                    "insert" | "merge" | "delete" => {
                        if next != if verb == "delete" { "from" } else { "into" } {
                            continue;
                        }
                        (next, end) = sql_name(lower, end);
                    },
                    "truncate" if next == "table" => (next, end) = sql_name(lower, end),
                    // `COPY [BINARY] table`: the legacy modifier precedes it.
                    "copy" if next == "binary" => (next, end) = sql_name(lower, end),
                    _ => {},
                }
                loop {
                    if next == "only" {
                        let paren = end + lower[end..].len() - lower[end..].trim_start().len();
                        let from = if lower[paren..].starts_with('(') {
                            paren + 1
                        } else {
                            end
                        };
                        (next, end) = sql_name(lower, from);
                    }
                    // The table is the name's last part (`public.x`, `db.public.x`).
                    let table = next.rsplit('.').next().unwrap_or_default();
                    if INPUTS.contains(&table) {
                        sites.push((file.sql_at[at], format!("{} {table}", verb.to_uppercase())));
                    }
                    // Past a `*` or the `)` of `ONLY (…)`, a `,` continues a
                    // `TRUNCATE` list.
                    let mut comma = end;
                    loop {
                        comma += lower[comma..].len() - lower[comma..].trim_start().len();
                        if !lower[comma..].starts_with(['*', ')']) {
                            break;
                        }
                        comma += 1;
                    }
                    if verb != "truncate" || !lower[comma..].starts_with(',') {
                        break;
                    }
                    (next, end) = sql_name(lower, comma + 1);
                }
            }
        }
        // Row locks of every strength (`FOR UPDATE`, `FOR NO KEY UPDATE`,
        // `FOR SHARE`, `FOR KEY SHARE`), read as words across any whitespace.
        for at in uses(lower, "for", 0, lower.len()) {
            let (first, end) = sql_name(lower, at + "for".len());
            let (second, end) = sql_name(lower, end);
            let third = sql_name(lower, end).0;
            let locks = matches!(first.as_str(), "update" | "share")
                || (first == "no" && second == "key" && third == "update")
                || (first == "key" && second == "share");
            if !locks {
                continue;
            }
            // Its statement: from the start of its (joined) literal, or the
            // literal's last statement separator before the lock.
            let literal = file.starts.iter().rev().find(|&&s| s <= at).copied().unwrap_or(0);
            let start = file
                .semicolons
                .iter()
                .rev()
                .find(|&&s| literal <= s && s < at)
                .map_or(literal, |s| s + 1);
            if names(lower, start, at, "accounting_state") {
                sites.push((file.sql_at[at], "accounting_state row lock".into()));
            }
        }
        if let Some(dir) = &file.dir {
            for (at, path) in includes(file) {
                let Ok(sql) = std::fs::read_to_string(dir.join(&path)) else {
                    continue;
                };
                if sql_writes(&sql) {
                    sites.push((at, format!("include_str!({path}) writes an input")));
                }
            }
        }
        // SQL the guard cannot read, a file sqlx reads at compile time: each
        // such call is a site, to follow the key like any write.
        for name in QUERY_FILE_MACROS {
            for at in uses(&file.text, name, 0, file.text.len()) {
                let rest = file.text[at + name.len()..].trim_start();
                if rest.starts_with('!') {
                    sites.push((at, format!("{name}!() SQL not read")));
                }
            }
        }
        for append in ["append_event", "append_event_tx"] {
            for at in calls(&file.text, append, 0, file.text.len(), Form::Free) {
                sites.push((at, format!("{append}()")));
            }
        }
        sites
    }

    /// Where the site at `at` is not covered: itself, or each uncovered use
    /// of the `const` or `let` holding its SQL, or each uncovered call of the
    /// delegated writer it is in.
    fn open(
        &self,
        f: usize,
        at: usize,
        what: &str,
        seen: &mut BTreeSet<(usize, usize)>,
    ) -> Vec<String> {
        let file = &self.files[f];
        let text = &file.text;
        let here = || format!("{}:{}: {what}", file.rel, file.line(at));
        let function = file.enclosing(at);
        // A `const` item holding the SQL: each use where Rust can see it, in
        // its file and its module's descendants or, with any visibility, in
        // every file; imports are not uses.
        if let Some(c) = text[..at].rfind("const ").filter(|c| {
            function.is_none_or(|(_, start, ..)| c > start) && !text[*c..at].contains(';')
        }) {
            if !seen.insert((f, c)) {
                return Vec::new();
            }
            let name: String = text[c + 6..].chars().take_while(|ch| is_ident(*ch)).collect();
            let end = text[at..].find(';').map_or(text.len(), |e| at + e);
            let public = without_visibility(&text[..c]).len() < text[..c].trim_end().len();
            let (mut open, mut used) = (Vec::new(), false);
            for (g, other) in self.files.iter().enumerate() {
                if !public && !self.descends(g, f) {
                    continue;
                }
                for u in uses(&other.text, &name, 0, other.text.len()) {
                    if (g == f && (c..=end).contains(&u)) || imported(&other.text, u) {
                        continue;
                    }
                    used = true;
                    open.extend(self.open(g, u, &format!("{what} via {name}"), seen));
                }
            }
            if !used {
                open.push(format!("{} (const {name} never used)", here()));
            }
            return open;
        }
        let Some((name, fn_start, fn_end, _)) = function.cloned() else {
            return vec![format!("{} (outside a function)", here())];
        };
        // No transaction boundary between the SQL (its literal, or the name
        // or call at `at`) and the end of its statement by matched
        // delimiters ([`matched_statement_end`]): one in a later argument
        // (`.bind({ tx.commit().await?; 0 })`) would release the key before
        // the statement runs.
        let built = if text[at..].starts_with(is_ident) {
            at + text[at..].find(|c: char| !is_ident(c)).unwrap_or(text.len() - at)
        } else {
            text[at..].find('"').map_or(at, |q| at + q + 1)
        };
        if !self.boundaries(f, built, matched_statement_end(text, at)).is_empty() {
            return vec![format!(
                "{} (in {name}(), a transaction boundary between it and its execution)",
                here()
            )];
        }
        let (mut start, mut end) = statement(text, at);
        // A macro's braces (`std::concat! { … }`) open no statement.
        while start > 0
            && text.as_bytes()[start - 1] == b'{'
            && text[..start - 1].trim_end().ends_with('!')
        {
            start = statement(text, start - 1).0;
        }
        // `let`, then `mut` if it follows, read as tokens (`let mut` and a
        // line break bind the name after them, never `mut`).
        let binding_at = |s: usize| {
            let head = text[s..at].trim_start();
            word(head, "let")
                .then(|| head[3..].trim_start())
                .map(|b| {
                    let b = if word(b, "mut") {
                        b[3..].trim_start()
                    } else {
                        b
                    };
                    let b = b.strip_prefix("r#").unwrap_or(b);
                    b.chars().take_while(|c| is_ident(*c)).collect::<String>()
                })
                .filter(|b| !b.is_empty() && b != "_")
        };
        let mut binding = binding_at(start);
        // A value that leaves its own statement (the tail of a block) is held
        // by the `let` around it; SQL in an `async` block or closure runs
        // where that is awaited or called ([`held_by`]).
        let body = fn_start + text[fn_start..].find('{').unwrap_or(0);
        let (mut held, mut scope) = held_by(text, at, body);
        let mut lazy = None;
        // A braceless closure's body holding the SQL (`async || sqlx::query(…)
        // .execute(pool).await`) runs when the closure is called, as a
        // closure's block does ([`braceless_closure`]); the innermost scope
        // counts.
        if let Some((cbody, bound)) = braceless_closure(text, start, at) {
            if scope.is_none_or(|(o, _)| o < cbody) {
                (held, scope) = (start, Some((cbody, bound)));
                if bound && binding.is_some() {
                    lazy = Some(cbody);
                }
            }
        }
        if binding.is_none() {
            if let Some(b) = (held != start).then(|| binding_at(held)).flatten() {
                (binding, start, end, lazy) =
                    (Some(b), held, matched_statement_end(text, held), scope.map(|(o, _)| o));
            }
        }
        let handed =
            self.called(&self.wrappers, f, start, end, |e, w| self.hands(f, e, w, start, at));
        // SQL that no `let` holds, neither run in its own statement nor
        // handed to a wrapper there, leaves it as a value or a future
        // (`tokio::spawn(sqlx::query(…).execute(pool))`): where it runs is not
        // known, so it is open. A use that only borrows a binding
        // (`black_box(&q)`) leaves it where it is.
        let borrowed = text[..at].trim_end().ends_with('&')
            || word_before(text, at, "mut") && {
                let b = text[..at].trim_end();
                b[..b.len() - 3].trim_end().ends_with('&')
            };
        if binding.is_none() && !handed && !borrowed && !runs_in_statement(text, start, at, end) {
            return vec![format!(
                "{} (in {name}(), not run where it is built)",
                here()
            )];
        }
        // SQL bound by `let` and not run in that statement (nor handed to a
        // wrapper there, [`runs_in_statement`]; SQL in a bound `async` block
        // or closure never runs there): each later use, up to a shadowing
        // `let` of the same name; and, when anything is awaited in the
        // statement (a call it is handed to may run it now), the key here as
        // well. An `async` block or closure that takes the key itself before
        // the SQL is keyed wherever it runs.
        let mut via = Vec::new();
        if let Some(binding) = binding {
            if lazy.is_some_and(|open| self.keyed_before(f, open, at)) {
                return via;
            }
            if !handed && (lazy.is_some() || !runs_in_statement(text, start, at, end)) {
                let later = later_uses(text, &binding, end, fn_end);
                if later.is_empty() {
                    return vec![format!("{} (let {binding} never used)", here())];
                }
                via = later
                    .into_iter()
                    .flat_map(|u| self.open(f, u, &format!("{what} via {binding}"), seen))
                    .collect();
                if lazy.is_some() || seq(text, ".await", start, end).is_empty() {
                    return via;
                }
            }
        }
        // Keyed where the SQL runs: inside the `async` block or closure
        // holding it, if any (a key outside may be released before it runs).
        if handed || self.keyed_before(f, scope.map_or(fn_start, |(o, _)| o), at) {
            return via;
        }
        // Else such a block runs where the `let` binding it is awaited or
        // called (each use keyed there); one passed on unbound is open.
        if let Some((_, bound)) = scope {
            if lazy.is_none() {
                let Some(holder) = bound.then(|| binding_at(held)).flatten() else {
                    via.push(format!(
                        "{} (in {name}(), in an async block or closure that takes no key before it)",
                        here()
                    ));
                    return via;
                };
                let later = later_uses(text, &holder, matched_statement_end(text, held), fn_end);
                if later.is_empty() {
                    via.push(format!("{} (let {holder} never used)", here()));
                    return via;
                }
                via.extend(
                    later
                        .into_iter()
                        .flat_map(|u| self.open(f, u, &format!("{what} via {holder}"), seen)),
                );
            }
            return via;
        }
        // A writer on its caller's transaction: each call must be covered.
        let Some(e) = self.delegated.iter().find(|e| e.file == file.rel && e.name == name) else {
            via.push(format!("{} (in {name}(), no key before it)", here()));
            return via;
        };
        if !seen.insert((f, fn_start)) {
            return via;
        }
        self.used.borrow_mut().insert((e.file, e.name));
        let (mut open, mut called) = (via, false);
        for (g, other) in self.files.iter().enumerate() {
            if !self.reaches(e, g) {
                continue;
            }
            for u in calls(&other.text, &name, 0, other.text.len(), e.form) {
                if g == f && (fn_start..fn_end).contains(&u) {
                    continue;
                }
                called = true;
                open.extend(self.open(g, u, &format!("{what} in {name}()"), seen));
            }
        }
        if !called {
            open.push(format!("{} (no caller of the delegated {name}())", here()));
        }
        open
    }

    /// A table entry that does not do what the table trusts it for: a
    /// missing function, a wrapper without `BEGIN`, the key and `COMMIT` in
    /// order (nor a call of another wrapper), a keying function without a
    /// [`MARKERS`] entry, or any entry that covered no site.
    fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        // A transaction boundary (a call or boundary SQL) in a closure's or an
        // `async` block's body runs where that is called or awaited: followed
        // to each use of the `let` binding it ([`Guard::boundaries`]), or run
        // where it stands when awaited there. Passed on, spawned or returned,
        // it fails, unless it is itself an opener (a `begin`, `begin_with` or
        // `try_begin` call, `BEGIN`/`START` SQL: it ends nothing outside), or
        // the scope opened its own transaction first and provably: an opener
        // at the scope's own level (in no block opened inside it), not
        // conditional ([`conditional`]) and run there (a call awaited along
        // its chain, SQL [`runs_here`]): the ledger's `bounded(async { … begin
        // … commit })`. Then it ends only that one.
        for (f, file) in self.files.iter().enumerate() {
            let text = &file.text;
            let closing = self.closing_points(f);
            let raw_closing = self.raw_closing(f);
            let opener_call = |p: usize| OPENERS.iter().any(|b| word(&text[p..], b));
            for (at, run) in &file.deferred {
                let Deferred::Escapes(open) = run else {
                    continue;
                };
                let raw = raw_closing.contains(at);
                let first = self.owned_before(f, *open, *at, &closing, raw);
                if !(opener_call(*at) || opener_sql(file, *at) || first) {
                    problems.push(format!(
                        "{}:{}: transaction boundary in a closure or async block passed on, run where that runs",
                        file.rel,
                        file.line(*at)
                    ));
                }
            }
        }
        // A `let`-bound scope's boundaries are followed to each use of its
        // name, which must run it there: a call (`release(…)`), an `.await`, or
        // handed to an awaited call that runs the future it is given
        // ([`RUNS_FUTURE`]: `timeout_at(deadline, work).await`), outside any
        // other closure or `async` block. Any other use (an alias, `let finish
        // = release;`, `std::future::ready(release).await`, a call inside
        // `async |c| release(c).await`, a push, a return) carries it where the
        // guard does not follow: it fails.
        for file in &self.files {
            let text = &file.text;
            let mut reported = BTreeSet::new();
            for (at, run) in &file.deferred {
                let Deferred::Bound(name) = run else {
                    continue;
                };
                let end = file.enclosing(*at).map_or(text.len(), |(_, _, end, _)| *end);
                for u in uses(text, name, *at, end) {
                    let after = text[u + name.len()..].trim_start();
                    let called = after.starts_with('(')
                        || after
                            .strip_prefix('.')
                            .map(str::trim_start)
                            .is_some_and(|r| word(r, "await"));
                    let handed = unclosed(text, u).is_some_and(|(o, b)| {
                        let head = text[..o].trim_end();
                        b == b'('
                            && RUNS_FUTURE.iter().any(|name| {
                                head.strip_suffix(name)
                                    .is_some_and(|before| !before.ends_with(|c: char| is_ident(c)))
                            })
                            && awaited_chain(text, closing(text, o))
                    });
                    // A use inside another closure or `async` block runs when
                    // that does, which this does not follow.
                    let nested = deferred_boundary(file, u).is_some();
                    if (nested || !called && !handed) && reported.insert(u) {
                        problems.push(format!(
                            "{}:{}: transaction boundary in {name}, used where it does not run",
                            file.rel,
                            file.line(u)
                        ));
                    }
                }
            }
        }
        // Boundary SQL a binding holds where no holder follows its uses
        // (`const DONE: &str = { "COMMIT" };`, `let q = sqlx::query("COMMIT");`,
        // `let done = std::future::ready("COMMIT").await;`) runs wherever its
        // value goes, unless it is proven to run there ([`runs_here`]): it
        // fails.
        for file in &self.files {
            for &at in &file.bounds {
                let text = &file.text;
                if holder(text, at).is_some() {
                    continue;
                }
                // In a closure's or an `async` block's body it runs where
                // that does (followed below, and in [`Guard::boundaries`]).
                if file.deferred.iter().any(|(p, _)| *p == at) {
                    continue;
                }
                if in_binding(text, at).is_none() {
                    continue;
                }
                if !runs_here(text, at) {
                    problems.push(format!(
                        "{}:{}: boundary SQL held where its uses are not followed",
                        file.rel,
                        file.line(at)
                    ));
                }
            }
        }
        // Boundary SQL in a closure's or an `async` block's body (or a use
        // there of a binding holding it) is followed to where that runs only
        // when it runs inside ([`runs_here`]). A value the body returns or
        // keeps (`async { "COMMIT" }`, `|| sqlx::query("COMMIT")`, `async {
        // DONE }`) goes where the guard does not follow: it fails.
        for file in &self.files {
            let text = &file.text;
            let calls_at: BTreeSet<usize> = BOUNDARY_CALLS
                .iter()
                .flat_map(|b| calls(text, b, 0, text.len(), Form::Method))
                .collect();
            for (at, _) in &file.deferred {
                let held = file.bounds.contains(at) && holder(text, *at).is_some();
                if calls_at.contains(at) || held {
                    continue;
                }
                if !runs_here(text, *at) {
                    problems.push(format!(
                        "{}:{}: boundary SQL in a closure or async block, not run there",
                        file.rel,
                        file.line(*at)
                    ));
                }
            }
        }
        // Boundary SQL anywhere else must run where it stands, too
        // ([`runs_here`]): a value it leaves as (a function's,
        // `fn release_sql() -> &'static str { "COMMIT" }`; handed to an
        // unreviewed call; kept) goes where the guard does not follow, and
        // fails. So does a use of a binding holding it that does not run it
        // there (`fn done() -> &'static str { DONE }`, `let alias = DONE;`).
        // Reviewed inert text ([`INERT`]) aside, each entry still matched.
        let mut inert_used = BTreeSet::new();
        for (f, file) in self.files.iter().enumerate() {
            let text = &file.text;
            let inert = |p: usize, held: Option<&str>| inert_entry(file, p, held);
            let deferred = |p: usize| file.deferred.iter().any(|(d, _)| *d == p);
            for &at in &file.bounds {
                if holder(text, at).is_some()
                    || in_binding(text, at).is_some()
                    || deferred(at)
                    || runs_here(text, at)
                {
                    continue;
                }
                if let Some(i) = inert(at, None) {
                    inert_used.insert(i);
                    continue;
                }
                problems.push(format!(
                    "{}:{}: boundary SQL not run where it stands",
                    file.rel,
                    file.line(at)
                ));
            }
            let mut reported = BTreeSet::new();
            for u in self.held_uses(f, 0, text.len()) {
                if deferred(u) || runs_here(text, u) || !reported.insert(u) {
                    continue;
                }
                let name: String = text[u..].chars().take_while(|c| is_ident(*c)).collect();
                if let Some(i) = inert(u, Some(&name)) {
                    inert_used.insert(i);
                    continue;
                }
                problems.push(format!(
                    "{}:{}: boundary SQL via {name} not run where it is used",
                    file.rel,
                    file.line(u)
                ));
            }
        }
        // A function that ends a transaction it did not provably open (a
        // `commit` or `rollback` call, closing boundary SQL, a use of a
        // binding holding closing SQL, or a call of a reviewed releaser, with
        // no proven opener before it in its own scope or any around it in the
        // function, [`opened_before`]) ends its caller's, which the guard does
        // not follow through an arbitrary call: it fails, unless reviewed
        // ([`RELEASERS`]), whose calls are then boundaries. A reviewed entry
        // that ends no such transaction fails too.
        let mut releasers_used = BTreeSet::new();
        for (f, file) in self.files.iter().enumerate() {
            let text = &file.text;
            let points = self.release_points(f);
            let closing = self.closing_points(f);
            let raw_closing = self.raw_closing(f);
            for (name, start, end, _) in &file.functions {
                let Some(body) = item_open(text, *start) else {
                    continue;
                };
                let opened = |r: usize| {
                    let mut at = r;
                    while let Some((o, _)) = unclosed(text, at).filter(|&(o, _)| o >= body) {
                        if text.as_bytes()[o] == b'{'
                            && self.owned_before(f, o, r, &closing, raw_closing.contains(&r))
                        {
                            return true;
                        }
                        at = o;
                    }
                    false
                };
                if !points.iter().any(|&r| (body..*end).contains(&r) && !opened(r)) {
                    continue;
                }
                if let Some(i) =
                    self.releasers.iter().position(|e| e.file == file.rel && e.name == name)
                {
                    releasers_used.insert(i);
                    continue;
                }
                problems.push(format!(
                    "{}:{}: {name}() ends a transaction it did not open",
                    file.rel,
                    file.line(*start)
                ));
            }
        }
        for (i, e) in self.releasers.iter().enumerate() {
            if self.files.iter().any(|file| file.rel == e.file) && !releasers_used.contains(&i) {
                problems.push(format!(
                    "{}: releaser entry {}() ends no transaction it did not open",
                    e.file, e.name
                ));
            }
        }
        // A reviewed opener function opens a transaction at its own level
        // and ends none.
        for e in &OPENER_FNS {
            let Some(f) = self.files.iter().position(|file| file.rel == e.file) else {
                continue;
            };
            let file = &self.files[f];
            let points = self.release_points(f);
            let verified =
                file.functions.iter().filter(|(n, ..)| n == e.name).any(|(_, start, end, _)| {
                    item_open(&file.text, *start).is_some_and(|body| {
                        opened_before(file, body, *end)
                            && !points.iter().any(|r| (body..*end).contains(r))
                    })
                });
            if !verified {
                problems.push(format!(
                    "{}: opener entry {}() opens no transaction, or ends one",
                    e.file, e.name
                ));
            }
        }
        // A reviewed owning function owns the transaction it opens (a proven
        // owning opener at its own level) and ends none; a reviewed connector
        // connects or acquires a connection.
        for e in &OWNING_FNS {
            let Some(f) = self.files.iter().position(|file| file.rel == e.file) else {
                continue;
            };
            let file = &self.files[f];
            let points = self.release_points(f);
            let owns =
                file.functions.iter().filter(|(n, ..)| n == e.name).any(|(_, start, end, _)| {
                    item_open(&file.text, *start).is_some_and(|body| {
                        self.owned_before(f, body, *end, &self.closing_points(f), false)
                            && !points.iter().any(|r| (body..*end).contains(r))
                    })
                });
            if !owns {
                problems.push(format!(
                    "{}: owning entry {}() owns no transaction, or ends one",
                    e.file, e.name
                ));
            }
        }
        for e in &CONNECTORS {
            let Some(file) = self.files.iter().find(|file| file.rel == e.file) else {
                continue;
            };
            let text = &file.text;
            let connects =
                file.functions.iter().filter(|(n, ..)| n == e.name).any(|(_, start, end, _)| {
                    ["connect", "acquire"]
                        .iter()
                        .any(|c| !calls(text, c, *start, *end, Form::Method).is_empty())
                });
            if !connects {
                problems.push(format!("{}: connector entry {}() connects nothing", e.file, e.name));
            }
        }
        // Every reviewed entry (a wrapper, keying function, delegated writer,
        // releaser, opener, owning function or connector) is followed only
        // through its calls: used any other way (`let append =
        // append_event_tx;`, `map(Self::run)`) or imported under another name
        // (`use … append_event_tx as append;`) it goes where the guard does
        // not follow, and fails. A use in a form no call of it takes is
        // another item's (`.persist` of a free function, `write!`), and so is
        // one a local binding of that name shadows ([`shadowed`]).
        let mut reported = BTreeSet::new();
        let entries: Vec<(Entry, bool)> = self
            .wrappers
            .iter()
            .chain(&self.keying)
            .chain(&self.delegated)
            .chain(OPENER_FNS.iter())
            .chain(OWNING_FNS.iter())
            .chain(CONNECTORS.iter())
            .map(|e| (*e, false))
            .chain(self.releasers.iter().map(|e| (*e, true)))
            .collect();
        for (f, file) in self.files.iter().enumerate() {
            let text = &file.text;
            for (e, releaser) in entries.iter().filter(|(e, _)| self.reaches(e, f)) {
                let called = calls(text, e.name, 0, text.len(), e.form);
                for u in uses(text, e.name, 0, text.len()) {
                    let before = text[..u].trim_end();
                    let after = text[u + e.name.len()..].trim_start();
                    let definition =
                        before.strip_suffix("fn").is_some_and(|b| !b.ends_with(is_ident));
                    let path = before.ends_with('.') || before.ends_with("::");
                    let other = match e.form {
                        Form::Free => before.ends_with('.'),
                        Form::Method => false,
                        Form::Receiver => !path,
                    } || after.starts_with('!');
                    let renamed = imported(text, u) && word(after, "as");
                    if !renamed
                        && (called.contains(&u)
                            || definition
                            || other
                            || imported(text, u)
                            || shadowed(text, file, u, e.name))
                        || !reported.insert((f, u))
                    {
                        continue;
                    }
                    problems.push(if *releaser {
                        format!(
                            "{}:{}: {} ends a transaction it did not open, used where it does not run",
                            file.rel,
                            file.line(u),
                            e.name
                        )
                    } else {
                        format!(
                            "{}:{}: {} is a reviewed entry, used where it is not called",
                            file.rel,
                            file.line(u),
                            e.name
                        )
                    });
                }
            }
        }
        for (i, (rel, name, what)) in INERT.iter().enumerate() {
            if self.files.iter().any(|file| file.rel == *rel) && !inert_used.contains(&i) {
                problems
                    .push(format!("{rel}: inert entry {name}() `{what}` matches no boundary SQL"));
            }
        }
        for e in &self.wrappers {
            let Some(body) = self.body(e) else {
                continue;
            };
            let f = self.files.iter().position(|file| file.rel == e.file).unwrap();
            let text = &self.files[f].text;
            let (_, start, end, _) = self.files[f]
                .functions
                .iter()
                .find(|(name, ..)| name == e.name)
                .cloned()
                .unwrap();
            // BEGIN, then the statement, keyed exactly as a site would be
            // ([`Guard::keyed_before`]: in its transaction and scope, by a key
            // that runs and has finished), then COMMIT: every execution of
            // the wrapper (each executor call and `append_event_tx`) wherever
            // it stands, one before the BEGIN or on an early-return path
            // included, in a transaction the wrapper opened itself (its last
            // live boundary a `.begin()`, not a COMMIT or ROLLBACK it ran
            // after nor its caller's), with no boundary in its arguments or
            // the rest of its statement ([`matched_statement_end`]): one there
            // releases the key before it runs.
            let mut runs: Vec<usize> = executes(text, start, end)
                .into_iter()
                .chain(calls(text, "append_event_tx", start, end, Form::Free))
                .collect();
            runs.sort_unstable();
            let ordered = !runs.is_empty()
                && runs.iter().all(|&run| {
                    let from = self.transaction_start(f, start, run);
                    calls(text, "begin", from, run, Form::Method).first() == Some(&from)
                        && self.keyed_before(f, start, run)
                        && self.boundaries(f, run, matched_statement_end(text, run)).is_empty()
                        && !calls(text, "commit", run, end, Form::Method).is_empty()
                });
            // Or a method wrapper that hands itself (`self`, its SQL) to
            // another wrapper and executes nothing itself.
            let delegates = e.form == Form::Method
                && executes(body, 0, body.len()).is_empty()
                && self.wrappers.iter().any(|w| {
                    (w.file, w.name) != (e.file, e.name)
                        && self.reaches(w, f)
                        && calls(body, w.name, 0, body.len(), w.form).into_iter().any(|c| {
                            let open = open_paren(body, c + w.name.len());
                            let args = &body[open..closing(body, open)];
                            !uses(args, "self", 0, args.len()).is_empty()
                        })
                });
            if !ordered && !delegates {
                problems
                    .push(format!("{}: {}() is not BEGIN, key, statement, COMMIT", e.file, e.name));
            }
        }
        // A keying function takes the key on every successful path.
        for e in &self.keying {
            let Some(f) = self.files.iter().position(|file| file.rel == e.file) else {
                continue;
            };
            let Some((_, start, end, _)) =
                self.files[f].functions.iter().find(|(name, ..)| name == e.name).cloned()
            else {
                continue;
            };
            let open = start + self.files[f].text[start..end].find('{').unwrap_or(0);
            if !self.keys(f, open, e) {
                problems.push(format!("{}: {}() takes no key", e.file, e.name));
            }
        }
        // Checked last: an entry may be used only by another's verification.
        for e in self.wrappers.iter().chain(&self.keying).chain(&self.delegated) {
            if self.body(e).is_none() {
                problems.push(format!("{}: {}() not found", e.file, e.name));
            } else if !self.used.borrow().contains(&(e.file, e.name)) {
                problems.push(format!("{}: {}() covers no site (stale entry)", e.file, e.name));
            }
        }
        problems
    }

    /// Whether every successful path through the block whose `{` is at `open`
    /// (in file `f`, of keying entry `me`) takes the key: each `return` before
    /// the tail after a key at the block's level, and a tail `match` all of
    /// whose arms do, a tail `if … else …` all of whose branches do (no
    /// `else`, a path without it), a tail `Box::pin(async … { … })` or
    /// `async … { … }` whose block does, each the whole returned expression
    /// (nothing after its last block: `match … { … }.or(Ok(()))` is
    /// unproved); otherwise the block itself, by [`Guard::keys_here`].
    fn keys(&self, f: usize, open: usize, me: &Entry) -> bool {
        let text = &self.files[f].text;
        let close = closing(text, open) - 1;
        let tail = top_level(text, open + 1, close, ";").last().map_or(open + 1, |s| s + 1);
        let tail = tail + text[tail..close].len() - text[tail..close].trim_start().len();
        let rest = &text[tail..close];
        // Where the tail's own head ends: a `match`'s scrutinee, an `if`'s
        // condition, are evaluated first.
        let head = match (word(rest, "match"), word(rest, "if")) {
            (true, _) => top_level(text, tail, close, "{").first().copied().unwrap_or(close),
            (_, true) => top_level(text, tail + 2, close, "{").first().copied().unwrap_or(close),
            _ => tail,
        };
        // A successful `return` before the tail's arms or blocks is a path of
        // its own: the key must have been taken, and have finished, before
        // it, at this block's own level.
        if !uses(text, "return", open + 1, head)
            .into_iter()
            .all(|r| self.keys_here(f, open + 1, r, me))
        {
            return false;
        }
        // The tail construct must be the whole returned expression: anything
        // after its last block (`match … { … }.or(Ok(()))`) is unproved.
        let whole = |end: usize| text[end..close].trim().is_empty();
        if word(rest, "match") {
            let Some(arms) = top_level(text, tail, close, "{").first().copied() else {
                return false;
            };
            if !whole(closing(text, arms)) {
                return false;
            }
            let arms = match_arms(text, arms);
            // A `return` in an arm's pattern or guard is a path of its own.
            let guarded = arms.iter().all(|&(head, from, ..)| {
                uses(text, "return", head, from)
                    .into_iter()
                    .all(|r| self.keys_here(f, open + 1, r, me))
            });
            return guarded
                && !arms.is_empty()
                && arms.into_iter().all(|(_, from, to, block)| match block {
                    true => self.keys(f, from, me),
                    false => self.keys_here(f, from, to, me),
                });
        }
        if word(rest, "if") {
            let mut at = tail;
            loop {
                let Some(block) = top_level(text, at + 2, close, "{").first().copied() else {
                    return false;
                };
                if !self.keys(f, block, me) {
                    return false;
                }
                let after = closing(text, block);
                let next = after + text[after..close].len() - text[after..close].trim_start().len();
                if !word(&text[next..close], "else") {
                    return false;
                }
                let branch = next + 4 + text[next + 4..close].len()
                    - text[next + 4..close].trim_start().len();
                if word(&text[branch..close], "if") {
                    // An `else if` condition with a `return` is a path of its
                    // own too.
                    let block = top_level(text, branch + 2, close, "{").first().copied();
                    if !uses(text, "return", branch, block.unwrap_or(close)).is_empty() {
                        return false;
                    }
                    at = branch;
                    continue;
                }
                return text[branch..].starts_with('{')
                    && whole(closing(text, branch))
                    && self.keys(f, branch, me);
            }
        }
        let pinned = rest.strip_prefix("Box::pin(").map_or(rest, str::trim_start);
        if let Some(inner) = pinned.strip_prefix("async") {
            let inner = inner.trim_start();
            let inner = inner.strip_prefix("move").map_or(inner, str::trim_start);
            if inner.starts_with('{') {
                let block = close - inner.len();
                let after = text[closing(text, block)..close].trim();
                let suffix = if pinned.len() < rest.len() { ")" } else { "" };
                return after == suffix && self.keys(f, block, me);
            }
        }
        self.keys_here(f, open + 1, close, me)
    }

    /// Whether `text[from..to]` of file `f` takes the key at its own level:
    /// key evidence that runs ([`credited`], not [`conditional`]) or an
    /// awaited, unconditional call of another keying entry, inside no block
    /// nested in the range, finished by `to`, with no `return` before it has
    /// finished (one in the key call's own arguments leaves first) and no
    /// transaction boundary after it ([`Guard::boundaries`]).
    fn keys_here(&self, f: usize, from: usize, to: usize, me: &Entry) -> bool {
        let text = &self.files[f].text;
        let level =
            |p: usize| !text[from..p].match_indices('{').any(|(o, _)| closing(text, from + o) > p);
        // Held from its acquisition (`acquired`, the call's end) to the end
        // checked: established by `end`, with no `return` before that, and no
        // transaction boundary from the acquisition on (one inside an
        // adapter's argument, `.map({ COMMIT …; drop })`, ends it before its
        // success point).
        let held = |acquired: usize, end: usize| {
            end <= to
                && uses(text, "return", from, end).is_empty()
                && self.boundaries(f, acquired, to).is_empty()
        };
        MARKERS.iter().any(|m| {
            seq(text, m, from, to).into_iter().any(|p| {
                level(p)
                    && credited(text, m, p, from).is_some_and(|acquired| {
                        let end = if proof(m) {
                            Some(acquired)
                        } else {
                            established(text, acquired, Some(to))
                        };
                        end.is_some_and(|end| held(acquired, end))
                    })
            })
        }) || self.called(&self.keying, f, from, to, |k, w| {
            (k.file, k.name) != (me.file, me.name)
                && level(w)
                && !conditional(text, w)
                && awaited(text, w).is_some_and(|acquired| {
                    established(text, acquired, Some(to)).is_some_and(|end| held(acquired, end))
                })
        })
    }

    /// The number of sites, and every open site and table problem.
    fn report(&self) -> (usize, Vec<String>) {
        let mut sites = 0;
        let mut open = Vec::new();
        for f in 0..self.files.len() {
            for (at, what) in self.sites(f) {
                sites += 1;
                open.extend(self.open(f, at, &what, &mut BTreeSet::new()));
            }
        }
        open.sort();
        open.dedup();
        open.extend(self.problems());
        (sites, open)
    }

    /// Each `.sql` file under `roots` that writes an input (or locks
    /// `accounting_state`) and that no source includes by a literal path: a
    /// file reached only through a macro argument, `concat!` or nothing at all
    /// is tied to no call the guard can check, so it fails.
    fn orphan_sql(&self, roots: &[PathBuf]) -> Vec<String> {
        let canonical = |p: PathBuf| p.canonicalize().unwrap_or(p);
        let included: BTreeSet<PathBuf> = self
            .files
            .iter()
            .filter_map(|file| file.dir.as_ref().map(|dir| (file, dir)))
            .flat_map(|(file, dir)| includes(file).into_iter().map(|(_, p)| canonical(dir.join(p))))
            .collect();
        let mut orphans = Vec::new();
        let mut dirs = roots.to_vec();
        while let Some(dir) = dirs.pop() {
            for path in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()) {
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "sql")
                    && std::fs::read_to_string(&path).is_ok_and(|sql| sql_writes(&sql))
                    && !included.contains(&canonical(path.clone()))
                {
                    orphans.push(format!(
                        "{} writes an input; no literal include reads it",
                        path.display()
                    ));
                }
            }
        }
        orphans.sort();
        orphans
    }
}

/// Whether SQL read from a file starts or ends a transaction.
fn sql_bounds(sql: &str) -> bool {
    let hashes = "#".repeat(sql.matches('#').count() + 1);
    let literal = format!("r{hashes}\"{sql}\"{hashes}");
    !Guard::new(vec![("sql".into(), None, literal, None)], &[], &[], &[]).files[0]
        .bounds
        .is_empty()
}

/// Where `file`'s SQL starts or ends a transaction, at source offsets: a SQL
/// literal statement whose first word is `BEGIN`, `START TRANSACTION`,
/// `COMMIT`, `END`, `ABORT` or `ROLLBACK` (`ROLLBACK TO` a savepoint too: it
/// releases every lock taken since the savepoint, the key's among them, and
/// the guard does not read where that was); or the include of a SQL file
/// holding one.
fn sql_boundaries(file: &Source) -> Vec<usize> {
    let lower = &file.lower;
    let mut found = Vec::new();
    for w in ["begin", "start", "commit", "end", "abort", "rollback"] {
        for i in uses(lower, w, 0, lower.len()) {
            let Some(literal) = file.starts.iter().rev().find(|&&s| s <= i).copied() else {
                continue;
            };
            let before = lower[literal..i].trim_end();
            let next = sql_name(lower, i + w.len()).0;
            let statement = match w {
                "start" => next == "transaction",
                _ => true,
            };
            // `BEGIN`/`END` after a `;` inside a quote (a `DO $$ … $$` body's
            // `END IF;`) are PL/pgSQL block words: only a separator outside
            // every SQL quote starts a statement there.
            let separated = before.ends_with(';')
                && (!matches!(w, "begin" | "end")
                    || file.semicolons.contains(&(literal + before.len() - 1)));
            if statement && (before.is_empty() || separated) {
                found.push(file.sql_at[i]);
            }
        }
    }
    if let Some(dir) = &file.dir {
        for (at, path) in includes(file) {
            if std::fs::read_to_string(dir.join(path)).is_ok_and(|sql| sql_bounds(&sql)) {
                found.push(at);
            }
        }
    }
    found
}

/// The name of the `const`, `static` or `let` whose whole initializer is the
/// literal or the include at `at`, or a `concat!` holding it
/// (`concat!("COM", "MIT")`, `std::concat![…]`, the literals `sql_view()`
/// joins); SQL inside a larger expression, such as
/// `conn.begin_with("BEGIN …")`, is held by no binding.
fn holder(text: &str, at: usize) -> Option<Held> {
    let (mut start, _) = statement(text, at);
    // A macro's braces (`concat! { … }`) open no statement.
    while start > 0
        && text.as_bytes()[start - 1] == b'{'
        && text[..start - 1].trim_end().ends_with('!')
    {
        start = statement(text, start - 1).0;
    }
    let mut head = text[start..at].trim_start();
    while let Some(bracket) = attribute(head) {
        head = head[closing(head, bracket)..].trim_start();
    }
    let public = visibility(head) > 0;
    head = head[visibility(head)..].trim_start();
    let item = !word(head, "let");
    // `const`, `static` or `let`, then `mut` if it follows: read as tokens.
    let rest = ["const", "static", "let"]
        .iter()
        .find_map(|k| word(head, k).then(|| head[k.len()..].trim_start()))?;
    let rest = if word(rest, "mut") {
        rest[3..].trim_start()
    } else {
        rest
    };
    let rest = rest.strip_prefix("r#").unwrap_or(rest);
    let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
    let after = rest[name.len()..].trim_start();
    if name.is_empty() || !(after.starts_with(':') || after.starts_with('=')) {
        return None;
    }
    let init = &after[after.find('=')? + 1..];
    let opening = init
        .trim()
        .trim_start_matches('r')
        .trim_start_matches('#')
        .trim_start_matches('"');
    let held = Held { name, item, public, start };
    if opening.trim().is_empty() {
        return Some(held);
    }
    // A `concat!` (`std::`/`core::` too, any delimiter) holding `at` that is
    // the whole initializer: nothing but `;` after its closing delimiter.
    let mut call = init.trim_start();
    call = call.strip_prefix("::").map_or(call, str::trim_start);
    for path in ["std", "core"] {
        if word(call, path) {
            if let Some(rest) = call[path.len()..].trim_start().strip_prefix("::") {
                call = rest.trim_start();
            }
        }
    }
    if !word(call, "concat") {
        return None;
    }
    let delimiter = call[6..].trim_start().strip_prefix('!')?.trim_start();
    if !delimiter.starts_with(['(', '[', '{']) {
        return None;
    }
    let open = at - delimiter.len();
    let close = closing(text, open);
    (at < close && text[close..].trim_start().starts_with(';')).then_some(held)
}

/// Calls that run the future handed to them before they return, awaited
/// (reviewed: tokio's deadlines, which poll it to completion or drop it): a
/// bound scope handed to one runs there. Any other call may return it unrun
/// (`std::future::ready(release).await`).
const RUNS_FUTURE: [&str; 2] = ["timeout_at", "timeout"];

/// Calls that open a transaction.
const OPENERS: [&str; 3] = ["begin", "begin_with", "try_begin"];

/// Reviewed functions that end a transaction they did not open (each call of
/// one, where it reaches and in its form, is a transaction boundary): the
/// seed's no-op exit commits the read transaction it is handed, and the
/// venue budget's source audit commits the connection it is handed.
const RELEASERS: [Entry; 2] = [
    entry("robson-db/src/init.rs", "nothing_to_seed", Reach::Module, Form::Free),
    entry(
        "robsond/src/venue_budget/source_audit/run.rs",
        "commit",
        Reach::Module,
        Form::Free,
    ),
];

/// Reviewed functions that open a transaction on the connection they are
/// handed, by name in their form (they own it only on a connection their
/// caller obtained itself, [`Guard::owned_before`]), each
/// verified to open one at its own level and end none: the venue budget's
/// `begin_bounded` runs `begin.sql` (`BEGIN ISOLATION LEVEL READ
/// COMMITTED`).
const OPENER_FNS: [Entry; 1] = [entry(
    "robsond/src/venue_budget/bootstrap.rs",
    "begin_bounded",
    Reach::Anywhere,
    Form::Free,
)];

/// Whether the boundary SQL at `b` in file `file` opens a transaction
/// ([`Source::opens`]).
fn opener_sql(file: &Source, b: usize) -> bool {
    file.opens.contains(&b)
}

/// Whether the boundary at `b` in `source` only opens a transaction: a
/// literal statement `BEGIN` or `START TRANSACTION`, or the include of SQL
/// whose boundaries all do (the venue budget's `begin.sql`).
fn opens_transaction(source: &Source, b: usize) -> bool {
    let word = source.raw[b..].trim_start().to_ascii_lowercase();
    if word.starts_with("begin") || word.starts_with("start") {
        return true;
    }
    let Some(dir) = &source.dir else {
        return false;
    };
    includes(source)
        .into_iter()
        .find(|(at, _)| *at == b)
        .and_then(|(_, path)| std::fs::read_to_string(dir.join(path)).ok())
        .is_some_and(|sql| {
            let hashes = "#".repeat(sql.matches('#').count() + 1);
            let literal = format!("r{hashes}\"{sql}\"{hashes}");
            let read = Guard::new(vec![("sql".into(), None, literal, None)], &[], &[], &[]);
            let file = &read.files[0];
            !file.bounds.is_empty() && file.bounds.iter().all(|b| file.opens.contains(b))
        })
}

/// The [`INERT`] entry the boundary SQL at `p` in `file` matches, if any: in
/// its function, by a literal's contents, or by `held`, the name of the
/// binding used there.
fn inert_entry(file: &Source, p: usize, held: Option<&str>) -> Option<usize> {
    let (function, ..) = file.enclosing(p)?;
    INERT.iter().position(|(rel, name, what)| {
        *rel == file.rel
            && *name == function.as_str()
            && held.map_or_else(
                || file.raw[p..].strip_prefix(*what).is_some_and(|rest| rest.starts_with('"')),
                |h| h == *what,
            )
    })
}

/// Whether the opener at `p` (an [`OPENERS`] call or opening SQL) is proven
/// for the scope whose body opens at `open`: at that scope's own level (in no
/// block opened inside it that closes past `p`), not conditional
/// ([`conditional`]) and run there (the call awaited along its chain, the
/// SQL by [`runs_here`]).
fn opens(file: &Source, open: usize, p: usize) -> bool {
    let text = &file.text;
    let level = !text[open + 1..p]
        .match_indices('{')
        .any(|(o, _)| closing(text, open + 1 + o) > p);
    let runs = if opener_sql(file, p) {
        runs_here(text, p)
    } else {
        let name_end = p + text[p..].find(|c: char| !is_ident(c)).unwrap_or(0);
        let paren = open_paren(text, name_end);
        text[paren..].starts_with('(') && awaited_chain(text, closing(text, paren))
    };
    level && !conditional(text, p) && runs
}

/// Whether the scope whose body opens at `open` provably opened its own
/// transaction before `at` ([`opens`]): a boundary there then ends only that
/// one.
fn opened_before(file: &Source, open: usize, at: usize) -> bool {
    let text = &file.text;
    OPENERS
        .iter()
        .flat_map(|b| calls(text, b, open, at, Form::Method))
        .chain(
            file.bounds
                .iter()
                .copied()
                .filter(|b| (open..at).contains(b) && opener_sql(file, *b)),
        )
        .any(|p| opens(file, open, p))
}

/// Reviewed functions whose call hands their caller a transaction they own
/// (each verified to own the one it opens and end none,
/// [`Guard::owned_before`]): the venue budget source audit's `begin`
/// connects a connection and runs `begin.sql` on it.
const OWNING_FNS: [Entry; 1] = [entry(
    "robsond/src/venue_budget/source_audit/run.rs",
    "begin",
    Reach::Module,
    Form::Free,
)];

/// Reviewed functions that return a connection they connected (each verified
/// to call `connect` or `acquire`): the N5 claim's `raw_connection`.
const CONNECTORS: [Entry; 1] = [entry(
    "robsond/src/venue_budget/n5_claim/mod.rs",
    "raw_connection",
    Reach::Module,
    Form::Free,
)];

/// Whether `name` holds a connection this function obtained itself before
/// `at` (its body opening at `body`): the last `let` binding it there
/// connects or acquires one (`options.connect()`, `pool.acquire()`, a
/// reviewed [`CONNECTORS`] call).
fn fresh(text: &str, body: usize, at: usize, name: &str) -> bool {
    let Some(u) = uses(text, name, body, at).into_iter().rfind(|&u| {
        let head: Vec<&str> = text[statement(text, u).0..u].split_whitespace().collect();
        head == ["let"] || head == ["let", "mut"]
    }) else {
        return false;
    };
    let init = &text[u + name.len()..statement(text, u).1.min(text.len())];
    ["connect", "acquire"]
        .iter()
        .any(|c| !calls(init, c, 0, init.len(), Form::Method).is_empty())
        || CONNECTORS
            .iter()
            .any(|e| !calls(init, e.name, 0, init.len(), e.form).is_empty())
}

/// Whether the boundary SQL at `b` in `file` rolls back to a savepoint
/// (`ROLLBACK TO s`): it releases the locks taken since, but leaves the
/// transaction open.
fn rolls_back_to(file: &Source, b: usize) -> bool {
    let sql = file.raw[b..].trim_start().to_ascii_lowercase();
    sql.strip_prefix("rollback").is_some_and(|rest| word(rest.trim_start(), "to"))
}

/// Whether `at` stands in an `else` branch of the `if` whose block closes at
/// `close`: the other side, exclusive with that block.
fn else_holds(text: &str, close: usize, at: usize) -> bool {
    let mut end = close;
    loop {
        let rest = text[end..].trim_start();
        if !word(rest, "else") {
            return false;
        }
        let at_else = text.len() - rest.len();
        let Some(open) = text[at_else..].find('{').map(|i| at_else + i) else {
            return false;
        };
        let next = closing(text, open);
        if open < at && at < next {
            return true;
        }
        end = next;
    }
}

/// The plain receiver of the method call whose name is at `c`
/// (`conn.begin()`: `conn`), if it is one.
fn receiver(text: &str, c: usize) -> Option<&str> {
    let before = text[..c].trim_end().strip_suffix('.')?.trim_end();
    let start = before.len() - before.chars().rev().take_while(|ch| is_ident(*ch)).count();
    let rest = before[..start].trim_end();
    (start < before.len() && !rest.ends_with('.') && !rest.ends_with("::") && !rest.ends_with(')'))
        .then(|| &before[start..])
}

/// Whether `name` is a parameter of the function starting at `fn_start`
/// (its body opening at `body`) whose type is a pool (`PgPool`,
/// `Pool<Postgres>`), which hands out a connection in no transaction.
fn pool_param(text: &str, fn_start: usize, body: usize, name: &str) -> bool {
    let signature = &text[fn_start..body];
    uses(signature, name, 0, signature.len()).into_iter().any(|u| {
        let Some(ty) = signature[u + name.len()..].trim_start().strip_prefix(':') else {
            return false;
        };
        let ty = ty.split([',', ')']).next().unwrap_or("");
        let token = ty
            .split(|c: char| c.is_whitespace() || c == '&')
            .rfind(|t| !t.is_empty() && !t.starts_with('\'') && *t != "mut")
            .unwrap_or("");
        let last = token.split('<').next().unwrap_or("").rsplit("::").next().unwrap_or("");
        last == "PgPool" || last == "Pool"
    })
}

/// Whether a local binding of `name` whose scope holds `u` shadows the item
/// of that name there, in the function around `u`: a parameter in that
/// function's own parameter list (its body; a function-pointer type's
/// parameter binds nothing), a `let NAME` or `let mut NAME` (from the end of
/// its statement, so not in its own initializer, to the end of its block), a
/// `match` arm's pattern (`Some(step) =>`, never its `if` guard: that arm
/// only), or a closure's parameter (`|step|`: that closure's body, braced,
/// or braceless to its first top-level `,` or enclosing delimiter); the
/// binding itself is a declaration, no use. A path
/// (`crate::append::append_event_tx`) names the item whatever is bound, and
/// any other binding (`if let`, `for`, a pattern `let`) is not read:
/// fail-closed, the use is the item's.
fn shadowed(text: &str, file: &Source, u: usize, name: &str) -> bool {
    if text[..u].trim_end().ends_with("::") {
        return false;
    }
    let Some((_, start, ..)) = file.enclosing(u) else {
        return false;
    };
    let body = item_open(text, *start).unwrap_or(*start);
    // The innermost block (`{`) around `p`, if inside the function body.
    let block = |p: usize| {
        let mut open = unclosed(text, p);
        while let Some((o, b)) = open.filter(|&(o, _)| o >= body) {
            if b == b'{' {
                return Some(o);
            }
            open = unclosed(text, o);
        }
        None
    };
    uses(text, name, *start, u + name.len()).into_iter().any(|b| {
        let before = text[..b].trim_end();
        let after = text[b + name.len()..].trim_start();
        if b < body {
            // In the function's own parameter list only (`fn(name: u8)`, a
            // function-pointer type's, binds nothing here).
            return after.starts_with(':')
                && !after.starts_with("::")
                && (before.ends_with('(') || before.ends_with(',') || before.ends_with("mut"))
                && unclosed(text, b)
                    .is_some_and(|(o, br)| br == b'(' && o >= *start && own_parameters(text, o));
        }
        let head: Vec<&str> = text[statement(text, b).0..b].split_whitespace().collect();
        if head == ["let"] || head == ["let", "mut"] {
            // From the end of its statement: its own initializer still names
            // the item (`let append_event_tx = append_event_tx;`).
            return b == u
                || block(b)
                    .is_some_and(|o| matched_statement_end(text, b) <= u && u < closing(text, o));
        }
        if (before.ends_with('(') || before.ends_with(',')) && after.starts_with(')') {
            return block(b).is_some_and(|o| {
                match_arms(text, o).into_iter().any(|(head, from, to, braced)| {
                    let arm_end = if braced { closing(text, from) } else { to };
                    // The pattern only, not the guard after its `if`.
                    let guard = top_level(text, head, from, "if")
                        .into_iter()
                        .find(|&i| word(&text[i..], "if") && !text[..i].ends_with(is_ident))
                        .unwrap_or(from);
                    head <= b && b < guard && (b == u || from <= u && u < arm_end)
                })
            });
        }
        if before.ends_with('|') && (after.starts_with('|') || after.starts_with(',')) {
            let Some(params) = text[b..].find('|').map(|i| b + i + 1) else {
                return false;
            };
            let rest = text[params..].trim_start();
            let scope_end = if rest.starts_with('{') {
                closing(text, text.len() - rest.len())
            } else {
                // A braceless body: to its first top-level `,`, the end of the
                // bracket around it, or its statement's end, whichever is
                // first (`(|x| x, append_event_tx)` ends at the `,`).
                let limit = unclosed(text, params)
                    .map_or(text.len(), |(o, _)| closing(text, o) - 1)
                    .min(matched_statement_end(text, b));
                top_level(text, params, limit, ",").first().copied().unwrap_or(limit)
            };
            return b == u || params <= u && u < scope_end;
        }
        false
    })
}

/// The first argument of the call whose `(` is at `open`.
fn first_arg(text: &str, open: usize) -> &str {
    if !text[open..].starts_with('(') {
        return "";
    }
    let inner = &text[open + 1..closing(text, open).saturating_sub(1).max(open + 1)];
    let mut depth = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return &inner[..i],
            _ => {},
        }
    }
    inner
}

/// The plain name an argument or receiver reads (`&mut *tx`, `&mut c`,
/// `connection`), if it is one.
fn plain_name(expr: &str) -> Option<&str> {
    let mut e = expr.trim();
    loop {
        let before = e;
        e = e.strip_prefix('&').map_or(e, str::trim_start);
        if word(e, "mut") {
            e = e[3..].trim_start();
        }
        e = e.strip_prefix('*').map_or(e, str::trim_start);
        if e == before {
            break;
        }
    }
    (!e.is_empty() && e.chars().all(is_ident)).then_some(e)
}

/// The connection the opening SQL at `b` runs on, read plainly: the argument
/// of the executor its query's chain reaches
/// (`sqlx::raw_sql("BEGIN").execute(&mut c)`), the receiver of an executor
/// taking it (`connection.execute(sqlx::raw_sql("BEGIN"))`), or the first
/// argument of a [`RUNS_QUERY`] call (`completion::run(&mut connection, …)`).
fn sql_connection(text: &str, b: usize) -> Option<&str> {
    let mut open = unclosed(text, b);
    while let Some((o, bracket)) = open {
        if bracket != b'(' {
            return None;
        }
        let head = text[..o].trim_end();
        let named = |name: &str| head.strip_suffix(name).is_some_and(|b| !b.ends_with(is_ident));
        if RUNS_QUERY.iter().any(|r| named(r)) {
            return plain_name(first_arg(text, o));
        }
        if let Some(e) = EXECUTORS.iter().find(|e| named(e)) {
            let receiver = head[..head.len() - e.len()].trim_end().strip_suffix('.')?;
            let start = receiver.trim_end().len()
                - receiver.trim_end().chars().rev().take_while(|c| is_ident(*c)).count();
            let name = &receiver.trim_end()[start..];
            let before = receiver[..start].trim_end();
            return (!name.is_empty() && !before.ends_with('.') && !before.ends_with("::"))
                .then_some(name);
        }
        if named("query")
            || named("raw_sql")
            || named("include_str!")
            || head.ends_with("include_str!")
        {
            // The chain after the query: its executor's argument.
            let mut p = closing(text, o);
            loop {
                let rest = text[p..].trim_start();
                let Some(r) = rest.strip_prefix('.').map(str::trim_start) else {
                    break;
                };
                let n = r.find(|c: char| !is_ident(c)).unwrap_or(r.len());
                let after = text.len() - r[n..].len();
                let paren = open_paren(text, after);
                if !text[paren..].starts_with('(') {
                    break;
                }
                if EXECUTORS.contains(&&r[..n]) {
                    return plain_name(first_arg(text, paren));
                }
                p = closing(text, paren);
            }
            open = unclosed(text, o);
            continue;
        }
        return None;
    }
    None
}

/// Reviewed text the boundary reader matches that no statement runs, so no
/// transaction boundary, by file, function and the text itself (a literal's
/// contents, or the binding's name at a use): the startup policy word
/// `abort` (its `Display` and `FromStr`). An entry that matches nothing
/// fails.
const INERT: [(&str, &str, &str); 2] = [
    ("robsond/src/config.rs", "fmt", "abort"),
    ("robsond/src/config.rs", "from_str", "abort"),
];

/// Calls that run the query handed to them before they return, awaited
/// (reviewed: the venue budget's `completion::run` drains
/// `connection.fetch_many(query)`): boundary SQL handed to one runs there.
const RUNS_QUERY: [&str; 1] = ["completion::run"];

/// Where boundary SQL in a closure's or an `async` block's body runs.
enum Deferred {
    /// Where the `let` binding the closure or block is called or awaited.
    Bound(String),
    /// Where it stands: an `async` block awaited there.
    Awaited,
    /// Passed on, spawned or returned (the scope's body opening here): where,
    /// the guard does not follow.
    Escapes(usize),
}

/// The [`Deferred`] run of the boundary SQL at `at` in file `file`, if a
/// closure's or an `async` block's body holds it in its function (the
/// innermost one, braced, [`held_by`], or not, [`braceless_closure`]).
fn deferred_boundary(file: &Source, at: usize) -> Option<Deferred> {
    let text = &file.text;
    let (_, fn_start, ..) = file.enclosing(at)?;
    let body = fn_start + text[*fn_start..].find('{').unwrap_or(0);
    let start = statement(text, at).0;
    let braced = held_by(text, at, body)
        .1
        .map(|(open, bound)| (open, bound, statement(text, open).0));
    let braceless = braceless_closure(text, start, at).map(|(open, bound)| (open, bound, start));
    let (open, bound, holder) = match (braced, braceless) {
        (Some(b), Some(c)) => {
            if c.0 > b.0 {
                c
            } else {
                b
            }
        },
        (Some(b), None) => b,
        (None, Some(c)) => c,
        (None, None) => return None,
    };
    if bound {
        let head = text[holder..].trim_start();
        if word(head, "let") {
            let rest = head[3..].trim_start();
            let rest = if word(rest, "mut") {
                rest[3..].trim_start()
            } else {
                rest
            };
            let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
            if !name.is_empty() && name != "_" {
                return Some(Deferred::Bound(name));
            }
        }
        if text.as_bytes()[open] == b'{' && awaited_chain(text, closing(text, open)) {
            return Some(Deferred::Awaited);
        }
    }
    Some(Deferred::Escapes(open))
}

/// Where the `let`, `const` or `static` whose initializer holds the boundary
/// SQL at `at` starts, if one does, out through the expression blocks around
/// it by bracket ancestry, whatever statements precede it there (`{ "COMMIT"
/// }`, `{ f(); sqlx::query("COMMIT") }`, a branch; never a function's or an
/// item's body): a value held there, whose uses only [`holder`] follows.
fn in_binding(text: &str, at: usize) -> Option<usize> {
    let mut start = statement(text, at).0;
    loop {
        let mut head = text[start..at].trim_start();
        while let Some(bracket) = attribute(head) {
            head = head[closing(head, bracket)..].trim_start();
        }
        head = head[visibility(head)..].trim_start();
        if ["let", "const", "static"].iter().any(|k| word(head, k)) {
            return Some(start);
        }
        let (open, b) = unclosed(text, start)?;
        if b != b'{' {
            return None;
        }
        let outer = statement(text, open).0;
        let block = &text[outer..open];
        let bodies = [
            "fn",
            "impl",
            "mod",
            "trait",
            "struct",
            "enum",
            "union",
            "extern",
            "macro_rules",
        ];
        if bodies.iter().any(|k| !uses(block, k, 0, block.len()).is_empty()) {
            return None;
        }
        start = outer;
    }
}

/// Whether the boundary SQL at `at` is proven to run where it stands:
/// executed and awaited in its statement ([`runs_in_statement`]), or, through
/// `sqlx::query(…)`/`raw_sql(…)` only, the argument of an awaited boundary call
/// (`begin_with("BEGIN …")`, the await found along its method chain:
/// `.instrument(…).await`) or of an awaited [`RUNS_QUERY`] call. Any other
/// await proves nothing (`std::future::ready("COMMIT").await`).
fn runs_here(text: &str, at: usize) -> bool {
    let (mut start, _) = statement(text, at);
    while start > 0
        && text.as_bytes()[start - 1] == b'{'
        && text[..start - 1].trim_end().ends_with('!')
    {
        start = statement(text, start - 1).0;
    }
    if runs_in_statement(text, start, at, matched_statement_end(text, at)) {
        return true;
    }
    let mut open = unclosed(text, at);
    while let Some((o, b)) = open.filter(|&(o, _)| o >= start) {
        let head = text[..o].trim_end();
        let named = |name: &str| {
            head.strip_suffix(name)
                .is_some_and(|before| !before.ends_with(|c: char| is_ident(c)))
        };
        if b != b'(' {
            return false;
        }
        if named("begin_with") && awaited_chain(text, closing(text, o)) {
            return true;
        }
        if RUNS_QUERY.iter().any(|r| named(r)) && awaited_chain(text, closing(text, o)) {
            return true;
        }
        if !(named("query") || named("raw_sql")) {
            return false;
        }
        open = unclosed(text, o);
    }
    false
}

/// Whether the method chain from `p` (`?`, `.name(…)`, a turbofish) reaches
/// `.await`.
fn awaited_chain(text: &str, mut p: usize) -> bool {
    loop {
        let rest = text[p..].trim_start();
        if let Some(r) = rest.strip_prefix('?') {
            p = text.len() - r.len();
            continue;
        }
        let Some(r) = rest.strip_prefix('.').map(str::trim_start) else {
            return false;
        };
        if word(r, "await") {
            return true;
        }
        let n = r.find(|c: char| !is_ident(c)).unwrap_or(r.len());
        let paren = open_paren(text, text.len() - r[n..].len());
        if n == 0 || !text[paren..].starts_with('(') {
            return false;
        }
        p = closing(text, paren);
    }
}

/// A binding holding SQL ([`holder`]): its name, whether it is an item
/// (`const`, `static`: usable from other files) rather than a `let`, whether
/// it has a visibility, and where its declaration starts.
struct Held {
    name: String,
    item: bool,
    public: bool,
    start: usize,
}

/// Whether SQL read from a file writes an input or locks `accounting_state`:
/// read as one raw literal no quote in it can end.
fn sql_writes(sql: &str) -> bool {
    let hashes = "#".repeat(sql.matches('#').count() + 1);
    let literal = format!("r{hashes}\"{sql}\"{hashes}");
    !Guard::new(vec![("sql".into(), None, literal, None)], &[], &[], &[])
        .sites(0)
        .is_empty()
}

/// Each `include_str!` in `file`'s code with a literal path, read across
/// whitespace and line breaks (`include_str!(\n "x.sql"\n)`), an ordinary
/// literal's path decoded as Rust decodes it ([`unescaped`]: `\x63`, `\u{…}`, a
/// line continuation), a raw one kept as written: `(offset, path)`. A path
/// given another way is not followed; `Guard::orphan_sql` fails a writer
/// reached only so.
fn includes(file: &Source) -> Vec<(usize, String)> {
    let (text, raw) = (&file.text, &file.raw);
    let skip = |q: usize| q + text[q..].len() - text[q..].trim_start().len();
    let mut found = Vec::new();
    for at in uses(text, "include_str", 0, text.len()) {
        // `include_str!`, then any macro delimiter: `(`, `[` or `{`.
        let mut q = skip(at + "include_str".len());
        let mut call = text[q..].starts_with('!');
        q = skip((q + 1).min(text.len()));
        call &= text[q..].starts_with(['(', '[', '{']);
        q = (q + 1).min(text.len());
        let r = skip(q);
        let quote = r + raw[r..]
            .strip_prefix('r')
            .map_or(0, |h| 1 + h.len() - h.trim_start_matches('#').len());
        if !call || !raw[quote..].starts_with('"') {
            continue;
        }
        let end = after_string(raw, quote);
        let mut path = raw[quote + 1..end - 1 - (quote - r).saturating_sub(1)].to_string();
        if quote == r {
            path = unescaped(&path);
        }
        found.push((at, path));
    }
    found
}

/// The offset after the bracket matching the one at `open`, skipping string
/// and char literals.
fn closing(text: &str, open: usize) -> usize {
    let bytes = text.as_bytes();
    let (up, down) = match bytes[open] {
        b'(' => (b'(', b')'),
        b'[' => (b'[', b']'),
        _ => (b'{', b'}'),
    };
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                // A raw string's hashes: `r#"…"#`.
                let hashes = text[..i].bytes().rev().take_while(|b| *b == b'#').count();
                let raw = i > hashes && bytes[i - hashes - 1] == b'r';
                let end = "\"".to_string() + &"#".repeat(hashes);
                i += 1;
                while i < bytes.len() {
                    if !raw && bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i..].starts_with(end.as_bytes()) {
                        i += end.len() - 1;
                        break;
                    }
                    i += 1;
                }
            },
            b'\'' if i + 2 < bytes.len() && bytes[i + 2] == b'\'' => i += 2,
            c if c == up => depth += 1,
            c if c == down => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            },
            _ => {},
        }
        i += 1;
    }
    text.len()
}

/// The offset after the string literal whose opening quote is at `quote`
/// (raw strings with their hashes included).
fn after_string(text: &str, quote: usize) -> usize {
    let bytes = text.as_bytes();
    let hashes = text[..quote].bytes().rev().take_while(|b| *b == b'#').count();
    let raw = quote > hashes && bytes[quote - hashes - 1] == b'r';
    let end = "\"".to_string() + &"#".repeat(hashes);
    let mut i = quote + 1;
    while i < bytes.len() {
        if !raw && bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i..].starts_with(end.as_bytes()) {
            return i + end.len();
        }
        i += 1;
    }
    text.len()
}

/// The keywords of a statement that ends with its block.
const BLOCK_STATEMENTS: [&str; 6] = ["if", "match", "loop", "while", "for", "unsafe"];

/// Whether `text` starts a block-like expression, one that ends with its
/// block (no `;` or `,` needed after it): a block, a labelled block, an
/// `if`/`match`/`loop`/`while`/`for`/`unsafe` (keywords as tokens, past a
/// label), a brace-delimited macro or a block expression with its own
/// keyword ([`block_expression`]).
fn block_like(text: &str) -> bool {
    let head = unlabelled(text.trim_start());
    head.starts_with('{')
        || BLOCK_STATEMENTS.iter().any(|k| word(head, k))
        || brace_macro(head)
        || block_expression(head)
}

/// Whether `text` starts a block expression with its own keyword: `const {`,
/// `async {`, `async move {` or `try {` (an `async` block needs its `;` or
/// `,` in Rust; read as ending at its block it still removes all its code).
fn block_expression(text: &str) -> bool {
    ["const", "async", "try"].iter().any(|k| {
        word(text, k) && {
            let rest = text[k.len()..].trim_start();
            let rest = if word(rest, "move") {
                rest[4..].trim_start()
            } else {
                rest
            };
            rest.starts_with('{')
        }
    })
}

/// Whether `text` starts a brace-delimited macro invocation: a path (`name`,
/// `a::b`, across whitespace), `!`, then `{`.
fn brace_macro(text: &str) -> bool {
    let rest = text.trim_start_matches(|c: char| is_ident(c) || c == ':' || c.is_whitespace());
    text[..text.len() - rest.len()].contains(|c: char| is_ident(c))
        && rest.strip_prefix('!').is_some_and(|r| r.trim_start().starts_with('{'))
}

/// `text` past a loop or block label starting it (`'outer:`, not a path's
/// `::`).
fn unlabelled(text: &str) -> &str {
    let Some(rest) = text.strip_prefix('\'') else {
        return text;
    };
    let name = rest.find(|c: char| !is_ident(c)).unwrap_or(rest.len());
    match rest[name..].trim_start().strip_prefix(':') {
        Some(body) if name > 0 && !body.starts_with(':') => body.trim_start(),
        _ => text,
    }
}

/// The end of the statement, field or arm starting at `from`: after its `;`
/// or `,`, at the bracket closing its list, or after the block ending an
/// `if`/`match`/`loop`/`while`/`for`/`unsafe` statement, a labelled block or
/// a brace-delimited macro statement (`println! { … }`, which needs no `;`)
/// (unless `else` follows), skipping nested brackets, strings and char
/// literals; a match arm's body after its `=>` is read the same way (an arm
/// whose body is one of those needs no `,`). The keywords are read as tokens,
/// past a label (`'outer:`).
fn item_end(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut block_statement = block_like(&text[from..]);
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'=' if bytes.get(i + 1) == Some(&b'>') => {
                block_statement = block_like(&text[i + 2..]);
                i += 2;
            },
            open @ (b'(' | b'[' | b'{') => {
                let close = closing(text, i);
                if open == b'{' && block_statement && !word(text[close..].trim_start(), "else") {
                    return close;
                }
                i = close;
            },
            b'"' => i = after_string(text, i),
            b'\'' if i + 2 < bytes.len() && bytes[i + 2] == b'\'' => i += 3,
            b';' | b',' => return i + 1,
            b')' | b']' | b'}' => return i,
            _ => i += 1,
        }
    }
    text.len()
}

/// The head of the statement whose block opens at `open`: scanned back over
/// balanced brackets to the enclosing `;` or opener.
fn statement_head(text: &str, open: usize) -> &str {
    let bytes = text.as_bytes();
    let (mut depth, mut head) = (0usize, 0);
    for i in (0..open).rev() {
        match bytes[i] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' | b';' if depth == 0 => {
                head = i + 1;
                break;
            },
            b'(' | b'[' | b'{' => depth -= 1,
            _ => {},
        }
    }
    &text[head..open]
}

/// Whether the block opened at `open` is the body of an `if`, an `else` or a
/// `match` arm, never of a loop, a closure, an `async` block or a bare
/// block: its [`statement_head`] starts with `if `, or `else` or `=>`
/// precedes it.
fn branch(text: &str, open: usize) -> bool {
    let before = text[..open].trim_end();
    statement_head(text, open).trim_start().starts_with("if ")
        || before.ends_with("else")
        || before.ends_with("=>")
}

/// Whether a `return` in the branch opened at `open` (in the function
/// starting at `start`) leaves a scope that holds `at`: the innermost `async`
/// block, closure or nested `fn` around the branch (a head with `async`, `|`
/// or `fn`), else the function itself. A branch's `return` inside an `async`
/// block or closure that closes before `at` leaves only that block.
fn returns_to(text: &str, start: usize, open: usize, at: usize) -> bool {
    let body = start + text[start..open].find('{').unwrap_or(open - start);
    text[body + 1..open]
        .rmatch_indices('{')
        .map(|(i, _)| body + 1 + i)
        .find(|&o| {
            let head = statement_head(text, o);
            closing(text, o) > open
                && (head.contains("async")
                    || head.contains('|')
                    || !uses(head, "fn", 0, head.len()).is_empty())
        })
        .is_none_or(|o| o < at && at < closing(text, o))
}

/// Whether the branch opened at `open` and closed at `close` leaves the
/// function after `q`, unconditionally: it is an `if`/`else`/`match`-arm body
/// ([`branch`]) with a `return` statement (in code, after `;`, `{` or `}`;
/// never a string or a braceless closure), the one holding `q` or a later
/// one, at its own level (every
/// block opened inside it before the `return` has closed) and no `break` or
/// `continue` from `q` to the end of that `return` statement that could
/// bypass it (its operand runs first: `return { break 'b; }` leaves the block
/// `'b`, not the function). Anything else (a conditional or nested `return`,
/// a loop, closure or `async` body) keeps the boundary.
fn leaves(text: &str, open: usize, q: usize, close: usize) -> bool {
    // The `return` may be the statement holding `q` itself (`return
    // nothing_to_seed(tx, …).await;`: its operand runs, then it leaves).
    let from = statement(text, q).0.max(open + 1);
    branch(text, open)
        && uses(text, "return", from, close).into_iter().any(|r| {
            let end = matched_statement_end(text, r).min(close);
            let after = r.min(q);
            text[..r].trim_end().ends_with([';', '{', '}'])
                && text[open + 1..r]
                    .match_indices('{')
                    .all(|(i, _)| closing(text, open + 1 + i) <= r)
                && uses(text, "break", after, end).is_empty()
                && uses(text, "continue", after, end).is_empty()
        })
}

/// `text` with every comment (line, and nested block) and the contents of
/// every string and char literal blanked (same offsets, newlines kept), so
/// the guard reads structure, names, markers and keywords from code only: a
/// `return`, a brace or `participate(` in a comment or a string is none.
fn code_only(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut code = bytes.to_vec();
    let blank = |code: &mut Vec<u8>, from: usize, to: usize| {
        for b in &mut code[from..to] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let end = text[i..].find('\n').map_or(bytes.len(), |e| i + e);
                blank(&mut code, i, end);
                i = end;
            },
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let (mut depth, mut end) = (0usize, i);
                while end < bytes.len() {
                    if bytes[end..].starts_with(b"/*") {
                        depth += 1;
                        end += 2;
                    } else if bytes[end..].starts_with(b"*/") {
                        depth -= 1;
                        end += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        end += 1;
                    }
                }
                blank(&mut code, i, end);
                i = end;
            },
            b'"' => {
                // The contents only: the quotes and a raw string's hashes stay,
                // so `closing` still finds the literal's end.
                let end = after_string(text, i);
                let hashes = text[..i].bytes().rev().take_while(|b| *b == b'#').count();
                blank(&mut code, i + 1, end.saturating_sub(1 + hashes).max(i + 1));
                i = end;
            },
            b'\'' if i + 2 < bytes.len() && bytes[i + 2] == b'\'' => {
                blank(&mut code, i + 1, i + 2);
                i += 3;
            },
            b'\'' if i + 3 < bytes.len() && bytes[i + 1] == b'\\' && bytes[i + 3] == b'\'' => {
                blank(&mut code, i + 1, i + 3);
                i += 4;
            },
            _ => i += 1,
        }
    }
    String::from_utf8(code).expect("blanking keeps UTF-8: whole bytes become spaces")
}

/// The SQL in `text`, as the database receives it. Only literal contents are
/// kept, everything else blanked (line breaks kept), so a Rust comment or
/// identifier is never read as SQL. Each ordinary (not raw) string literal is
/// decoded as Rust decodes it ([`escape`]), raw ones are copied, and each run
/// of literal arguments of a `concat!` ([`concat_runs`]) is joined into one
/// literal; every literal's SQL comments are blanked ([`blank_sql_comments`]).
fn sql_view(text: &str) -> SqlView {
    let bytes = text.as_bytes();
    let code = code_only(text);
    let runs = concat_runs(&code, text);
    let mut view = SqlView::default();
    let blank = |view: &mut SqlView, from: usize, to: usize| {
        view.sql
            .extend(bytes[from..to].iter().map(|b| if *b == b'\n' { b'\n' } else { b' ' }));
        view.sql_at.extend(from..to);
    };
    let (mut from, mut run) = (0, 0);
    loop {
        while runs.get(run).is_some_and(|r| r.0 < from) {
            run += 1;
        }
        let quote = code[from..].find('"').map(|q| from + q);
        // A literal is one `concat!` run, or one string literal.
        let (begin, end, pieces) = match (runs.get(run), quote) {
            (Some(r), q) if q.is_none_or(|q| r.0 <= q) => {
                run += 1;
                (r.0, r.1, r.2.clone())
            },
            (_, Some(q)) => {
                let end = after_string(text, q);
                let open = text[..q].trim_end_matches('#').trim_end_matches('r').len();
                (open, end, vec![Piece::Str(open)])
            },
            (_, None) => break,
        };
        blank(&mut view, from, begin);
        let start = view.sql.len();
        view.starts.push(start);
        for piece in pieces {
            piece.decode(text, &mut view);
        }
        let separators = blank_sql_comments(&mut view.sql[start..]);
        view.semicolons.extend(separators.into_iter().map(|s| start + s));
        from = end;
    }
    blank(&mut view, from, bytes.len());
    view
}

/// [`sql_view`]'s result: the SQL, each byte's offset in the source, where
/// each literal starts, and the statement separators (`;` outside any quote).
#[derive(Default)]
struct SqlView {
    sql: Vec<u8>,
    sql_at: Vec<usize>,
    starts: Vec<usize>,
    semicolons: Vec<usize>,
}

/// One literal argument of a `concat!`, at its source offset.
#[derive(Clone, Copy)]
enum Piece {
    /// A string literal (`"…"`, `r#"…"#`) starting there.
    Str(usize),
    /// A char literal, a number or `true`/`false`: `[from, to)`.
    Other(usize, usize),
}

impl Piece {
    /// Appends what the piece contributes to the SQL, with source offsets.
    fn decode(self, text: &str, view: &mut SqlView) {
        let bytes = text.as_bytes();
        let push = |view: &mut SqlView, s: &str, at: usize| {
            view.sql.extend_from_slice(s.as_bytes());
            view.sql_at.extend(std::iter::repeat_n(at, s.len()));
        };
        match self {
            Piece::Str(open) => {
                let quote = open + text[open..].find('"').unwrap_or(0);
                let hashes = quote - open - usize::from(open < quote);
                let raw = open < quote;
                let end = after_string(text, quote);
                let close = end.saturating_sub(1 + hashes).max(quote + 1);
                let mut i = quote + 1;
                while i < close {
                    if raw || bytes[i] != b'\\' {
                        let c = text[i..].chars().next().unwrap_or(' ');
                        push(view, c.encode_utf8(&mut [0; 4]), i);
                        i += c.len_utf8();
                        continue;
                    }
                    let (decoded, len) = escape(&text[i..close]);
                    push(view, &decoded.map(String::from).unwrap_or_default(), i);
                    i += len;
                }
            },
            Piece::Other(from, to) => {
                let piece = text[from..to].trim();
                let value = match piece.strip_prefix('\'').and_then(|c| c.strip_suffix('\'')) {
                    Some(c) if c.starts_with('\\') => {
                        escape(c).0.map(String::from).unwrap_or_default()
                    },
                    Some(c) => c.to_string(),
                    None => piece.to_string(),
                };
                push(view, &value, from);
            },
        }
    }
}

/// Each run of literal arguments of a `concat!` in `code` (the masked
/// `text`), as Rust joins them: `(start, end, pieces)`, in source order.
/// `concat`, `!` and the opening delimiter (`(`, `[` or `{`) are tokens with
/// any whitespace or comment between them; arguments split at the top-level
/// commas; a run is broken by any argument that is not a literal (a macro
/// call such as `include_str!`).
fn concat_runs(code: &str, text: &str) -> Vec<(usize, usize, Vec<Piece>)> {
    let skip = |i: usize| i + code[i..].len() - code[i..].trim_start().len();
    let mut runs = Vec::new();
    for at in uses(code, "concat", 0, code.len()) {
        let bang = skip(at + "concat".len());
        let open = skip(bang + 1);
        if !code[bang..].starts_with('!') || !code[open..].starts_with(['(', '[', '{']) {
            continue;
        }
        let close = closing(code, open) - 1;
        let mut args = Vec::new();
        let (mut depth, mut arg) = (0usize, open + 1);
        for (i, b) in code.bytes().enumerate().take(close).skip(open + 1) {
            match b {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    args.push((arg, i));
                    arg = i + 1;
                },
                _ => {},
            }
        }
        args.push((arg, close));
        let mut current: Option<(usize, usize, Vec<Piece>)> = None;
        for (from, to) in args {
            let from = skip(from);
            let to = from + code[from..to].trim_end().len();
            let piece = &text[from..to];
            let literal = if piece.starts_with('"')
                || piece
                    .strip_prefix('r')
                    .is_some_and(|r| r.trim_start_matches('#').starts_with('"'))
            {
                Some(Piece::Str(from))
            } else if (piece.starts_with('\'') && piece.ends_with('\'') && piece.len() > 2)
                || piece == "true"
                || piece == "false"
                || piece.trim_start_matches('-').starts_with(|c: char| c.is_ascii_digit())
            {
                Some(Piece::Other(from, to))
            } else {
                None
            };
            match (literal, current.as_mut()) {
                (Some(p), Some(run)) => {
                    run.1 = to;
                    run.2.push(p);
                },
                (Some(p), None) => current = Some((from, to, vec![p])),
                (None, _) => runs.extend(current.take()),
            }
        }
        runs.extend(current);
    }
    runs.sort_by_key(|r| r.0);
    runs
}

/// An ordinary string literal's contents as Rust reads them: each escape
/// decoded ([`escape`]), a line continuation dropped.
fn unescaped(s: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with('\\') {
            let (c, len) = escape(&s[i..]);
            out.extend(c);
            i += len.max(1);
        } else {
            let c = s[i..].chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// The character the Rust string escape starting `s` (at its backslash)
/// stands for (none for a line continuation), and the escape's length in the
/// source.
fn escape(s: &str) -> (Option<char>, usize) {
    match s.as_bytes().get(1) {
        Some(b'n') => (Some('\n'), 2),
        Some(b't') => (Some('\t'), 2),
        Some(b'r') => (Some('\r'), 2),
        Some(b'0') => (Some('\0'), 2),
        Some(b'x') => (s.get(2..4).and_then(|h| u8::from_str_radix(h, 16).ok()).map(char::from), 4),
        Some(b'u') => {
            // `\u{…}`: up to six hex digits, underscores allowed.
            let close = s.find('}').map_or(s.len(), |c| c + 1);
            let hex = s.get(3..close.saturating_sub(1)).unwrap_or_default();
            (
                u32::from_str_radix(&hex.replace('_', ""), 16).ok().and_then(char::from_u32),
                close,
            )
        },
        // A line continuation skips the line break and the ASCII whitespace
        // after it, as Rust does.
        Some(b'\n' | b'\r') => {
            let rest = s[1..].trim_start_matches([' ', '\t', '\n', '\r']);
            (None, s.len() - rest.len())
        },
        Some(&c) => (Some(char::from(c)), 2),
        None => (None, 1),
    }
}

/// `sql` with its comments (`-- …` to the line's end, nested `/* … */`)
/// blanked, line breaks kept, as PostgreSQL's lexer reads them: none starts
/// inside a `'…'` string (`''` a quote in it; an `E'…'` one honouring
/// backslash escapes throughout, its continuations past a line break
/// included: [`sql_string_continues`]), a
/// `"…"` identifier or a `$tag$…$tag$` string, whose body is read as SQL in
/// its own right (a `DO` block's code). Returns the statement separators: each
/// `;` outside every quote (one in a dollar-quoted body separates nothing at
/// this level).
fn blank_sql_comments(sql: &mut [u8]) -> Vec<usize> {
    let mut separators = Vec::new();
    let mut i = 0;
    while i < sql.len() {
        match sql[i] {
            b'\'' => {
                let escapes = i > 0
                    && sql[i - 1].eq_ignore_ascii_case(&b'e')
                    && !(i > 1 && sql_ident_byte(sql[i - 2]));
                i += 1;
                loop {
                    while i < sql.len() && sql[i] != b'\'' {
                        i += if escapes && sql[i] == b'\\' { 2 } else { 1 };
                    }
                    // `''` is a quote inside the string, which keeps its kind.
                    if sql.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    // So does a continuation (`'a'` newline `'b'`).
                    let Some(quote) = sql_string_continues(sql, i + 1) else {
                        break;
                    };
                    i = quote + 1;
                }
                i += 1;
            },
            b'"' => {
                i += 1 + sql[i + 1..].iter().position(|b| *b == b'"').map_or(sql.len(), |e| e + 1)
            },
            // A dollar quote starts no identifier's `$` (`foo$a$` is a name).
            b'$' if !(i > 0 && sql_ident_byte(sql[i - 1])) && dollar_tag(&sql[i..]).is_some() => {
                let tag = dollar_tag(&sql[i..]).unwrap_or_default();
                let body = i + tag;
                let end = sql[body..].windows(tag).position(|w| w == &sql[i..i + tag]);
                let Some(end) = end.map(|e| body + e) else {
                    break;
                };
                blank_sql_comments(&mut sql[body..end]);
                i = end + tag;
            },
            b';' => {
                separators.push(i);
                i += 1;
            },
            // A line comment ends at a line feed or a carriage return.
            b'-' if sql.get(i + 1) == Some(&b'-') => {
                while i < sql.len() && !matches!(sql[i], b'\n' | b'\r') {
                    sql[i] = b' ';
                    i += 1;
                }
            },
            b'/' if sql.get(i + 1) == Some(&b'*') => {
                let mut depth = 0usize;
                while i < sql.len() {
                    let pair = sql.get(i..i + 2);
                    if pair == Some(b"/*") || pair == Some(b"*/") {
                        depth = if pair == Some(b"/*") {
                            depth + 1
                        } else {
                            depth - 1
                        };
                        sql[i..i + 2].fill(b' ');
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        if sql[i] != b'\n' {
                            sql[i] = b' ';
                        }
                        i += 1;
                    }
                }
            },
            _ => i += 1,
        }
    }
    separators
}

/// The quote at which a string closed just before `from` continues, if it
/// does, as PostgreSQL's `quotecontinue` reads it: horizontal space and `--`
/// comments, a line break, then any space and line-ended comments, then a
/// quote (`'a'` newline `'b'` is the one string `ab`, an `E'…'` one's
/// escapes kept). The comments it passes are blanked.
fn sql_string_continues(sql: &mut [u8], from: usize) -> Option<usize> {
    let mut i = from;
    let mut newline = false;
    while i < sql.len() {
        match sql[i] {
            b'\n' | b'\r' => newline = true,
            b' ' | b'\t' | b'\x0b' | b'\x0c' => {},
            b'-' if sql.get(i + 1) == Some(&b'-') => {
                while i < sql.len() && !matches!(sql[i], b'\n' | b'\r') {
                    sql[i] = b' ';
                    i += 1;
                }
                continue;
            },
            b'\'' if newline => return Some(i),
            _ => return None,
        }
        i += 1;
    }
    None
}

/// Whether `b` continues a PostgreSQL identifier: a letter, a digit, `_`,
/// `$` or any non-ASCII byte.
fn sql_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// The length of the dollar-quote tag (`$$`, `$name$`) starting `sql`, if one
/// does (a positional parameter such as `$1` is none); a tag's letters may be
/// non-ASCII, as PostgreSQL's identifiers'.
fn dollar_tag(sql: &[u8]) -> Option<usize> {
    let name = sql[1..]
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b >= 0x80)
        .count();
    let starts_ok = name == 0 || !sql[1].is_ascii_digit();
    (starts_ok && sql.get(1 + name) == Some(&b'$')).then_some(name + 2)
}

/// The SQL name at or after `from` in the lowercased SQL `sql`, and the offset
/// after it: whitespace skipped; a qualified name's parts joined with `.`
/// across whitespace around each dot; each part a plain identifier, a `"…"`
/// identifier (unquoted, `""` a quote) or a `U&"…"` one (decoded, with its
/// `UESCAPE` character if it names one). A keyword reads as a one-part name.
fn sql_name(sql: &str, from: usize) -> (String, usize) {
    let skip = |i: usize| i + sql[i..].len() - sql[i..].trim_start().len();
    let mut parts = Vec::new();
    let mut i = skip(from);
    while let Some((part, end)) = name_part(sql, i) {
        parts.push(part);
        i = end;
        let dot = skip(end);
        if !sql[dot..].starts_with('.') {
            break;
        }
        i = skip(dot + 1);
    }
    (parts.join("."), i)
}

/// Whether `sql[from..to]` names `table`: any name in it, read as
/// [`sql_name`] reads one (quoted and `U&"…"` names decoded), whose last part
/// is `table`.
fn names(sql: &str, from: usize, to: usize, table: &str) -> bool {
    let mut i = from;
    while let Some(c) = sql[i..to].chars().next() {
        let starts =
            c == '"' || (is_ident(c) && !sql[..i].chars().next_back().is_some_and(is_ident));
        if !starts {
            i += c.len_utf8();
            continue;
        }
        let (name, end) = sql_name(sql, i);
        if name.rsplit('.').next() == Some(table) {
            return true;
        }
        i = end.clamp(i + c.len_utf8(), to);
    }
    false
}

/// One part of a SQL name at `i` (see [`sql_name`]), and the offset after it.
fn name_part(sql: &str, i: usize) -> Option<(String, usize)> {
    let rest = &sql[i..];
    if let Some(quoted) = rest.strip_prefix("u&\"") {
        let (body, len) = quoted_identifier(quoted)?;
        let end = i + 3 + len;
        let after = end + sql[end..].len() - sql[end..].trim_start().len();
        let (escape, end) = match sql[after..].strip_prefix("uescape") {
            Some(clause) => {
                let q = sql.len() - clause.trim_start().len();
                let c = sql[q..].strip_prefix('\'').and_then(|c| c.chars().next());
                match c {
                    Some(c) if sql[q + 1 + c.len_utf8()..].starts_with('\'') => {
                        (c, q + 2 + c.len_utf8())
                    },
                    _ => ('\\', end),
                }
            },
            None => ('\\', end),
        };
        return Some((unicode_identifier(&body, escape).to_ascii_lowercase(), end));
    }
    if let Some(quoted) = rest.strip_prefix('"') {
        let (body, len) = quoted_identifier(quoted)?;
        return Some((body, i + 1 + len));
    }
    let len = rest.find(|c: char| !(is_ident(c) || c == '$')).unwrap_or(rest.len());
    (len > 0).then(|| (rest[..len].to_string(), i + len))
}

/// A `"…"` identifier's body (`""` a quote) and its length with the closing
/// quote, from just after the opening one.
fn quoted_identifier(s: &str) -> Option<(String, usize)> {
    let (mut body, mut i) = (String::new(), 0);
    loop {
        let q = s[i..].find('"')? + i;
        body.push_str(&s[i..q]);
        if s[q + 1..].starts_with('"') {
            body.push('"');
            i = q + 2;
        } else {
            return Some((body, q + 1));
        }
    }
}

/// A `U&"…"` identifier's body decoded: `<e>XXXX`, `<e>+XXXXXX`, `<e><e>`.
fn unicode_identifier(body: &str, e: char) -> String {
    let mut out = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != e {
            out.push(c);
            continue;
        }
        let digits = match chars.peek() {
            Some(n) if *n == e => {
                chars.next();
                out.push(e);
                continue;
            },
            Some('+') => {
                chars.next();
                6
            },
            _ => 4,
        };
        let hex: String = chars.by_ref().take(digits).collect();
        out.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
    }
    out
}

/// Its newlines only: removed source keeps every later line's number.
fn blank(text: &str) -> String {
    text.chars().filter(|c| *c == '\n').collect()
}

/// Whether a `cfg` predicate holds only under `test`: `test`, an `all(…)`
/// with such an argument, or an `any(…)` whose arguments all are. `not(…)`
/// and features alone are production.
fn requires_test(predicate: &str) -> bool {
    let predicate = predicate.trim();
    let arguments = |inner: &str| {
        let mut parts = Vec::new();
        let (mut depth, mut from) = (0usize, 0);
        for (i, c) in inner.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    parts.push(inner[from..i].to_string());
                    from = i + 1;
                },
                _ => {},
            }
        }
        parts.push(inner[from..].to_string());
        parts.into_iter().filter(|p| !p.trim().is_empty()).collect::<Vec<_>>()
    };
    // `all(…)`, `all (…)`: the name, then its arguments in parentheses.
    let inner = |name: &str| {
        predicate
            .strip_prefix(name)
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('('))
            .and_then(|rest| rest.trim_end().strip_suffix(')'))
            .map(arguments)
    };
    if predicate == "test" {
        true
    } else if let Some(all) = inner("all") {
        all.iter().any(|a| requires_test(a))
    } else if let Some(any) = inner("any") {
        !any.is_empty() && any.iter().all(|a| requires_test(a))
    } else {
        false
    }
}

/// A source's production text, line numbers kept: what an
/// inner `#![cfg(…)]` requiring `test` encloses (the file at its top level,
/// else the block, an inline module's contents, it stands in), and every item
/// (inline module, `mod x;` declaration, function, impl, use) under an outer
/// `#[cfg(…)]` requiring `test` (`test`, `all(test, feature = "…")`) are
/// blanked; the text after each such item is kept, because production handlers
/// follow an inline tests module
/// (`robson-projector/src/handlers/positions.rs`). 8l-4b's `production()`,
/// which truncates at the first tests module, is not reused.
fn without_test_modules(source: &str) -> String {
    // The source as written: comments are masked where structure is read
    // (`code_only`), never removed by line, which could cut a block comment.
    let mut source = source.to_string();
    // Attributes are found and read in code (comments and literals blanked),
    // and the same offsets are removed from the source: `"#![cfg(test)]"` in a
    // string or a comment hides nothing.
    let mut code = code_only(&source);
    while let Some(at) = seq(&code, "#![cfg(", 0, code.len()).into_iter().find(|&at| {
        let open = at + code[at..].find('(').unwrap_or(0);
        requires_test(&code[open + 1..closing(&code, open) - 1])
    }) {
        // The innermost block still open at the attribute is what it gates;
        // the text after that block is production again.
        let mut opens = Vec::new();
        for (i, b) in code[..at].bytes().enumerate() {
            match b {
                b'{' => opens.push(i),
                b'}' => {
                    opens.pop();
                },
                _ => {},
            }
        }
        let Some(&open) = opens.last() else {
            return blank(&source);
        };
        let close = closing(&code, open) - 1;
        source =
            format!("{}{}{}", &source[..=open], blank(&source[open + 1..close]), &source[close..]);
        code = code_only(&source);
    }
    let mut kept = String::with_capacity(source.len());
    let (mut done, mut from) = (0, 0);
    while let Some(at) = seq(&code, "#[cfg(", from, code.len()).first().copied() {
        let open = at + code[at..].find('(').unwrap_or(0);
        let close = closing(&code, open);
        if !requires_test(&code[open + 1..close - 1]) {
            from = close;
            continue;
        }
        // Past the attribute's `]`, further attributes (`#[path = "…"]`), then
        // the item.
        let mut item = closing(&code, at + code[at..].find('[').unwrap_or(0));
        loop {
            let trimmed = code[item..].trim_start();
            item = code.len() - trimmed.len();
            let Some(bracket) = attribute(trimmed) else {
                break;
            };
            item = closing(&code, item + bracket);
        }
        // A gated item is removed to its `;` or the end of its body; a gated
        // statement, block, field or arm is test-only too, removed to its end,
        // so it neither adds a site nor supplies a key.
        let end = if item_keyword(&code[item..]).is_none() {
            statement_end(&code, item)
        } else {
            match item_open(&code, item) {
                Some(brace) if code.as_bytes()[brace] == b'{' => closing(&code, brace),
                Some(semicolon) => semicolon + 1,
                None => code.len(),
            }
        };
        kept.push_str(&source[done..at]);
        kept.push_str(&blank(&source[at..end]));
        (done, from) = (end, end);
    }
    kept.push_str(&source[done..]);
    kept
}

/// The length of the visibility starting `text`, read as tokens: `pub`,
/// with its restriction if one follows (`pub(crate)`, `pub (super)`,
/// `pub(self)`, `pub(in a::b)`); 0 without one.
fn visibility(text: &str) -> usize {
    let word =
        |t: &str, w: &str| t.strip_prefix(w).is_some_and(|r| !r.starts_with(|c: char| is_ident(c)));
    if !word(text, "pub") {
        return 0;
    }
    let rest = text[3..].trim_start();
    let restricted = rest
        .strip_prefix('(')
        .map(str::trim_start)
        .is_some_and(|inner| ["crate", "self", "super", "in"].iter().any(|w| word(inner, w)));
    if restricted {
        closing(text, text.len() - rest.len())
    } else {
        3
    }
}

/// `head` without the visibility ending it (`pub`, `pub (crate)`,
/// `pub(in a::b)`, read back as tokens), trimmed.
fn without_visibility(head: &str) -> &str {
    let head = head.trim_end();
    let before = match head.strip_suffix(')') {
        Some(_) => {
            let open = opening(head, head.len() - 1);
            let inner = head[open + 1..head.len() - 1].trim_start();
            let word = |w: &str| {
                inner.strip_prefix(w).is_some_and(|r| !r.starts_with(|c: char| is_ident(c)))
            };
            if !["crate", "self", "super", "in"].iter().any(|w| word(w)) {
                return head;
            }
            head[..open].trim_end()
        },
        None => head,
    };
    match before.strip_suffix("pub") {
        Some(rest) if !rest.ends_with(|c: char| is_ident(c)) => rest.trim_end(),
        _ => head,
    }
}

/// The keyword of the item `text` starts with, past its visibility and its
/// qualifiers (`default`, `async`, `unsafe`, `safe`), read as tokens; none
/// for a statement, block, field or arm.
fn item_keyword(text: &str) -> Option<&'static str> {
    let text = text.trim_start();
    let mut rest = text[visibility(text)..].trim_start();
    loop {
        let word = &rest[..rest.find(|c: char| !is_ident(c)).unwrap_or(rest.len())];
        if let Some(keyword) = ITEM_KEYWORDS.iter().find(|k| **k == word) {
            return Some(*keyword);
        }
        if word.is_empty() || !["default", "async", "unsafe", "safe"].contains(&word) {
            return None;
        }
        rest = rest[word.len()..].trim_start();
    }
}

/// The keywords that start an item (`extern` a block, a crate or a
/// function; `const` an item or a function).
const ITEM_KEYWORDS: [&str; 14] = [
    "mod",
    "fn",
    "impl",
    "use",
    "const",
    "static",
    "struct",
    "enum",
    "trait",
    "type",
    "union",
    "extern",
    "macro_rules",
    "auto",
];

/// The end of the gated statement, block, field or arm starting at `item`.
fn statement_end(code: &str, item: usize) -> usize {
    if code[item..].starts_with('{') {
        closing(code, item)
    } else {
        item_end(code, item)
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Word-bounded occurrences of `name` in `text[from..to]`.
fn uses(text: &str, name: &str, from: usize, to: usize) -> Vec<usize> {
    text[from..to]
        .match_indices(name)
        .map(|(i, _)| from + i)
        .filter(|&i| {
            !text[..i].chars().next_back().is_some_and(is_ident)
                && !text[i + name.len()..].chars().next().is_some_and(is_ident)
        })
        .collect()
}

/// The first `{` or `;` in `text` from `from` outside square brackets: an
/// item's body or its end (an array type's `;`, `-> [u8; 2]`, ends neither).
fn item_open(text: &str, from: usize) -> Option<usize> {
    let mut depth = 0usize;
    text[from..].char_indices().find_map(|(i, c)| {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            '{' | ';' if depth == 0 => return Some(from + i),
            _ => {},
        }
        None
    })
}

/// Every function in `text`: `(name, start of "fn", end of body, public)`.
fn functions(text: &str) -> Vec<(String, usize, usize, bool)> {
    let mut found = Vec::new();
    for at in uses(text, "fn", 0, text.len()) {
        let rest = text[at + 2..].trim_start();
        let rest = rest.strip_prefix("r#").unwrap_or(rest);
        let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
        if name.is_empty() {
            continue;
        }
        let Some(open) = item_open(text, at) else {
            continue;
        };
        if text.as_bytes()[open] == b';' {
            continue;
        }
        let line = text[..at].rfind('\n').map_or(0, |n| n + 1);
        found.push((name, at, closing(text, open), text[line..at].contains("pub")));
    }
    found
}

/// The production module tree from a crate root, as rustc resolves it: each
/// `mod name;` not under a `cfg` requiring `test`, from its module's
/// directory (a `mod.rs`-like file's own directory, for a crate root, a
/// `mod.rs` or a file a `#[path]` named, whose modules are its siblings;
/// else `stem/` beside the file), each inline `mod m { … }` around it adding
/// `m/` (or, with `#[path = "p"]`, being `p` from the directory around it);
/// `#[path = "…"]` from that directory (outside inline modules, the file's
/// own one), and the file a `cfg_attr(…, path = "…")` not requiring `test`
/// picks, read besides the default. A path attribute it cannot read, or a
/// `cfg_attr` path on an inline module, fails. Each file with the file
/// declaring it.
fn production_files(root: &Path) -> BTreeMap<PathBuf, Option<PathBuf>> {
    let mut files = BTreeMap::from([(root.to_path_buf(), None)]);
    let mut mod_rs = BTreeSet::from([root.to_path_buf()]);
    let mut pending = vec![root.to_path_buf()];
    let mut seen = BTreeSet::new();
    while let Some(file) = pending.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        let source = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{file:?}: {e}"));
        let source = without_test_modules(&source);
        let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
        let dir = file.parent().unwrap();
        // The component a file that is not `mod.rs`-like adds for its
        // modules (`a/b.rs`: `a/b/`), which a `#[path]` outside inline
        // modules does not see.
        let relative = (!mod_rs.contains(&file) && stem != "mod").then_some(stem);
        // Declarations are read as tokens of code (comments and literals
        // blanked): `mod x;` with any trailing comment, attributes on the same
        // line, never one inside a string; so are their attributes, whose
        // path literals are read from the source at the same offsets
        // ([`module_paths`]).
        let code = code_only(&source);
        // `include!` inserts Rust source this walk would not read: none may
        // stand in production code.
        if let Some(at) = uses(&code, "include", 0, code.len())
            .into_iter()
            .find(|&at| code[at + "include".len()..].trim_start().starts_with('!'))
        {
            panic!(
                "{file:?}: `include!` at byte {at} brings in Rust source the guard does not walk"
            );
        }
        let paths = |at: usize| {
            module_attributes(&code, &source, at).unwrap_or_else(|e| panic!("{file:?}: {e}"))
        };
        // Inline `mod x { … }` spans, with the `#[path]` that moves one.
        let mut inline = Vec::new();
        let mut declared = Vec::new();
        for at in uses(&code, "mod", 0, code.len()) {
            let rest = code[at + 3..].trim_start();
            // A raw identifier (`r#name`) names the module `name`.
            let ident = rest.strip_prefix("r#").unwrap_or(rest);
            let name: String = ident.chars().take_while(|c| is_ident(*c)).collect();
            let after = ident[name.len()..].trim_start();
            if name.is_empty() {
                continue;
            }
            if after.starts_with('{') {
                let open = code.len() - after.len();
                let (moved, also) = paths(at);
                assert!(also.is_empty(), "{file:?}: a `cfg_attr` path on inline `mod {name}`");
                inline.push((name, moved, open, closing(&code, open)));
            } else if after.starts_with(';') {
                declared.push((at, name));
            }
        }
        for (at, name) in declared {
            let (path, also) = paths(at);
            // The declaring module's directory, through the inline modules
            // around it, outermost first: `relative` is added by the first.
            let (mut base, mut relative) = (dir.to_path_buf(), relative.clone());
            for (module, moved, ..) in
                inline.iter().filter(|(_, _, open, close)| *open < at && at < *close)
            {
                if let Some(moved) = moved {
                    base.push(moved);
                } else {
                    base.extend(relative.as_deref());
                    base.push(module);
                }
                relative = None;
            }
            let own = relative.map_or(base.clone(), |r| base.join(r));
            let default = match own.join(format!("{name}.rs")) {
                rs if rs.exists() => rs,
                _ => own.join(&name).join("mod.rs"),
            };
            // A `#[path]` file reads as `mod.rs`; a `cfg_attr` path is the
            // module's file under its predicate, read besides the default
            // (each that exists, one at least).
            let mut children = vec![match path {
                Some(path) => (base.join(path), true),
                None => (default, false),
            }];
            children.extend(also.iter().map(|p| (base.join(p), true)));
            if children.len() > 1 && children.iter().any(|(c, _)| c.exists()) {
                children.retain(|(c, _)| c.exists());
            }
            for (child, path_file) in children {
                if path_file {
                    mod_rs.insert(child.clone());
                }
                files.entry(child.clone()).or_insert_with(|| Some(file.clone()));
                pending.push(child);
            }
        }
    }
    files
}

/// The path attributes of the `mod` keyword at `at` ([`module_paths`] for
/// each attribute before it, past its visibility, the first plain `path`
/// winning as in rustc).
fn module_attributes(
    code: &str,
    source: &str,
    at: usize,
) -> Result<(Option<String>, Vec<String>), String> {
    let mut head = without_visibility(&code[..at]);
    let (mut path, mut also) = (None, Vec::new());
    while head.ends_with(']') {
        let end = head.len();
        // The attribute's `[` (after `#`, across whitespace).
        let Some(bracket) = head
            .rmatch_indices('[')
            .map(|(i, _)| i)
            .find(|&i| closing(code, i) == end && code[..i].trim_end().ends_with('#'))
        else {
            break;
        };
        let (plain, more) = module_paths(code, source, bracket + 1, end - 1)?;
        if plain.is_some() {
            path = plain;
        }
        also.extend(more);
        head = code[..code[..bracket].trim_end().len() - 1].trim_end();
    }
    Ok((path, also))
}

/// The module files one attribute of a `mod` declaration names, its contents
/// at `code[from..to]` read as tokens (comments masked) and its literals from
/// `source` at the same offsets: `path = "…"` (the module's file), or each
/// `path = "…"` inside a `cfg_attr(…)` whose predicate does not require
/// `test` (a file to read besides the default); an ordinary literal decoded
/// as Rust decodes it ([`unescaped`]), a raw one kept. A `path` not followed
/// by `=` and a string literal is an error: the guard fails rather than read
/// the default file in its place.
fn module_paths(
    code: &str,
    source: &str,
    from: usize,
    to: usize,
) -> Result<(Option<String>, Vec<String>), String> {
    let skip = |q: usize| q + code[q..to].len() - code[q..to].trim_start().len();
    let unread = || format!("unread path attribute `#[{}]`", source[from..to].trim());
    let literal = |at: usize| -> Result<String, String> {
        let q = skip(at + "path".len());
        if !code[q..to].starts_with('=') {
            return Err(unread());
        }
        let r = skip(q + 1);
        let quote = r + source[r..to]
            .strip_prefix('r')
            .map_or(0, |h| 1 + h.len() - h.trim_start_matches('#').len());
        if !source[quote..to].starts_with('"') {
            return Err(unread());
        }
        let end = after_string(source, quote);
        let path = &source[quote + 1..end - 1 - (quote - r).saturating_sub(1)];
        Ok(if quote == r {
            unescaped(path)
        } else {
            path.to_string()
        })
    };
    let head = skip(from);
    let word = |w: &str| {
        code[head..to]
            .strip_prefix(w)
            .is_some_and(|rest| !rest.starts_with(|c: char| is_ident(c)))
    };
    if word("path") {
        return Ok((Some(literal(head)?), Vec::new()));
    }
    if word("cfg_attr") {
        let open = skip(head + "cfg_attr".len());
        if !code[open..to].starts_with('(') {
            return Err(unread());
        }
        let close = closing(code, open) - 1;
        let predicate = top_level(code, open + 1, close, ",").first().copied().unwrap_or(close);
        if requires_test(&code[open + 1..predicate]) {
            return Ok((None, Vec::new()));
        }
        let also: Result<Vec<String>, String> =
            uses(code, "path", predicate, close).into_iter().map(literal).collect();
        return also.map(|also| (None, also));
    }
    Ok((None, Vec::new()))
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// One source as a production tree, with its own tables: its open sites and
/// table problems.
fn check(source: &str, wrappers: &[Entry], keying: &[Entry], delegated: &[Entry]) -> Vec<String> {
    Guard::new(
        vec![("inline.rs".into(), None, source.into(), None)],
        wrappers,
        keying,
        delegated,
    )
    .report()
    .1
}

/// The production module trees of the four crates: each file, relative to
/// the workspace, with its directory, text and declaring file.
fn production_sources() -> Vec<(String, Option<PathBuf>, String, Option<String>)> {
    let ws = workspace();
    let rel = |file: &Path| file.strip_prefix(&ws).unwrap_or(file).display().to_string();
    [
        "robsond/src/main.rs",
        "robsond/src/lib.rs",
        "robson-projector/src/lib.rs",
        "robson-eventlog/src/lib.rs",
        "robson-db/src/lib.rs",
    ]
    .iter()
    .flat_map(|root| production_files(&ws.join(root)))
    .collect::<BTreeMap<_, _>>()
    .into_iter()
    .map(|(file, parent)| {
        let text = std::fs::read_to_string(&file).unwrap();
        (rel(&file), file.parent().map(Path::to_path_buf), text, parent.map(|p| rel(&p)))
    })
    .collect()
}

fn unguarded(source: &str) -> Vec<String> {
    check(source, &[], &[], &[])
}

/// Killer of "a new unguarded production writer, or a test-only file counted
/// as production": every input write, `accounting_state` row lock, included
/// SQL file writing an input and event append in the production module trees
/// runs after the key in its transaction, directly, through a reviewed
/// wrapper or keying function, or in a reviewed delegated writer every call
/// of which is covered; every table entry is verified and covers a site; and
/// the inventory is the reviewed one (a writer added or a detection lost
/// changes it).
#[test]
fn every_production_input_writer_participates() {
    assert_eq!(robson_db::CERTIFICATE_PARTICIPATE, robson_eventlog::certificate::PARTICIPATE);
    let ws = workspace();
    let sources = production_sources();
    let canonical: BTreeSet<PathBuf> = sources
        .iter()
        .map(|(rel, ..)| ws.join(rel).canonicalize().unwrap_or_else(|_| ws.join(rel)))
        .collect();
    for test_only in [
        "robsond/src/order_journal_transition_positive.rs",
        "robsond/src/order_journal_transition_prepared.rs",
        "robsond/src/order_journal_transition_r1r3.rs",
        "robsond/src/spot_order_set_store/cold_install.rs",
        "robsond/src/balance_accounting_pg_tests.rs",
        "robsond/src/certificate_participation_pg_tests.rs",
        "robsond/src/certificate_projector_pg_tests.rs",
        // `#[cfg(all(test, feature = "…"))]` declarations.
        "robsond/tests/bounded_migration/mod.rs",
        "robsond/tests/bounded_migration/migration46_constraints.rs",
        "robsond/src/daemon/boundary_tests.rs",
        "robsond/src/daemon_non_trading_pg_tests.rs",
        "robsond/src/venue_budget/n5_claim/commission_tests.rs",
        "robsond/src/venue_clock_owner/ps3_pg_tests.rs",
    ] {
        let path = ws.join(test_only);
        let path = path.canonicalize().unwrap_or(path);
        assert!(!canonical.contains(&path), "{test_only} counted as production");
    }
    for production in [
        "robson-projector/src/handlers/positions.rs",
        "robson-projector/src/handlers/monthly_state.rs",
        "robson-eventlog/src/append.rs",
        "robson-eventlog/src/certificate.rs",
        "robson-db/src/init.rs",
        "robsond/src/process_epoch.rs",
        "robsond/src/order_journal_store.rs",
        "robsond/src/balance_accounting.rs",
        "robsond/src/venue_ledger_store/cash/writer.rs",
    ] {
        let path = ws.join(production);
        assert!(canonical.contains(&path.canonicalize().unwrap()), "{production} not walked");
    }
    let guard = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED);
    let (sites, open) = guard.report();
    assert!(
        open.is_empty(),
        "unguarded input writers or table problems:\n{}",
        open.join("\n")
    );
    // The projector's 20 `positions_current` and 3 `monthly_state` writes; the
    // manager's month-peak upsert; 4 `append_event` calls, the boundary's and
    // the append's own `append_event_tx` calls and the event log's insert
    // behind them; the seed's insert; 7 journal-store writes, the
    // transition's cancel insert and the balance journal's bump; 4 epoch
    // writes; 3 suspension writes; the saga upsert; the ledger's
    // accounting-row insert and 6 bumps; the balance context's 3
    // `accounting_state` statements; 3 cash statements and the re-anchor's
    // included SQL.
    assert_eq!(sites, 63, "the reviewed inventory of input write sites changed");
    // Every SQL file in the four crates that writes an input is one of those
    // sites: reached from production by a literal include.
    let roots = [
        "robsond/src",
        "robson-projector/src",
        "robson-eventlog/src",
        "robson-db/src",
    ];
    let orphans = guard.orphan_sql(&roots.map(|r| ws.join(r)));
    assert!(orphans.is_empty(), "{orphans:?}");
}

/// Killer of "a writer after an inline tests module skipped" and of "a
/// `cfg(all(test, …))` item counted": a write after `#[cfg(test)] mod tests
/// { … }` is still read and fails the guard; one inside it, or under
/// `cfg(all(test, feature))`, is not; `cfg(not(test))` and `any(test, x)`
/// are production; a `#[cfg(test)] mod x;` declaration is not walked; an
/// inner `#![cfg(test)]` in a nested module gates that module only; a `//`
/// inside a block comment cuts nothing; line numbers survive the removal.
#[test]
fn the_guard_reads_past_inline_test_modules() {
    let source = r#"
async fn before(c: &mut PgConnection) { participate(c).await?; sqlx::query("UPDATE positions_current SET x = 1").execute(c).await; }
#[cfg(test)]
mod tests {
    fn inside() { sqlx::query("UPDATE positions_current SET x = 1"); { } }
}
#[cfg(all(test, feature = "postgres"))]
#[path = "x_tests.rs"]
mod x;
#[cfg(all(feature = "postgres", test))]
async fn fixture(pool: &PgPool) { sqlx::query("UPDATE positions_current SET x = 3").execute(pool).await; }
async fn after(pool: &PgPool) { sqlx::query("UPDATE positions_current SET x = 2").execute(pool).await; }
"#;
    let kept = without_test_modules(source);
    assert!(!kept.contains("fn inside") && !kept.contains("fn fixture"), "{kept}");
    assert!(!kept.contains("mod x;"), "{kept}");
    assert!(kept.contains("fn after"), "{kept}");
    assert_eq!(kept.lines().count(), source.lines().count());
    let open = unguarded(source);
    assert_eq!(open.len(), 1, "{open:?}");
    assert!(open[0].starts_with("inline.rs:12: UPDATE positions_current"), "{open:?}");
    assert!(requires_test("all(test, feature = \"crash-points\")"));
    assert!(!requires_test("not(test)") && !requires_test("any(test, feature = \"x\")"));
    assert!(!requires_test("feature = \"postgres\""));
    let declared = "#[cfg(test)]\nmod hidden;\nmod shown;\n";
    let kept = without_test_modules(declared);
    assert!(!kept.contains("hidden") && kept.contains("mod shown;"), "{kept}");
    let whole = "//! doc\n#![cfg(all(test, feature = \"postgres\"))]\nfn w() { sqlx::query(\"INSERT INTO event_log VALUES (1)\"); }\n";
    assert!(unguarded(whole).is_empty());
    // A `//` that a block comment holds ends nothing: the writer after the
    // comment's real end is read.
    let cut = "/*\n// */\nasync fn writer(pool: &PgPool) { \
               sqlx::query(\"UPDATE positions_current SET last_seq = last_seq\").execute(pool).await; }\n";
    let open = unguarded(cut);
    assert!(open.len() == 1 && open[0].contains(":3: UPDATE positions_current"), "{open:?}");
    // A gated item is read past its visibility (restricted, spaced) and its
    // qualifiers, as tokens: removed to its own end, the writer after it kept.
    let restricted = "#[cfg(test)]\npub(super) mod tests {}\n\
                      #[cfg(test)]\npub (in crate::x) async unsafe fn t() {}\n\
                      async fn writer(pool: &PgPool) { \
                      sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\").execute(pool).await.unwrap(); }\n";
    let open = unguarded(restricted);
    assert!(open.len() == 1 && open[0].contains(":5: UPDATE monthly_state"), "{open:?}");
    // A gated block statement's keyword is a token (alone on its line, after
    // a label): the statement ends with its block, as a brace-delimited macro
    // statement does, and `else` is a word, so each writer after one is read
    // at its own line.
    let blocks = "async fn w(pool: &PgPool) {\n\
                  #[cfg(test)]\n\
                  if\n\
                  true {}\n\
                  sqlx::query(\"UPDATE monthly_state SET trades_opened = 1\").execute(pool).await;\n\
                  #[cfg(test)]\n\
                  match\n\
                  x { _ => {} }\n\
                  sqlx::query(\"UPDATE positions_current SET x = 1\").execute(pool).await;\n\
                  #[cfg(test)]\n\
                  'outer: loop { break 'outer; }\n\
                  sqlx::query(\"DELETE FROM order_intents\").execute(pool).await;\n\
                  #[cfg(test)]\n\
                  if t {}\n\
                  elsewhere.push(sqlx::query(\"UPDATE cash_anchors SET x = 1\").execute(pool).await);\n\
                  #[cfg(test)]\n\
                  println! { \"test only\" }\n\
                  sqlx::query(\"UPDATE event_log SET x = 1\").execute(pool).await;\n\
                  }\n";
    let open = unguarded(blocks);
    assert_eq!(open.len(), 5, "{open:?}");
    for (line, what) in [
        (5, "UPDATE monthly_state"),
        (9, "UPDATE positions_current"),
        (12, "DELETE order_intents"),
        (15, "UPDATE cash_anchors"),
        (18, "UPDATE event_log"),
    ] {
        assert!(open.iter().any(|p| p.contains(&format!(":{line}: {what}"))), "{what}: {open:?}");
    }
    // A gated match arm ends with its body when that is a block, an `if …
    // else …`, a `const { … }` block or another block statement (no `,`
    // needed): the next arm's writer is read.
    let arms = "async fn arms(pool: &PgPool, k: u8) {\n\
                match k {\n\
                #[cfg(test)]\n\
                0 => {}\n\
                1 => { sqlx::query(\"UPDATE funding_sagas SET x = 1\").execute(pool).await; }\n\
                #[cfg(test)]\n\
                2 => if k > 0 {} else {}\n\
                3 => { sqlx::query(\"DELETE FROM cash_anchors\").execute(pool).await; }\n\
                #[cfg(test)]\n\
                4 => const { () }\n\
                _ => { sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\").execute(pool).await; }\n\
                }\n\
                }\n";
    let open = unguarded(arms);
    assert_eq!(open.len(), 3, "{open:?}");
    for at in [
        ":5: UPDATE funding_sagas",
        ":8: DELETE cash_anchors",
        ":11: UPDATE monthly_state",
    ] {
        assert!(open.iter().any(|p| p.contains(at)), "{at}: {open:?}");
    }
    // Attributes and predicates are read as tokens: a spaced `# [ cfg ( test ) ]`
    // key is test-only, so the write after it is open; a spaced inner
    // attribute gates its file.
    let spaced = "async fn w(c: &mut PgConnection) { # [ cfg ( test ) ] participate(c).await?; \
                  sqlx::query(\"UPDATE monthly_state SET x = 1\").execute(c).await; }\n";
    assert_eq!(unguarded(spaced).len(), 1, "{:?}", unguarded(spaced));
    // A spaced outer attribute gates its item whole: a test-only function's
    // write is no site.
    let spaced_item = "# [ cfg ( test ) ]\n\
                       fn t(c: &mut PgConnection) { sqlx::query(\"UPDATE positions_current SET x = 1\").execute(c); }\n";
    assert!(unguarded(spaced_item).is_empty(), "{:?}", unguarded(spaced_item));
    let inner = "#! [ cfg ( all ( test , feature = \"x\" ) ) ]\n\
                 fn w() { sqlx::query(\"INSERT INTO event_log VALUES (1)\"); }\n";
    assert!(unguarded(inner).is_empty(), "{:?}", unguarded(inner));
    assert!(requires_test("all (test, feature = \"x\")") && requires_test("any ( test )"));
    // An inner attribute in a nested module gates that module, not the file.
    let nested = "mod tests {\n    #![cfg(test)]\n    \
                  fn t() { sqlx::query(\"UPDATE monthly_state SET x = 1\"); }\n}\n\
                  async fn write(pool: &PgPool) { \
                  sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\").execute(pool).await; }\n";
    let kept = without_test_modules(nested);
    assert!(!kept.contains("fn t()") && kept.contains("fn write"), "{kept}");
    assert_eq!(kept.lines().count(), nested.lines().count());
    let open = unguarded(nested);
    assert_eq!(open.len(), 1, "{open:?}");
    assert!(open[0].contains(":5: UPDATE monthly_state"), "{open:?}");
    let keyed = "async fn w(tx: &mut PgConnection) { participate(tx).await?; \
                 sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(tx).await; }";
    assert!(unguarded(keyed).is_empty());
    // The key after the write does not cover it.
    let late =
        "async fn w(tx: &mut PgConnection) { sqlx::query(\"INSERT INTO event_log VALUES (1)\")\
                .execute(&mut *tx).await; participate(tx).await?; }";
    assert_eq!(unguarded(late).len(), 1);
    // A cfg attribute is read in code: one in a string or a comment hides
    // nothing.
    for decoy in [
        "const HELP: &str = \"#![cfg(test)]\";",
        "/* #![cfg(test)] */",
    ] {
        let source = format!(
            "{decoy}\nasync fn w(c: &mut PgConnection) {{ \
             sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(c).await; }}\n"
        );
        assert_eq!(unguarded(&source).len(), 1, "{decoy}: {:?}", unguarded(&source));
    }
}

/// Killers of "SQL formatted, bound or held in a `const` escapes the guard"
/// and of "a wrapper covers later code": the SQL's coverage is each use's; a
/// use handed to a wrapper passes, a use run raw fails; an unused binding or
/// `const` fails; an import is not a use; a shadowing `let` ends a binding;
/// an autocommit write after a wrapper call fails; mixed case, `public.` and
/// line breaks are seen.
#[test]
fn the_guard_follows_sql_into_bindings_and_wrappers() {
    let w = [entry(
        "inline.rs",
        "execute_participating",
        Reach::Module,
        Form::Free,
    )];
    let wrapper = "async fn execute_participating(pool: &PgPool, q: Query) { \
                   let mut tx = pool.begin().await; participate(&mut tx).await?; \
                   q.execute(&mut *tx).await; tx.commit().await; }\n";
    let formatted = format!(
        "{wrapper}async fn f(pool: &PgPool) {{ let statement = format!(\"insert into\n  \
         public.monthly_state VALUES (1)\"); execute_participating(pool, sqlx::query(&statement)).await; }}\n"
    );
    assert!(
        check(&formatted, &w, &[], &[]).is_empty(),
        "{:?}",
        check(&formatted, &w, &[], &[])
    );
    let raw = formatted.replace(
        "execute_participating(pool, sqlx::query(&statement))",
        "sqlx::query(&statement).execute(pool)",
    );
    let open = check(&raw, &w, &[], &[]);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(open[0].contains("INSERT monthly_state via statement (in f(), no key before it)"));
    assert!(open[1].contains("covers no site (stale entry)"), "{open:?}");
    let unused = format!(
        "{wrapper}fn g() {{ let s = \"UPDATE order_intents SET x = 1\"; }}\n\
         async fn h(pool: &PgPool) {{ execute_participating(pool, sqlx::query(\"UPDATE order_intents SET y = 1\")).await; }}\n"
    );
    let open = check(&unused, &w, &[], &[]);
    assert!(open.len() == 1 && open[0].contains("(let s never used)"), "{open:?}");
    // `let mut` and a line break bind the name after them: its use after the
    // COMMIT is followed, not a keyed `&mut` read as the binding `mut`.
    let split = "async fn m(pool: &PgPool) { let mut tx = pool.begin().await?; \
                 participate(&mut tx).await?; let mut\n    q = sqlx::query(\"UPDATE monthly_state SET x = 1\"); \
                 sqlx::query(\"SELECT 1\").execute(&mut *tx).await?; tx.commit().await?; \
                 q.execute(pool).await?; }\n";
    let open = unguarded(split);
    assert!(
        open.len() == 1
            && open[0].contains("UPDATE monthly_state via q (in m(), no key before it)"),
        "{open:?}"
    );
    let constant = format!(
        "{wrapper}const CLEAR: &str = \"DELETE FROM stream_suspensions\";\n\
         use super::CLEAR;\n\
         async fn h(pool: &PgPool) {{ execute_participating(pool, sqlx::query(CLEAR)).await; }}\n\
         async fn i(pool: &PgPool) {{ sqlx::query(CLEAR).execute(pool).await; }}\n\
         const LOST: &str = \"INSERT INTO event_log VALUES (1)\";\n"
    );
    let open = check(&constant, &w, &[], &[]);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open[0].starts_with("inline.rs:5: DELETE stream_suspensions via CLEAR"),
        "{open:?}"
    );
    assert!(open[1].contains("(const LOST never used)"), "{open:?}");
    let shadowed = format!(
        "{wrapper}async fn k(pool: &PgPool) {{ let q = sqlx::query(\"UPDATE process_epochs SET x = 1\"); \
         execute_participating(pool, q).await; let q = 5; drop(q); }}\n"
    );
    assert!(
        check(&shadowed, &w, &[], &[]).is_empty(),
        "{:?}",
        check(&shadowed, &w, &[], &[])
    );
    // A shadow in a nested block hides the binding only in that block: the
    // outer `q` run after the COMMIT is still followed (`k2()`); a use in the
    // shadow's own initializer is the outer binding's (`k3()`).
    let nested_shadow = "async fn k2(pool: &PgPool) { let mut tx = pool.begin().await?; \
                         participate(&mut tx).await?; \
                         let q = sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\"); \
                         std::hint::black_box(&q); { let q = 0_u8; std::hint::black_box(q); } \
                         tx.commit().await?; q.execute(pool).await?; }\n\
                         async fn k3(pool: &PgPool) { let mut tx = pool.begin().await?; \
                         participate(&mut tx).await?; \
                         let q = sqlx::query(\"UPDATE cash_anchors SET x = 1\"); \
                         { let q = q.bind(1); tx.commit().await?; q.execute(pool).await?; } }\n";
    let open = check(nested_shadow, &[], &[], &[]);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open[0].contains("UPDATE monthly_state via q (in k2(), no key before it)"),
        "{open:?}"
    );
    assert!(
        open[1].contains("UPDATE cash_anchors via q via q (in k3(), no key before it)"),
        "{open:?}"
    );
    // A binding in a condition shadows nothing outside its branch: the outer
    // `q` run after the COMMIT is still followed (`k4()`).
    let conditional = "async fn k4(pool: &PgPool, maybe: Option<u8>) { \
                       let mut tx = pool.begin().await?; participate(&mut tx).await?; \
                       let q = sqlx::query(\"UPDATE process_epochs SET x = 1\"); \
                       if let q @ Some(_) = maybe { std::hint::black_box(q); } \
                       tx.commit().await?; q.execute(pool).await?; }\n";
    let open = check(conditional, &[], &[], &[]);
    assert!(
        open.len() == 1
            && open[0].contains("UPDATE process_epochs via q (in k4(), no key before it)"),
        "{open:?}"
    );
    // A query that leaves its statement unbound as a future is open
    // (`s1()`: spawned, it may run after the COMMIT); one whose future is
    // awaited there through a call that polls it runs there (`s2()`, a
    // deadline; `s5()`, a pin). An awaited call or method that may return
    // the future unpolled runs nothing there: its binding is followed
    // (`s3()`, `std::future::ready(…)`; `s4()`, an unreviewed method), and
    // awaited after the COMMIT it is open.
    let escaped = "async fn s1(pool: &'static PgPool) { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; \
                   tokio::spawn(sqlx::query(\"UPDATE order_intents SET x = 1\").execute(pool)); \
                   tx.commit().await?; }\n\
                   async fn s2(pool: &PgPool, d: Duration) { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; \
                   tokio::time::timeout(d, sqlx::query(\"UPDATE recovery_incidents SET x = 1\")\
                   .execute(&mut *tx)).await??; tx.commit().await?; }\n\
                   async fn s3(pool: &PgPool) { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; \
                   let pending = std::future::ready(sqlx::query(\"UPDATE funding_sagas SET x = 1\")\
                   .execute(pool)).await; tx.commit().await?; pending.await?; }\n\
                   async fn s4(pool: &PgPool) { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; \
                   let later = sqlx::query(\"UPDATE stream_suspensions SET x = 1\")\
                   .execute(pool).later().await; tx.commit().await?; later.await?; }\n\
                   async fn s5(pool: &PgPool) { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; \
                   Box::pin(sqlx::query(\"UPDATE process_epochs SET x = 1\").execute(&mut *tx)).await?; \
                   tx.commit().await?; }\n";
    let open = check(escaped, &[], &[], &[]);
    assert_eq!(open.len(), 3, "{open:?}");
    for what in [
        "UPDATE order_intents (in s1(), not run where it is built)",
        "UPDATE funding_sagas via pending (in s3(), no key before it)",
        "UPDATE stream_suspensions via later (in s4(), no key before it)",
    ] {
        assert!(open.iter().any(|p| p.contains(what)), "{what}: {open:?}");
    }
    // A braceless closure's body runs when it is called: bound, where its
    // binding is called (`c1()` after the COMMIT, open; `c2()` before it,
    // covered); passed on, it must take the key itself (`c3()`, open); a
    // `||` operator is no closure (`c4()`, covered).
    let closures = "async fn c1(pool: &PgPool) { let mut tx = pool.begin().await?; \
                    let _key = participate(&mut tx).await?; \
                    let run = async || sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\")\
                    .execute(pool).await; tx.commit().await?; run().await?; }\n\
                    async fn c2(pool: &PgPool) { let mut tx = pool.begin().await?; \
                    participate(&mut tx).await?; \
                    let run = async || sqlx::query(\"UPDATE cash_anchors SET x = 1\")\
                    .execute(&mut *tx).await; run().await?; tx.commit().await?; }\n\
                    async fn c3(pool: &PgPool) { let mut tx = pool.begin().await?; \
                    participate(&mut tx).await?; \
                    run_now(async || sqlx::query(\"UPDATE funding_sagas SET x = 1\").execute(pool).await)\
                    .await?; tx.commit().await?; }\n\
                    async fn c4(pool: &PgPool, ready: bool) { let mut tx = pool.begin().await?; \
                    participate(&mut tx).await?; \
                    let ok = ready || sqlx::query(\"UPDATE event_log SET x = 1\")\
                    .execute(&mut *tx).await.is_ok(); tx.commit().await?; keep(&ok); }\n";
    let open = check(closures, &[], &[], &[]);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open[0].contains("UPDATE monthly_state via run (in c1(), no key before it)"),
        "{open:?}"
    );
    assert!(
        open[1].contains(
            "UPDATE funding_sagas (in c3(), in an async block or closure that takes no key before it)"
        ),
        "{open:?}"
    );
    // A query built under the key but run after the COMMIT through its
    // binding is followed there, whatever else its `let` awaits (a bind
    // value); one run in its own statement is checked there, and its result
    // may outlive the COMMIT; one handed to an awaited call that is no
    // executor (it may run it now) must also follow the key there.
    let built = "async fn g(pool: &PgPool) { let mut tx = pool.begin().await?; \
                 let _key = participate(&mut tx).await?; \
                 let q = sqlx::query(\"UPDATE monthly_state SET trades_opened = $1\")\
                 .bind(std::future::ready(0_i32).await); \
                 tx.commit().await?; q.execute(pool).await?; }\n\
                 async fn r(pool: &PgPool) { let mut tx = pool.begin().await?; \
                 participate(&mut tx).await?; \
                 let r = sqlx::query(\"UPDATE monthly_state SET x = 1\")\
                 .execute(&mut *tx).instrument(span()).await?; \
                 tx.commit().await?; keep(r); }\n\
                 async fn h(pool: &PgPool) { let mut tx = pool.begin().await?; \
                 let r = run(sqlx::query(\"UPDATE monthly_state SET y = 1\")).await?; \
                 participate(&mut tx).await?; keep(&r); }\n";
    let open = check(built, &[], &[], &[]);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open[0].contains("UPDATE monthly_state via q (in g(), no key before it)"),
        "{open:?}"
    );
    assert!(open[1].contains("UPDATE monthly_state (in h(), no key before it)"), "{open:?}");
    // A value that leaves its statement is followed from the `let` around
    // it: the tail of a block (`let q = { sqlx::query(…) };`, run after the
    // COMMIT through `q`), or SQL in an `async` block, run where it is
    // awaited (after the COMMIT in `v()`, before it in `w()`) unless the
    // block takes the key itself before the SQL (`x()`).
    let held = "async fn u(pool: &PgPool) { let mut tx = pool.begin().await?; \
                let _key = participate(&mut tx).await?; \
                let q = { sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\") }; \
                tx.commit().await?; q.execute(pool).await?; }\n\
                async fn v(pool: &PgPool) { let mut tx = pool.begin().await?; \
                participate(&mut tx).await?; \
                let f = async { sqlx::query(\"UPDATE cash_anchors SET x = 1\").execute(pool).await }; \
                tx.commit().await?; f.await?; }\n\
                async fn w(pool: &PgPool) { let mut tx = pool.begin().await?; \
                participate(&mut tx).await?; \
                let f = async { sqlx::query(\"UPDATE process_epochs SET x = 1\")\
                .execute(&mut *tx).await }; f.await?; tx.commit().await?; }\n\
                async fn x(pool: &PgPool) { let f = async { let mut tx = pool.begin().await?; \
                participate(&mut tx).await?; \
                sqlx::query(\"UPDATE order_intents SET x = 1\").execute(&mut *tx).await?; \
                tx.commit().await }; f.await?; }\n";
    let open = check(held, &[], &[], &[]);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open[0].contains("UPDATE monthly_state via q (in u(), no key before it)"),
        "{open:?}"
    );
    assert!(
        open[1].contains("UPDATE cash_anchors via f (in v(), no key before it)"),
        "{open:?}"
    );
    // SQL inside an `async` block is keyed inside it, whatever its own
    // statement binds (`let result = …execute(pool).await?;` in `d1()`,
    // awaited after the COMMIT through `f`); one passed on unbound
    // (`self.bounded(async { … })` in `d2()`) that takes no key itself is
    // open; a branch whose condition holds `||` is no closure (`d3()`).
    let deferred = "async fn d1(pool: &PgPool) { let mut tx = pool.begin().await?; \
                    let _key = participate(&mut tx).await?; \
                    let f = async { let result = \
                    sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\").execute(pool).await?; \
                    Ok::<_, sqlx::Error>(result) }; tx.commit().await?; f.await?; }\n\
                    async fn d2(&self, pool: &PgPool) { let mut tx = pool.begin().await?; \
                    participate(&mut tx).await?; self.bounded(async { \
                    sqlx::query(\"UPDATE funding_sagas SET x = 1\").execute(&mut *tx).await }).await?; \
                    tx.commit().await?; }\n\
                    async fn d3(pool: &PgPool, a: bool, b: bool) { let mut tx = pool.begin().await?; \
                    participate(&mut tx).await?; if a || b { \
                    sqlx::query(\"UPDATE event_log SET x = 1\").execute(&mut *tx).await?; } \
                    tx.commit().await?; }\n";
    let open = check(deferred, &[], &[], &[]);
    assert_eq!(open.len(), 2, "{open:?}");
    // Nor when the SQL sits in a branch inside the `async` block, bound
    // (`e1()`, awaited after the COMMIT) or passed on (`e2()`).
    let nested = "async fn e1(pool: &PgPool, enabled: bool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; let f = async { if enabled { \
                  sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\").execute(pool).await?; } \
                  Ok::<(), sqlx::Error>(()) }; tx.commit().await?; f.await?; }\n\
                  async fn e2(&self, pool: &PgPool, enabled: bool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; self.bounded(async { if enabled { \
                  sqlx::query(\"UPDATE cash_anchors SET x = 1\").execute(&mut *tx).await?; } \
                  Ok::<(), sqlx::Error>(()) }).await?; tx.commit().await?; }\n";
    let nested_open = check(nested, &[], &[], &[]);
    assert_eq!(nested_open.len(), 2, "{nested_open:?}");
    assert!(
        nested_open[0].contains("UPDATE monthly_state via f (in e1(), no key before it)"),
        "{nested_open:?}"
    );
    assert!(
        nested_open[1].contains(
            "UPDATE cash_anchors (in e2(), in an async block or closure that takes no key before it)"
        ),
        "{nested_open:?}"
    );
    assert!(
        open[0].contains("UPDATE monthly_state via f (in d1(), no key before it)"),
        "{open:?}"
    );
    assert!(
        open[1].contains(
            "UPDATE funding_sagas (in d2(), in an async block or closure that takes no key before it)"
        ),
        "{open:?}"
    );
    // A method wrapper covers its receiver chain, raw-string SQL included.
    let m = [entry(
        "inline.rs",
        "execute_participating",
        Reach::Module,
        Form::Method,
    )];
    let method = "async fn execute_participating(self, pool: &PgPool) { \
                  certificate::execute_participating(pool, self).await }\n\
                  async fn k(pool: &PgPool) { sqlx::query(r#\"UPDATE positions_current SET x = (1)\"#)\n\
                  .bind(1)\n.execute_participating(pool)\n.await; }\n";
    let open = check(method, &m, &[], &[]);
    assert!(open.iter().all(|p| !p.contains("positions_current")), "{open:?}");
    // Not a method wrapper inside a later argument of the write's own chain:
    // that inner query commits before the outer write runs autocommit.
    let nested = "async fn execute_participating(self, pool: &PgPool) { \
                  certificate::execute_participating(pool, self).await }\n\
                  async fn n(pool: &PgPool) { sqlx::query(\"UPDATE positions_current SET last_seq = $1\")\n\
                  .bind(sqlx::query(\"SELECT 1\").execute_participating(pool).await.rows_affected() as i64)\n\
                  .execute(pool)\n.await; }\n";
    let open = check(nested, &m, &[], &[]);
    assert!(open.iter().any(|p| p.contains("UPDATE positions_current (in n()")), "{open:?}");
    // A wrapper covers only the SQL it is handed: not a raw write beside it in
    // the same statement.
    let beside = format!(
        "{wrapper}async fn t(pool: &PgPool) {{ let _ = (execute_participating(pool, sqlx::query(\"SELECT 1\")), \
         sqlx::query(\"UPDATE process_epochs SET x = 1\").execute(pool)); }}\n"
    );
    let open = check(&beside, &w, &[], &[]);
    assert!(open.iter().any(|p| p.contains("UPDATE process_epochs (in t()")), "{open:?}");
    // Nor SQL executed while its arguments are evaluated.
    let eager = format!(
        "{wrapper}async fn e(pool: &PgPool) {{ execute_participating(pool, {{ \
         sqlx::query(\"UPDATE process_epochs SET x = 1\").execute(pool).await; \
         sqlx::query(\"SELECT 1\") }}).await; }}\n"
    );
    let open = check(&eager, &w, &[], &[]);
    assert!(open.iter().any(|p| p.contains("UPDATE process_epochs (in e()")), "{open:?}");
    // The same without a block: an argument's own `.bind(…)` runs a write.
    let bound = format!(
        "{wrapper}async fn b(pool: &PgPool) {{ execute_participating(pool, sqlx::query(\"SELECT $1\")\
         .bind(sqlx::query_scalar(\"UPDATE process_epochs SET x = 1 RETURNING 1\").fetch_one(pool).await)).await; }}\n"
    );
    let open = check(&bound, &w, &[], &[]);
    assert!(open.iter().any(|p| p.contains("UPDATE process_epochs (in b()")), "{open:?}");
    // The same written with spaces and a comment, or as a UFCS call.
    let spaced = bound.replace(".fetch_one(pool)", " . fetch_one /* now */ (pool)");
    let open = check(&spaced, &w, &[], &[]);
    assert!(open.iter().any(|p| p.contains("UPDATE process_epochs (in b()")), "{open:?}");
    let ufcs = format!(
        "{wrapper}async fn u(pool: &PgPool) {{ execute_participating(pool, {{ \
         sqlx::Executor::execute(pool, sqlx::query(\"UPDATE process_epochs SET x = 2\")).await; \
         sqlx::query(\"SELECT 1\") }}).await; }}\n"
    );
    let open = check(&ufcs, &w, &[], &[]);
    assert!(open.iter().any(|p| p.contains("UPDATE process_epochs (in u()")), "{open:?}");
    // And blockless: the UFCS execution sits in the wrapper call's statement.
    let ufcs_bound = format!(
        "{wrapper}async fn v(pool: &PgPool) {{ execute_participating(pool, sqlx::query(\"SELECT $1\")\
         .bind(sqlx::Executor::execute(pool, sqlx::query(\"UPDATE process_epochs SET x = 3\")).await)).await; }}\n"
    );
    let open = check(&ufcs_bound, &w, &[], &[]);
    assert!(open.iter().any(|p| p.contains("UPDATE process_epochs (in v()")), "{open:?}");
    let after = format!(
        "{wrapper}async fn j(pool: &PgPool) {{ execute_participating(pool, sqlx::query(\"SELECT 1\")).await; \
         sqlx::query(\"UPDATE process_epochs SET x = 1\").execute(pool).await; }}\n"
    );
    let open = check(&after, &w, &[], &[]);
    assert!(open.len() == 2 && open[0].contains("UPDATE process_epochs (in j()"), "{open:?}");
}

/// Killer of "a delegated writer passes without its callers": a writer on its
/// caller's transaction is covered only when every caller takes the key
/// first, and is open itself when not delegated; a delegated writer nobody
/// calls is open; a keying function counts only in its declared form
/// (`pool.begin()` is never `boundary::begin`); an `accounting_state` row
/// lock is a site.
#[test]
fn the_guard_checks_every_caller_of_a_delegated_writer() {
    let stamp = [entry("inline.rs", "stamp", Reach::Module, Form::Free)];
    let writer = "async fn stamp(tx: &mut PgConnection) { \
                  sqlx::query(\"UPDATE accounting_state SET v = v + 1\").execute(tx).await; }\n";
    let keyed = format!(
        "{writer}async fn a(c: &mut PgConnection) {{ participate(c).await?; stamp(c).await; }}\n"
    );
    assert!(
        check(&keyed, &[], &[], &stamp).is_empty(),
        "{:?}",
        check(&keyed, &[], &[], &stamp)
    );
    assert!(unguarded(&keyed)[0].contains("(in stamp(), no key before it)"));
    let one_raw = format!("{keyed}async fn b(c: &mut PgConnection) {{ stamp(c).await; }}\n");
    let open = check(&one_raw, &[], &[], &stamp);
    assert_eq!(open.len(), 1, "{open:?}");
    assert!(
        open[0].starts_with("inline.rs:3: UPDATE accounting_state in stamp()"),
        "{open:?}"
    );
    assert!(check(writer, &[], &[], &stamp)[0].contains("no caller of the delegated stamp()"));
    let begin = [entry("inline.rs", "begin", Reach::Module, Form::Free)];
    let typed = "async fn begin(c: &mut PgConnection) -> (Tx, Participating) { participate(c).await }\n\
                 async fn k(pool: &PgPool) { let (mut tx, _key) = begin(pool).await?; \
                 sqlx::query(\"INSERT INTO cash_anchors VALUES (1)\").execute(&mut tx).await; }\n\
                 async fn l(pool: &PgPool) { let mut tx = pool.begin().await; \
                 sqlx::query(\"INSERT INTO venue_accounts VALUES (1)\").execute(&mut tx).await; }\n";
    let open = check(typed, &[], &begin, &[]);
    assert_eq!(open.len(), 1, "{open:?}");
    assert!(open[0].contains("venue_accounts"), "{open:?}");
    let m = [entry("inline.rs", "m", Reach::Module, Form::Free)];
    let locked = "async fn m(c: &mut PgConnection) { \
                  sqlx::query(\"SELECT 1 FROM accounting_state FOR UPDATE\").fetch_one(c).await; }\n\
                  async fn n(c: &mut PgConnection) { participate(c).await?; m(c).await; }\n\
                  async fn o(c: &mut PgConnection) { m(c).await; }\n";
    let open = check(locked, &[], &[], &m);
    assert!(
        open.len() == 1 && open[0].starts_with("inline.rs:3: accounting_state row lock"),
        "{open:?}"
    );
    // A private method's calls count in its module's descendants, by any
    // receiver or path; an unrelated file's `lock.write()` is not one.
    let write = [entry("store.rs", "write", Reach::Module, Form::Receiver)];
    let store = "async fn write(&self, tx: &mut PgConnection) { \
                 sqlx::query(\"INSERT INTO order_intents VALUES (1)\").execute(tx).await; }\n\
                 async fn commit(&self, tx: &mut PgConnection) { participate(tx).await?; self.write(tx).await; }\n";
    let tree = |child: &str| {
        vec![
            ("store.rs".to_string(), None, store.to_string(), None),
            (
                "store/child.rs".to_string(),
                None,
                child.to_string(),
                Some("store.rs".to_string()),
            ),
            (
                "other.rs".to_string(),
                None,
                "async fn unrelated(lock: &RwLock<u8>) { lock.write().await; }\n".to_string(),
                None,
            ),
        ]
    };
    let raw_child = "async fn clocked(&self, tx: &mut PgConnection) { self.write(tx).await; }\n";
    let open = Guard::new(tree(raw_child), &[], &[], &write).report().1;
    assert!(
        open.len() == 1 && open[0].starts_with("store/child.rs:1: INSERT order_intents in write()"),
        "{open:?}"
    );
    let keyed_child = "async fn clocked(&self, tx: &mut PgConnection) { participate(tx).await?; \
                       self.write(tx).await; }\n";
    let open = Guard::new(tree(keyed_child), &[], &[], &write).report().1;
    assert!(open.is_empty(), "{open:?}");
    let keyed_child = "async fn clocked(&self, tx: &mut PgConnection) { participate(tx).await?; \
                       Self::write(self, tx).await; }\n";
    let open = Guard::new(tree(keyed_child), &[], &[], &write).report().1;
    assert!(open.is_empty(), "{open:?}");
    // A call read across whitespace, a comment or a turbofish is a call; so
    // is one by a path (UFCS) or on another receiver, whose identity the
    // guard does not read.
    for raw_child in [
        "async fn clocked(&self, tx: &mut PgConnection) { self . write /* routed */ (tx).await; }\n",
        "async fn clocked(&self, tx: &mut PgConnection) { self.write::<u8>(tx).await; }\n",
        "async fn clocked(&self, tx: &mut PgConnection) { self.r#write(tx).await; }\n",
        "async fn clocked(&self, tx: &mut PgConnection) { Self::write(self, tx).await; }\n",
        "async fn clocked(&self, tx: &mut PgConnection) { crate::store::Store :: write(self, tx).await; }\n",
        "async fn clocked(&self, tx: &mut PgConnection) { <Store as Journal>::write(self, tx).await; }\n",
        "async fn clocked(&self, tx: &mut PgConnection) { (*self).write(tx).await; }\n",
        "async fn clocked(&self, tx: &mut PgConnection) { let this = self; this.write(tx).await; }\n",
    ] {
        let open = Guard::new(tree(raw_child), &[], &[], &write).report().1;
        assert!(
            open.len() == 1
                && open[0].starts_with("store/child.rs:1: INSERT order_intents in write()"),
            "{raw_child}: {open:?}"
        );
    }
    let spaced =
        format!("{keyed}async fn b(c: &mut PgConnection) {{ stamp\n    /* raw */ (c).await; }}\n");
    let open = check(&spaced, &[], &[], &stamp);
    assert!(
        open.len() == 1 && open[0].starts_with("inline.rs:3: UPDATE accounting_state in stamp()"),
        "{open:?}"
    );
    let appends = "async fn p(pool: &PgPool) { append_event(pool, \"s\", None, e).await; }\n\
                   async fn q(tx: &mut PgConnection) { append_event_tx(tx, \"s\", e).await; }\n";
    let open = unguarded(appends);
    assert!(open.len() == 2 && open[1].contains("append_event_tx() (in q()"), "{open:?}");
    // A delegated writer used other than called (an alias, a renamed import)
    // goes where the guard does not follow: it fails, as any reviewed entry.
    for added in [
        "pub async fn unkeyed(pool: &PgPool, stream: &str, event: Event) -> Result<Uuid> {\n    \
         let mut tx = pool.begin().await?;\n    let append = append_event_tx;\n    \
         let id = append(&mut tx, stream, None, event).await?;\n    tx.commit().await?;\n    Ok(id)\n}\n",
        // At the top, among the imports, where it reads as one.
        "use crate::append::append_event_tx as append_raw;\n",
        // A local binding whose block has ended shadows nothing.
        "pub async fn expired(pool: &PgPool, stream: &str, event: Event) -> Result<Uuid> {\n    \
         let mut tx = pool.begin().await?;\n    \
         {\n        let append_event_tx = ();\n        let _ = append_event_tx;\n    }\n    \
         let append = append_event_tx;\n    let id = append(&mut tx, stream, None, event).await?;\n    \
         tx.commit().await?;\n    Ok(id)\n}\n",
        // A path names the item, whatever parameter shares its name.
        "pub async fn pathed(pool: &PgPool, append_event_tx: u8, event: Event) -> Result<Uuid> {\n    \
         let mut tx = pool.begin().await?;\n    let append = crate::append::append_event_tx;\n    \
         let id = append(&mut tx, \"s\", None, event).await?;\n    tx.commit().await?;\n    Ok(id)\n}\n",
        // A binding's own initializer still names the item.
        "pub async fn selfinit(pool: &PgPool, stream: &str, event: Event) -> Result<Uuid> {\n    \
         let mut tx = pool.begin().await?;\n    let append_event_tx = append_event_tx;\n    \
         let append = append_event_tx;\n    let id = append(&mut tx, stream, None, event).await?;\n    \
         tx.commit().await?;\n    Ok(id)\n}\n",
        // A function-pointer type's parameter binds nothing.
        "pub async fn pointer(pool: &PgPool, _callback: fn(append_event_tx: u8), event: Event) -> Result<Uuid> {\n    \
         let mut tx = pool.begin().await?;\n    let append = append_event_tx;\n    \
         let id = append(&mut tx, \"s\", None, event).await?;\n    tx.commit().await?;\n    Ok(id)\n}\n",
        // A braceless closure's parameter holds only in its body, not in a
        // sibling element after it.
        "pub async fn tupled(pool: &PgPool, stream: &str, event: Event) -> Result<Uuid> {\n    \
         let (identity, append) = (|append_event_tx| append_event_tx, append_event_tx);\n    \
         identity(0_u8);\n    let mut tx = pool.begin().await?;\n    \
         let id = append(&mut tx, stream, None, event).await?;\n    tx.commit().await?;\n    Ok(id)\n}\n",
        // A `match` arm's guard binds nothing.
        "fn accepts<T>(_: T) -> bool {\n    true\n}\n\
         pub async fn guarded(pool: &PgPool, stream: &str, event: Event) -> Result<Uuid> {\n    \
         match () {\n        _ if accepts(append_event_tx) => {\n            let append = append_event_tx;\n            \
         let mut tx = pool.begin().await?;\n            let id = append(&mut tx, stream, None, event).await?;\n            \
         tx.commit().await?;\n            Ok(id)\n        },\n        _ => unreachable!(),\n    }\n}\n",
        // A closure's parameter holds only in its body.
        "pub fn closed() -> u8 {\n    let f = |append_event_tx| append_event_tx;\n    \
         let append = append_event_tx;\n    f(0)\n}\n",
        // A `match` arm's binding holds only in that arm.
        "pub fn armed(x: Option<u8>) -> u8 {\n    match x {\n        Some(append_event_tx) => append_event_tx,\n        \
         None => {\n            let append = append_event_tx;\n            0\n        },\n    }\n}\n",
    ] {
        let mut sources = production_sources();
        let (.., text, _) = sources
            .iter_mut()
            .find(|(rel, ..)| rel == "robson-eventlog/src/append.rs")
            .expect("append.rs");
        if added.starts_with("use ") {
            text.insert_str(0, added);
        } else {
            text.push_str(added);
        }
        let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
        assert!(
            open.iter().any(|p| p.starts_with("robson-eventlog/src/append.rs:")
                && p.contains("append_event_tx is a reviewed entry, used where it is not called")),
            "{added}: {open:?}"
        );
    }
}

/// Killer of "a table entry trusted without its proof": a wrapper whose body
/// has no key between `BEGIN` and `COMMIT`, a keying function with no key, an
/// entry that covers no site and a missing function each fail the guard.
#[test]
fn the_guard_verifies_its_tables() {
    let w = [entry("inline.rs", "wrap", Reach::Module, Form::Free)];
    let fake = "async fn wrap(pool: &PgPool, q: Query) { let mut tx = pool.begin().await; \
                q.execute(&mut *tx).await; tx.commit().await; }\n\
                async fn f(pool: &PgPool) { wrap(pool, sqlx::query(\"UPDATE positions_current SET x = 1\")).await; }\n";
    let open = check(fake, &w, &[], &[]);
    assert!(
        open.iter().any(|p| p.contains("wrap() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    let late = fake.replace("tx.commit().await;", "tx.commit().await; participate(&mut c).await?;");
    assert!(!check(&late, &w, &[], &[]).is_empty());
    let real = fake.replace(
        "let mut tx = pool.begin().await;",
        "let mut tx = pool.begin().await; participate(&mut tx).await?;",
    );
    assert!(check(&real, &w, &[], &[]).is_empty(), "{:?}", check(&real, &w, &[], &[]));
    // Nor one whose key is out of the statement's scope.
    let scoped = fake.replace(
        "let mut tx = pool.begin().await;",
        "let mut tx = pool.begin().await; if false { participate(&mut tx).await?; }",
    );
    let open = check(&scoped, &w, &[], &[]);
    assert!(
        open.iter().any(|p| p.contains("wrap() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    // A wrapper whose key never runs is none.
    let dropped = fake.replace(
        "let mut tx = pool.begin().await;",
        "let mut tx = pool.begin().await; drop(participate(&mut tx));",
    );
    let open = check(&dropped, &w, &[], &[]);
    assert!(
        open.iter().any(|p| p.contains("wrap() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    // `.begin()` is never a free keying `begin`, so a wrapper without its key
    // still fails beside one; so does a key after the statement.
    let begin = [entry("inline.rs", "begin", Reach::Module, Form::Free)];
    let open = check(fake, &w, &begin, &[]);
    assert!(
        open.iter().any(|p| p.contains("wrap() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    let after =
        fake.replace("tx.commit().await;", "participate(&mut tx).await?; tx.commit().await;");
    let open = check(&after, &w, &[], &[]);
    assert!(
        open.iter().any(|p| p.contains("wrap() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    // A method wrapper may delegate only by handing itself (`self`) to
    // another wrapper and executing nothing itself.
    let ws = [
        entry("inline.rs", "execute_participating", Reach::Module, Form::Free),
        entry("inline.rs", "run_participating", Reach::Module, Form::Method),
    ];
    let inner = "async fn execute_participating(pool: &PgPool, q: Query) { \
                 let mut tx = pool.begin().await; participate(&mut tx).await?; \
                 q.execute(&mut *tx).await; tx.commit().await; }\n";
    let honest = format!(
        "{inner}async fn run_participating(self, pool: &PgPool) {{ execute_participating(pool, self).await }}\n\
         async fn f(pool: &PgPool) {{ \
         sqlx::query(\"UPDATE positions_current SET x = 1\").run_participating(pool).await; \
         execute_participating(pool, sqlx::query(\"UPDATE order_intents SET y = 1\")).await; }}\n"
    );
    assert!(check(&honest, &ws, &[], &[]).is_empty(), "{:?}", check(&honest, &ws, &[], &[]));
    let delegate = "execute_participating(pool, self).await }";
    for dishonest in [
        "execute_participating(pool, sqlx::query(\"SELECT 1\")).await }",
        "self.execute(pool).await; execute_participating(pool, self).await }",
    ] {
        let open = check(&honest.replace(delegate, dishonest), &ws, &[], &[]);
        assert!(
            open.iter()
                .any(|p| p.contains("run_participating() is not BEGIN, key, statement, COMMIT")),
            "{dishonest}: {open:?}"
        );
    }
    let k = [entry("inline.rs", "open_tx", Reach::Module, Form::Free)];
    let no_key =
        "async fn open_tx(c: &mut PgConnection) { sqlx::query(\"SELECT 1\").execute(c).await; }\n\
                  async fn g(c: &mut PgConnection) { open_tx(c).await; \
                  sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(c).await; }\n";
    assert!(check(no_key, &[], &k, &[]).iter().any(|p| p.contains("open_tx() takes no key")));
    let stale = [entry("inline.rs", "h", Reach::Module, Form::Free)];
    assert!(check("fn h() {}", &[], &[], &stale).iter().any(|p| p.contains("(stale entry)")));
    let gone = [entry("inline.rs", "gone", Reach::Module, Form::Free)];
    assert!(check("fn h() {}", &[], &[], &gone)
        .iter()
        .any(|p| p.contains("gone() not found")));
}

/// Killer of "the reviewed tables trust a function that lost its key": on the
/// production sources with the reviewed tables, removing the key from 8m-2a's
/// `execute_participating`, from the projector's month `write` or from
/// `boundary::begin` fails the guard (its own `.begin()` and `COMMIT` are
/// not a key).
#[test]
fn the_reviewed_tables_refuse_a_function_without_its_key() {
    let cut = |file: &str, from: &str, key: &str| {
        let mut sources = production_sources();
        let (.., text, _) = sources.iter_mut().find(|(rel, ..)| rel == file).expect(file);
        let at = text.find(from).expect(from);
        let off = at + text[at..].find(key).expect(key);
        text.replace_range(off..off + key.len(), "");
        Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1
    };
    let open = cut(
        "robson-eventlog/src/certificate.rs",
        "pub async fn execute_participating",
        "let _key = participate(&mut tx).await?;",
    );
    assert!(
        open.iter().any(|p| p.contains(
            "certificate.rs: execute_participating() is not BEGIN, key, statement, COMMIT"
        )),
        "{open:?}"
    );
    let open = cut(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn write<'c>(",
        "step(&mut tx, accounting).await?;",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("monthly_state.rs: write() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    let open = cut("robsond/src/boundary.rs", "async fn begin(", "participate(");
    assert!(open.iter().any(|p| p.contains("boundary.rs: begin() takes no key")), "{open:?}");
    // A keying function takes the key on every successful path: the month
    // step's key-only arm with its key in a skipped block takes none, nor
    // does the balance step without `advance`.
    let swap = |file: &str, from: &str, old: &str, new: &str| {
        let mut sources = production_sources();
        let (.., text, _) = sources.iter_mut().find(|(rel, ..)| rel == file).expect(file);
        let at = text.find(from).expect(from);
        let off = at + text[at..].find(old).expect(old);
        text.replace_range(off..off + old.len(), new);
        Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1
    };
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "None => participate(connection).await.map(drop).map_err(ProjectionError::from),",
        "None => {\n            if false {\n                participate(connection).await?;\n            }\n            Ok(())\n        },",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    // The append's try-lock is a key only with its refusal: kept but no longer
    // tied to the try, it is none.
    let open = swap(
        "robson-eventlog/src/append.rs",
        "pub async fn append_event(",
        "if crate::certificate::try_participate(&mut tx).await?.is_none() {",
        "let _key = crate::certificate::try_participate(&mut tx).await?;\n    if false {",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("append.rs: append_event() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    // Nor with its condition negated (the refusal runs on a granted key and
    // the write on a refused one), or joined to another.
    for condition in [
        "if !crate::certificate::try_participate(&mut tx).await?.is_none() {",
        "if false && crate::certificate::try_participate(&mut tx).await?.is_none() {",
    ] {
        let open = swap(
            "robson-eventlog/src/append.rs",
            "pub async fn append_event(",
            "if crate::certificate::try_participate(&mut tx).await?.is_none() {",
            condition,
        );
        assert!(
            open.iter().any(|p| {
                p.contains("append.rs: append_event() is not BEGIN, key, statement, COMMIT")
            }),
            "{condition}: {open:?}"
        );
    }
    // Nor with a refusal that does not leave: a `return` inside another word
    // is no `return`. The whole refusal body is replaced, so no rollback is
    // left to refuse the wrapper for another reason.
    let open = {
        let mut sources = production_sources();
        let (.., text, _) = sources
            .iter_mut()
            .find(|(rel, ..)| rel == "robson-eventlog/src/append.rs")
            .expect("append.rs");
        let at = text.find("pub async fn append_event(").expect("append_event");
        let refusal = "if crate::certificate::try_participate(&mut tx).await?.is_none() ";
        let open = at + text[at..].find(refusal).expect("refusal") + refusal.len();
        let close = closing(text, open);
        text.replace_range(open..close, "{\n        let _return_value = ();\n    }");
        Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1
    };
    assert!(
        open.iter()
            .any(|p| p.contains("append.rs: append_event() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    // A wrapper's every execution is checked: an early autocommit return
    // before its BEGIN (the P2 mutant's month write) is refused, and so is a
    // statement run after a COMMIT, its key taken after that COMMIT.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn write<'c>(",
        "    let mut tx = on\n",
        "    if accounting.is_none() {\n        return Ok(statement.execute(on).await?);\n    }\n    let mut tx = on\n",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("monthly_state.rs: write() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    let open = swap(
        "robson-eventlog/src/certificate.rs",
        "pub async fn execute_participating<'q>(",
        "let _key = participate(&mut tx).await?;",
        "tx.commit().await?;\n    let _key = participate(&mut tx).await?;",
    );
    assert!(
        open.iter().any(|p| {
            p.contains(
                "certificate.rs: execute_participating() is not BEGIN, key, statement, COMMIT",
            )
        }),
        "{open:?}"
    );
    // Nor a keying call whose failure it swallows: the month write with
    // `step`'s error ignored runs its statement unkeyed when the step refuses.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn write<'c>(",
        "step(&mut tx, accounting).await?;",
        "let _ = step(&mut tx, accounting).await;",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("monthly_state.rs: write() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    // Nor one whose failure it recovers before the `?` (`.or(…)`): the
    // refusal becomes a success and the statement runs unkeyed; nor a keying
    // function whose returned key value is recovered (`.or_else(…)`).
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn write<'c>(",
        "step(&mut tx, accounting).await?;",
        "step(&mut tx, accounting)\n        .await\n        .or(Ok::<(), ProjectionError>(()))?;",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("monthly_state.rs: write() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "None => participate(connection).await.map(drop).map_err(ProjectionError::from),",
        "None => participate(connection).await.map(drop).or_else(|_| Ok(())),",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    // Nor one whose arm without a key has a block-like body, needing no `,`:
    // it is an arm of its own, never merged with the keyed arm after it.
    for body in ["const { Ok(()) }", "if true { Ok(()) } else { Ok(()) }"] {
        let open = swap(
            "robson-projector/src/handlers/monthly_state.rs",
            "async fn step(",
            "        Some(step) => step.run(connection).await.map_err(ProjectionError::Accounting),\n        \
             None => participate(connection).await.map(drop).map_err(ProjectionError::from),\n",
            &format!(
                "        None => {body}\n        \
                 Some(step) => step.run(connection).await.map_err(ProjectionError::Accounting),\n"
            ),
        );
        assert!(
            open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
            "{body}: {open:?}"
        );
    }
    // Nor a keying function whose error-keeping adapter's argument returns
    // success before the adapter sees the refusal.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "Some(step) => step.run(connection).await.map_err(ProjectionError::Accounting),",
        "Some(step) => step.run(connection).await.map_err({\n            if accounting.is_some() {\n                return Ok(());\n            }\n            ProjectionError::Accounting\n        }),",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    // Nor one whose braced arm a chain continues with a recovery (`{ … }
    // .or(Ok(()))`): the arm is the whole expression, whose refusal is
    // recovered before the month statement.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "Some(step) => step.run(connection).await.map_err(ProjectionError::Accounting),",
        "Some(step) => {\n            step.run(connection).await.map_err(ProjectionError::Accounting)\n        }.or(Ok(())),",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    // Nor one whose whole tail `match`, `if … else …` or pinned `async`
    // block a chain continues: the recovery after it is unproved.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "None => participate(connection).await.map(drop).map_err(ProjectionError::from),\n    }\n",
        "None => participate(connection).await.map(drop).map_err(ProjectionError::from),\n    }\n    .or(Ok(()))\n",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "    match accounting {\n        Some(step) => step.run(connection).await.map_err(ProjectionError::Accounting),\n        \
         None => participate(connection).await.map(drop).map_err(ProjectionError::from),\n    }\n",
        "    if let Some(step) = accounting {\n        step.run(connection).await.map_err(ProjectionError::Accounting)\n    \
         } else {\n        participate(connection).await.map(drop).map_err(ProjectionError::from)\n    }\n    .or(Ok(()))\n",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    let open = swap(
        "robsond/src/balance_accounting.rs",
        "    fn run<'c>(",
        "            self.advance(connection).await.map(drop).map_err(|error| error.to_string())\n        })\n",
        "            self.advance(connection).await.map(drop).map_err(|error| error.to_string())\n        \
         }.map(|result| result.or(Ok(()))))\n",
    );
    assert!(
        open.iter().any(|p| p.contains("balance_accounting.rs: run() takes no key")),
        "{open:?}"
    );
    // The same rule leaves a wrapper's statement unkeyed: a keying call whose
    // adapter's argument may return success (`map_err({ … return Ok(…); … })`)
    // leaves the wrapper before its statement and COMMIT, so it is not BEGIN,
    // key, statement, COMMIT. (A keying function's own `return` before its
    // key's success point, the step's case above, is refused by
    // [`Guard::keys_here`] too; a statement keyed by the call has only this.)
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn write<'c>(",
        "    step(&mut tx, accounting).await?;\n",
        "    step(&mut tx, accounting).await.map_err({\n        if accounting.is_none() {\n            return Ok(PgQueryResult::default());\n        }\n        ProjectionError::from\n    })?;\n",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("monthly_state.rs: write() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    // Nor a wrapper whose key, or keying call, stands in an `else if`
    // condition: an earlier branch taken skips it.
    let open = swap(
        "robson-eventlog/src/certificate.rs",
        "pub async fn execute_participating<'q>(",
        "    let _key = participate(&mut tx).await?;\n",
        "    if true {\n    } else if participate(&mut tx).await.map(drop)? == () {\n    }\n",
    );
    assert!(
        open.iter().any(|p| p.contains(
            "certificate.rs: execute_participating() is not BEGIN, key, statement, COMMIT"
        )),
        "{open:?}"
    );
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn write<'c>(",
        "    step(&mut tx, accounting).await?;\n",
        "    if accounting.is_none() {\n    } else if step(&mut tx, accounting).await.map(drop)? == () {\n    }\n",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("monthly_state.rs: write() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    // Nor the seed, whose key a closure's COMMIT could release once carried
    // past the guard by an awaited call that returns it unrun.
    let open = swap(
        "robson-db/src/init.rs",
        "pub async fn init_minimal_data(",
        "    let mut tx = pool.begin().await?;\n    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n",
        "    let release = async |c: &mut PgConnection| { sqlx::query(\"COMMIT\").execute(c).await };\n    \
         let finish = std::future::ready(release).await;\n    \
         let mut tx = pool.begin().await?;\n    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n    \
         finish(&mut *tx).await?;\n",
    );
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("transaction boundary in release, used where it does not run")),
        "{open:?}"
    );
    // Nor when a closure passed on holds an opener it never runs.
    let open = swap(
        "robson-db/src/init.rs",
        "pub async fn init_minimal_data(",
        "    let mut tx = pool.begin().await?;\n    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n",
        "    let finish = std::future::ready(async |c: &mut PgConnection| {\n        \
         if false {\n            let _unused = pool.begin().await?;\n        }\n        \
         sqlx::query(\"COMMIT\").execute(c).await\n    })\n    .await;\n    \
         let mut tx = pool.begin().await?;\n    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n    \
         finish(&mut *tx).await?;\n",
    );
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("transaction boundary in a closure or async block passed on")),
        "{open:?}"
    );
    // Nor when another closure calls that closure.
    let open = swap(
        "robson-db/src/init.rs",
        "pub async fn init_minimal_data(",
        "    let mut tx = pool.begin().await?;\n    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n",
        "    let release = async |c: &mut PgConnection| { sqlx::query(\"COMMIT\").execute(c).await };\n    \
         let finish = async |c: &mut PgConnection| release(c).await;\n    \
         let mut tx = pool.begin().await?;\n    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n    \
         finish(&mut *tx).await?;\n",
    );
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("transaction boundary in release, used where it does not run")),
        "{open:?}"
    );
    // Nor the seed, whose key a COMMIT an ordinary function returns could
    // release: that SQL does not run where it stands, so it fails.
    let mut sources = production_sources();
    let (.., text, _) = sources
        .iter_mut()
        .find(|(rel, ..)| rel == "robson-db/src/init.rs")
        .expect("init.rs");
    let at = text.find("pub async fn init_minimal_data(").expect("the seed");
    let key = "    let mut tx = pool.begin().await?;\n    \
               sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n";
    let after_key = at + text[at..].find(key).expect("its key") + key.len();
    text.insert_str(after_key, "    sqlx::query(release_sql()).execute(&mut *tx).await?;\n");
    text.push_str("fn release_sql() -> &'static str { \"COMMIT\" }\n");
    let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("boundary SQL not run where it stands")),
        "{open:?}"
    );
    // Nor the seed, whose key a helper ending the transaction it is handed
    // releases: a function that ends a transaction it did not open fails
    // unless reviewed (then each call of it is a boundary).
    let mut sources = production_sources();
    let (.., text, _) = sources
        .iter_mut()
        .find(|(rel, ..)| rel == "robson-db/src/init.rs")
        .expect("init.rs");
    let at = text.find("pub async fn init_minimal_data(").expect("the seed");
    let after_key = at + text[at..].find(key).expect("its key") + key.len();
    text.insert_str(after_key, "    finish_seed(&mut *tx).await?;\n");
    text.push_str(
        "async fn finish_seed(c: &mut PgConnection) -> sqlx::Result<()> {\n    \
         sqlx::query(\"COMMIT\").execute(c).await?;\n    Ok(())\n}\n",
    );
    let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("finish_seed() ends a transaction it did not open")),
        "{open:?}"
    );
    // A reviewed opener function is verified: one that opens nothing, or
    // ends a transaction, fails.
    let open = swap(
        "robsond/src/venue_budget/bootstrap.rs",
        "pub(super) async fn begin_bounded(",
        "include_str!(\"sql/begin.sql\")",
        "include_str!(\"sql/commit.sql\")",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("opener entry begin_bounded() opens no transaction, or ends one")),
        "{open:?}"
    );
    // Nor the seed, whose key a helper's raw `BEGIN` on the connection it is
    // handed does not protect: nested, it leaves the seed's transaction
    // open, and the helper's `COMMIT` ends it. Only an opener on a
    // connection the helper obtained itself proves it owns what it ends.
    let mut sources = production_sources();
    let (.., text, _) = sources
        .iter_mut()
        .find(|(rel, ..)| rel == "robson-db/src/init.rs")
        .expect("init.rs");
    let at = text.find("pub async fn init_minimal_data(").expect("the seed");
    let after_key = at + text[at..].find(key).expect("its key") + key.len();
    text.insert_str(after_key, "    finish_seed(&mut *tx).await?;\n");
    text.push_str(
        "async fn finish_seed(c: &mut PgConnection) -> sqlx::Result<()> {\n    \
         sqlx::query(\"BEGIN\").execute(&mut *c).await?;\n    \
         sqlx::query(\"COMMIT\").execute(c).await?;\n    Ok(())\n}\n",
    );
    let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("finish_seed() ends a transaction it did not open")),
        "{open:?}"
    );
    // Nor when the helper's own savepoint is closed again before its
    // `COMMIT` on the same connection: only an opener still outstanding
    // proves ownership.
    let mut sources = production_sources();
    let (.., text, _) = sources
        .iter_mut()
        .find(|(rel, ..)| rel == "robson-db/src/init.rs")
        .expect("init.rs");
    let at = text.find("pub async fn init_minimal_data(").expect("the seed");
    let after_key = at + text[at..].find(key).expect("its key") + key.len();
    text.insert_str(after_key, "    finish_seed(&mut *tx).await?;\n");
    text.push_str(
        "async fn finish_seed(c: &mut PgConnection) -> sqlx::Result<()> {\n    \
         use sqlx::Connection as _;\n    let sp = c.begin().await?;\n    sp.commit().await?;\n    \
         sqlx::query(\"COMMIT\").execute(c).await?;\n    Ok(())\n}\n",
    );
    let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("finish_seed() ends a transaction it did not open")),
        "{open:?}"
    );
    // Nor when the helper's opener is an sqlx savepoint and its close a raw
    // `COMMIT`: that ends the seed's own transaction, whatever savepoint is
    // open.
    let mut sources = production_sources();
    let (.., text, _) = sources
        .iter_mut()
        .find(|(rel, ..)| rel == "robson-db/src/init.rs")
        .expect("init.rs");
    let at = text.find("pub async fn init_minimal_data(").expect("the seed");
    let after_key = at + text[at..].find(key).expect("its key") + key.len();
    text.insert_str(after_key, "    finish_seed(&mut *tx).await?;\n");
    text.push_str(
        "async fn finish_seed(c: &mut PgConnection) -> sqlx::Result<()> {\n    \
         use sqlx::Connection as _;\n    let mut sp = c.begin().await?;\n    \
         sqlx::query(\"COMMIT\").execute(&mut *sp).await?;\n    std::mem::forget(sp);\n    \
         Ok(())\n}\n",
    );
    let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("finish_seed() ends a transaction it did not open")),
        "{open:?}"
    );
    // The reviewed owning function and connector are verified: one that
    // obtains no connection of its own fails.
    let open = swap(
        "robsond/src/venue_budget/source_audit/run.rs",
        "async fn begin(pool: &PgPool",
        "options(pool).connect()",
        "options(pool).reuse()",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("owning entry begin() owns no transaction, or ends one")),
        "{open:?}"
    );
    let open = swap(
        "robsond/src/venue_budget/n5_claim/mod.rs",
        "async fn raw_connection(",
        "options.connect().await",
        "options.reuse().await",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("connector entry raw_connection() connects nothing")),
        "{open:?}"
    );
    // The reviewed inert text is pinned by its contents: other boundary SQL
    // in its function is none, and an entry matching nothing fails.
    let open = swap(
        "robsond/src/config.rs",
        "impl std::fmt::Display for StartupStaleActivePolicy",
        "write!(f, \"abort\")",
        "write!(f, \"COMMIT\")",
    );
    assert!(
        open.iter().any(|p| p.starts_with("robsond/src/config.rs:")
            && p.contains("boundary SQL not run where it stands")),
        "{open:?}"
    );
    assert!(
        open.iter()
            .any(|p| p.contains("inert entry fmt() `abort` matches no boundary SQL")),
        "{open:?}"
    );
    // Nor the month's fallback write when an awaited call returns its
    // executor's future unpolled, awaited after the COMMIT.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn add_to_month_counters(",
        "    sqlx::query(ADD)\n        .bind(year)\n        .bind(month)\n        .bind(trades_opened)\n        \
         .bind(realized_loss)\n        .execute(&mut *tx)\n",
        "    let pending = std::future::ready(\n        sqlx::query(ADD)\n            .bind(year)\n            \
         .bind(month)\n            .bind(trades_opened)\n            .bind(realized_loss)\n            \
         .execute(pool),\n    )\n    .await;\n    tx.commit().await?;\n    rollback_observation.disarm();\n    \
         pending\n",
    );
    assert!(
        open.iter()
            .any(|p| p.starts_with("robson-projector/src/handlers/monthly_state.rs:")
                && p.contains("via pending")),
        "{open:?}"
    );
    // Nor when an `async` block returns the COMMIT unrun, run later through
    // its value.
    let open = swap(
        "robson-db/src/init.rs",
        "pub async fn init_minimal_data(",
        "    let mut tx = pool.begin().await?;\n    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n",
        "    let work = async { \"COMMIT\" };\n    let done = work.await;\n    \
         let mut tx = pool.begin().await?;\n    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n    \
         sqlx::query(done).execute(&mut *tx).await?;\n",
    );
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("boundary SQL in a closure or async block, not run there")),
        "{open:?}"
    );
    // Nor one whose key a COMMIT held in a block-initialized constant, or
    // carried by an unrelated await, could release: neither runs where it
    // stands, so both fail.
    let mut sources = production_sources();
    let (.., text, _) = sources
        .iter_mut()
        .find(|(rel, ..)| rel == "robson-eventlog/src/certificate.rs")
        .expect("certificate.rs");
    let at = text.find("pub async fn execute_participating<'q>(").expect("the wrapper");
    let key = "    let _key = participate(&mut tx).await?;\n";
    let after_key = at + text[at..].find(key).expect("its key") + key.len();
    text.insert_str(after_key, "    sqlx::query(DONE).execute(&mut *tx).await?;\n");
    text.insert_str(at, "const DONE: &str = { \"COMMIT\" };\n");
    let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
    assert!(
        open.iter().any(|p| p.starts_with("robson-eventlog/src/certificate.rs:")
            && p.contains("boundary SQL held where its uses are not followed")),
        "{open:?}"
    );
    let open = swap(
        "robson-eventlog/src/certificate.rs",
        "pub async fn execute_participating<'q>(",
        "    let mut tx = pool.begin().instrument(io(2)).await?;\n",
        "    let done = std::future::ready(\"COMMIT\").await;\n    \
         let mut tx = pool.begin().instrument(io(2)).await?;\n",
    );
    assert!(
        open.iter().any(|p| p.starts_with("robson-eventlog/src/certificate.rs:")
            && p.contains("boundary SQL held where its uses are not followed")),
        "{open:?}"
    );
    // Nor one whose key a COMMIT held by a constant of another file releases
    // before its statement (`crate::append::DONE`).
    let mut sources = production_sources();
    for (rel, .., text, _) in sources.iter_mut() {
        if rel == "robson-eventlog/src/append.rs" {
            text.push_str("pub(crate) const DONE: &str = \"COMMIT\";\n");
        } else if rel == "robson-eventlog/src/certificate.rs" {
            let at = text.find("pub async fn execute_participating<'q>(").expect("the wrapper");
            let key = "    let _key = participate(&mut tx).await?;\n";
            let after_key = at + text[at..].find(key).expect("its key") + key.len();
            text.insert_str(
                after_key,
                "    sqlx::query(crate::append::DONE).execute(&mut *tx).await?;\n",
            );
        }
    }
    let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
    assert!(
        open.iter().any(|p| p.contains(
            "certificate.rs: execute_participating() is not BEGIN, key, statement, COMMIT"
        )),
        "{open:?}"
    );
    // Nor one whose key a COMMIT a `concat!` constant holds, defined before
    // the function, releases before its statement.
    let mut sources = production_sources();
    let (.., text, _) = sources
        .iter_mut()
        .find(|(rel, ..)| rel == "robson-eventlog/src/certificate.rs")
        .expect("certificate.rs");
    let at = text.find("pub async fn execute_participating<'q>(").expect("the wrapper");
    let key = "    let _key = participate(&mut tx).await?;\n";
    let after_key = at + text[at..].find(key).expect("its key") + key.len();
    text.insert_str(after_key, "    sqlx::query(DONE).execute(&mut *tx).await?;\n");
    text.insert_str(at, "const DONE: &str = concat!(\"COM\", \"MIT\");\n");
    let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
    assert!(
        open.iter().any(|p| p.contains(
            "certificate.rs: execute_participating() is not BEGIN, key, statement, COMMIT"
        )),
        "{open:?}"
    );
    // Nor one whose key a COMMIT brought by `include_str![…]` releases before
    // its statement.
    let open = swap(
        "robson-eventlog/src/certificate.rs",
        "pub async fn execute_participating<'q>(",
        "    let _key = participate(&mut tx).await?;\n",
        "    let _key = participate(&mut tx).await?;\n    \
         sqlx::query(include_str![\"../../robsond/src/venue_budget/sql/commit.sql\"])\
         .execute(&mut *tx).await?;\n",
    );
    assert!(
        open.iter().any(|p| p.contains(
            "certificate.rs: execute_participating() is not BEGIN, key, statement, COMMIT"
        )),
        "{open:?}"
    );
    // Nor one whose key, or keying call, a `cfg` attribute may compile out or
    // a macro may never evaluate.
    for (file, from, old, new, message) in [
        (
            "robson-eventlog/src/certificate.rs",
            "pub async fn execute_participating<'q>(",
            "    let _key = participate(&mut tx).await?;\n",
            "    #[cfg(debug_assertions)]\n    let _key = participate(&mut tx).await?;\n",
            "certificate.rs: execute_participating() is not BEGIN, key, statement, COMMIT",
        ),
        (
            "robson-eventlog/src/certificate.rs",
            "pub async fn execute_participating<'q>(",
            "    let _key = participate(&mut tx).await?;\n",
            "    debug_assert_eq!(participate(&mut tx).await.map(drop)?, ());\n",
            "certificate.rs: execute_participating() is not BEGIN, key, statement, COMMIT",
        ),
        (
            "robson-projector/src/handlers/monthly_state.rs",
            "async fn write<'c>(",
            "    step(&mut tx, accounting).await?;\n",
            "    #[cfg_attr(not(debug_assertions), cfg(any()))]\n    step(&mut tx, accounting).await?;\n",
            "monthly_state.rs: write() is not BEGIN, key, statement, COMMIT",
        ),
        (
            "robson-projector/src/handlers/monthly_state.rs",
            "async fn write<'c>(",
            "    step(&mut tx, accounting).await?;\n",
            "    tracing::debug!(step = ?step(&mut tx, accounting).await?);\n",
            "monthly_state.rs: write() is not BEGIN, key, statement, COMMIT",
        ),
    ] {
        let open = swap(file, from, old, new);
        assert!(open.iter().any(|p| p.contains(message)), "{new}: {open:?}");
    }
    // Nor one whose statement runs inside the keying call's adapter argument,
    // before the `?` that establishes the key: a refused step leaves the
    // statement unkeyed until that `?`.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn write<'c>(",
        "    step(&mut tx, accounting).await?;\n    let written = statement\n        .execute(&mut *tx)\n        .instrument(\n            tracing::info_span!(target: \"robson_operation\", \"operation_io\", io_kind = 1_u64),\n        )\n        .await?;\n",
        "    let written;\n    step(&mut tx, accounting).await.map_err({\n        written = statement.execute(&mut *tx).await?;\n        ProjectionError::from\n    })?;\n",
    );
    assert!(
        open.iter()
            .any(|p| p.contains("monthly_state.rs: write() is not BEGIN, key, statement, COMMIT")),
        "{open:?}"
    );
    // Nor a keying function whose key a COMMIT inside an adapter's argument
    // releases before the key's success point.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "None => participate(connection).await.map(drop).map_err(ProjectionError::from),",
        "None => participate(&mut *connection)\n            .await\n            .map({\n                sqlx::query(\"COMMIT\").execute(&mut *connection).await?;\n                drop\n            })\n            .map_err(ProjectionError::from),",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    // Nor a keying function whose key sits in a nested block of its checked
    // block (a loop that may not run): the key counts only at that block's
    // own level.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "None => participate(connection).await.map(drop).map_err(ProjectionError::from),",
        "None => {\n            for _ in 0..0 {\n                participate(&mut *connection).await?;\n            }\n            Ok(())\n        },",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    // Nor a boundary inside its executor's arguments: COMMIT and a new,
    // unkeyed BEGIN evaluated before the statement runs.
    let open = swap(
        "robson-eventlog/src/certificate.rs",
        "pub async fn execute_participating<'q>(",
        "let written = query.execute(&mut *tx).instrument(statement()).await?;",
        "let written = query\n        .execute({\n            tx.commit().await?;\n            tx = pool.begin().await?;\n            &mut *tx\n        })\n        .instrument(statement())\n        .await?;",
    );
    assert!(
        open.iter().any(|p| {
            p.contains(
                "certificate.rs: execute_participating() is not BEGIN, key, statement, COMMIT",
            )
        }),
        "{open:?}"
    );
    // Nor a `begin` that ends its keyed transaction and returns a new one.
    let open = swap(
        "robsond/src/boundary.rs",
        "async fn begin(",
        "let key = participate(&mut tx).await?;",
        "let key = participate(&mut tx).await?;\n    tx.rollback().await?;\n    let tx = held.connection().begin().instrument(io(2)).await?;",
    );
    assert!(open.iter().any(|p| p.contains("boundary.rs: begin() takes no key")), "{open:?}");
    // Nor with a `return` in the key call's own argument, before it runs.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "None => participate(connection).await.map(drop).map_err(ProjectionError::from),",
        "None => {\n            participate({\n                if accounting.is_none() {\n                    return Ok(());\n                }\n                connection\n            })\n            .await?;\n            Ok(())\n        },",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    // Nor with a successful `return` in an arm's guard.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "match accounting {",
        "match accounting {\n        None if {\n            if accounting.is_none() {\n                return Ok(());\n            }\n            true\n        } => participate(connection).await.map(drop).map_err(ProjectionError::from),",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    // Nor with a successful `return` before its `match`.
    let open = swap(
        "robson-projector/src/handlers/monthly_state.rs",
        "async fn step(",
        "match accounting {",
        "if accounting.is_none() {\n            return Ok(());\n        };\n        match accounting {",
    );
    assert!(
        open.iter().any(|p| p.contains("monthly_state.rs: step() takes no key")),
        "{open:?}"
    );
    let open = cut("robsond/src/balance_accounting.rs", "fn run<'c>(", ".advance(connection)");
    assert!(
        open.iter().any(|p| p.contains("balance_accounting.rs: run() takes no key")),
        "{open:?}"
    );
}

/// Killer of "a key counted past its transaction": the key covers only the
/// transaction that took it, not a later one after its `COMMIT` nor another
/// opened after it, while a `rollback` in an exited branch does not end it;
/// the production seed with its key moved into its read-only transaction
/// fails.
#[test]
fn the_guard_scopes_the_key_to_its_transaction() {
    let moved = "async fn seed(pool: &PgPool) { let mut tx = pool.begin().await; \
                 participate(&mut tx).await?; tx.commit().await; let mut tx = pool.begin().await; \
                 sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(&mut *tx).await; \
                 tx.commit().await; }\n";
    let open = unguarded(moved);
    assert!(
        open.len() == 1 && open[0].contains("INSERT event_log (in seed(), no key before it)"),
        "{open:?}"
    );
    let right = moved.replacen("participate(&mut tx).await?; ", "", 1).replace(
        "let mut tx = pool.begin().await; sqlx::query",
        "let mut tx = pool.begin().await; participate(&mut tx).await?; sqlx::query",
    );
    assert!(unguarded(&right).is_empty(), "{:?}", unguarded(&right));
    let interleaved = "async fn two(pool: &PgPool) { let mut a = pool.begin().await; \
                       participate(&mut a).await?; let mut b = pool.begin().await; \
                       sqlx::query(\"UPDATE process_epochs SET x = 1\").execute(&mut *b).await; }\n";
    assert_eq!(unguarded(interleaved).len(), 1);
    // A COMMIT in a branch whose `return` operand may `break` out of a
    // labelled block instead (`return { if resume { break 'resume; } Ok(()) }`)
    // is not left: the write after the block follows it, unkeyed.
    let operand = "async fn ro(pool: &PgPool, finish: bool, resume: bool) -> Result<(), E> { \
                   let mut tx = pool.begin().await?; participate(&mut tx).await?; \
                   'resume: { if finish { tx.commit().await?; \
                   return { if resume { break 'resume; } Ok(()) }; } } \
                   sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\").execute(pool).await?; \
                   Ok(()) }\n";
    let open = unguarded(operand);
    assert!(
        open.len() == 1 && open[0].contains("UPDATE monthly_state (in ro(), no key before it)"),
        "{open:?}"
    );
    // A boundary a `const` with a restricted visibility holds is one at each
    // use.
    let held = "pub (crate) const DONE: &str = \"COMMIT\";\n\
                async fn hc(pool: &PgPool) { let mut tx = pool.begin().await?; \
                participate(&mut tx).await?; sqlx::query(DONE).execute(&mut *tx).await?; \
                sqlx::query(\"UPDATE funding_sagas SET x = 1\").execute(&mut *tx).await?; }\n";
    let open = unguarded(held);
    assert!(
        open.len() == 1 && open[0].contains("UPDATE funding_sagas (in hc(), no key before it)"),
        "{open:?}"
    );
    // So is one a `const` or `let mut` holds with its keyword on its own line.
    let spaced = "const\nDONE2: &str = \"COMMIT\";\n\
                  async fn hd(pool: &PgPool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; sqlx::query(DONE2).execute(&mut *tx).await?; \
                  sqlx::query(\"UPDATE cash_anchors SET x = 1\").execute(&mut *tx).await?; }\n\
                  async fn he(pool: &PgPool) { let mut tx = pool.begin().await?; \
                  let\n    mut done = \"COMMIT\"; participate(&mut tx).await?; \
                  sqlx::query(done).execute(&mut *tx).await?; \
                  sqlx::query(\"DELETE FROM order_intents\").execute(&mut *tx).await?; }\n";
    let open = unguarded(spaced);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(open[0].contains("UPDATE cash_anchors (in hd(), no key before it)"), "{open:?}");
    assert!(open[1].contains("DELETE order_intents (in he(), no key before it)"), "{open:?}");
    // So is one a `concat!` of literals holds as a whole initializer, the
    // constant standing outside the function.
    let joined = "const DONE3: &str = concat!(\"COM\", \"MIT\");\n\
                  const DONE4: &str = std::concat! {\"COM\", \"MIT\"};\n\
                  async fn hj(pool: &PgPool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; sqlx::query(DONE3).execute(&mut *tx).await?; \
                  sqlx::query(\"UPDATE stream_suspensions SET x = 1\").execute(&mut *tx).await?; }\n\
                  async fn hk(pool: &PgPool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; sqlx::query(DONE4).execute(&mut *tx).await?; \
                  sqlx::query(\"DELETE FROM funding_sagas\").execute(&mut *tx).await?; }\n";
    let open = unguarded(joined);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open[0].contains("UPDATE stream_suspensions (in hj(), no key before it)"),
        "{open:?}"
    );
    assert!(open[1].contains("DELETE funding_sagas (in hk(), no key before it)"), "{open:?}");
    // A boundary or SQL `const` is followed into every file that can see it:
    // a private one into its module's descendants (`super::DONE`).
    let tree = |parent: &str, child: &str| {
        vec![
            ("db.rs".to_string(), None, parent.to_string(), None),
            ("db/child.rs".to_string(), None, child.to_string(), Some("db.rs".to_string())),
        ]
    };
    let parent = "const DONE: &str = \"COMMIT\";\n\
                  const SQL: &str = \"UPDATE monthly_state SET x = 1\";\n\
                  async fn own(pool: &PgPool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; sqlx::query(SQL).execute(&mut *tx).await?; }\n";
    let child = "async fn c1(pool: &PgPool) { let mut tx = pool.begin().await?; \
                 participate(&mut tx).await?; sqlx::query(super::DONE).execute(&mut *tx).await?; \
                 sqlx::query(\"DELETE FROM process_epochs\").execute(&mut *tx).await?; }\n\
                 async fn c2(pool: &PgPool) { sqlx::query(super::SQL).execute(pool).await?; }\n";
    let open = Guard::new(tree(parent, child), &[], &[], &[]).report().1;
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open.iter()
            .any(|p| p.starts_with("db/child.rs:1: DELETE process_epochs (in c1()")),
        "{open:?}"
    );
    assert!(
        open.iter()
            .any(|p| p.starts_with("db/child.rs:2: UPDATE monthly_state via SQL (in c2()")),
        "{open:?}"
    );
    // Boundary SQL a binding holds where no holder follows it fails, unless
    // it is proven to run there (an awaited `begin_with`, through its method
    // chain); an unrelated await proves nothing.
    let unfollowed = "const BLOCK: &str = { \"COMMIT\" };\n\
                      async fn ub(pool: &PgPool, c: bool) { let mut tx = pool.begin().await?; \
                      let q = sqlx::query(\"COMMIT\"); \
                      let d = if c { \"ROLLBACK\" } else { \"SELECT 1\" }; \
                      let done = std::future::ready(\"COMMIT\").await; \
                      let mut held = pool.begin_with(\"BEGIN ISOLATION LEVEL SERIALIZABLE\").await?; \
                      let mut spanned = pool.begin_with(\"BEGIN\").instrument(span).await?; }\n";
    let open = unguarded(unfollowed);
    assert_eq!(open.len(), 4, "{open:?}");
    for line in ["inline.rs:1: boundary SQL", "inline.rs:2: boundary SQL"] {
        assert!(open.iter().any(|p| p.starts_with(line)), "{line}: {open:?}");
    }
    assert!(
        open.iter()
            .all(|p| p.contains("boundary SQL held where its uses are not followed")),
        "{open:?}"
    );
    // Boundary SQL a closure or an `async` block holds runs where that is
    // called or awaited: a `let`-bound one is a boundary at each call (the
    // seed's shape: built before its `BEGIN`, called after the key, a
    // statement before the COMMIT or an inner binding notwithstanding, so the
    // insert after the call is open), whose every use must run it (an alias,
    // `let finish = release4;`, fails; `timeout_at(deadline, work).await` runs
    // it); one passed on fails; and a binding's block holding it after another
    // statement is walked out of.
    let deferred = "async fn seedish(pool: &PgPool) -> Result<(), E> { \
                    let release = async |c: &mut PgConnection| { std::hint::black_box(()); \
                    sqlx::query(\"COMMIT\").execute(c).await }; \
                    let quick = async |c: &mut PgConnection| sqlx::query(\"ROLLBACK\").execute(c).await; \
                    let release3 = async |c: &mut PgConnection| { \
                    let result = sqlx::query(\"ROLLBACK\").execute(c).await; result }; \
                    let x = { std::hint::black_box(()); sqlx::query(\"COMMIT\") }; \
                    tokio::spawn(async move { sqlx::query(\"COMMIT\").execute(&mut *conn).await }); \
                    let mut tx = pool.begin().await?; \
                    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?; \
                    release(&mut *tx).await?; \
                    sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(&mut *tx).await?; \
                    quick(&mut *tx).await?; release3(&mut *tx).await?; \
                    let release4 = async |c: &mut PgConnection| { sqlx::query(\"COMMIT\").execute(c).await }; \
                    let finish = release4; finish(&mut *tx).await?; \
                    let work = async { sqlx::query(\"COMMIT\").execute(&mut *conn).await }; \
                    timeout_at(deadline, work).await?; \
                    let release5 = async |c: &mut PgConnection| { sqlx::query(\"COMMIT\").execute(c).await }; \
                    let later = std::future::ready(release5).await; later(&mut *tx).await?; \
                    let release6 = async |c: &mut PgConnection| { sqlx::query(\"COMMIT\").execute(c).await }; \
                    let finish6 = async |c: &mut PgConnection| release6(c).await; \
                    finish6(&mut *tx).await?; Ok(()) }\n";
    let open = unguarded(deferred);
    assert_eq!(open.len(), 7, "{open:?}");
    assert!(
        open.iter().any(|p| p.contains("seedish() ends a transaction it did not open")),
        "{open:?}"
    );
    assert!(
        open.iter()
            .any(|p| p.contains("transaction boundary in release6, used where it does not run")),
        "{open:?}"
    );
    assert!(
        open.iter()
            .any(|p| p.contains("transaction boundary in release5, used where it does not run")),
        "{open:?}"
    );
    assert!(
        open.iter()
            .any(|p| p.contains("transaction boundary in release4, used where it does not run")),
        "{open:?}"
    );
    for what in [
        "INSERT event_log (in seedish(), no key before it)",
        "boundary SQL held where its uses are not followed",
        "transaction boundary in a closure or async block passed on",
    ] {
        assert!(open.iter().any(|p| p.contains(what)), "{what}: {open:?}");
    }
    // So does a boundary call there: `tx.commit()`, `tx.rollback()` in a
    // closure called after the key, or passed on; a scope passed on that opens
    // its own transaction first, provably (at its own level, unconditionally,
    // awaited), ends only that one, while an opener in a skipped branch or
    // never awaited proves nothing.
    let calls_deferred = "async fn calls(pool: &PgPool) -> Result<(), E> { \
                          let finish = async |tx: Transaction<'static, Postgres>| { tx.commit().await }; \
                          let undo = async |tx: Transaction<'static, Postgres>| tx.rollback().await; \
                          let mut tx = pool.begin().await?; \
                          participate(&mut tx).await?; \
                          finish(tx).await?; \
                          sqlx::query(\"UPDATE monthly_state SET trades_opened = trades_opened\").execute(pool).await?; \
                          let mut tx = pool.begin().await?; \
                          participate(&mut tx).await?; \
                          undo(tx).await?; \
                          sqlx::query(\"DELETE FROM cash_anchors\").execute(pool).await?; \
                          tokio::spawn(async move { other.commit().await }); \
                          run(async { let mut own = pool.begin().await?; own.commit().await }).await; \
                          run(async { if false { let _unused = pool.begin().await?; } other.commit().await }).await; \
                          run(async { let _dropped = pool.begin(); other.rollback().await }).await; Ok(()) }\n";
    let open = unguarded(calls_deferred);
    assert_eq!(open.len(), 6, "{open:?}");
    assert!(
        open.iter().any(|p| p.contains("calls() ends a transaction it did not open")),
        "{open:?}"
    );
    assert_eq!(
        open.iter()
            .filter(|p| p.contains("transaction boundary in a closure or async block passed on"))
            .count(),
        3,
        "{open:?}"
    );
    for what in [
        "UPDATE monthly_state (in calls(), no key before it)",
        "DELETE cash_anchors (in calls(), no key before it)",
        "transaction boundary in a closure or async block passed on",
    ] {
        assert!(open.iter().any(|p| p.contains(what)), "{what}: {open:?}");
    }
    // Boundary SQL in a closure or an `async` block counts where that runs
    // only when it runs inside: a value the body returns (`async { "COMMIT"
    // }`, a query only built, a held constant's use, a runner's call never
    // awaited) fails; a held constant run inside is followed to the call (the
    // insert after it is open); an awaited `completion::run(…)` runs it, and a
    // `let` holding it there is followed to its use.
    let returned = "async fn returned(pool: &PgPool) -> Result<(), E> { \
                    let work = async { \"COMMIT\" }; let done = work.await; \
                    let built = || sqlx::query(\"ROLLBACK\"); \
                    const DONE: &str = \"COMMIT\"; \
                    let release = async |c: &mut PgConnection| sqlx::query(DONE).execute(c).await; \
                    let pending = async { DONE }; \
                    let ran = async |c: &mut PgConnection| completion::run(c, sqlx::query(\"COMMIT\"), false).await; \
                    let unrun = async |c: &mut PgConnection| { \
                    let p = completion::run(c, sqlx::query(\"COMMIT\"), false); p }; \
                    let inner = async |c: &mut PgConnection| { \
                    let sql = \"ROLLBACK\"; sqlx::query(sql).execute(c).await }; \
                    let mut tx = pool.begin().await?; \
                    participate(&mut tx).await?; \
                    release(&mut *tx).await?; \
                    sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(&mut *tx).await?; \
                    Ok(()) }\n";
    let open = unguarded(returned);
    assert_eq!(open.len(), 6, "{open:?}");
    assert!(
        open.iter().any(|p| p.contains("returned() ends a transaction it did not open")),
        "{open:?}"
    );
    // Boundary SQL anywhere else must run where it stands: a function's value
    // (`release_sql()`, a branch of `maybe()`), a constant's use as one
    // (`done()`) or kept (`let alias = DONE;`), and SQL handed to an
    // unreviewed call (`forward(…)`) fail; run where it stands, it passes.
    let produced = "fn release_sql() -> &'static str { \"COMMIT\" }\n\
                    fn maybe(c: bool) -> &'static str { if c { \"ROLLBACK\" } else { \"SELECT 1\" } }\n\
                    const DONE: &str = \"COMMIT\";\n\
                    fn done() -> &'static str { DONE }\n\
                    async fn ran(c: &mut PgConnection) -> Result<(), E> { \
                    sqlx::query(\"COMMIT\").execute(&mut *c).await?; \
                    sqlx::query(DONE).execute(c).await?; Ok(()) }\n\
                    async fn handed(c: &mut PgConnection) -> Result<(), E> { \
                    forward(c, \"COMMIT\").await?; let alias = DONE; Ok(()) }\n";
    let found = unguarded(produced);
    assert_eq!(found.len(), 6, "{found:?}");
    for line in [
        "inline.rs:1: boundary SQL not run where it stands",
        "inline.rs:2: boundary SQL not run where it stands",
        "inline.rs:4: boundary SQL via DONE not run where it is used",
        "inline.rs:6: boundary SQL not run where it stands",
        "inline.rs:6: boundary SQL via DONE not run where it is used",
        "inline.rs:5: ran() ends a transaction it did not open",
    ] {
        assert!(found.iter().any(|p| p.starts_with(line)), "{line}: {found:?}");
    }
    // A reviewed releaser ends its caller's transaction: each call of it is
    // a boundary (`h1()`), through another (`h2()`), out of a closure
    // (`h5()`, built before the `BEGIN`, called after the key, so `h5()`
    // itself ends one it did not open); a function that opened its own first
    // ends only that one (`own()`, `inner()` in a block: `h3()`, `h6()`
    // covered); a releaser used other than called fails (`h4()`).
    let helpers = "async fn finish_seed(c: &mut PgConnection) -> Result<(), E> { \
                   sqlx::query(\"COMMIT\").execute(c).await?; Ok(()) }\n\
                   async fn finish_twice(c: &mut PgConnection) -> Result<(), E> { finish_seed(c).await }\n\
                   async fn own(pool: &PgPool) -> Result<(), E> { let mut t = pool.begin().await?; \
                   sqlx::query(\"COMMIT\").execute(&mut *t).await?; Ok(()) }\n\
                   async fn inner(pool: &PgPool, x: bool) -> Result<(), E> { if x { \
                   let t = pool.begin().await?; t.commit().await?; } Ok(()) }\n\
                   async fn h1(pool: &PgPool) -> Result<(), E> { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; finish_seed(&mut *tx).await?; \
                   sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(&mut *tx).await?; Ok(()) }\n\
                   async fn h2(pool: &PgPool) -> Result<(), E> { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; finish_twice(&mut *tx).await?; \
                   sqlx::query(\"UPDATE funding_sagas SET x = 1\").execute(&mut *tx).await?; Ok(()) }\n\
                   async fn h3(pool: &PgPool) -> Result<(), E> { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; own(pool).await?; \
                   sqlx::query(\"UPDATE process_epochs SET x = 1\").execute(&mut *tx).await?; Ok(()) }\n\
                   async fn h4(pool: &PgPool) -> Result<(), E> { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; let f = finish_seed; f(&mut *tx).await?; Ok(()) }\n\
                   async fn h5(pool: &PgPool) -> Result<(), E> { \
                   let release = async |c: &mut PgConnection| finish_seed(c).await; \
                   let mut tx = pool.begin().await?; participate(&mut tx).await?; release(&mut *tx).await?; \
                   sqlx::query(\"UPDATE stream_suspensions SET x = 1\").execute(&mut *tx).await?; Ok(()) }\n\
                   async fn h6(pool: &PgPool) -> Result<(), E> { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; inner(pool, true).await?; \
                   sqlx::query(\"UPDATE recovery_incidents SET x = 1\").execute(&mut *tx).await?; Ok(()) }\n";
    let found = Guard::new(vec![("inline.rs".into(), None, helpers.into(), None)], &[], &[], &[])
        .with_releasers(&[
            entry("inline.rs", "finish_seed", Reach::Module, Form::Free),
            entry("inline.rs", "finish_twice", Reach::Module, Form::Free),
        ])
        .report()
        .1;
    assert_eq!(found.len(), 5, "{found:?}");
    for what in [
        "INSERT event_log (in h1(), no key before it)",
        "UPDATE funding_sagas (in h2(), no key before it)",
        "finish_seed ends a transaction it did not open, used where it does not run",
        "UPDATE stream_suspensions (in h5(), no key before it)",
        "inline.rs:9: h5() ends a transaction it did not open",
    ] {
        assert!(found.iter().any(|p| p.contains(what)), "{what}: {found:?}");
    }
    // Unreviewed, a function ending a transaction it did not open fails; a
    // reviewed entry that ends none fails too.
    let unreviewed = "async fn finish_seed(c: &mut PgConnection) -> Result<(), E> { \
                      sqlx::query(\"COMMIT\").execute(c).await?; Ok(()) }\n\
                      async fn own(pool: &PgPool) -> Result<(), E> { let mut t = pool.begin().await?; \
                      t.commit().await?; Ok(()) }\n";
    let found =
        Guard::new(vec![("inline.rs".into(), None, unreviewed.into(), None)], &[], &[], &[])
            .with_releasers(&[entry("inline.rs", "own", Reach::Module, Form::Free)])
            .report()
            .1;
    assert_eq!(found.len(), 2, "{found:?}");
    for what in [
        "inline.rs:1: finish_seed() ends a transaction it did not open",
        "inline.rs: releaser entry own() ends no transaction it did not open",
    ] {
        assert!(found.iter().any(|p| p.contains(what)), "{what}: {found:?}");
    }
    // A `DO $$ … $$` body's `BEGIN` and `END IF;` are PL/pgSQL, no
    // transaction boundary: the insert after it stays covered.
    let plpgsql = "async fn pl(pool: &PgPool) -> Result<(), E> { let mut tx = pool.begin().await?; \
                   participate(&mut tx).await?; \
                   sqlx::query(\"DO $$ BEGIN IF true THEN PERFORM 1; END IF; END $$\").execute(&mut *tx).await?; \
                   sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(&mut *tx).await?; \
                   tx.commit().await?; Ok(()) }\n";
    assert!(unguarded(plpgsql).is_empty(), "{:?}", unguarded(plpgsql));
    // An opener proves a function owns what it ends only on a connection it
    // obtained itself (`raw_fresh()`, `opener_fresh()`), or as an sqlx
    // opener method (`savepoint()`: on a connection in a transaction, a
    // savepoint); a raw `BEGIN`, or an opener function, on a connection it
    // was handed proves nothing (`raw_supplied()`, `opener_supplied()`); nor
    // does one closed again before the release (`closed_savepoint()`),
    // unless closed in a branch that leaves first (`early_exit()`); a raw
    // `COMMIT` needs an opener that cannot be nested: a savepoint proves
    // nothing for it (`savepoint_raw()`, nor for a scope passed on,
    // `escaping_raw()`), a pool's `begin()` does (`pool_raw()`).
    let owners = "async fn raw_supplied(c: &mut PgConnection) -> Result<(), E> { \
                  sqlx::query(\"BEGIN\").execute(&mut *c).await?; \
                  sqlx::query(\"COMMIT\").execute(c).await?; Ok(()) }\n\
                  async fn raw_fresh(pool: &PgPool) -> Result<(), E> { let mut c = pool.acquire().await?; \
                  sqlx::query(\"BEGIN\").execute(&mut *c).await?; \
                  sqlx::query(\"COMMIT\").execute(&mut *c).await?; Ok(()) }\n\
                  async fn opener_supplied(c: &mut PgConnection) -> Result<(), E> { \
                  begin_bounded(c, d, t).await?; c.execute(sqlx::raw_sql(\"COMMIT\")).await?; Ok(()) }\n\
                  async fn opener_fresh(pool: &PgPool) -> Result<(), E> { \
                  let mut connection = options.connect().await?; \
                  begin_bounded(&mut connection, d, t).await?; \
                  completion::run(&mut connection, sqlx::query(\"COMMIT\"), false).await?; Ok(()) }\n\
                  async fn savepoint(c: &mut PgConnection) -> Result<(), E> { \
                  let t = c.begin().await?; t.commit().await?; Ok(()) }\n\
                  async fn closed_savepoint(c: &mut PgConnection) -> Result<(), E> { \
                  let sp = c.begin().await?; sp.commit().await?; \
                  sqlx::query(\"COMMIT\").execute(c).await?; Ok(()) }\n\
                  async fn early_exit(c: &mut PgConnection, x: bool) -> Result<(), E> { \
                  let t = c.begin().await?; if x { t.rollback().await?; return Ok(()); } \
                  t.commit().await?; Ok(()) }\n\
                  async fn savepoint_raw(c: &mut PgConnection) -> Result<(), E> { \
                  let mut sp = c.begin().await?; \
                  sqlx::query(\"COMMIT\").execute(&mut *sp).await?; std::mem::forget(sp); Ok(()) }\n\
                  async fn pool_raw(pool: &'static PgPool) -> Result<(), E> { \
                  let mut t = pool.begin().await?; \
                  sqlx::query(\"COMMIT\").execute(&mut *t).await?; Ok(()) }\n\
                  async fn escaping_raw(c: &mut PgConnection) -> Result<(), E> { \
                  run(async { let mut sp = c.begin().await?; \
                  sqlx::query(\"COMMIT\").execute(&mut *sp).await }).await; Ok(()) }\n";
    let found = unguarded(owners);
    assert_eq!(found.len(), 6, "{found:?}");
    for what in [
        "inline.rs:1: raw_supplied() ends a transaction it did not open",
        "inline.rs:3: opener_supplied() ends a transaction it did not open",
        "inline.rs:6: closed_savepoint() ends a transaction it did not open",
        "inline.rs:8: savepoint_raw() ends a transaction it did not open",
        "inline.rs:10: escaping_raw() ends a transaction it did not open",
        "inline.rs:10: transaction boundary in a closure or async block passed on",
    ] {
        assert!(found.iter().any(|p| p.contains(what)), "{what}: {found:?}");
    }
    assert_eq!(
        open.iter()
            .filter(|p| p.contains("boundary SQL in a closure or async block, not run there"))
            .count(),
        4,
        "{open:?}"
    );
    assert!(
        open.iter()
            .any(|p| p.contains("INSERT event_log (in returned(), no key before it)")),
        "{open:?}"
    );
    // A rollback to a savepoint releases every lock taken since, the key's
    // among them: the key before one covers nothing after it, in a literal or
    // a binding (conservatively, wherever the savepoint was set).
    let savepoint = "async fn sp(pool: &PgPool) { let mut tx = pool.begin().await?; \
                     sqlx::query(\"SAVEPOINT s\").execute(&mut *tx).await?; \
                     participate(&mut tx).await?; \
                     sqlx::query(\"ROLLBACK TO SAVEPOINT s\").execute(&mut *tx).await?; \
                     sqlx::query(\"UPDATE positions_current SET x = 1\").execute(&mut *tx).await?; \
                     tx.commit().await?; }\n\
                     const UNDO: &str = \"rollback to s\";\n\
                     async fn sq(pool: &PgPool) { let mut tx = pool.begin().await?; \
                     sqlx::query(\"SAVEPOINT s\").execute(&mut *tx).await?; \
                     participate(&mut tx).await?; \
                     sqlx::query(UNDO).execute(&mut *tx).await?; \
                     sqlx::query(\"DELETE FROM order_intents\").execute(&mut *tx).await?; \
                     tx.commit().await?; }\n";
    let open = unguarded(savepoint);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open[0].contains("UPDATE positions_current (in sp(), no key before it)"),
        "{open:?}"
    );
    assert!(open[1].contains("DELETE order_intents (in sq(), no key before it)"), "{open:?}");
    // A boundary between the SQL and its execution, in a later argument,
    // releases the key before the statement runs (a `;` before it in that
    // block ends nothing).
    // A key whose failure is not ruled out is none: its result dropped
    // (`let _ = participate(…).await;`) or turned into a value (`.ok()`).
    let swallowed = "async fn z1(pool: &PgPool) { let mut tx = pool.begin().await?; \
                     let _ = participate(&mut tx).await; \
                     sqlx::query(\"UPDATE monthly_state SET x = 1\").execute(&mut *tx).await?; \
                     tx.commit().await?; }\n\
                     async fn z2(pool: &PgPool) { let mut tx = pool.begin().await?; \
                     participate(&mut tx).await.ok(); \
                     sqlx::query(\"UPDATE cash_anchors SET x = 1\").execute(&mut *tx).await?; \
                     tx.commit().await?; }\n";
    let open = unguarded(swallowed);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(open[0].contains("UPDATE monthly_state (in z1(), no key before it)"), "{open:?}");
    assert!(open[1].contains("UPDATE cash_anchors (in z2(), no key before it)"), "{open:?}");
    let late = "async fn l(pool: &PgPool) { let mut tx = pool.begin().await?; \
                participate(&mut tx).await?; \
                sqlx::query(\"UPDATE monthly_state SET trades_opened = $1\")\
                .bind({ tx.commit().await?; 0_i32 }).execute(pool).await?; }\n\
                async fn m(pool: &PgPool) { let mut tx = pool.begin().await?; \
                participate(&mut tx).await?; \
                sqlx::query(\"UPDATE monthly_state SET trades_opened = $1\")\
                .bind({ let v = 0_i32; tx.commit().await?; v }).execute(pool).await?; }\n";
    let open = unguarded(late);
    assert_eq!(open.len(), 2, "{open:?}");
    for (o, f) in open.iter().zip(["l", "m"]) {
        assert!(
            o.contains(&format!(
                "UPDATE monthly_state (in {f}(), a transaction boundary between it and its execution)"
            )),
            "{open:?}"
        );
    }
    // Nor when the SQL is the value of a block whose value flows on in the
    // statement (inside the call: `{ "…" }`, an `if`'s branches; or a block,
    // an `if … else` or a `match` followed by the chain): the statement goes
    // on past the block. A site that is the tail of a branch whose value
    // nothing chains on ends with it: a boundary in the other branch is not
    // its own.
    let nested = "async fn n(pool: &PgPool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; \
                  sqlx::query({ \"UPDATE monthly_state SET trades_opened = $1\" })\
                  .bind({ tx.commit().await?; 0_i32 }).execute(pool).await?; }\n\
                  async fn o(pool: &PgPool, c: bool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; \
                  sqlx::query(if c { \"UPDATE monthly_state SET x = 1\" } \
                  else { \"UPDATE cash_anchors SET x = 2\" })\
                  .bind({ tx.commit().await?; 0_i32 }).execute(pool).await?; }\n\
                  async fn p(pool: &PgPool, c: bool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; \
                  let r = if c { sqlx::query(\"UPDATE monthly_state SET y = 1\")\
                  .execute(&mut *tx).await? } \
                  else { tx.rollback().await?; return Ok(()); }; tx.commit().await?; keep(r); }\n\
                  async fn q(pool: &PgPool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; \
                  let r = { sqlx::query(\"UPDATE process_epochs SET z = 1\") }\
                  .bind({ tx.commit().await?; 0_i32 }).execute(pool).await?; keep(r); }\n\
                  async fn s(pool: &PgPool, c: bool) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; \
                  let r = if c { sqlx::query(\"UPDATE order_intents SET z = 2\") } \
                  else { sqlx::query(\"UPDATE recovery_incidents SET z = 3\") }\
                  .bind({ tx.commit().await?; 0_i32 }).execute(pool).await?; keep(r); }\n\
                  async fn t(pool: &PgPool, k: u8) { let mut tx = pool.begin().await?; \
                  participate(&mut tx).await?; \
                  let r = match k { 1 => { sqlx::query(\"UPDATE stream_suspensions SET a = 1\") } \
                  _ => { sqlx::query(\"UPDATE funding_sagas SET a = 2\") } }\
                  .bind({ tx.commit().await?; 0_i32 }).execute(pool).await?; keep(r); }\n";
    let open = unguarded(nested);
    assert_eq!(open.len(), 8, "{open:?}");
    for (f, table) in [
        ("n", "monthly_state"),
        ("o", "monthly_state"),
        ("o", "cash_anchors"),
        ("q", "process_epochs"),
        ("s", "order_intents"),
        ("s", "recovery_incidents"),
        ("t", "stream_suspensions"),
        ("t", "funding_sagas"),
    ] {
        let expected = format!(
            "UPDATE {table} (in {f}(), a transaction boundary between it and its execution)"
        );
        assert!(open.iter().any(|o| o.contains(&expected)), "{expected}: {open:?}");
    }
    let exited = "async fn branch(pool: &PgPool) { let mut tx = pool.begin().await; \
                  participate(&mut tx).await?; if refused { tx.rollback().await; return; } \
                  sqlx::query(\"UPDATE order_intents SET x = 1\").execute(&mut *tx).await; \
                  tx.commit().await; }\n";
    assert!(unguarded(exited).is_empty(), "{:?}", unguarded(exited));
    // A COMMIT in a block that does not leave the function still ends the
    // key's transaction: a bare block, a loop left by `break`.
    let bare = "async fn released(pool: &PgPool) { let mut tx = pool.begin().await; \
                participate(&mut tx).await?; { tx.commit().await; } \
                sqlx::query(\"UPDATE positions_current SET x = 1\").execute(pool).await; }\n";
    assert_eq!(unguarded(bare).len(), 1, "{:?}", unguarded(bare));
    // A key in one arm of a `match` reaches no write in another.
    let arms = "async fn arms(pool: &PgPool, skip: bool) { let mut tx = pool.begin().await; \
                match skip { true => participate(&mut tx).await.map(drop)?, \
                false => { sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\")\
                .execute(&mut *tx).await?; } }; tx.commit().await; }\n";
    assert_eq!(unguarded(arms).len(), 1, "{:?}", unguarded(arms));
    // A transaction opened by `begin_with`, or ended by a SQL `COMMIT`, is a
    // boundary (after that `COMMIT`, the later `tx.commit()` has no
    // outstanding opener: `seed()` ends a transaction it did not open).
    for (boundary, count) in [
        ("let mut tx = conn.begin_with(begin_sql).await;", 1),
        ("sqlx::query(\"COMMIT\").execute(&mut *tx).await;", 2),
    ] {
        let source = right.replace(
            "participate(&mut tx).await?;",
            &format!("participate(&mut tx).await?; {boundary}"),
        );
        let open = unguarded(&source);
        assert_eq!(open.len(), count, "{boundary}: {open:?}");
        assert!(
            count == 1
                || open.iter().any(|p| p.contains("seed() ends a transaction it did not open")),
            "{open:?}"
        );
    }
    // A boundary or a key read across whitespace or a comment is one.
    let spaced = bare.replace("{ tx.commit().await; }", "tx . commit /* end */ ().await;");
    assert_eq!(unguarded(&spaced).len(), 1, "{:?}", unguarded(&spaced));
    let spaced_key = right.replace("participate(&mut tx)", "participate /* key */ (&mut tx)");
    assert!(unguarded(&spaced_key).is_empty(), "{:?}", unguarded(&spaced_key));
    // Only a key that runs counts: not a dropped or stored future, not one in
    // a closure, not the key's SQL left unexecuted, not a proof outside the
    // signature.
    let pinned =
        right.replace("participate(&mut tx).await?;", "Box::pin(participate(&mut tx)).await?;");
    assert!(unguarded(&pinned).is_empty(), "{:?}", unguarded(&pinned));
    let statement = right.replace(
        "participate(&mut tx).await?;",
        "sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;",
    );
    assert!(unguarded(&statement).is_empty(), "{:?}", unguarded(&statement));
    // A key call's own arguments run before the key: a write there is open,
    // the write after the call is keyed.
    let eager = right.replace(
        "participate(&mut tx).await?;",
        "participate({ sqlx::query(\"UPDATE monthly_state SET trades_opened = 0\")\
         .execute(&mut *tx).await; &mut *tx }).await?;",
    );
    let open = unguarded(&eager);
    assert!(open.len() == 1 && open[0].contains("UPDATE monthly_state"), "{open:?}");
    for unrun in [
        "drop(participate(&mut tx));",
        "let _ = false && participate(&mut tx).await.is_ok();",
        "drop(Box::pin(participate(&mut tx)));",
        "let k = async || participate(&mut tx).await?;",
        "let _ = sqlx::query(CERTIFICATE_PARTICIPATE);",
        "let _ = sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx);",
        "sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).now_or_never().transpose()?;",
        "let _ = (sqlx::query::<Postgres>(CERTIFICATE_PARTICIPATE), \
         sqlx::query(\"SELECT 1\").execute(&mut *tx).await);",
        // The same with their failure propagated (`?`): still no key, since
        // none of them is a key that runs.
        "participate(&mut tx).now_or_never().transpose()?;",
        "let _ = false && participate(&mut tx).await?.is_some();",
        "let _ = (sqlx::query::<Postgres>(CERTIFICATE_PARTICIPATE), \
         sqlx::query(\"SELECT 1\").execute(&mut *tx).await?);",
        "let _p: Option<&Participating> = None;",
    ] {
        let source = right.replace("participate(&mut tx).await?;", unrun);
        assert_eq!(unguarded(&source).len(), 1, "{unrun}: {:?}", unguarded(&source));
    }
    // The await requirement itself, apart from any success check
    // ([`established`]): a key call or the key's SQL executor not awaited is no
    // credit at all; awaited, each is.
    for (form, m, awaited) in [
        ("participate(&mut tx).now_or_never();", "participate(", false),
        ("participate(&mut tx).await;", "participate(", true),
        (
            "sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).now_or_never();",
            "CERTIFICATE_PARTICIPATE",
            false,
        ),
        (
            "sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await;",
            "CERTIFICATE_PARTICIPATE",
            true,
        ),
    ] {
        let text = code_only(form);
        let pos = text.find(m).expect(m);
        assert_eq!(credited(&text, m, pos, text.len()).is_some(), awaited, "{form}");
    }
    // The proof counts only as a parameter's whole type in the function's
    // own parameter list: an optional proof (`o1()`, callable with `None`),
    // one in the return type (`o2()`) or a function-pointer parameter's
    // (`o4()`) is none; a direct one (`o3()`, `o5<'c>()`) is the key, held by
    // the caller.
    let proofs = "async fn o1(pool: &PgPool, _proof: Option<&Participating>) { \
                  sqlx::query(\"UPDATE monthly_state SET trades_opened = 1\").execute(pool).await?; }\n\
                  async fn o2(pool: &PgPool) -> Option<&Participating> { \
                  sqlx::query(\"UPDATE cash_anchors SET x = 1\").execute(pool).await?; None }\n\
                  async fn o3(tx: &mut PgConnection, _key: &Participating) { \
                  sqlx::query(\"UPDATE process_epochs SET x = 1\").execute(&mut *tx).await?; }\n\
                  async fn o4(pool: &PgPool, _callback: fn(key: &Participating)) { \
                  sqlx::query(\"UPDATE order_intents SET x = 1\").execute(pool).await?; }\n\
                  async fn o5<'c>(tx: &mut PgConnection, _key: &Participating) { \
                  sqlx::query(\"UPDATE recovery_incidents SET x = 1\").execute(&mut *tx).await?; }\n";
    let open = unguarded(proofs);
    assert_eq!(open.len(), 3, "{open:?}");
    assert!(open[0].contains("UPDATE monthly_state (in o1(), no key before it)"), "{open:?}");
    assert!(open[1].contains("UPDATE cash_anchors (in o2(), no key before it)"), "{open:?}");
    assert!(open[2].contains("UPDATE order_intents (in o4(), no key before it)"), "{open:?}");
    let looped = bare.replace("{ tx.commit().await; }", "loop { tx.commit().await; break; }");
    assert_eq!(unguarded(&looped).len(), 1, "{:?}", unguarded(&looped));
    let conditional =
        bare.replace("{ tx.commit().await; }", "{ tx.commit().await; if leave { return; } }");
    assert_eq!(unguarded(&conditional).len(), 1, "{:?}", unguarded(&conditional));
    // A `return` that a `break` can bypass, in a loop body or in a branch
    // inside a loop, and one in an `async` block (it leaves the block, not
    // the function) keep the boundary too.
    let looped_return = bare.replace(
        "{ tx.commit().await; }",
        "loop { tx.commit().await; if resume { break; } return; }",
    );
    assert_eq!(unguarded(&looped_return).len(), 1, "{:?}", unguarded(&looped_return));
    let bypassed = bare.replace(
        "{ tx.commit().await; }",
        "loop { if x { tx.commit().await; if resume { break; } return; } }",
    );
    assert_eq!(unguarded(&bypassed).len(), 1, "{:?}", unguarded(&bypassed));
    let in_async =
        bare.replace("{ tx.commit().await; }", "let done = async { tx.commit().await; return; };");
    assert_eq!(unguarded(&in_async).len(), 1, "{:?}", unguarded(&in_async));
    // A branch's conditional `return` keeps it; so does a branch's `return`
    // inside an `async` block that the write follows (it leaves the block).
    let branch_conditional = bare.replace(
        "{ tx.commit().await; }",
        "if released { tx.commit().await; if leave { return; } }",
    );
    assert_eq!(unguarded(&branch_conditional).len(), 1, "{:?}", unguarded(&branch_conditional));
    let async_branch = bare.replace(
        "{ tx.commit().await; }",
        "let done = async { if true { tx.commit().await; return; } }; done.await;",
    );
    assert_eq!(unguarded(&async_branch).len(), 1, "{:?}", unguarded(&async_branch));
    // A `return` in a string, or ending a braceless closure, is no exit.
    let quoted = bare.replace(
        "{ tx.commit().await; }",
        "if released { tx.commit().await; tracing::debug!(\"done; return\"); }",
    );
    assert_eq!(unguarded(&quoted).len(), 1, "{:?}", unguarded(&quoted));
    let closure = bare.replace(
        "{ tx.commit().await; }",
        "if released { tx.commit().await; let done = || return; }",
    );
    assert_eq!(unguarded(&closure).len(), 1, "{:?}", unguarded(&closure));
    // Key evidence in a block closed before the write does not reach it: a
    // scoped transaction, a conditional key.
    let scoped = "async fn s(pool: &PgPool) { { let mut tx = pool.begin().await; \
                  participate(&mut tx).await?; } \
                  sqlx::query(\"UPDATE positions_current SET x = 1\").execute(pool).await; }\n";
    assert_eq!(unguarded(scoped).len(), 1, "{:?}", unguarded(scoped));
    let conditional_key = "async fn c(pool: &PgPool) { let mut tx = pool.begin().await; \
                           if x { participate(&mut tx).await?; } \
                           sqlx::query(\"UPDATE positions_current SET x = 1\").execute(&mut *tx).await; }\n";
    assert_eq!(unguarded(conditional_key).len(), 1, "{:?}", unguarded(conditional_key));
    // A nested `fn`'s `return` leaves only it (and that `fn` ends a
    // transaction it did not open).
    let nested_fn = bare
        .replace("{ tx.commit().await; }", "fn inner() { if x { tx.commit().await; return; } }");
    let open = unguarded(&nested_fn);
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open.iter().any(|p| p.contains("inner() ends a transaction it did not open")),
        "{open:?}"
    );
    // Nor is a `return` in a line or block comment.
    let line_comment = bare
        .replace("{ tx.commit().await; }", "if released { tx.commit().await; // done; return\n }");
    assert_eq!(unguarded(&line_comment).len(), 1, "{:?}", unguarded(&line_comment));
    let block_comment = bare.replace(
        "{ tx.commit().await; }",
        "if released { tx.commit().await; /* done; /* nested */; return */ }",
    );
    assert_eq!(unguarded(&block_comment).len(), 1, "{:?}", unguarded(&block_comment));
    // The ledger writers' shape: the whole transaction in one `async` block,
    // whose refusal branch rolls back and returns from that same block.
    let bounded = "async fn ledger(&self, pool: &PgPool) { self.bounded(async { \
                   let mut tx = pool.begin().await; participate(&mut tx).await?; \
                   if refused { tx.rollback().await; return; } \
                   sqlx::query(\"UPDATE accounting_state SET v = v + 1\").execute(&mut *tx).await; \
                   tx.commit().await; }).await; }\n";
    assert!(unguarded(bounded).is_empty(), "{:?}", unguarded(bounded));
    let mut sources = production_sources();
    let (.., text, _) = sources
        .iter_mut()
        .find(|(rel, ..)| rel == "robson-db/src/init.rs")
        .expect("init.rs");
    let key = "    sqlx::query(CERTIFICATE_PARTICIPATE).execute(&mut *tx).await?;\n";
    assert_eq!(text.matches(key).count(), 1);
    *text = text.replacen(key, "", 1);
    let seed = text.find("pub async fn init_minimal_data(").expect("the seed");
    let read_commit =
        seed + text[seed..].find("    tx.commit().await?;\n").expect("its read's COMMIT");
    text.insert_str(read_commit, key);
    let open = Guard::new(sources, &WRAPPERS, &KEYING, &DELEGATED).report().1;
    assert!(
        open.iter().any(|p| p.starts_with("robson-db/src/init.rs:")
            && p.contains("INSERT event_log (in init_minimal_data(), no key before it)")),
        "{open:?}"
    );
}

/// Killer of "test-only statements read as production": a gated statement,
/// block or field is removed whole and once: a `#[cfg(test)]` key covers
/// nothing, a write in a gated block is no site, a gated field repeats no
/// text before it (one site, every line number kept), and a production write
/// after a gated `if` without `;` is still read.
#[test]
fn the_guard_removes_test_only_statements() {
    let statement =
        "async fn w(c: &mut PgConnection) {\n    #[cfg(test)]\n    participate(c).await?;\n    \
                     sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(c).await;\n}\n";
    let open = unguarded(statement);
    assert!(
        open.len() == 1 && open[0].starts_with("inline.rs:4: INSERT event_log"),
        "{open:?}"
    );
    let block = statement.replace(
        "#[cfg(test)]\n    participate(c).await?;",
        "#[cfg(test)]\n    { participate(c).await?; }",
    );
    assert_eq!(unguarded(&block).len(), 1, "{:?}", unguarded(&block));
    let inside = "async fn w(c: &mut PgConnection) {\n    #[cfg(test)]\n    \
                  { sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(c).await; }\n}\n";
    assert!(unguarded(inside).is_empty(), "{:?}", unguarded(inside));
    let field = "async fn w(c: &mut PgConnection) {\n    \
                 sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(c).await;\n    \
                 let s = S {\n        a: 1,\n        #[cfg(test)]\n        b: 2,\n        c: 3,\n    };\n}\n";
    let kept = without_test_modules(field);
    assert_eq!(kept.lines().count(), field.lines().count());
    assert!(kept.contains("c: 3") && !kept.contains("b: 2"), "{kept}");
    let (sites, open) =
        Guard::new(vec![("inline.rs".into(), None, field.into(), None)], &[], &[], &[]).report();
    assert_eq!(sites, 1);
    assert!(
        open.len() == 1 && open[0].starts_with("inline.rs:2: INSERT event_log"),
        "{open:?}"
    );
    let gated_if = "async fn w(c: &mut PgConnection) {\n    #[cfg(test)]\n    if x { y(); }\n    \
                    sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(c).await;\n}\n";
    let open = unguarded(gated_if);
    assert!(
        open.len() == 1 && open[0].starts_with("inline.rs:4: INSERT event_log"),
        "{open:?}"
    );
    // An item's body or end is its first `{` or `;` outside square brackets:
    // an array type's `;` (`[u8; 2]`) ends neither a function's signature,
    // whose write is that function's site, nor a test-only item, removed
    // whole.
    let arrays = "async fn arr(c: &mut PgConnection) -> Result<[u8; 2], E> {\n    \
                  sqlx::query(\"INSERT INTO event_log VALUES (1)\").execute(c).await?;\n    Ok([0; 2])\n}\n\
                  #[cfg(test)]\nasync fn t(c: &mut PgConnection) -> [u8; 1] {\n    \
                  sqlx::query(\"INSERT INTO event_log VALUES (2)\").execute(c).await;\n    [0]\n}\n";
    let open = unguarded(arrays);
    assert!(
        open.len() == 1
            && open[0].starts_with("inline.rs:2: INSERT event_log (in arr(), no key before it)"),
        "{open:?}"
    );
}

/// Killer of "a declared module or a quoted table missed": on a module tree
/// written to a temporary directory, a `mod x;` with a trailing comment, one
/// with its attribute on the same line and a raw `mod r#x;` are walked, a `"mod
/// x;"` in a string is no declaration, and these are sites: quoted tables in a
/// raw literal (`UPDATE "monthly_state"`), escaped ones in ordinary literals
/// (`\"monthly_state\"`, `\"public\".\"event_log\"`, a `DELETE`), tables after
/// an escaped newline and a line continuation, around SQL comments (a `--` in
/// a SQL string or a quoted identifier, raw or escaped, an `E'…'` string or a
/// dollar-quoted one starting none), behind hex and unicode escapes (with
/// underscores), across a `concat!` (spaced, any delimiter, char and number
/// pieces), with `ONLY`, `MERGE INTO`, a `TRUNCATE`
/// list, `COPY` (and `COPY BINARY`), a spaced or commented qualified name and a
/// `U&"…"` name with its `UESCAPE` (Rust comments and code are not SQL), a row
/// lock of any strength whose statement holds a quoted or dollar-quoted `;`,
/// after a comment ended by a carriage return (in a literal and in a file),
/// after a dollar quote with a non-ASCII tag, after an identifier holding `$`
/// (no dollar quote, no `E'…'` string starts in it), a catalog-qualified table,
/// a `sqlx::query_file!` (its SQL unread, so a site), a row lock on a `U&"…"`
/// spelling of `accounting_state` (inline with `UESCAPE`, included, and an
/// orphan file), an included
/// `INSERT INTO "public"."event_log"`, a file included across lines, row locks
/// on `"accounting_state"` before the key (raw, escaped, and split over lines)
/// and an included one; an input-writing SQL file reached only through a macro
/// argument fails.
#[test]
fn the_guard_reads_quoted_tables_and_commented_declarations() {
    let dir = std::env::temp_dir().join(format!("robson-8m2b-guard-{}", std::process::id()));
    // A fresh tree: the temporary directory outlives a run, and pids repeat.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    write(
        "lib.rs",
        "mod input_writer; // a production writer\n\
         mod r#raw_writer;\n\
         #[path=\"pathed_writer.rs\"] mod pathed;\n\
         #[cfg(feature = \"postgres\")] mod included_writer;\n\
         const DOC: &str = \"mod missing_module;\";\n\
         #[path /* the override */ = \"db_live.rs\"] mod db;\n\
         #[path = \"esc\\x61ped.rs\"] mod escaped;\n\
         #[cfg_attr(feature = \"postgres\", path = r#\"pg_alt.rs\"#)] mod alt;\n\
         #[cfg_attr(test, path = \"test_alt.rs\")] mod tested;\n\
         mod nest { #[path = \"nested.rs\"] mod writer; }\n\
         #[path = \"elsewhere\"] mod moved { mod inner; }\n\
         #[path = \"live.rs\"]\n\
         pub (crate) mod writer;\n\
         #[path = \"live2.rs\"] pub(in crate) mod writer2;\n",
    );
    write(
        "input_writer.rs",
        r##"async fn w(pool: &PgPool) {
    sqlx::query(r#"UPDATE "monthly_state" SET x = 0"#).execute(pool).await;
}
async fn e(pool: &PgPool) {
    sqlx::query("UPDATE \"monthly_state\" SET trades_opened = 0").execute(pool).await;
    sqlx::query("INSERT INTO \"public\".\"event_log\" VALUES (1)").execute(pool).await;
    sqlx::query("DELETE FROM \"stream_suspensions\" WHERE x = 1").execute(pool).await;
    sqlx::query("INSERT INTO\n  positions_current VALUES (1)").execute(pool).await;
    sqlx::query("UPDATE \
        cash_anchors SET x = 1").execute(pool).await;
    sqlx::query("UPDATE /* why */ process_epochs SET x = 1").execute(pool).await;
    sqlx::query("INSERT -- why\n INTO funding_sagas VALUES (1)").execute(pool).await;
    sqlx::query(r#"SELECT '--', "a--b"; DELETE /* a /* nested */ b */ FROM order_intents"#).execute(pool).await;
    sqlx::query("WITH \"a--b\" AS (SELECT 1) UPDATE monthly_state SET trades_opened = 0").execute(pool).await;
    sqlx::query("SELECT \x27--\x27; UPDATE cash_discrepancies SET x = 1").execute(pool).await;
    sqlx::query("\u{55}PDATE recovery_incidents SET x = 1").execute(pool).await;
}
async fn x(pool: &PgPool) {
    sqlx::query("\u{5_5}PDATE positions_current SET x = 1").execute(pool).await;
    sqlx::query("UPDATE ONLY event_log SET x = 1").execute(pool).await;
    sqlx::query("DELETE FROM ONLY (process_epochs) WHERE x = 1").execute(pool).await;
    sqlx::query("MERGE INTO ONLY funding_sagas USING s ON true WHEN MATCHED THEN DO NOTHING").execute(pool).await;
    sqlx::query("TRUNCATE TABLE scratch, ONLY public . cash_anchors *, x").execute(pool).await;
    sqlx::query("COPY stream_suspensions FROM STDIN").execute(pool).await;
    sqlx::query("COPY BINARY monthly_state FROM STDIN").execute(pool).await;
    sqlx::query(r#"UPDATE U&"order!005fintents" UESCAPE '!' SET x = 1"#).execute(pool).await;
    sqlx::query(r#"INSERT INTO "public" /* c */ . U&"process\005fepochs" VALUES (1)"#).execute(pool).await;
    sqlx::query(r#"SELECT E'\'--'; UPDATE monthly_state SET x = 1"#).execute(pool).await;
    sqlx::query(r#"WITH v AS (SELECT E'a''\'--') UPDATE monthly_state SET trades_opened = 0"#).execute(pool).await;
    sqlx::query(r#"WITH v AS (SELECT E'a'
'\'--') UPDATE funding_sagas SET x = 0"#).execute(pool).await;
    sqlx::query(r#"WITH v AS (SELECT E'a' -- UPDATE venue_accounts
  -- a line-ended comment
'\'--') UPDATE activation_bases SET x = 0"#).execute(pool).await;
    sqlx::query("SELECT foo$a$, '$a$ -- '; DELETE FROM cash_discrepancies").execute(pool).await;
    sqlx::query(r#"SELECT x$e'\', '--'; DELETE FROM event_log"#).execute(pool).await;
    sqlx::query("SELECT $q$--$q$; DELETE FROM recovery_incidents").execute(pool).await;
    sqlx::query(concat!("UPD", "ATE cash_discrepancies SET x = 1")).execute(pool).await;
    sqlx::query(concat!("SELECT 1 FROM accounting_state ", r"FOR UPDATE")).execute(pool).await;
    sqlx::query("SELECT 1 FROM accounting_state WHERE budget_model <> ';' FOR UPDATE").fetch_one(pool).await;
    sqlx::query(concat !("UPD", "ATE monthly_state SET trades_opened = 0")).execute(pool).await;
    sqlx::query(std::concat! {"DEL", 'E', "TE FROM cash_anchors WHERE x = ", 1}).execute(pool).await;
    sqlx::query("SELECT 1 FROM accounting_state FOR KEY SHARE").fetch_one(pool).await;
    sqlx::query("SELECT 1 FROM accounting_state WHERE x <> $$;$$ FOR SHARE").fetch_one(pool).await;
    sqlx::query("-- maintenance\rUPDATE monthly_state SET trades_opened = 0").execute(pool).await;
    sqlx::query("SELECT $é$--$é$; DELETE FROM monthly_state").execute(pool).await;
    sqlx::query("DELETE FROM robson.public.funding_sagas").execute(pool).await;
    sqlx::query(r#"SELECT 1 FROM U&"accounting!005fstate" UESCAPE '!' FOR UPDATE"#).fetch_one(pool).await;
    sqlx::query_file !("queries/any.sql").execute(pool).await;
    // update monthly_state, copy positions_current: Rust text, not SQL.
    let _ = (copy_rows, truncate);
}
async fn l(pool: &PgPool) {
    let mut tx = pool.begin().await?;
    sqlx::query(r#"SELECT 1 FROM "accounting_state" FOR UPDATE"#).execute(&mut *tx).await?;
    sqlx::query("SELECT 1 FROM \"accounting_state\" FOR NO KEY UPDATE").execute(&mut *tx).await?;
    sqlx::query(r#"SELECT 1 FROM accounting_state
        FOR
        UPDATE"#).execute(&mut *tx).await?;
    participate(&mut tx).await?;
    tx.commit().await?;
}
async fn shared(pool: &PgPool) {
    sqlx::query(SHARED).execute(pool).await;
}
"##,
    );
    write(
        "included_writer.rs",
        "const Q: &str = include_str!(\"quoted.sql\");\n\
         const L: &str = include_str!(\"locks.sql\");\n\
         const M: &str = include_str!(\n    \"multi.sql\"\n);\n\
         const C: &str = include_str!(\"cr.sql\");\n\
         const K: &str = include_str!(\"commit.sql\");\n\
         const U: &str = include_str!(\"ulock.sql\");\n\
         macro_rules! page { ($p:literal) => { include_str!($p) }; }\n\
         const O: &str = page!(\"orphan.sql\");\n\
         const S: &str = page!(\"stray_lock.sql\");\n\
         async fn g(pool: &PgPool) { sqlx::query(Q).execute(pool).await; \
         sqlx::query(L).execute(pool).await; sqlx::query(M).execute(pool).await; \
         sqlx::query(O).execute(pool).await; sqlx::query(C).execute(pool).await; }\n\
         async fn h(pool: &PgPool) { let mut tx = pool.begin().await; participate(&mut tx).await?; \
         sqlx::query(K).execute(&mut *tx).await; \
         sqlx::query(\"UPDATE positions_current SET y = 2\").execute(&mut *tx).await; }\n\
         async fn x2(pool: &PgPool) { let mut tx = pool.begin().await; participate(&mut tx).await?; \
         sqlx::query(include_str!(\"tx_\\x63ommit.sql\")).execute(&mut *tx).await; \
         sqlx::query(\"INSERT INTO recovery_incidents VALUES (1)\").execute(&mut *tx).await; }\n\
         async fn x3(pool: &PgPool) { let mut tx = pool.begin().await; participate(&mut tx).await?; \
         sqlx::query(include_str![\"commit.sql\"]).execute(&mut *tx).await; \
         sqlx::query(\"INSERT INTO order_intents VALUES (1)\").execute(&mut *tx).await; }\n\
         async fn x4(pool: &PgPool) { sqlx::query(include_str! {\"quoted.sql\"}).execute(pool).await; }\n",
    );
    write("quoted.sql", "INSERT INTO \"public\".\"event_log\" VALUES (1);\n");
    write("locks.sql", "SELECT 1 FROM \"accounting_state\" WHERE id = 1 FOR UPDATE;\n");
    write(
        "pathed_writer.rs",
        "async fn p(pool: &PgPool) { sqlx::query(\"DELETE FROM venue_accounts\").execute(pool).await; }\n\
         pub (crate) const SHARED: &str = \"UPDATE stream_suspensions SET x = 1\";\n\
         async fn q(pool: &PgPool) { let mut tx = pool.begin().await?; participate(&mut tx).await?; \
         sqlx::query(SHARED).execute(&mut *tx).await?; tx.commit().await?; }\n",
    );
    write(
        "raw_writer.rs",
        "async fn r(pool: &PgPool) { sqlx::query(\"INSERT INTO activation_bases VALUES (1)\").execute(pool).await; }\n",
    );
    write("multi.sql", "UPDATE -- why\n /* and */ recovery_incidents SET x = 1;\n");
    write("orphan.sql", "UPDATE monthly_state SET x = 1;\n");
    write(
        "cr.sql",
        "-- a note ended by a carriage return\rUPDATE stream_suspensions SET x = 1;\n",
    );
    write("commit.sql", "COMMIT;\n");
    // Included only as `"tx_\x63ommit.sql"`: a path whose literal is no SQL
    // statement of its own, so only the decoded include brings its COMMIT.
    write("tx_commit.sql", "COMMIT;\n");
    write("ulock.sql", "SELECT 1 FROM U&\"accounting\\005fstate\" FOR SHARE;\n");
    write("stray_lock.sql", "SELECT 1 FROM U&\"accounting\\005fstate\" FOR UPDATE;\n");
    // A path attribute is read as tokens (a comment inside it, its literal's
    // escapes decoded): the override, never the default beside it; a
    // `cfg_attr` path is read besides the default, unless only under `test`.
    for name in [
        "db.rs",
        "escaped.rs",
        "alt.rs",
        "pg_alt.rs",
        "tested.rs",
        "test_alt.rs",
    ] {
        write(name, "async fn empty() {}\n");
    }
    // rustc's directories: a `#[path]` inside an inline module from that
    // module's directory, an inline module's `#[path]` its directory, and a
    // `#[path]` file's modules its siblings; a decoy where each is not.
    for sub in ["nest", "moved", "elsewhere", "db_live"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    write("db_live.rs", "mod sibling;\n");
    for name in [
        "nest/nested.rs",
        "nested.rs",
        "elsewhere/inner.rs",
        "moved/inner.rs",
        "sibling.rs",
        "db_live/sibling.rs",
        "live.rs",
        "writer.rs",
        "live2.rs",
        "writer2.rs",
    ] {
        write(name, "async fn empty() {}\n");
    }
    let files = production_files(&dir.join("lib.rs"));
    let rel = |file: &Path| file.strip_prefix(&dir).unwrap().display().to_string();
    let walked: BTreeSet<String> = files.keys().map(|file| rel(file)).collect();
    assert_eq!(
        walked,
        [
            "alt.rs",
            "db_live.rs",
            "elsewhere/inner.rs",
            "escaped.rs",
            "included_writer.rs",
            "input_writer.rs",
            "lib.rs",
            "live.rs",
            "live2.rs",
            "nest/nested.rs",
            "pathed_writer.rs",
            "pg_alt.rs",
            "raw_writer.rs",
            "sibling.rs",
            "tested.rs"
        ]
        .map(String::from)
        .into(),
        "{walked:?}"
    );
    // A path attribute it cannot read fails the walk, never falls back to
    // the default file (which exists here).
    std::fs::create_dir_all(dir.join("unread")).unwrap();
    write("unread/db.rs", "async fn empty() {}\n");
    write("unread.rs", "#[path = concat!(\"db\", \"_live.rs\")] mod db;\n");
    let unread = std::panic::catch_unwind(|| production_files(&dir.join("unread.rs")));
    // Nor is Rust source an `include!` brings in.
    write("included_rust.rs", "include! { \"inc_body.rs\" }\n");
    write("inc_body.rs", "async fn empty() {}\n");
    let rust = std::panic::catch_unwind(|| production_files(&dir.join("included_rust.rs")));
    assert!(rust.is_err(), "{:?}", rust.map(|files| files.into_keys().collect::<Vec<_>>()));
    // Nor is a `cfg_attr` path on an inline module followed.
    write("unread_inline.rs", "#[cfg_attr(feature = \"x\", path = \"p\")] mod m {}\n");
    let inline = std::panic::catch_unwind(|| production_files(&dir.join("unread_inline.rs")));
    assert!(
        inline.is_err(),
        "{:?}",
        inline.map(|files| files.into_keys().collect::<Vec<_>>())
    );
    assert!(
        unread.is_err(),
        "{:?}",
        unread.map(|files| files.into_keys().collect::<Vec<_>>())
    );
    let sources = files
        .iter()
        .map(|(file, parent)| {
            let text = std::fs::read_to_string(file).unwrap();
            (rel(file), Some(dir.clone()), text, parent.as_deref().map(rel))
        })
        .collect();
    let guard = Guard::new(sources, &[], &[], &[]);
    let (sites, open) = guard.report();
    assert_eq!(sites, 55, "{open:?}");
    // Every reader fixture's write runs where it is built (a macro's braces,
    // `std::concat! {…}`, are a call's): each is open for its key alone.
    assert!(!open.iter().any(|p| p.contains("not run where it is built")), "{open:?}");
    let count = |what: &str| open.iter().filter(|p| p.contains(what)).count();
    // The second: after an included `COMMIT`, past its key.
    assert_eq!(count("UPDATE positions_current"), 2, "{open:?}");
    assert_eq!(
        count("UPDATE monthly_state"),
        7,
        "raw, escaped, CTE, E-strings, concat !, a CR-ended comment: {open:?}"
    );
    assert_eq!(count("UPDATE cash_discrepancies"), 2, "hex escape, concat!: {open:?}");
    // `include_str![…]` and `include_str! {…}` are includes too: a COMMIT one
    // brings ends the key (`x3`), and a second, unkeyed include of a writer
    // (`x4`) is a site of its own.
    assert_eq!(count("INSERT order_intents"), 1, "{open:?}");
    assert_eq!(count("include_str!(quoted.sql) writes an input"), 2, "{open:?}");
    // A `const` with a restricted visibility (`pub (crate)`) is followed into
    // every file: its unkeyed use there is open, its keyed one not.
    assert!(
        count("UPDATE stream_suspensions via SHARED") == 1
            && open.iter().any(|p| p.starts_with("input_writer.rs:")
                && p.contains("UPDATE stream_suspensions via SHARED")),
        "{open:?}"
    );
    // An `E'…'` string continued past a line break keeps its escapes (`\'`
    // no closing quote), past `--` comments too, which are blanked.
    assert_eq!(count("UPDATE venue_accounts"), 0, "a comment: {open:?}");
    for what in [
        "INSERT event_log",
        "DELETE stream_suspensions",
        "INSERT positions_current",
        "UPDATE cash_anchors",
        "UPDATE process_epochs",
        "INSERT funding_sagas",
        "DELETE order_intents",
        "UPDATE recovery_incidents",
        "UPDATE event_log",
        "DELETE process_epochs",
        "MERGE funding_sagas",
        "TRUNCATE cash_anchors",
        "COPY stream_suspensions",
        "COPY monthly_state",
        "UPDATE order_intents",
        "INSERT process_epochs",
        "DELETE recovery_incidents",
        // After the `COMMIT` an escaped include path (`tx_\x63ommit.sql`) brings.
        "INSERT recovery_incidents",
        "DELETE cash_anchors",
        "DELETE monthly_state",
        "DELETE funding_sagas",
        "INSERT activation_bases",
        "DELETE venue_accounts",
        "DELETE cash_discrepancies",
        "DELETE event_log",
        "UPDATE funding_sagas",
        "UPDATE activation_bases",
        "query_file!() SQL not read",
        "include_str!(cr.sql) writes an input",
        "include_str!(ulock.sql) writes an input",
        "include_str!(multi.sql) writes an input",
    ] {
        assert_eq!(count(what), 1, "{what}: {open:?}");
    }
    assert_eq!(
        count("accounting_state row lock"),
        8,
        "every row lock before the key is open: {open:?}"
    );
    assert!(
        open.iter().any(|p| p.contains("include_str!(locks.sql) writes an input")),
        "{open:?}"
    );
    let orphans = guard.orphan_sql(std::slice::from_ref(&dir));
    assert!(
        orphans.len() == 2
            && orphans[0].contains("/orphan.sql")
            && orphans[1].contains("/stray_lock.sql"),
        "{orphans:?}"
    );
}
