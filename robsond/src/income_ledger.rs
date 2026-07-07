//! Typed income-ledger reconciliation (ADR-0045 §1/§2).
//!
//! Polls the exchange's typed income stream, ingests each item idempotently,
//! and matches it against governed state. An unexplained residual becomes a
//! named, persistent alarm — never an automatic write to `capital_base`.
//! Only a residual the ledger explains as 100% `TRANSFER` may recalibrate
//! automatically (ADR-0045 §2); this module is the *only* place that does
//! so. `reconciliation_worker::recalibrate_capital_base_after_pure_financial_drift`
//! no longer writes `capital_base` for the generic case — see its doc
//! comment.
//!
//! # Matching limitation (documented, not silent)
//!
//! Binance's income items carry a `tradeId`/`tranId` linkage back to the
//! originating trade for `REALIZED_PNL`/`COMMISSION`. Robson does not
//! currently persist a queryable `exchange_trade_id` column on
//! `positions_current` or in `event_log` payloads for ordinary (non
//! reconciled-close) fills, so exact id-level matching is not available yet.
//! This module matches `REALIZED_PNL`/`COMMISSION` by (symbol, time
//! proximity to `entry_filled_at`/`closed_at`) instead — a coarser but safe
//! substitute: if more than one position falls inside the window, the item
//! is left unmatched (alarm) rather than guessed, mirroring the "ambiguous
//! evidence -> don't act" discipline `gather_user_trade_evidence` already
//! uses in `reconciliation_worker.rs`. The raw `trade_id`/`tran_id` is
//! preserved on every ledger row regardless, so a future, more precise
//! matcher (once exact linkage is persisted) can use it without re-ingesting
//! history.
//!
//! `FUNDING_FEE` never links to a governed fill by construction (it is a
//! cost of holding, not a robsond-authored action) — it is always recognized
//! and folded into `expected_wallet_balance`, never alarmed.

use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Datelike, Duration as ChronoDuration, Utc};
use robson_exec::{ports::IncomePort, ExchangePort};
use robson_store::Store;
use rust_decimal::Decimal;
use sqlx::PgPool;
use tokio::{
    sync::RwLock,
    time::{interval, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    error::DaemonResult,
    event_bus::{DaemonEvent, EventBus},
    position_manager::PositionManager,
};

/// Matching tolerance for correlating an income item's timestamp against a
/// governed position's entry/close timestamp. Robson runs a small number of
/// concurrent positions (confirmed live 2026-07-07: at most one occupied
/// slot at a time) — a coarse time+symbol window is unambiguous in practice;
/// see the module-level doc comment on why this substitutes for exact
/// tradeId linkage.
const MATCH_WINDOW: ChronoDuration = ChronoDuration::seconds(120);

/// Grace period before an unmatched, non-funding item is treated as a
/// confirmed anomaly rather than a fill still in flight (ADR-0045 failure
/// mode: "governed fill lagging its income record").
const UNMATCHED_ALARM_GRACE: ChronoDuration = ChronoDuration::minutes(5);

fn financial_drift_tolerance() -> Decimal {
    Decimal::new(1, 2) // 0.01
}

/// One poll→ingest→match→alarm cycle's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PollOutcome {
    pub ingested: usize,
    pub matched: usize,
    pub newly_alarmed: usize,
}

pub struct IncomeLedgerWorker<E: ExchangePort + IncomePort + 'static, S: Store + 'static> {
    exchange: Arc<E>,
    position_manager: Arc<RwLock<PositionManager<E, S>>>,
    store: Arc<S>,
    pool: PgPool,
    event_bus: Arc<EventBus>,
    poll_interval: Duration,
    shutdown_token: CancellationToken,
}

impl<E: ExchangePort + IncomePort + 'static, S: Store + 'static> IncomeLedgerWorker<E, S> {
    pub fn new(
        exchange: Arc<E>,
        position_manager: Arc<RwLock<PositionManager<E, S>>>,
        store: Arc<S>,
        pool: PgPool,
        event_bus: Arc<EventBus>,
        poll_interval: Duration,
        shutdown_token: CancellationToken,
    ) -> Self {
        Self {
            exchange,
            position_manager,
            store,
            pool,
            event_bus,
            poll_interval,
            shutdown_token,
        }
    }

    /// Run the periodic poll→ingest→match loop until shutdown.
    pub async fn run(self) -> DaemonResult<()> {
        info!("Income ledger worker started");
        let mut ticker = interval(self.poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ticker.tick().await;

        loop {
            tokio::select! {
                _ = self.shutdown_token.cancelled() => {
                    info!("Income ledger worker shutting down");
                    break Ok(());
                }
                _ = ticker.tick() => {
                    match self.poll_and_match().await {
                        Ok(outcome) if outcome.newly_alarmed > 0 => {
                            warn!(?outcome, "Income ledger poll found new unmatched items");
                        }
                        Ok(outcome) => debug!(?outcome, "Income ledger poll complete"),
                        Err(error) => error!(
                            %error,
                            "Income ledger poll failed — pausing until next tick, no accounting writes"
                        ),
                    }
                }
            }
        }
    }

    /// One poll→ingest→match cycle, plus a best-effort transfer-confirmed
    /// recalibration attempt. Public so tests and a startup pass can drive
    /// it directly.
    pub async fn poll_and_match(&self) -> DaemonResult<PollOutcome> {
        let since = checkpoint(&self.pool).await?;
        let items = self.exchange.get_income_since(since, 1000).await?;
        let ingested = ingest_items(&self.pool, &items).await?;
        let matched = match_pending_items(&self.pool, MATCH_WINDOW).await?;
        let newly_alarmed = count_confirmed_anomalies(&self.pool, UNMATCHED_ALARM_GRACE).await?;

        if newly_alarmed > 0 {
            self.event_bus.send(DaemonEvent::IncomeLedgerAnomaliesDetected {
                count: newly_alarmed,
                detected_at: Utc::now(),
            });
        }

        if let Err(error) = self.recalibrate_from_confirmed_transfers().await {
            warn!(%error, "Transfer-confirmed recalibration check failed this cycle");
        }

        Ok(PollOutcome { ingested, matched, newly_alarmed })
    }

    /// The only remaining path that may write `capital_base` automatically
    /// (ADR-0045 §2): the pure-financial-drift check now requires the ledger
    /// to explain the wallet delta as 100% matched `TRANSFER` items, with
    /// zero other unmatched items in the same window. Everything else is an
    /// alarm, never a write — see
    /// `reconciliation_worker::recalibrate_capital_base_after_pure_financial_drift`,
    /// which no longer performs the write itself.
    async fn recalibrate_from_confirmed_transfers(&self) -> DaemonResult<()> {
        let now = Utc::now();
        let manager = self.position_manager.read().await;

        let risk_open = manager.live_risk_open_positions().await?;
        let all_open = self.store.positions().find_active().await?;
        let armed_count = all_open
            .iter()
            .filter(|p| matches!(p.state, robson_domain::PositionState::Armed))
            .count();
        let in_flight_count = all_open
            .iter()
            .filter(|p| !matches!(p.state, robson_domain::PositionState::Armed))
            .count();

        if !risk_open.is_empty() || armed_count > 0 || in_flight_count > 0 {
            return Ok(());
        }

        let wallet_balance = self.exchange.get_futures_balance().await?.wallet_balance;
        let previous_capital_base = manager.load_monthly_state(now).await?.capital_base;
        let robson_month_net = manager.robson_month_net(now).await?;
        let expected_wallet_balance = previous_capital_base + robson_month_net;
        let unexplained_delta = wallet_balance - expected_wallet_balance;

        if unexplained_delta.abs() <= financial_drift_tolerance() {
            return Ok(());
        }

        // Window: since the position manager's own capital_base was last
        // set. Using month-start as a bound is sufficient here because
        // recalibration only ever runs when the book is fully idle
        // (checked above) — there is no in-progress month boundary race.
        let month_start = DateTime::<Utc>::from_naive_utc_and_offset(
            chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
                .expect("valid calendar month")
                .and_hms_opt(0, 0, 0)
                .expect("midnight is valid"),
            Utc,
        );

        let explanation = transfer_explains_delta(
            &self.pool,
            month_start,
            unexplained_delta,
            financial_drift_tolerance(),
        )
        .await?;

        let Some(matched_transfer_sum) = explanation else {
            debug!(
                %unexplained_delta,
                "Financial drift not fully explained by matched TRANSFER items — no write"
            );
            return Ok(());
        };

        let carried_risk = Decimal::ZERO; // book confirmed idle above.
        let new_capital_base =
            (wallet_balance - robson_month_net - carried_risk).max(Decimal::ZERO);

        if new_capital_base == previous_capital_base {
            return Ok(());
        }

        info!(
            %previous_capital_base,
            %new_capital_base,
            %wallet_balance,
            %matched_transfer_sum,
            "Capital base recalibrated: ledger-confirmed TRANSFER explains 100% of drift"
        );

        let event = robson_domain::Event::CapitalBaseRecalibrated {
            previous_capital_base,
            new_capital_base,
            wallet_balance,
            carried_risk,
            reason: format!("income_ledger:transfer_confirmed:sum={matched_transfer_sum}"),
            evidence: format!(
                "unexplained_delta={unexplained_delta};matched_transfer_sum={matched_transfer_sum}"
            ),
            month: now.month(),
            year: now.year(),
            timestamp: now,
        };

        self.event_bus.send(DaemonEvent::DomainEvent(event.clone()));
        manager.emit_domain_event(event).await?;

        Ok(())
    }
}

// =============================================================================
// Pure / pool-backed free functions — independently unit- and integration-
// tested without the worker shell.
// =============================================================================

/// Resume point for the next poll: `MAX(income_time)` across the ledger
/// doubles as the checkpoint, so no separate checkpoint table is needed. On
/// an empty table (first run), bound the initial backfill to 24h rather
/// than pulling full account history.
pub async fn checkpoint(pool: &PgPool) -> DaemonResult<DateTime<Utc>> {
    let max_time: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT MAX(income_time) FROM income_ledger")
            .fetch_one(pool)
            .await?;

    Ok(max_time.unwrap_or_else(|| Utc::now() - ChronoDuration::hours(24)))
}

/// Idempotently insert new income items. Returns the count of rows actually
/// inserted (items already seen, per `exchange_income_id`, are no-ops).
pub async fn ingest_items(
    pool: &PgPool,
    items: &[robson_exec::ports::IncomeRecord],
) -> DaemonResult<usize> {
    let mut inserted = 0usize;

    for item in items {
        let symbol = item.symbol.as_ref().map(|s| s.as_pair());
        let result = sqlx::query(
            r#"
            INSERT INTO income_ledger (
                exchange_income_id, symbol, income_type, amount, asset,
                exchange_trade_id, income_time
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (exchange_income_id) DO NOTHING
            "#,
        )
        .bind(&item.exchange_income_id)
        .bind(symbol)
        .bind(item.income_type.as_exchange_str())
        .bind(item.amount)
        .bind(&item.asset)
        .bind(&item.exchange_trade_id)
        .bind(item.income_time)
        .execute(pool)
        .await?;

        if result.rows_affected() > 0 {
            inserted += 1;
        }
    }

    Ok(inserted)
}

/// Match every currently-unmatched ledger row.
///
/// - `TRANSFER`: trivially matched (no fill linkage expected).
/// - `FUNDING_FEE`: always recognized (see module docs — never links to a
///   governed fill by construction).
/// - `REALIZED_PNL` / `COMMISSION`: matched by (symbol, time proximity) to
///   exactly one `positions_current` row's `entry_filled_at` or `closed_at`.
///   Zero or multiple candidates -> left unmatched (alarm).
/// - anything else: left unmatched (named anomaly).
///
/// Returns the count of rows matched this pass.
pub async fn match_pending_items(pool: &PgPool, window: ChronoDuration) -> DaemonResult<usize> {
    let mut matched = 0usize;

    let trivial = sqlx::query(
        r#"
        UPDATE income_ledger
        SET matched_at = NOW()
        WHERE matched_at IS NULL AND income_type IN ('TRANSFER', 'FUNDING_FEE')
        "#,
    )
    .execute(pool)
    .await?;
    matched += trivial.rows_affected() as usize;

    #[derive(sqlx::FromRow)]
    struct PendingItem {
        id: Uuid,
        symbol: Option<String>,
        income_time: DateTime<Utc>,
    }

    let pending: Vec<PendingItem> = sqlx::query_as(
        r#"
        SELECT id, symbol, income_time FROM income_ledger
        WHERE matched_at IS NULL AND income_type IN ('REALIZED_PNL', 'COMMISSION')
        "#,
    )
    .fetch_all(pool)
    .await?;

    for item in pending {
        let Some(symbol) = &item.symbol else { continue };
        let window_start = item.income_time - window;
        let window_end = item.income_time + window;

        let candidates: Vec<Uuid> = sqlx::query_scalar(
            r#"
            SELECT last_event_id FROM positions_current
            WHERE symbol = $1
              AND (
                (entry_filled_at IS NOT NULL AND entry_filled_at BETWEEN $2 AND $3)
                OR (closed_at IS NOT NULL AND closed_at BETWEEN $2 AND $3)
              )
            "#,
        )
        .bind(symbol)
        .bind(window_start)
        .bind(window_end)
        .fetch_all(pool)
        .await?;

        if candidates.len() != 1 {
            continue; // zero or ambiguous — stays unmatched, alarms after
                      // grace.
        }

        let result = sqlx::query(
            "UPDATE income_ledger SET matched_at = NOW(), matched_event_id = $2 WHERE id = $1",
        )
        .bind(item.id)
        .bind(candidates[0])
        .execute(pool)
        .await?;

        if result.rows_affected() > 0 {
            matched += 1;
        }
    }

    Ok(matched)
}

/// Count unmatched items older than `grace` — a confirmed anomaly, not a
/// fill still catching up (ADR-0045 failure mode: "governed fill lagging
/// its income record").
pub async fn count_confirmed_anomalies(
    pool: &PgPool,
    grace: ChronoDuration,
) -> DaemonResult<usize> {
    let cutoff = Utc::now() - grace;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM income_ledger WHERE matched_at IS NULL AND income_time < $1",
    )
    .bind(cutoff)
    .fetch_one(pool)
    .await?;

    Ok(count as usize)
}

/// Whether the ledger explains `unexplained_delta` as 100% matched
/// `TRANSFER` items since `since`, with zero other unmatched items in the
/// same window. Returns `Some(matched_transfer_sum)` only when both hold —
/// `None` means "do not recalibrate" (ADR-0045 §2: never write from a
/// partially- or un-attributed residual).
pub async fn transfer_explains_delta(
    pool: &PgPool,
    since: DateTime<Utc>,
    unexplained_delta: Decimal,
    tolerance: Decimal,
) -> DaemonResult<Option<Decimal>> {
    let unmatched_other: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM income_ledger WHERE matched_at IS NULL AND income_time >= $1",
    )
    .bind(since)
    .fetch_one(pool)
    .await?;

    if unmatched_other > 0 {
        return Ok(None);
    }

    let transfer_sum: Option<Decimal> = sqlx::query_scalar(
        r#"
        SELECT SUM(amount) FROM income_ledger
        WHERE income_type = 'TRANSFER' AND matched_at IS NOT NULL AND income_time >= $1
        "#,
    )
    .bind(since)
    .fetch_one(pool)
    .await?;

    let transfer_sum = transfer_sum.unwrap_or(Decimal::ZERO);

    if (transfer_sum - unexplained_delta).abs() <= tolerance {
        Ok(Some(transfer_sum))
    } else {
        Ok(None)
    }
}
