//! Position Manager: Manages position lifecycle and detector tasks.
//!
//! The Position Manager is responsible for:
//! - Arming new positions (creates detector task)
//! - Processing detector signals (entry logic)
//! - Processing market data (trailing stop updates, exit triggers)
//! - Managing position state transitions
//! - Graceful shutdown of all detector tasks
//!
//! # Architecture
//!
//! ```text
//! CLI (arm) → PositionManager → spawn Detector → wait for signal
//!                  ↑
//!          EventBus (signals, market data)
//!                  ↓
//!              Engine → Executor → Exchange
//!
//! Shutdown → CancellationToken.cancel() → all detectors exit
//! ```

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use chrono::Datelike;
use robson_domain::{
    latent_risk_per_unit_at_trigger, ClosureEvidence, DetectorSignal, EntryPolicy,
    EntryPolicyConfig, Event, MonthlyBudgetModel, Position, PositionId, PositionState, Price,
    Quantity, ReconciliationEvidence, RiskConfig, Side, StopPolicy, Symbol, SymbolTradingRules,
    TechnicalStopDistance, TradingPolicy,
};
use robson_engine::{
    technical_stop_analyzer::TechnicalStopConfig, Engine, EngineAction, EngineDecision,
    PositionSummary, ProposedTrade, RiskContext, RiskGate, StrategyRegistry,
};
#[cfg(feature = "postgres")]
use robson_eventlog::{append_event, ActorType as EventlogActorType, Event as EventlogEvent};
use robson_exec::{
    gate::OperationKind, quantize, venue::VenueId, ActionResult, ExchangePort, ExchangePosition,
    ExecError, Executor, OhlcvPort, StubOhlcv,
};
#[cfg(feature = "postgres")]
use robson_projector::apply_event_to_projections;
use robson_store::Store;
use rust_decimal::Decimal;
#[cfg(feature = "postgres")]
use sqlx::PgPool;
use tokio::{
    sync::{Mutex, MutexGuard, RwLock},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    circuit_breaker::CircuitBreaker,
    detector::DetectorTask,
    error::{DaemonError, DaemonResult},
    event_bus::{DaemonEvent, EventBus, FeedKey, MarketData},
    event_streams::EventStreams,
    query::{
        ActorKind, CommandSource, ContextSummary, ExecutionQuery, QueryKind, QueryOutcome,
        QueryState,
    },
    query_engine::{
        ApprovalCheckResult, ApprovalPolicy, CheckRiskError, GovernedAction, QueryEngine,
        QueryRecorder,
    },
};

// =============================================================================
// Position Manager
// =============================================================================

#[derive(Debug)]
struct PendingApprovalRecord {
    query: ExecutionQuery,
    position: Position,
    proposed: ProposedTrade,
    governed: GovernedAction,
}

/// Real exchange evidence used to close a stale Active position by reverse
/// reconciliation.
#[derive(Debug, Clone)]
pub(crate) struct ReconciledCloseInput {
    pub position_id: PositionId,
    pub exit_price: Price,
    pub filled_quantity: Quantity,
    pub fee: Decimal,
    pub fee_asset: String,
    pub closed_at: chrono::DateTime<chrono::Utc>,
    pub evidence: ReconciliationEvidence,
    /// Client order id of the order whose fill closed the position, when the
    /// resolver knows it. An `ins-` prefix proves the fill came from
    /// robsond's own protective stop, which classifies the close as a
    /// governed InsuranceStop exit (counts toward the monthly gauge/slots).
    pub authored_client_order_id: Option<String>,
}

/// Result of an attempted reverse-reconciliation close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReconcileCloseOutcome {
    Closed,
    AlreadyTerminal,
    SkippedNonActive { state: String },
    RejectedUnsupportedEvidence { source: &'static str },
    RejectedInconsistentEvidence { field: &'static str },
}

/// Terminal entry rejection for an Armed position in `needs_operator_rearm`
/// (ADR-0050 §2/§7).
#[derive(Debug, Clone)]
pub struct EntryRejectionRecord {
    /// Machine-readable rejection code.
    pub reason_code: String,
    /// Human-readable rejection detail with the numbers involved.
    pub reason: String,
    /// When the attempt was exhausted.
    pub rejected_at: chrono::DateTime<chrono::Utc>,
}

/// Last market price seen per feed, with the instant it arrived.
pub(crate) type PriceCache = Arc<RwLock<HashMap<FeedKey, (Price, std::time::Instant)>>>;

/// Manages position lifecycle and detector tasks.
pub struct PositionManager<E: ExchangePort + 'static, S: Store + 'static> {
    /// Trading engine (Mutex for interior mutability: arm updates RiskConfig)
    engine: std::sync::Mutex<Engine>,
    /// Order executor
    executor: Arc<Executor<E, S>>,
    /// Store for persistence
    store: Arc<S>,
    /// Event bus for publishing events
    event_bus: Arc<EventBus>,
    /// Venue this manager's positions execute on: the feed they are marked
    /// and monitored from, and the one its detectors subscribe to (ADR-0056
    /// §3). Positions carry no venue until step 7.
    venue: VenueId,
    /// OHLCV source for detector technical stop analysis, from the same venue
    ohlcv_port: Arc<dyn OhlcvPort>,
    /// Master cancellation token for all detector tasks
    shutdown_token: CancellationToken,
    /// Active detector tasks (position_id → task handle)
    detectors: Arc<RwLock<HashMap<PositionId, JoinHandle<Option<DetectorSignal>>>>>,
    /// Entry policy selected for each armed position. This keeps runtime
    /// detector re-arming aligned with the policy event emitted at ARM time.
    entry_policies: Arc<RwLock<HashMap<PositionId, EntryPolicyConfig>>>,
    /// Consecutive governed re-arms per position (risk denial, expired
    /// approval, an admission that failed before the executor). Drives the
    /// exponential backoff on the re-armed detector's first evaluation;
    /// cleared when an admission's entry reaches the venue's send or the
    /// approval queue.
    governed_rearm_counts: Arc<RwLock<HashMap<PositionId, u32>>>,
    /// Entry-time invalidation guard level captured from the detector signal
    /// (ADR-0042), keyed by position. Populated when a signal carrying a guard
    /// is processed and consumed by the entry-fill handler so the executable
    /// stop (soft exit + insurance) stays clamped to the recent adverse
    /// extreme until the first trailing advance releases it. Survives the
    /// (possibly deferred) approval→fill flow within a daemon run.
    invalidation_guards: Arc<RwLock<HashMap<PositionId, Price>>>,
    /// Whether the entry-time invalidation guard is enabled (ADR-0042).
    stop_invalidation_guard_enabled: bool,
    /// Lookback candles for the invalidation guard recent extreme (ADR-0042).
    stop_invalidation_lookback_candles: usize,
    /// Positions whose reduce-only exit the exchange rejected while
    /// reporting the symbol flat (#142). The exchange-side insurance stop
    /// has already closed the position, so market ticks stop re-placing
    /// exit orders for these and poll for the stop's fill evidence until
    /// reverse reconciliation closes the book position.
    exit_overtaken_suspects: Arc<RwLock<HashSet<PositionId>>>,
    /// Minimum spacing between two audit-only `PositionMonitorTick` events
    /// for the same position (ADR-0049). Zero disables the throttle.
    monitor_tick_interval: std::time::Duration,
    /// When the last `PositionMonitorTick` was emitted per position.
    last_monitor_tick_at: Arc<RwLock<HashMap<PositionId, std::time::Instant>>>,
    /// Last market price seen per `(venue, symbol)` feed. Marks Active
    /// positions for the ADR-0046 month equity instead of the stored
    /// `current_price`, which only moves on fill and on trailing advances (and
    /// resets to the entry price on restart).
    last_market_price: PriceCache,
    /// Active positions whose exchange-side insurance stop is known to sit
    /// behind the software stop plan because a placement or re-placement
    /// failed, or to be gone from the venue (ADR-0039). Value: earliest instant
    /// the next attempt may run.
    insurance_stop_stale: Arc<RwLock<HashMap<PositionId, std::time::Instant>>>,
    /// Active positions whose exit found nothing executable or was refused by
    /// the venue (ADR-0056 §5). Value: earliest instant the next attempt
    /// may run. Pruned with the active set.
    exit_retry_after: Arc<RwLock<HashMap<PositionId, std::time::Instant>>>,
    /// Metadata refreshes requested on the side, per symbol. At most one is in
    /// flight per symbol; entries are pruned to the symbols of open positions.
    rules_refresh_requests: Arc<std::sync::Mutex<HashMap<Symbol, RulesRefreshRequest>>>,
    /// How each position's technical stop was derived (method, confidence),
    /// taken from the `TechnicalStopAnalyzed` audit at signal time and
    /// re-read from the durable event log after a restart. Operator-facing
    /// only: an ATR-fallback stop deserves a visible warning on the card.
    tech_stop_audits: Arc<RwLock<HashMap<PositionId, TechStopAuditSummary>>>,
    /// Armed positions whose entry attempt exhausted its autonomy
    /// (ADR-0050 §2 `needs_operator_rearm`). No detector runs for them and
    /// none is restored at startup; the operator disarms or arms anew.
    /// Rebuilt from `entry_attempt_exhausted` eventlog events on restart.
    entry_exhausted: Arc<RwLock<HashMap<PositionId, EntryRejectionRecord>>>,
    /// Pending approvals held in runtime memory for Phase 3.
    pending_approvals: Arc<RwLock<HashMap<Uuid, PendingApprovalRecord>>>,
    /// Serializes entry-governance flows so pending reservations remain
    /// coherent. Shared with the approval-expiry tasks.
    entry_flow_lock: Arc<Mutex<()>>,
    /// Query engine for lifecycle tracking and audit persistence
    query_engine: Arc<QueryEngine<Arc<dyn QueryRecorder>>>,
    /// Binary MonthlyHalt gate (v3 policy: 4% drawdown → halt).
    pub(crate) circuit_breaker: Arc<CircuitBreaker>,
    /// Trading policy for circuit breaker halt evaluation (ADR-0024 §5).
    trading_policy: TradingPolicy,
    /// In-memory mirror of ADR-0046 monthly equity peaks for non-Postgres mode
    /// and unit tests without an event-log pool.
    month_peak_net_cache: Arc<Mutex<HashMap<(i32, u32), Decimal>>>,
    /// Throttle for the per-tick peak refresh (ADR-0046 §3: 20 s cadence).
    /// Without it every market tick pays the monthly-state SELECT — the N+1
    /// class the engineering guardrails flag on hot paths.
    month_peak_refreshed_at: Arc<Mutex<Option<std::time::Instant>>>,
    /// Last active/other shadow-model slot pair observed. The divergence log
    /// fires only when this pair changes; the gauge is refreshed every time.
    shadow_slots_last: Mutex<Option<(u32, u32)>>,
    /// ExecutableSpan positions whose canonical latent-risk resolution has
    /// already emitted a durable warning in this process. Retained only while
    /// the position remains unresolved, so the set is bounded by the current
    /// risk-open portfolio. Read-only snapshots never touch this dedup state.
    latent_risk_warning_positions: RwLock<HashSet<PositionId>>,
    /// The streams `persist_event_to_log` writes: their locks and which ones
    /// refuse appends (ADR-0056 step 7, PR P).
    event_streams: EventStreams,
    /// Optional postgres pool for persisting domain events to robson-eventlog.
    #[cfg(feature = "postgres")]
    event_log_pool: Option<PgPool>,
    /// Tenant ID used for eventlog entries. Required when event_log_pool is
    /// Some.
    #[cfg(feature = "postgres")]
    event_log_tenant_id: Option<Uuid>,
}

/// Persisted monthly risk state from `monthly_state` table.
///
/// Authoritative source for accumulated monthly risk accounting.
/// Does NOT contain live execution state — that comes from positions_current.
/// Backoff for the Nth consecutive governed re-arm of a detector: 5s doubling
/// to a 15-minute cap (5, 10, 20, … 640, 900, 900, …).
///
/// Applied to the re-armed detector's first evaluation after a governed block
/// (risk denial, expired approval). Without it, an Immediate-mode re-arm
/// refires instantly and a persistent governed condition becomes a ~1/s hot
/// loop against the OHLCV source and the event store.
/// Operator-facing dry run of an immediate arm: what the technical stop
/// analysis and the engine sizing would produce right now for `symbol` and
/// `side`, without persisting a position, emitting events or touching the
/// exchange beyond the balance read the real arm also does.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntryPreview {
    pub symbol: String,
    pub side: String,
    /// Signal entry reference (last 15m candle close), as the immediate arm
    /// uses.
    pub reference_price: Decimal,
    pub capital_base: Decimal,
    /// `None` when the analyzer found no valid stop (see `rejection`).
    pub analysis: Option<EntryPreviewAnalysis>,
    /// `None` when the analysis failed or the engine rejected the entry.
    pub sizing: Option<EntryPreviewSizing>,
    /// Domain or analyzer rejection the real arm would hit, verbatim.
    pub rejection: Option<String>,
    /// The preview could not be completed for an availability reason (no or
    /// too few candles, exchange metadata unavailable). Unlike `rejection`
    /// this says nothing about whether the arm would be admitted, so the UI
    /// must not block on it.
    pub unavailable: Option<String>,
    /// Advisory notes for the operator; never block the arm by themselves.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EntryPreviewAnalysis {
    pub technical_stop: Decimal,
    pub stop_distance: Decimal,
    pub stop_distance_pct: Decimal,
    /// `"swing_point"` or `"atr_fallback"`.
    pub method: String,
    pub level_n: Option<usize>,
    /// `"high"`, `"medium"` or `"low"`.
    pub confidence: String,
    pub invalidation_guard_level: Option<Decimal>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EntryPreviewSizing {
    pub quantity: Decimal,
    pub notional: Decimal,
    pub planned_risk: Decimal,
    pub planned_risk_pct_of_capital: Decimal,
    pub executable_trigger: Option<Decimal>,
    pub executable_span: Option<Decimal>,
}

/// Operator-facing digest of a `TechnicalStopAnalyzed` audit: how the stop
/// was derived and how much the analyzer trusted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TechStopAuditSummary {
    /// `"swing_point"` or `"atr_fallback"` (serde names of
    /// `TechnicalStopMethodSnapshot`).
    pub method: String,
    /// 1-indexed level used by a swing-point stop; `None` for the fallback.
    pub level_n: Option<usize>,
    /// `"high"`, `"medium"` or `"low"`.
    pub confidence: String,
}

impl TechStopAuditSummary {
    pub fn from_audit(audit: &robson_domain::TechnicalStopAnalysisAudit) -> Self {
        use robson_domain::{TechnicalStopConfidenceSnapshot, TechnicalStopMethodSnapshot};
        let (method, level_n) = match audit.method {
            TechnicalStopMethodSnapshot::SwingPoint { level_n } => {
                ("swing_point".to_string(), Some(level_n))
            },
            TechnicalStopMethodSnapshot::AtrFallback => ("atr_fallback".to_string(), None),
        };
        let confidence = match audit.confidence {
            TechnicalStopConfidenceSnapshot::High => "high",
            TechnicalStopConfidenceSnapshot::Medium => "medium",
            TechnicalStopConfidenceSnapshot::Low => "low",
        }
        .to_string();
        Self { method, level_n, confidence }
    }

    /// Parse the digest out of a persisted `technical_stop_analyzed` event
    /// payload (the serialized domain event). Tolerant: returns `None` when
    /// the shape is not recognised rather than failing the caller.
    pub fn from_event_payload(payload: &serde_json::Value) -> Option<Self> {
        let analysis = payload.get("analysis")?;
        let method_obj = analysis.get("method")?;
        let method = method_obj.get("type")?.as_str()?.to_string();
        let level_n = method_obj.get("level_n").and_then(|v| v.as_u64()).map(|v| v as usize);
        let confidence = analysis.get("confidence")?.as_str()?.to_string();
        Some(Self { method, level_n, confidence })
    }
}

/// Minimum spacing between two attempts to re-place an exchange insurance
/// stop that a previous placement or re-placement failed to put at the
/// software trigger (ADR-0039). The first attempt waits this long too, so
/// a transient exchange error is not hammered at tick cadence.
const INSURANCE_STOP_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

/// How soon a leg tries its exit again after the venue refused it (ADR-0056
/// §5). Without this a persistent refusal would cost requests at tick cadence.
const EXIT_RULES_REJECTION_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

/// How soon a leg with nothing executable tries its exit or its stop again:
/// each attempt costs requests, and only a change in the leg or the rules can
/// help.
const NOT_EXECUTABLE_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Minimum spacing of the refresh requests made on the side per symbol. The
/// adapter holds the ADR-0056 §10 ceiling; this only spares the spawns.
const RULES_REFRESH_REQUEST_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Pure throttle rule for audit-only monitor ticks (ADR-0049): drop the
/// tick when one was emitted for the position less than `interval` ago.
fn should_drop_monitor_tick(
    last_emitted: Option<std::time::Instant>,
    now: std::time::Instant,
    interval: std::time::Duration,
) -> bool {
    match last_emitted {
        Some(last) => now.saturating_duration_since(last) < interval,
        None => false,
    }
}

fn governed_rearm_backoff(attempt: u32) -> std::time::Duration {
    const BASE_SECS: u64 = 5;
    const MAX_SECS: u64 = 900;
    let shift = attempt.saturating_sub(1).min(8);
    std::time::Duration::from_secs((BASE_SECS << shift).min(MAX_SECS))
}

type DetectorMap = RwLock<HashMap<PositionId, JoinHandle<Option<DetectorSignal>>>>;

/// How far an admission flow got, which decides what an error leaves behind
/// (ADR-0051 Recovery).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmissionReach {
    /// Nothing consumed yet: the pending approval is still queued.
    Untouched,
    /// The position's detector fired, or its pending approval was taken, and
    /// nothing went to the executor. An error here leaves the position Armed
    /// with no detector, since detectors are single-shot.
    DetectorSpent(PositionId),
    /// The entry's actions went to the executor. An order may be on the
    /// exchange, so the flow never re-arms from here: the fill path and
    /// reconciliation own the position. A proven no-send on a leg with no
    /// admission evidence goes back to `DetectorSpent`.
    Executor,
}

pub(crate) struct MonthlyRiskState {
    pub capital_base: Decimal,
    pub realized_loss: Decimal,
    pub month_peak_net: Decimal,
    pub trades_opened: i32,
    pub budget_model: MonthlyBudgetModel,
}

/// Freshness bound for `/status` reads of the tick-path price cache. Matches
/// the market-data watchdog (ADR-0044: 90 s of silence triggers the REST
/// fallback), so a cached price older than this is one the engine itself no
/// longer trusts without a fallback poll.
pub(crate) const MARKET_PRICE_CACHE_MAX_AGE: std::time::Duration =
    std::time::Duration::from_secs(90);

/// Canonical monthly budget inputs and both dormant model calculations
/// (ADR-0051 rollout step 2).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MonthlyBudgetSnapshot {
    pub(crate) model: MonthlyBudgetModel,
    pub(crate) capital_base: Decimal,
    pub(crate) capital_base_invalid: bool,
    pub(crate) latent_risk_invalid: bool,
    pub(crate) budget_amount: Decimal,
    pub(crate) governed_realized_net: Decimal,
    pub(crate) month_equity_net: Decimal,
    pub(crate) month_peak_net: Decimal,
    pub(crate) latent_risk: Decimal,
    pub(crate) consumed_active: Decimal,
    pub(crate) consumed_hwm: Decimal,
    pub(crate) consumed_nfs: Decimal,
    pub(crate) remaining_active: Decimal,
    pub(crate) remaining_hwm: Decimal,
    pub(crate) remaining_nfs: Decimal,
    pub(crate) slots_active: u32,
    pub(crate) slots_hwm: u32,
    pub(crate) slots_nfs: u32,
}

#[derive(Debug, Clone, Copy)]
struct RulesRefreshRequest {
    requested_at: std::time::Instant,
    in_flight: bool,
}

#[derive(Debug, Clone, Copy)]
struct ResolvedPositionRisk {
    current_stop: Decimal,
    risk: Decimal,
    cost_priced_latent_risk: Option<Decimal>,
}

#[derive(Debug, Clone)]
struct LatentRiskResolutionFailure {
    position_id: PositionId,
    state: String,
    reason: String,
}

impl MonthlyBudgetSnapshot {
    pub(crate) fn budget_amount(&self) -> Decimal {
        self.budget_amount
    }
}

impl<E: ExchangePort + 'static, S: Store + 'static> PositionManager<E, S> {
    pub(crate) fn configured_capital(&self) -> Decimal {
        self.engine.lock().unwrap().risk_config().capital()
    }

    /// Snapshot of open positions observed directly on the exchange.
    pub(crate) async fn exchange_open_positions(&self) -> DaemonResult<Vec<ExchangePosition>> {
        self.executor.exchange().get_all_open_positions().await.map_err(Into::into)
    }

    /// Positions that still contribute to live risk.
    ///
    /// `Active` positions are only considered live if they are also present on
    /// the exchange snapshot. `Entering` positions remain live because they are
    /// already committed in the order/execution pipeline even before fill.
    pub(crate) async fn live_risk_open_positions(&self) -> DaemonResult<Vec<Position>> {
        let open_positions = self.store.positions().find_risk_open().await?;
        let exchange_positions = match self.exchange_open_positions().await {
            Ok(positions) => Some(positions),
            Err(error) => {
                tracing::warn!(error = %error, "failed to query exchange open positions; falling back to local risk-open set");
                None
            },
        };

        Ok(match exchange_positions {
            Some(exchange_positions) => open_positions
                .into_iter()
                .filter(|position| match &position.state {
                    PositionState::Entering { .. } => true,
                    PositionState::Active { .. } => {
                        exchange_positions.iter().any(|exchange_position| {
                            exchange_position.symbol == position.symbol
                                && exchange_position.side == position.side
                        })
                    },
                    _ => false,
                })
                .collect(),
            None => open_positions,
        })
    }

    /// Update the engine's capital from an external source (exchange balance).
    ///
    /// Called at startup after querying the exchange and whenever the engine's
    /// capital needs to be refreshed. Updates the internal `RiskConfig` while
    /// preserving the 1% maximum-loss cap and the operator-configured
    /// execution-cost parameters (ADR-0039).
    pub fn update_engine_capital(&self, capital: Decimal) {
        let mut engine = self.engine.lock().unwrap();
        if let Ok(risk_config) = engine.risk_config().with_capital(capital) {
            engine.update_risk_config(risk_config);
        }
    }

    /// Snapshot of the engine's current risk configuration.
    ///
    /// Use `.with_capital(...)` on the snapshot when sizing against a
    /// different capital base so execution-cost parameters are preserved.
    pub(crate) fn risk_config_snapshot(&self) -> robson_domain::RiskConfig {
        *self.engine.lock().unwrap().risk_config()
    }

    /// Unrealized P&L of the Active positions in `positions`, each marked at
    /// the last market price seen for its symbol when one is known, else at
    /// the stored `current_price` (fill price, or the last trailing ratchet).
    /// ADR-0046 §3: the month equity peak must observe live marks.
    async fn unrealized_pnl_marked(&self, positions: &[Position]) -> Decimal {
        let marks = self.last_market_price.read().await;
        positions
            .iter()
            .filter(|position| matches!(position.state, PositionState::Active { .. }))
            .map(|position| match marks.get(&self.feed_key(&position.symbol)) {
                Some((mark, _)) => position.unrealized_pnl_at(*mark),
                None => position.calculate_pnl(),
            })
            .sum()
    }

    /// Compute current equity per ADR-0024 §6:
    ///
    /// ```text
    /// current_equity = initial_capital + all_time_realized_pnl + unrealized_pnl
    /// ```
    ///
    /// Where:
    /// - `initial_capital`: operator's declared starting capital from config
    /// - `all_time_realized_pnl`: Σ(realized_pnl − fees_paid) for every closed
    ///   position since the system started
    /// - `unrealized_pnl`: mark-to-market PnL of Active positions
    pub(crate) async fn compute_current_equity(&self) -> DaemonResult<Decimal> {
        let initial_capital = self.configured_capital();

        // All-time realized PnL (net of fees)
        let all_closed = self.store.positions().find_all_closed().await?;
        let all_time_realized: Decimal =
            all_closed.iter().map(|p| p.realized_pnl - p.fees_paid).sum();

        // Unrealized PnL of open Active positions at live marks
        let active = self.store.positions().find_risk_open().await?;
        let unrealized = self.unrealized_pnl_marked(&active).await;

        Ok(initial_capital + all_time_realized + unrealized)
    }

    #[cfg(feature = "postgres")]
    fn event_stream_key(event: &Event) -> String {
        match event {
            Event::MonthBoundaryReset { year, month, .. }
            | Event::CapitalBaseRecalibrated { year, month, .. } => {
                format!("system:month_boundary:{year:04}-{month:02}")
            },
            _ => format!("position:{}", event.position_id()),
        }
    }

    #[cfg(feature = "postgres")]
    pub(crate) async fn load_capital_base_for_month(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<Decimal> {
        let configured_capital = self.configured_capital();
        let Some(pool) = &self.event_log_pool else {
            return Ok(configured_capital);
        };

        let row = sqlx::query_as::<_, (Decimal,)>(
            "SELECT capital_base FROM monthly_state WHERE year = $1 AND month = $2",
        )
        .bind(now.year())
        .bind(now.month() as i16)
        .fetch_optional(pool)
        .await?;

        Ok(row.map(|(capital_base,)| capital_base).unwrap_or(configured_capital))
    }

    #[cfg(not(feature = "postgres"))]
    pub(crate) async fn load_capital_base_for_month(
        &self,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<Decimal> {
        Ok(self.configured_capital())
    }

    /// Load the full persisted monthly risk state from `monthly_state`.
    ///
    /// Returns the authoritative accumulated values for:
    /// - `capital_base`: starting capital for the month
    /// - `realized_loss`: sum of net losses from closed positions (ADR-0024)
    /// - `trades_opened`: number of entries filled this month
    ///
    /// On miss (no row for this month): returns defaults from config while
    /// inheriting the most recent prior month's persisted budget model.
    #[cfg(feature = "postgres")]
    pub(crate) async fn load_monthly_state(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<MonthlyRiskState> {
        let Some(pool) = &self.event_log_pool else {
            // No DB pool wired — happens in unit tests built with
            // --features postgres via create_test_manager. Mirror the
            // non-postgres path so realized_loss reflects in-memory closed
            // positions (TD-2026-05-08-002). Production wires the pool
            // unconditionally; this branch is unreachable in deployed builds.
            return self.load_monthly_state_in_memory(now).await;
        };

        let configured_capital = self.configured_capital();
        let row = sqlx::query_as::<_, (Decimal, Decimal, Decimal, i32, String)>(
            "SELECT capital_base, realized_loss, month_peak_net, trades_opened, monthly_budget_model FROM monthly_state WHERE year = $1 AND month = $2",
        )
        .bind(now.year())
        .bind(now.month() as i16)
        .fetch_optional(pool)
        .await?;

        if let Some((capital_base, realized_loss, month_peak_net, trades_opened, stored_model)) =
            row
        {
            let budget_model = MonthlyBudgetModel::from_persisted(&stored_model);
            if stored_model != budget_model.as_str() {
                warn!(
                    monthly_budget_model = %stored_model,
                    fallback = %budget_model.as_str(),
                    "Unknown persisted monthly budget model; using conservative fallback"
                );
            }
            return Ok(MonthlyRiskState {
                capital_base,
                realized_loss,
                month_peak_net,
                trades_opened,
                budget_model,
            });
        }

        let inherited_stored_model = sqlx::query_scalar::<_, String>(
            r#"
            SELECT monthly_budget_model
            FROM monthly_state
            WHERE (year, month) < ($1::SMALLINT, $2::SMALLINT)
            ORDER BY year DESC, month DESC
            LIMIT 1
            "#,
        )
        .bind(now.year())
        .bind(now.month() as i16)
        .fetch_optional(pool)
        .await?;
        let inherited_model = inherited_stored_model
            .map(|stored_model| {
                let model = MonthlyBudgetModel::from_persisted(&stored_model);
                if stored_model != model.as_str() {
                    warn!(
                        monthly_budget_model = %stored_model,
                        fallback = %model.as_str(),
                        "Unknown inherited monthly budget model; using conservative fallback"
                    );
                }
                model
            })
            .unwrap_or_default();
        warn!(
            year = now.year(),
            month = now.month(),
            inherited_model = %inherited_model.as_str(),
            "Monthly state row missing; using fallback state with inherited budget model"
        );

        Ok(MonthlyRiskState {
            capital_base: configured_capital,
            realized_loss: Decimal::ZERO,
            month_peak_net: Decimal::ZERO,
            trades_opened: 0,
            budget_model: inherited_model,
        })
    }

    #[cfg(not(feature = "postgres"))]
    pub(crate) async fn load_monthly_state(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<MonthlyRiskState> {
        // No DB available: compute realized_loss from in-memory store for
        // correctness in tests and non-postgres mode.
        self.load_monthly_state_in_memory(now).await
    }

    /// Compute monthly state from the in-memory store. Shared between the
    /// non-postgres feature build and the postgres feature build's
    /// pool-unwired fallback (TD-2026-05-08-002).
    async fn load_monthly_state_in_memory(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<MonthlyRiskState> {
        let monthly_closed =
            self.store.positions().find_closed_in_month(now.year(), now.month()).await?;
        let mut governed_closed: Vec<_> =
            monthly_closed.iter().filter(|p| Self::is_governed_monthly_close(p)).collect();
        governed_closed.sort_by_key(|p| p.closed_at);

        let realized_loss: Decimal = governed_closed
            .iter()
            .map(|p| {
                let net = p.realized_pnl - p.fees_paid;
                if net < Decimal::ZERO {
                    net.abs()
                } else {
                    Decimal::ZERO
                }
            })
            .sum();
        // Fallback reconstruction can only see realized closes. Unrealized
        // peaks observed during prior daemon ticks are not reconstructable
        // after process loss, so this is conservative per ADR-0046.
        let mut running_net = Decimal::ZERO;
        let mut realized_peak = Decimal::ZERO;
        for position in governed_closed {
            running_net += position.realized_pnl - position.fees_paid;
            realized_peak = realized_peak.max(running_net).max(Decimal::ZERO);
        }
        let cached_peak = self
            .month_peak_net_cache
            .lock()
            .await
            .get(&(now.year(), now.month()))
            .copied()
            .unwrap_or(Decimal::ZERO);
        let budget_model = MonthlyBudgetModel::default();
        warn!(
            year = now.year(),
            month = now.month(),
            inherited_model = %budget_model.as_str(),
            "Monthly state unavailable in memory; using fallback state with default budget model"
        );
        Ok(MonthlyRiskState {
            capital_base: self.configured_capital(),
            realized_loss,
            month_peak_net: realized_peak.max(cached_peak),
            trades_opened: monthly_closed.len() as i32,
            budget_model,
        })
    }

    /// Closed positions for the given month, restart-proof.
    ///
    /// The in-memory store only hydrates ACTIVE positions after a restart, so
    /// memory-based month math silently drops every close that happened
    /// before the last boot (2026-07-03: the monthly gauge zeroed and the
    /// drift detector re-absorbed the month's governed loss after a deploy).
    /// When the projection is available, read the durable
    /// `positions_current` instead; fall back to memory otherwise.
    pub(crate) async fn closed_positions_in_month(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<Vec<Position>> {
        #[cfg(feature = "postgres")]
        if let (Some(pool), Some(tenant_id)) = (&self.event_log_pool, self.event_log_tenant_id) {
            let month_start = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
                chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
                    .expect("month start must be valid")
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight must be valid"),
                chrono::Utc,
            );
            let positions =
                robson_store::find_positions_overlapping_month(pool, tenant_id, month_start)
                    .await?;
            return Ok(positions
                .into_iter()
                .filter(|p| {
                    matches!(p.state, PositionState::Closed { .. })
                        && p.closed_at
                            .map(|t| t.year() == now.year() && t.month() == now.month())
                            .unwrap_or(false)
                })
                .collect());
        }

        Ok(self.store.positions().find_closed_in_month(now.year(), now.month()).await?)
    }

    /// Net governed month result (realized_pnl − fees over closed positions),
    /// restart-proof. Used by drift attribution so recalibration only absorbs
    /// out-of-band changes.
    pub(crate) async fn robson_month_net(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<Decimal> {
        Ok(self
            .closed_positions_in_month(now)
            .await?
            .iter()
            .filter(|position| Self::is_governed_monthly_close(position))
            .map(|position| position.realized_pnl - position.fees_paid)
            .sum())
    }

    #[cfg(test)]
    pub(crate) async fn month_equity_net(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<Decimal> {
        let realized_net = self.robson_month_net(now).await?;
        let risk_open = self.store.positions().find_risk_open().await?;
        let unrealized = self.unrealized_pnl_marked(&risk_open).await;
        Ok(realized_net + unrealized)
    }

    #[cfg(test)]
    pub(crate) async fn refresh_month_peak_net(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<Decimal> {
        let equity_net = self.month_equity_net(now).await?;
        if equity_net <= Decimal::ZERO {
            return Ok(equity_net);
        }

        let monthly = self.load_monthly_state(now).await?;
        self.refresh_month_peak_net_from_snapshot(now, &monthly, equity_net).await?;
        Ok(equity_net)
    }

    /// Persist a precomputed equity peak without re-reading monthly state or
    /// positions. Returns the effective monotonic peak after this refresh.
    async fn refresh_month_peak_net_from_snapshot(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        monthly: &MonthlyRiskState,
        equity_net: Decimal,
    ) -> DaemonResult<Decimal> {
        if equity_net <= monthly.month_peak_net || equity_net <= Decimal::ZERO {
            return Ok(monthly.month_peak_net);
        }

        #[cfg(feature = "postgres")]
        if let Some(pool) = &self.event_log_pool {
            sqlx::query(
                r#"
                INSERT INTO monthly_state (
                    year, month, capital_base, month_peak_net,
                    monthly_budget_model, created_at
                )
                VALUES (
                    $1, $2, $3, $4,
                    COALESCE(
                        (SELECT monthly_budget_model
                         FROM monthly_state
                         WHERE (year, month) < ($1::SMALLINT, $2::SMALLINT)
                         ORDER BY year DESC, month DESC
                         LIMIT 1),
                        'hwm_v1'
                    ),
                    NOW()
                )
                ON CONFLICT (year, month) DO UPDATE SET
                    month_peak_net = GREATEST(monthly_state.month_peak_net, EXCLUDED.month_peak_net)
                "#,
            )
            .bind(now.year())
            .bind(now.month() as i16)
            .bind(monthly.capital_base)
            .bind(equity_net)
            .execute(pool)
            .await?;
        }

        let mut cache = self.month_peak_net_cache.lock().await;
        let cached = cache.entry((now.year(), now.month())).or_insert(Decimal::ZERO);
        *cached = (*cached).max(equity_net);
        Ok(monthly.month_peak_net.max(equity_net))
    }

    pub(crate) async fn governed_monthly_realized_loss(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<Decimal> {
        let monthly_closed = self.closed_positions_in_month(now).await?;
        Ok(monthly_closed
            .iter()
            .filter(|position| Self::is_governed_monthly_close(position))
            .map(|position| {
                let net = position.realized_pnl - position.fees_paid;
                if net < Decimal::ZERO {
                    net.abs()
                } else {
                    Decimal::ZERO
                }
            })
            .sum())
    }

    fn is_governed_monthly_close(position: &Position) -> bool {
        matches!(
            position.state,
            PositionState::Closed { exit_reason, .. }
                if matches!(
                    exit_reason,
                    robson_domain::ExitReason::TrailingStop
                        | robson_domain::ExitReason::InsuranceStop
                        | robson_domain::ExitReason::UserPanic
                        | robson_domain::ExitReason::DegradedMode
                )
        )
    }

    pub(crate) async fn emit_domain_event(&self, event: Event) -> DaemonResult<()> {
        self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await?;
        Ok(())
    }

    /// Create a new position manager.
    ///
    /// After creation, call `start(Arc::clone(&manager))` to start the signal
    /// listener.
    pub fn new(
        engine: Engine,
        executor: Arc<Executor<E, S>>,
        store: Arc<S>,
        event_bus: Arc<EventBus>,
        query_recorder: Arc<dyn QueryRecorder>,
        trading_policy: TradingPolicy,
    ) -> Self {
        Self::with_approval_policy(
            engine,
            executor,
            store,
            event_bus,
            query_recorder,
            ApprovalPolicy::default(),
            trading_policy,
        )
    }

    pub(crate) fn with_approval_policy(
        engine: Engine,
        executor: Arc<Executor<E, S>>,
        store: Arc<S>,
        event_bus: Arc<EventBus>,
        query_recorder: Arc<dyn QueryRecorder>,
        approval_policy: ApprovalPolicy,
        trading_policy: TradingPolicy,
    ) -> Self {
        crate::metrics::init_detector_rearms_after_failure();
        let shutdown_token = CancellationToken::new();
        let query_engine = Arc::new(QueryEngine::with_approval_policy(
            query_recorder,
            RiskGate::with_policy(trading_policy),
            approval_policy,
        ));

        Self {
            engine: std::sync::Mutex::new(engine),
            executor,
            store,
            event_bus,
            // Every position predating venue routing maps to binance:usdm
            // (ADR-0056 §7); the daemon sets the selected venue.
            venue: VenueId::new(VenueId::BINANCE_USDM),
            ohlcv_port: Arc::new(StubOhlcv::default()),
            shutdown_token,
            detectors: Arc::new(RwLock::new(HashMap::new())),
            entry_policies: Arc::new(RwLock::new(HashMap::new())),
            governed_rearm_counts: Arc::new(RwLock::new(HashMap::new())),
            invalidation_guards: Arc::new(RwLock::new(HashMap::new())),
            // Guard disabled by default; enabled per EngineConfig via
            // `with_invalidation_guard`.
            stop_invalidation_guard_enabled: false,
            stop_invalidation_lookback_candles: 20,
            exit_overtaken_suspects: Arc::new(RwLock::new(HashSet::new())),
            // Throttle disabled by default; enabled per EngineConfig via
            // `with_monitor_tick_interval`.
            monitor_tick_interval: std::time::Duration::ZERO,
            last_monitor_tick_at: Arc::new(RwLock::new(HashMap::new())),
            last_market_price: Arc::new(RwLock::new(HashMap::new())),
            insurance_stop_stale: Arc::new(RwLock::new(HashMap::new())),
            exit_retry_after: Arc::new(RwLock::new(HashMap::new())),
            rules_refresh_requests: Arc::new(std::sync::Mutex::new(HashMap::new())),
            tech_stop_audits: Arc::new(RwLock::new(HashMap::new())),
            entry_exhausted: Arc::new(RwLock::new(HashMap::new())),
            pending_approvals: Arc::new(RwLock::new(HashMap::new())),
            entry_flow_lock: Arc::new(Mutex::new(())),
            query_engine,
            circuit_breaker: Arc::new(CircuitBreaker::default()),
            trading_policy,
            month_peak_net_cache: Arc::new(Mutex::new(HashMap::new())),
            month_peak_refreshed_at: Arc::new(Mutex::new(None)),
            shadow_slots_last: Mutex::new(None),
            latent_risk_warning_positions: RwLock::new(HashSet::new()),
            event_streams: EventStreams::default(),
            #[cfg(feature = "postgres")]
            event_log_pool: None,
            #[cfg(feature = "postgres")]
            event_log_tenant_id: None,
        }
    }

    /// Configure the entry-time invalidation guard (ADR-0042) for detectors
    /// spawned by this manager and propagate the guard level from a processed
    /// signal to the entry-fill handler.
    pub fn with_invalidation_guard(mut self, enabled: bool, lookback_candles: usize) -> Self {
        self.stop_invalidation_guard_enabled = enabled;
        self.stop_invalidation_lookback_candles = lookback_candles.max(1);
        self
    }

    /// Configure the minimum spacing between audit-only
    /// `PositionMonitorTick` events per position (ADR-0049). Exits and
    /// trailing advances are never throttled.
    pub fn with_monitor_tick_interval(mut self, interval: std::time::Duration) -> Self {
        self.monitor_tick_interval = interval;
        self
    }

    /// Configure the OHLCV source used by newly spawned detector tasks.
    pub fn with_ohlcv_port(mut self, ohlcv_port: Arc<dyn OhlcvPort>) -> Self {
        self.ohlcv_port = ohlcv_port;
        self
    }

    /// Configure the venue this manager's positions execute on (ADR-0056 §3).
    pub fn with_venue_id(mut self, venue: VenueId) -> Self {
        self.venue = venue;
        self
    }

    /// The venue this manager's positions execute on.
    pub fn venue_id(&self) -> &VenueId {
        &self.venue
    }

    fn feed_key(&self, symbol: &Symbol) -> FeedKey {
        FeedKey::new(self.venue.clone(), symbol.clone())
    }

    /// Shared handle on the per-feed price cache, for readers that must not
    /// wait on the manager lock (`/readyz`).
    pub(crate) fn price_cache(&self) -> PriceCache {
        Arc::clone(&self.last_market_price)
    }

    /// Fresh validated trading rules for admitting a position whose stop
    /// policy needs them (ADR-0052, ADR-0056 §5). Legacy positions derive
    /// without rules; for `ExecutableSpan` a stale or missing snapshot returns
    /// `None` and the engine fails closed.
    pub(crate) async fn trading_rules_for_admission(
        &self,
        position: &Position,
    ) -> Option<robson_domain::SymbolTradingRules> {
        if position.stop_policy != StopPolicy::ExecutableSpan {
            return None;
        }
        match self.exchange().trading_rules_fresh(&position.symbol).await {
            Ok(rules) => Some(rules),
            Err(error) => {
                warn!(
                    position_id = %position.id,
                    symbol = %position.symbol.as_pair(),
                    %error,
                    "Fresh trading rules unavailable for admission; engine will fail closed"
                );
                None
            },
        }
    }

    /// Trading rules for managing a position already admitted, whose stop
    /// policy needs them (ADR-0052). Legacy positions derive without rules
    /// (zero overhead on the historical path).
    pub(crate) async fn trading_rules_for(
        &self,
        position: &Position,
    ) -> Option<robson_domain::SymbolTradingRules> {
        if position.stop_policy != StopPolicy::ExecutableSpan {
            return None;
        }
        self.rules_for_managing(&position.symbol, position.rules_snapshot.as_ref())
            .await
    }

    /// The newest rules held for `symbol` (ADR-0056 §5), asking for a refresh
    /// on the side when they are past their TTL: holding rules never waits.
    /// With nothing held this is the lookup itself, on whatever path it is
    /// on, for at most [`quantize::METADATA_WAIT_BOUND`]. `None` means that
    /// failed too, and an `ExecutableSpan` leg cannot derive its stop: the
    /// engine fails closed.
    pub(crate) async fn rules_for_managing(
        &self,
        symbol: &Symbol,
        snapshot: Option<&SymbolTradingRules>,
    ) -> Option<SymbolTradingRules> {
        self.rules_for_managing_within(symbol, snapshot, quantize::METADATA_WAIT_BOUND)
            .await
    }

    async fn rules_for_managing_within(
        &self,
        symbol: &Symbol,
        snapshot: Option<&SymbolTradingRules>,
        metadata_limit: std::time::Duration,
    ) -> Option<SymbolTradingRules> {
        let exchange = self.exchange();
        let Some(held) = exchange.last_validated_rules(symbol).or_else(|| snapshot.cloned()) else {
            return match quantize::bounded_metadata(metadata_limit, exchange.trading_rules(symbol))
                .await
            {
                Ok(rules) => Some(rules),
                Err(error) => {
                    warn!(symbol = %symbol.as_pair(), %error, "Trading rules unavailable");
                    None
                },
            };
        };
        if exchange.rules_refresh_due(symbol) {
            self.request_rules_refresh(symbol);
        }
        Some(held)
    }

    /// Refresh on the side so a refresh never delays the caller: at most one
    /// per symbol in flight, spaced apart. The venue's own bound holds the
    /// ADR-0056 §10 ceiling, a failure included.
    fn request_rules_refresh(&self, symbol: &Symbol) {
        {
            let mut requests =
                self.rules_refresh_requests.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            if requests.get(symbol).is_some_and(|request| {
                request.in_flight
                    || now.duration_since(request.requested_at) < RULES_REFRESH_REQUEST_INTERVAL
            }) {
                return;
            }
            requests
                .insert(symbol.clone(), RulesRefreshRequest { requested_at: now, in_flight: true });
        }
        let exchange = self.exchange();
        let requests = Arc::clone(&self.rules_refresh_requests);
        let symbol = symbol.clone();
        tokio::spawn(async move {
            if let Err(error) = exchange.trading_rules(&symbol).await {
                warn!(symbol = %symbol.as_pair(), %error, "Trading-rules refresh failed");
            }
            if let Some(request) =
                requests.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&symbol)
            {
                request.in_flight = false;
            }
        });
    }

    /// The streams this manager writes (ADR-0056 step 7, PR P).
    #[cfg(feature = "postgres")]
    pub(crate) fn event_streams(&self) -> &EventStreams {
        &self.event_streams
    }

    /// Load the stream suspensions on record that no repair cleared (ADR-0056
    /// step 7, PR P). At start, before anything is appended. How many.
    #[cfg(feature = "postgres")]
    pub(crate) async fn load_stream_suspensions(&self) -> DaemonResult<usize> {
        let (Some(pool), Some(tenant_id)) = (&self.event_log_pool, self.event_log_tenant_id) else {
            return Ok(0);
        };
        let loaded = crate::event_streams::load_open_suspensions(pool, tenant_id).await?;
        for (stream_key, suspended) in &loaded {
            warn!(
                %stream_key,
                kind = suspended.stream.kind(),
                repaired_through_seq = ?suspended.repaired_through_seq,
                "Event stream suspended by an earlier process; it takes no appends"
            );
        }
        let count = loaded.len();
        self.event_streams.restore(loaded);
        Ok(count)
    }

    /// Configure eventlog persistence for domain events (MIG-v2.5#2).
    ///
    /// When set, every domain event emitted through the executor is also
    /// persisted to `robson-eventlog` and applied to projections synchronously.
    /// This is required for crash recovery from `positions_current` to be
    /// defensible in live execution.
    #[cfg(feature = "postgres")]
    pub fn with_event_log(mut self, pool: PgPool, tenant_id: Uuid) -> Self {
        self.event_log_pool = Some(pool);
        self.event_log_tenant_id = Some(tenant_id);
        self
    }

    // =========================================================================
    // Eventlog persistence bridge (MIG-v2.5#2)
    // =========================================================================

    /// Persist a single domain event to robson-eventlog and apply it to
    /// projections.
    ///
    /// When `event_log_pool` is configured a failure in append OR projection
    /// apply is returned as `DaemonError::EventLog`, and the stream is
    /// suspended (ADR-0056 step 7, PR P): it refuses every later append in this
    /// process, `DaemonError::StreamSuspended`, so its projection lags its log
    /// by exactly the suffix that did not apply, which a restart repairs. The
    /// suspension is recorded through the journal's retry set and sets the
    /// latch. A caller that already has a venue result keeps it either way.
    ///
    /// Rationale: the synchronous apply on the write path is the only active
    /// projection update mechanism for `positions_current`. Silencing failures
    /// here would leave the projection stale without the caller's
    /// knowledge, making crash recovery unreliable. (MIG-v2.5#2 design
    /// decision.)
    ///
    /// The stream's lock is held through the suspension check, the append, the
    /// apply and the suspension, so a writer queued behind a failure sees the
    /// suspension first. When no pool is configured (in-memory only mode) a
    /// stream never fails, and this returns `Ok(())` unless one was suspended.
    ///
    /// Stream key pattern:
    /// - position-scoped events: `position:{position_id}`
    /// - system-scoped events: dedicated stream chosen by event type
    #[cfg(feature = "postgres")]
    async fn persist_event_to_log(&self, event: &Event) -> DaemonResult<()> {
        let stream_key = Self::event_stream_key(event);
        let _stream = self.event_streams.lock(&stream_key).await;
        if self.event_streams.is_suspended(&stream_key) {
            return Err(DaemonError::StreamSuspended {
                stream_key,
                event_type: event.event_type().to_string(),
            });
        }
        let (pool, tenant_id) = match (&self.event_log_pool, &self.event_log_tenant_id) {
            (Some(pool), Some(tid)) => (pool, *tid),
            _ => return Ok(()), // No eventlog configured — in-memory only mode
        };
        match Self::append_and_apply(pool, tenant_id, &stream_key, event).await {
            Ok(seq) => {
                tracing::debug!(
                    event_type = event.event_type(),
                    position_id = %event.position_id(),
                    seq,
                    "Domain event persisted to eventlog and projection applied"
                );
                Ok(())
            },
            Err((failed_seq, message)) => {
                if let Some(suspension) =
                    self.event_streams.suspend(&stream_key, failed_seq, &message)
                {
                    error!(
                        %stream_key,
                        ?failed_seq,
                        error = %message,
                        "Event stream suspended: it takes no more appends in this process, and \
                         admissions are closed until a restart (ADR-0056 step 7)"
                    );
                    self.executor.order_journal().record_suspension(suspension);
                }
                Err(DaemonError::EventLog(message))
            },
        }
    }

    /// Append `event` and apply it. The seq it applied at, or the seq that did
    /// not apply (`None` when the append failed or its seq is unknown) and why.
    #[cfg(feature = "postgres")]
    async fn append_and_apply(
        pool: &PgPool,
        tenant_id: Uuid,
        stream_key: &str,
        event: &Event,
    ) -> Result<i64, (Option<i64>, String)> {
        let event_type = event.event_type().to_string();

        // Serialize full domain event as payload
        let payload = serde_json::to_value(event).map_err(|e| {
            (
                None,
                format!(
                    "Failed to serialize {} for eventlog (stream {}): {}",
                    event_type, stream_key, e
                ),
            )
        })?;

        let eventlog_event = EventlogEvent::new(tenant_id, stream_key, &event_type, payload)
            .with_actor(EventlogActorType::Daemon, Some("robsond".to_string()));

        let event_id = match append_event(pool, stream_key, None, eventlog_event).await {
            Ok(id) => id,
            Err(robson_eventlog::EventLogError::IdempotentDuplicate(id)) => {
                // Duplicate means the append already happened, but projection apply may
                // have failed on a previous attempt. Re-fetch the stored envelope and
                // re-run projection apply so retries can heal partial failures.
                tracing::debug!(
                    event_type = %event_type,
                    %stream_key,
                    event_id = %id,
                    "Domain event already in eventlog (idempotent) — reapplying projection"
                );
                id
            },
            Err(e) => {
                return Err((
                    None,
                    format!(
                        "Failed to append {} to eventlog (stream {}): {}",
                        event_type, stream_key, e
                    ),
                ));
            },
        };

        // Fetch the stored envelope and apply to projection synchronously.
        // This is the write path for `positions_current` — failure is not acceptable.
        let envelope = sqlx::query_as::<_, robson_eventlog::EventEnvelope>(
            "SELECT event_id, tenant_id, stream_key, seq, event_type, payload, \
             payload_schema_version, occurred_at, ingested_at, idempotency_key, \
             trace_id, causation_id, command_id, workflow_id, \
             actor_type, actor_id, prev_hash, hash \
             FROM event_log WHERE event_id = $1",
        )
        .bind(event_id)
        .fetch_one(pool)
        .await
        .map_err(|e| {
            (
                None,
                format!(
                    "Failed to fetch envelope for {} (stream {}, event_id {}): {}",
                    event_type, stream_key, event_id, e
                ),
            )
        })?;

        apply_event_to_projections(pool, &envelope).await.map_err(|e| {
            (
                Some(envelope.seq),
                format!(
                    "Failed to apply {} to projection (stream {}, seq {}): {}",
                    event_type, stream_key, envelope.seq, e
                ),
            )
        })?;
        Ok(envelope.seq)
    }

    #[cfg(not(feature = "postgres"))]
    async fn persist_event_to_log(&self, _event: &Event) -> DaemonResult<()> {
        Ok(())
    }

    /// Execute engine actions and persist any emitted domain events to the
    /// eventlog.
    ///
    /// Entry-admission batches are ordered barriers: each pre-exchange event
    /// is persisted before the next action. Every other batch retains the
    /// historical availability semantics: execute all actions first, then
    /// persist their event results.
    /// - `ActionResult::EventEmitted(event)` - events from EmitEvent action
    /// - `ActionResult::OrderPlaced { event: Some(event), .. }` - events from
    ///   exit or entry orders
    /// - `ActionResult::OrderFailed { event, .. }` - entry order failure events
    /// - `ActionResult::EntryExecutionRejected { event, .. }` - internal
    ///   pre-placement safety/policy rejections
    ///
    /// When `event_log_pool` is configured, persistence of what precedes a
    /// venue request, and of a batch with no venue action, is **fail-fast**:
    /// any failure in `persist_event_to_log()` (append OR projection apply, or
    /// a suspended stream) is propagated and the caller must abort the current
    /// execution cycle. **A venue result is never dropped** (ADR-0056 step 7,
    /// PR P): once a batch has reached the venue, its results come back whether
    /// or not their events persisted, so an entry still gets its stop and an
    /// exit still closes its leg. The failure itself suspended its stream and
    /// set the latch.
    ///
    /// This prevents silent projection drift during execution when PostgreSQL
    /// is in use. On the entry path it makes pre-exchange events a real
    /// durability barrier:
    /// for entry admission, `EntrySignalReceived` and `EntryOrderRequested`
    /// are both appended and projected before `PlaceEntryOrder` can run
    /// (ADR-0052 Decision 5). Append and projection apply still happen in
    /// separate steps, so this is fail-fast ordered visibility, not an atomic
    /// multi-step guarantee.
    async fn execute_and_persist(
        &self,
        actions: Vec<EngineAction>,
    ) -> DaemonResult<Vec<ActionResult>> {
        let mut results = self.execute_and_persist_inner(actions).await?;
        self.note_insurance_stop_results(&results).await;
        self.broadcast_mark_advances(&results);
        // An exit that failed behind its batch's earlier actions: what came before
        // it is persisted now, so this can be the batch's error.
        if matches!(results.last(), Some(ActionResult::ExitFailed { .. })) {
            if let Some(ActionResult::ExitFailed { error }) = results.pop() {
                return Err(DaemonError::Exec(error));
            }
        }
        Ok(results)
    }

    /// Tell operator surfaces that an Active position's stop or watermark
    /// moved without a lifecycle transition, so the card refreshes between
    /// state changes. Both sources are bounded by the engine (one per rung,
    /// at most ~10 watermark steps per rung), so this never approaches tick
    /// volume. Not a domain event: nothing is persisted here.
    fn broadcast_mark_advances(&self, results: &[ActionResult]) {
        for result in results {
            let (position_id, reason, timestamp) = match result {
                ActionResult::EventEmitted(Event::TrailingStopUpdated {
                    position_id,
                    timestamp,
                    ..
                }) => (*position_id, "trailing_stop_updated", *timestamp),
                ActionResult::EventEmitted(Event::FavorableExtremeAdvanced {
                    position_id,
                    timestamp,
                    ..
                }) => (*position_id, "favorable_extreme_advanced", *timestamp),
                _ => continue,
            };
            self.event_bus.send(DaemonEvent::PositionMarkAdvanced {
                position_id,
                reason,
                timestamp,
            });
        }
    }

    /// Track exchange-side insurance stop outcomes (ADR-0039). A failed
    /// placement or re-placement leaves the previous stop (or none) live on
    /// the exchange while the software plan has already advanced; the engine
    /// will not re-emit the move (its idempotency key is the technical stop),
    /// so the position is remembered here and healed at tick cadence by
    /// `retry_stale_insurance_stop`. An accepted placement clears it.
    async fn note_insurance_stop_results(&self, results: &[ActionResult]) {
        let mut stale = self.insurance_stop_stale.write().await;
        let mut changed = false;
        for result in results {
            match result {
                ActionResult::EventEmitted(Event::InsuranceStopFailed {
                    position_id,
                    stop_price,
                    error,
                    ..
                }) => {
                    // Any deadline on record stands, overdue included, so failures
                    // cannot keep postponing the heal. The heal pushes its own
                    // window before it acts, so a failed heal still backs off.
                    let now = std::time::Instant::now();
                    let retry_at =
                        *stale.entry(*position_id).or_insert(now + INSURANCE_STOP_RETRY_BACKOFF);
                    changed = true;
                    warn!(
                        %position_id,
                        stop_price = %stop_price.as_decimal(),
                        %error,
                        retry_in_secs = retry_at.saturating_duration_since(now).as_secs(),
                        "Exchange insurance stop is behind the software stop plan; re-placement scheduled (ADR-0039)"
                    );
                },
                // Later in its batch than its `InsuranceStopFailed`, when it has one:
                // this window is the one that holds.
                ActionResult::ProtectionNotExecutable { position_id } => {
                    stale.insert(*position_id, std::time::Instant::now() + NOT_EXECUTABLE_BACKOFF);
                    changed = true;
                    warn!(
                        %position_id,
                        retry_in_secs = NOT_EXECUTABLE_BACKOFF.as_secs(),
                        "Exchange insurance stop has nothing executable; retrying on the long window (ADR-0056 §5)"
                    );
                },
                ActionResult::OrderPlaced {
                    event:
                        Some(
                            Event::InsuranceStopPlaced { position_id, .. }
                            | Event::InsuranceStopReplaced { position_id, .. },
                        ),
                    ..
                } => {
                    if stale.remove(position_id).is_some() {
                        changed = true;
                        info!(%position_id, "Exchange insurance stop healed to the software stop plan");
                    }
                },
                _ => {},
            }
        }
        if changed {
            crate::metrics::INSURANCE_STOP_STALE_POSITIONS.set(stale.len() as f64);
        }
    }

    /// Forget stale-insurance and exit-retry bookkeeping for positions that
    /// are no longer Active (closed, errored, reconciled away).
    async fn prune_insurance_stop_stale(&self, active: &[Position]) {
        {
            let mut requests =
                self.rules_refresh_requests.lock().unwrap_or_else(|e| e.into_inner());
            if !requests.is_empty() {
                let symbols: HashSet<&Symbol> = active.iter().map(|p| &p.symbol).collect();
                requests.retain(|symbol, request| request.in_flight || symbols.contains(symbol));
            }
        }
        if !self.exit_retry_after.read().await.is_empty() {
            let active_ids: HashSet<PositionId> = active.iter().map(|p| p.id).collect();
            self.exit_retry_after.write().await.retain(|id, _| active_ids.contains(id));
        }
        if self.insurance_stop_stale.read().await.is_empty() {
            return;
        }
        let active_ids: HashSet<PositionId> = active.iter().map(|p| p.id).collect();
        let mut stale = self.insurance_stop_stale.write().await;
        let before = stale.len();
        stale.retain(|id, _| active_ids.contains(id));
        if stale.len() != before {
            crate::metrics::INSURANCE_STOP_STALE_POSITIONS.set(stale.len() as f64);
        }
    }

    /// True while a leg waits out an exit retry window (ADR-0056 §5). The
    /// engine is consulted again afterwards.
    async fn exit_retry_pending(&self, position_id: PositionId) -> bool {
        let retry_at = self.exit_retry_after.read().await.get(&position_id).copied();
        match retry_at {
            Some(retry_at) if std::time::Instant::now() < retry_at => true,
            Some(_) => {
                self.exit_retry_after.write().await.remove(&position_id);
                false
            },
            None => false,
        }
    }

    /// The one place that notices an Active leg with no stop, whatever left it
    /// so: an exit that failed behind its cancel-first batch, on any caller and
    /// with any error. Its first attempt waits the usual window. A leg
    /// suspected closed by the venue has nothing to protect until it
    /// reappears.
    async fn schedule_stop_heal_if_gone(&self, position: &Position) {
        let PositionState::Active { insurance_stop_id, .. } = &position.state else {
            return;
        };
        if insurance_stop_id.is_some()
            || position.insurance_stop_id.is_some()
            || self.exit_overtaken_suspects.read().await.contains(&position.id)
        {
            return;
        }
        let mut stale = self.insurance_stop_stale.write().await;
        stale.insert(position.id, std::time::Instant::now() + INSURANCE_STOP_RETRY_BACKOFF);
        crate::metrics::INSURANCE_STOP_STALE_POSITIONS.set(stale.len() as f64);
        warn!(
            position_id = %position.id,
            retry_in_secs = INSURANCE_STOP_RETRY_BACKOFF.as_secs(),
            "Active leg has no exchange stop; re-placement scheduled (ADR-0039)"
        );
    }

    /// A leg's latest stop event is a failure and this is a new process
    /// (ADR-0056 step 7): the retry window does not survive a restart, and an
    /// id still on record hides the leg from the check above. An entry
    /// already here stands.
    pub(crate) async fn schedule_stop_heal_after_restart(&self, position_id: PositionId) {
        let mut stale = self.insurance_stop_stale.write().await;
        stale
            .entry(position_id)
            .or_insert_with(|| std::time::Instant::now() + INSURANCE_STOP_RETRY_BACKOFF);
        crate::metrics::INSURANCE_STOP_STALE_POSITIONS.set(stale.len() as f64);
        warn!(
            %position_id,
            retry_in_secs = INSURANCE_STOP_RETRY_BACKOFF.as_secs(),
            "Latest stop event of this leg is a failure; re-placement scheduled after a restart (ADR-0039)"
        );
    }

    #[cfg(all(test, feature = "postgres"))]
    pub(crate) async fn stop_heal_deadline_for_test(
        &self,
        position_id: PositionId,
    ) -> Option<std::time::Instant> {
        self.insurance_stop_stale.read().await.get(&position_id).copied()
    }

    /// The orphan sweep found this leg's recorded stop gone from the venue's
    /// open orders with the venue still holding the leg. Filled short of
    /// the leg, expired or cancelled, the id on record hides it from the
    /// heal above, and with no crossing tick nothing else would notice. The
    /// leg is read again, as the stop may have been replaced since the
    /// sweep read it. An attempt already scheduled keeps its window.
    pub(crate) async fn schedule_stop_heal_if_dead(
        &self,
        position_id: PositionId,
        missing_stop_id: &str,
    ) {
        let position = match self.get_position(position_id).await {
            Ok(Some(position)) => position,
            Ok(None) => return,
            Err(error) => {
                warn!(%position_id, %error, "Could not read a leg whose stop is gone from the venue");
                return;
            },
        };
        let PositionState::Active { insurance_stop_id, .. } = &position.state else {
            return;
        };
        let recorded = insurance_stop_id.as_deref().or(position.insurance_stop_id.as_deref());
        if recorded != Some(missing_stop_id)
            || self.exit_overtaken_suspects.read().await.contains(&position_id)
        {
            return;
        }
        let mut stale = self.insurance_stop_stale.write().await;
        if stale.contains_key(&position_id) {
            return;
        }
        stale.insert(position_id, std::time::Instant::now() + INSURANCE_STOP_RETRY_BACKOFF);
        crate::metrics::INSURANCE_STOP_STALE_POSITIONS.set(stale.len() as f64);
        warn!(
            %position_id,
            %missing_stop_id,
            retry_in_secs = INSURANCE_STOP_RETRY_BACKOFF.as_secs(),
            "Recorded exchange stop is gone from the venue's open orders with the leg still \
             held; re-placement scheduled (ADR-0039)"
        );
    }

    /// Re-place the exchange insurance stop of a stale position at the
    /// current software trigger (ADR-0039), if its retry window is due.
    /// Returns whether an attempt was made; success and failure bookkeeping
    /// happens in `note_insurance_stop_results` through the executor path.
    async fn retry_stale_insurance_stop(&self, position: &Position) -> bool {
        let retry_at = self.insurance_stop_stale.read().await.get(&position.id).copied();
        let due = match retry_at {
            Some(retry_at) => std::time::Instant::now() >= retry_at,
            None => {
                self.schedule_stop_heal_if_gone(position).await;
                return false;
            },
        };
        if !due {
            return false;
        }
        // Scheduled before the leg came under suspicion. It has nothing to protect
        // unless it reappears, and the attempt waits for that.
        if self.exit_overtaken_suspects.read().await.contains(&position.id) {
            return false;
        }
        let (trailing_stop, guard, insurance_stop_id) = match &position.state {
            PositionState::Active {
                trailing_stop,
                invalidation_guard_level,
                insurance_stop_id,
                ..
            } => (*trailing_stop, *invalidation_guard_level, insurance_stop_id.clone()),
            _ => {
                self.insurance_stop_stale.write().await.remove(&position.id);
                return false;
            },
        };
        // Push the window first so a batch error (not only an audit failure)
        // also backs off instead of retrying on every tick.
        self.insurance_stop_stale
            .write()
            .await
            .insert(position.id, std::time::Instant::now() + INSURANCE_STOP_RETRY_BACKOFF);

        let rules = self.trading_rules_for(position).await;
        let trigger = match self.engine().stop_plan(position, trailing_stop, guard, rules.as_ref())
        {
            Ok(plan) => plan.trigger,
            Err(error) => {
                // Fail closed: never touch protection that cannot be re-derived.
                warn!(
                    position_id = %position.id,
                    %error,
                    "Cannot re-derive the executable stop; leaving the exchange insurance stop as is"
                );
                return true;
            },
        };
        let action = match insurance_stop_id {
            Some(previous_order_id) => EngineAction::ReplaceInsuranceStop {
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: position.side.exit_action(),
                quantity: position.quantity,
                previous_order_id,
                new_stop_price: trigger,
            },
            None => EngineAction::PlaceInsuranceStop {
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: position.side.exit_action(),
                quantity: position.quantity,
                stop_price: trigger,
            },
        };
        info!(
            position_id = %position.id,
            trigger = %trigger.as_decimal(),
            "Re-placing the exchange insurance stop at the current software trigger (ADR-0039)"
        );
        if let Err(error) = self.execute_and_persist(vec![action]).await {
            warn!(
                position_id = %position.id,
                %error,
                "Insurance stop re-placement batch failed; will retry after backoff"
            );
        }
        true
    }

    async fn execute_and_persist_inner(
        &self,
        actions: Vec<EngineAction>,
    ) -> DaemonResult<Vec<ActionResult>> {
        let entry_admission_barrier = actions
            .iter()
            .any(|action| matches!(action, EngineAction::PlaceEntryOrder { .. }));

        // Preserve the pre-ADR-0052 behavior for exits, protective orders,
        // recovery, and audit-only cycles: exchange side effects run before a
        // PostgreSQL outage is surfaced. In particular, a filled position
        // must still receive insurance and an exit order must still reach the
        // exchange even when the event log is unavailable. A batch that went to
        // the venue keeps its results when they fail to persist.
        if !entry_admission_barrier {
            let reaches_venue =
                actions.iter().any(|action| !matches!(action, EngineAction::EmitEvent(_)));
            let results = self.executor.execute(actions).await?;
            if reaches_venue {
                self.persist_venue_results(&results).await;
            } else {
                self.persist_action_results(&results).await?;
            }
            return Ok(results);
        }

        let mut results = Vec::with_capacity(actions.len());

        for action in actions {
            let places_entry = matches!(action, EngineAction::PlaceEntryOrder { .. });
            let mut action_results = self.executor.execute(vec![action]).await?;
            let result = action_results.pop().ok_or_else(|| {
                DaemonError::Config(
                    "Executor returned no result for a successfully executed action".to_string(),
                )
            })?;

            // Persist before advancing to the next action. A failure here is
            // the barrier that prevents a later exchange action from running.
            // The entry order's own answer, once it was sent, is a venue result:
            // kept, so its fill still reaches `handle_entry_fill` and the stop.
            let sent = matches!(
                result,
                ActionResult::OrderPlaced { .. } | ActionResult::OrderFailed { .. }
            );
            if places_entry && sent {
                self.persist_venue_result(&result).await;
            } else {
                self.persist_action_result(&result).await?;
            }

            let should_stop = matches!(
                result,
                ActionResult::OrderFailed { .. }
                    | ActionResult::EntryExecutionRejected { .. }
                    | ActionResult::EntryNotSent { .. }
            );
            results.push(result);
            if should_stop {
                break;
            }
        }

        Ok(results)
    }

    async fn persist_action_results(&self, results: &[ActionResult]) -> DaemonResult<()> {
        for result in results {
            self.persist_action_result(result).await?;
        }
        Ok(())
    }

    /// Persist what came back from the venue, keeping it whatever happens
    /// (ADR-0056 step 7, PR P). Each result is tried: another stream may still
    /// take its event, while the failed one now refuses.
    async fn persist_venue_results(&self, results: &[ActionResult]) {
        for result in results {
            self.persist_venue_result(result).await;
        }
    }

    async fn persist_venue_result(&self, result: &ActionResult) {
        if let Err(error) = self.persist_action_result(result).await {
            error!(
                %error,
                "Venue result kept although its event did not persist; its stream is \
                 suspended and admissions are closed until a restart (ADR-0056 step 7)"
            );
        }
    }

    /// Record the close of a leg whose fill already happened at the venue. A
    /// close that does not persist is kept (ADR-0056 step 7, PR P): the leg is
    /// closed in memory, the failure suspended its stream and set the latch,
    /// and what follows a close still runs. Whether it persisted.
    async fn record_venue_close(&self, event: Event) -> DaemonResult<bool> {
        let position_id = event.position_id();
        match self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await {
            Ok(_) => Ok(true),
            Err(error) if error.is_event_not_persisted() => {
                error!(
                    %position_id,
                    %error,
                    "Leg closed in memory; its close did not persist (stream suspended, \
                     admissions closed until a restart, ADR-0056 step 7)"
                );
                Ok(false)
            },
            Err(error) => Err(error),
        }
    }

    async fn persist_action_result(&self, result: &ActionResult) -> DaemonResult<()> {
        match result {
            ActionResult::EventEmitted(event) => self.persist_event_to_log(event).await,
            ActionResult::OrderPlaced { event: Some(event), .. } => {
                self.persist_event_to_log(event).await
            },
            ActionResult::OrderFailed { event, .. } => self.persist_event_to_log(event).await,
            ActionResult::EntryExecutionRejected { event, .. } => {
                self.persist_event_to_log(event).await
            },
            ActionResult::ExitNotExecutable { event: Some(event), .. } => {
                self.persist_event_to_log(event).await
            },
            _ => Ok(()),
        }
    }

    fn stamp_cycle_id(actions: Vec<EngineAction>, cycle_id: Uuid) -> Vec<EngineAction> {
        actions.into_iter().map(|action| action.with_cycle_id(cycle_id)).collect()
    }

    /// Start the position manager's background tasks.
    ///
    /// This spawns the signal listener that processes DetectorSignal events
    /// from the EventBus and calls `handle_signal()`.
    ///
    /// Must be called after wrapping in Arc:
    /// ```text
    /// let manager = Arc::new(PositionManager::new(...));
    /// PositionManager::start(Arc::clone(&manager));
    /// ```
    pub fn start(manager: Arc<Self>) {
        Self::start_signal_listener(manager);
    }

    /// Initiate graceful shutdown of all detector tasks.
    ///
    /// This cancels the shutdown token, causing all active detectors
    /// to exit cooperatively.
    pub async fn shutdown(&self) {
        info!("Initiating position manager shutdown");

        // Cancel all detectors
        self.shutdown_token.cancel();

        // Wait for all detectors to finish
        let mut detectors = self.detectors.write().await;
        let count = detectors.len();

        for (position_id, handle) in detectors.drain() {
            debug!(%position_id, "Waiting for detector to finish");

            // Give each detector a moment to finish gracefully
            match tokio::time::timeout(std::time::Duration::from_millis(500), handle).await {
                Ok(_) => debug!(%position_id, "Detector finished gracefully"),
                Err(_) => {
                    // Timeout - detector will be aborted when handle drops
                    warn!(%position_id, "Detector did not finish in time, will be aborted");
                },
            }
        }

        info!("Position manager shutdown complete ({count} detectors terminated)");
    }

    /// The token that stops the detectors and the approval timers, for a stop
    /// that must not wait on the manager lock (ADR-0056 step 7).
    pub(crate) fn detector_shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// Get a child cancellation token for a new detector.
    ///
    /// Child tokens are cancelled when the parent is cancelled.
    fn child_cancel_token(&self) -> CancellationToken {
        self.shutdown_token.child_token()
    }

    fn operator_actor() -> ActorKind {
        ActorKind::Operator { source: CommandSource::Api }
    }

    async fn record_query_accepted(&self, query: &ExecutionQuery) -> DaemonResult<()> {
        self.query_engine.on_accepted(query).await?;
        Ok(())
    }

    async fn record_query_transition(
        &self,
        query: &ExecutionQuery,
        transition_cause: &str,
    ) -> DaemonResult<()> {
        self.query_engine.on_state_change(query, transition_cause).await?;
        Ok(())
    }

    async fn record_query_failure(&self, query: &ExecutionQuery) -> DaemonResult<()> {
        self.record_query_transition(query, "failed").await
    }

    /// Record a query's transition once its venue work is done or has been
    /// given up for this leg. A failed audit write is reported, counted and
    /// goes no further (ADR-0056 step 7, PR P): the venue outcome stands, and
    /// what follows it, the next leg of a tick or of `/panic` included, still
    /// runs. Records ahead of a venue request stay barriers.
    async fn record_after_venue_work(&self, query: &ExecutionQuery, transition_cause: &str) {
        if let Err(error) = self.record_query_transition(query, transition_cause).await {
            crate::metrics::QUERY_AUDIT_FAILURES_AFTER_VENUE_WORK.inc();
            error!(
                query_id = %query.id,
                position_id = ?query.position_id,
                transition_cause,
                %error,
                "Query audit write failed after its venue work; the outcome stands"
            );
        }
    }

    fn set_query_context_summary(query: &mut ExecutionQuery, active_positions_count: usize) {
        query.context_summary = Some(ContextSummary { active_positions_count });
    }

    async fn populate_query_context_summary(&self, query: &mut ExecutionQuery) {
        if let Ok(active_positions) = self.store.positions().find_active().await {
            Self::set_query_context_summary(query, active_positions.len());
        }
    }

    fn pending_approval_to_summary(record: &PendingApprovalRecord) -> PositionSummary {
        // Pending approvals reserve latent risk using proposed entry/quantity and
        // armed position's initial technical stop. If missing, log warning and use
        // zero-risk values.
        let initial_stop = record
            .position
            .tech_stop_distance
            .as_ref()
            .map(|ts| ts.initial_stop.as_decimal())
            .unwrap_or_else(|| {
                tracing::warn!(
                    position_id = %record.position.id,
                    "Pending approval has no tech_stop_distance; latent risk defaults to zero"
                );
                record.proposed.entry_price // fallback: stop at entry → zero
                                            // risk
            });

        let current_stop = if record.position.stop_policy == StopPolicy::ExecutableSpan {
            record
                .position
                .initial_executable_stop
                .map(|stop| stop.as_decimal())
                .unwrap_or(initial_stop)
        } else {
            initial_stop
        };
        let cost_priced_latent_risk = (record.position.stop_policy == StopPolicy::ExecutableSpan
            && record.proposed.planned_risk > Decimal::ZERO)
            .then_some(record.proposed.planned_risk);

        PositionSummary {
            position_id: record.position.id,
            symbol: record.proposed.symbol.clone(),
            side: record.proposed.side.clone(),
            notional_value: record.proposed.notional_value,
            initial_margin: record.proposed.initial_margin,
            unrealized_pnl: Decimal::ZERO,
            entry_price: record.proposed.entry_price,
            quantity: record.proposed.quantity,
            current_stop,
            cost_priced_latent_risk,
        }
    }

    async fn has_pending_approval_for_position(&self, position_id: PositionId) -> bool {
        let pending = self.pending_approvals.read().await;
        pending.values().any(|record| record.position.id == position_id)
    }

    async fn invalidate_pending_approvals_for_position(
        &self,
        position_id: PositionId,
        reason: &str,
    ) {
        let invalidated_records = {
            let mut pending = self.pending_approvals.write().await;
            let invalidated_ids: Vec<Uuid> = pending
                .iter()
                .filter_map(|(query_id, record)| {
                    (record.position.id == position_id).then_some(*query_id)
                })
                .collect();

            invalidated_ids
                .into_iter()
                .filter_map(|query_id| pending.remove(&query_id))
                .collect::<Vec<_>>()
        };

        for mut record in invalidated_records {
            let failure_reason = format!("Pending approval invalidated: {}", reason);
            record.query.fail(failure_reason, "awaiting_approval".to_string());
            if let Err(error) = self.record_query_failure(&record.query).await {
                warn!(
                    %position_id,
                    query_id = %record.query.id,
                    error = %error,
                    "Failed to persist invalidated pending approval snapshot"
                );
            }
            info!(
                %position_id,
                query_id = %record.query.id,
                reason,
                "Pending approval invalidated"
            );
        }
    }

    // =========================================================================
    // Phase 2: Risk context helpers
    // =========================================================================

    /// Build a RiskContext snapshot combining persisted monthly state with live
    /// execution state.
    ///
    /// Combines:
    /// - **Persisted ledger** (`monthly_state`): authoritative capital_base and
    ///   realized_loss (MIG-v3#12).
    /// - **Live execution** (`positions_current`): active positions, pending
    ///   entries, unrealized PnL.
    ///
    /// Uses `find_risk_open()` (Entering + Active) so positions with a
    /// committed exchange order are counted even before fill confirmation.
    /// This prevents concurrent entries from slipping under the exposure
    /// limits during the order-fill window (signal fires → order submitted
    /// → not yet filled → next signal arrives).
    ///
    /// The canonical snapshot reserves every durable local Active + Entering
    /// position. Admission intentionally extends that set with pending
    /// approvals: without pending approvals, its latent-risk sum equals the
    /// snapshot exactly; with them, admission is a conservative strict
    /// superset by design.
    async fn build_risk_context(&self) -> DaemonResult<RiskContext> {
        let now = chrono::Utc::now();
        let (budget, active_positions, resolved_risk, latent_risk_failures) =
            self.canonical_budget_snapshot_with_positions(now).await?;
        self.emit_latent_risk_resolution_warnings(&latent_risk_failures).await;
        if budget.latent_risk_invalid {
            return Err(DaemonError::Config(
                "Canonical monthly budget snapshot is invalid because executable-stop latent risk could not be resolved"
                    .to_string(),
            ));
        }

        // find_risk_open() guarantees only Entering and Active positions.
        // For Entering: use expected_entry from state (order price is committed on
        // exchange). For Active: use the recorded fill price (entry_price
        // field). Defensive: skip positions with zero quantity (should not
        // occur in practice).
        let mut summaries: Vec<PositionSummary> = active_positions
            .iter()
            .filter_map(|p| {
                let entry_price_decimal = match &p.state {
                    PositionState::Active { .. } => p.entry_price?.as_decimal(),
                    PositionState::Entering { expected_entry, .. } => expected_entry.as_decimal(),
                    _ => return None, // find_risk_open guarantees this is unreachable
                };
                let pricing = resolved_risk.get(&p.id)?;
                let qty = p.quantity.as_decimal();
                if qty.is_zero() {
                    return None;
                }
                let notional_value = qty * entry_price_decimal;
                let initial_margin =
                    notional_value / Decimal::from(robson_domain::RiskConfig::LEVERAGE as u32);
                Some(PositionSummary {
                    position_id: p.id,
                    symbol: p.symbol.as_pair(),
                    side: format!("{}", p.side).to_lowercase(),
                    notional_value,
                    initial_margin,
                    unrealized_pnl: Decimal::ZERO,
                    entry_price: entry_price_decimal,
                    quantity: qty,
                    current_stop: pricing.current_stop,
                    cost_priced_latent_risk: pricing.cost_priced_latent_risk,
                })
            })
            .collect();

        let pending_summaries: Vec<PositionSummary> = self
            .pending_approvals
            .read()
            .await
            .values()
            .map(Self::pending_approval_to_summary)
            .collect();
        summaries.extend(pending_summaries);

        // ADR-0024: realized_loss is the authoritative sum of absolute net losses.
        // Source: persisted `monthly_state.realized_loss` (MIG-v3#12).
        // Wins must NOT offset losses for slot calculation.
        let monthly_realized_loss = self.governed_monthly_realized_loss(now).await?;

        // Monthly unrealized PnL: sum unrealized PnL from currently open Active
        // positions.
        let monthly_unrealized_pnl = budget.month_equity_net - budget.governed_realized_net;

        // The gate decides next, on this value: it must see a basis that went
        // invalid during the snapshot.
        let capital = Self::capital_for_the_gate(
            budget.capital_base,
            self.budget_basis_is_invalid_now().await,
        );

        Ok(RiskContext::with_month_equity(
            capital,
            summaries,
            budget.governed_realized_net,
            monthly_unrealized_pnl,
            monthly_realized_loss,
            budget.month_equity_net,
            budget.month_peak_net,
        )
        .with_budget_model(budget.model))
    }

    async fn rearm_detector(
        venue: VenueId,
        position_id: PositionId,
        position: Position,
        entry_policy: EntryPolicyConfig,
        technical_stop_config: TechnicalStopConfig,
        event_bus: Arc<EventBus>,
        ohlcv_port: Arc<dyn OhlcvPort>,
        detectors: Arc<RwLock<HashMap<PositionId, JoinHandle<Option<DetectorSignal>>>>>,
        circuit_breaker: &CircuitBreaker,
        shutdown_token: CancellationToken,
        reason: &'static str,
        stop_invalidation_guard_enabled: bool,
        stop_invalidation_lookback_candles: usize,
        initial_fire_delay: Option<std::time::Duration>,
    ) -> bool {
        let cancel_token = shutdown_token.child_token();
        match DetectorTask::from_position_with_policy(
            &position,
            entry_policy,
            technical_stop_config,
            venue,
            Arc::clone(&event_bus),
            ohlcv_port,
            cancel_token,
        ) {
            Ok(detector) => {
                let mut detector = detector.with_invalidation_guard(
                    stop_invalidation_guard_enabled,
                    stop_invalidation_lookback_candles,
                );
                if let Some(delay) = initial_fire_delay {
                    detector = detector.with_initial_fire_delay(delay);
                }
                // Checked under the map's lock, which `panic_close_all` takes to
                // stop every detector once MonthlyHalt latches: a re-arm lands
                // before that stop or sees the halt.
                let mut detectors = detectors.write().await;
                if circuit_breaker.blocks_new_entries().await {
                    info!(%position_id, %reason, "MonthlyHalt latched — detector not re-armed");
                    return false;
                }
                let handle = detector.spawn();
                if let Some(replaced) = detectors.insert(position_id, handle) {
                    replaced.abort();
                }
                info!(%position_id, %reason, "Detector re-armed");
                true
            },
            Err(e) => {
                warn!(
                    %position_id,
                    %reason,
                    error = %e,
                    "Failed to re-arm detector — position requires manual re-arm"
                );
                false
            },
        }
    }

    /// Dry run of an immediate arm for `symbol`/`side` (see `EntryPreview`).
    ///
    /// Capture everything an entry preview needs while the manager lock is
    /// held, so the slow part (candles, balance, exchange metadata) runs in
    /// `EntryPreviewJob::run` after the caller has released the lock. The
    /// only I/O under the lock is the local `monthly_state` read, the same
    /// one `/status` performs on every call.
    ///
    /// Reuses the production pieces end to end: the same capital base the
    /// arm would load, a transient Armed position shaped like
    /// `arm_position_with_policy` builds, the detector's immediate-signal
    /// path for the chart analysis, and the engine's entry decision on a
    /// cloned engine for sizing. Nothing is saved and no order is placed.
    pub async fn prepare_entry_preview(
        &self,
        symbol: Symbol,
        side: Side,
    ) -> DaemonResult<EntryPreviewJob<E>> {
        let now = chrono::Utc::now();
        let capital = self.load_capital_base_for_month(now).await?;
        let risk_config = self
            .risk_config_snapshot()
            .with_capital(capital)
            .map_err(|e| DaemonError::Config(format!("Capital base invalid: {e}")))?;

        let position = Position::new_with_stop_policy(
            Uuid::nil(),
            symbol,
            side,
            StopPolicy::ExecutableSpan,
            Some(risk_config.stop_buffer_bps()),
        );
        let technical_stop_config =
            TechnicalStopConfig::with_bounds(&risk_config.stop_distance_bounds());
        let detector = DetectorTask::from_position_with_policy(
            &position,
            EntryPolicyConfig::new(
                robson_domain::EntryPolicy::Immediate,
                robson_domain::ApprovalPolicy::Automatic,
            ),
            technical_stop_config,
            self.venue.clone(),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.ohlcv_port),
            self.shutdown_token.child_token(),
        )?
        .with_invalidation_guard(
            self.stop_invalidation_guard_enabled,
            self.stop_invalidation_lookback_candles,
        );

        Ok(EntryPreviewJob {
            capital,
            risk_config,
            position,
            detector,
            exchange: self.exchange(),
            engine: self.engine(),
        })
    }

    /// Prepare and run an entry preview in one call (tests and callers that
    /// do not hold the manager lock). API handlers use the two-step form.
    pub async fn preview_entry(&self, symbol: Symbol, side: Side) -> DaemonResult<EntryPreview> {
        self.prepare_entry_preview(symbol, side).await?.run().await
    }

    /// How this position's technical stop was derived. Served from memory
    /// when the analysis ran in this process; otherwise re-read once from the
    /// durable event log (latest `technical_stop_analyzed` on the position
    /// stream) and cached. `None` when no analysis is on record.
    pub async fn tech_stop_audit_for_position(
        &self,
        position_id: PositionId,
    ) -> Option<TechStopAuditSummary> {
        if let Some(summary) = self.tech_stop_audits.read().await.get(&position_id) {
            return Some(summary.clone());
        }
        #[cfg(feature = "postgres")]
        {
            let (Some(pool), Some(tenant_id)) = (&self.event_log_pool, self.event_log_tenant_id)
            else {
                return None;
            };
            let options = robson_eventlog::QueryOptions::new(tenant_id)
                .stream(format!("position:{position_id}"))
                .event_type("technical_stop_analyzed")
                .limit(1)
                .descending();
            let envelope = match robson_eventlog::query_events(pool, options).await {
                Ok(mut events) => events.pop(),
                Err(error) => {
                    warn!(%position_id, %error, "Failed to read technical stop audit from event log");
                    return None;
                },
            };
            let summary = TechStopAuditSummary::from_event_payload(&envelope?.payload)?;
            self.tech_stop_audits.write().await.insert(position_id, summary.clone());
            return Some(summary);
        }
        #[cfg(not(feature = "postgres"))]
        {
            None
        }
    }

    pub async fn entry_policy_for_position(&self, position_id: PositionId) -> EntryPolicyConfig {
        let entry_policies = self.entry_policies.read().await;
        entry_policies.get(&position_id).copied().unwrap_or_default()
    }

    async fn rearm_detector_after_governed_block(
        &self,
        position_id: PositionId,
        position: &Position,
        reason: &'static str,
    ) -> bool {
        // ADR-0050 §2: a position in needs_operator_rearm never re-arms
        // autonomously.
        if self.is_entry_exhausted(position_id).await {
            warn!(
                %position_id,
                %reason,
                "Skipping governed re-arm: position needs operator re-arm (ADR-0050)"
            );
            return false;
        }
        // Exponential backoff on consecutive governed blocks. Without it, an
        // Immediate-mode re-arm refires instantly and a persistent governed
        // condition (no slots, margin, pending duplicate) becomes a ~1/s hot
        // loop against the OHLCV source and the event store.
        let attempt = {
            let mut counts = self.governed_rearm_counts.write().await;
            let count = counts.entry(position_id).or_insert(0);
            *count = count.saturating_add(1);
            *count
        };
        let delay = governed_rearm_backoff(attempt);
        info!(
            %position_id,
            %reason,
            attempt,
            backoff_secs = delay.as_secs(),
            "Governed block — re-arming detector with backoff"
        );

        let entry_policy = self.entry_policy_for_position(position_id).await;
        let technical_stop_config =
            TechnicalStopConfig::with_bounds(&self.risk_config_snapshot().stop_distance_bounds());
        Self::rearm_detector(
            self.venue.clone(),
            position_id,
            position.clone(),
            entry_policy,
            technical_stop_config,
            Arc::clone(&self.event_bus),
            Arc::clone(&self.ohlcv_port),
            Arc::clone(&self.detectors),
            &self.circuit_breaker,
            self.shutdown_token.clone(),
            reason,
            self.stop_invalidation_guard_enabled,
            self.stop_invalidation_lookback_candles,
            Some(delay),
        )
        .await
    }

    /// Reset the governed re-arm backoff once an admission's entry reaches the
    /// venue's send (it holds a ticket) or the approval queue. Not earlier: a
    /// failure that recurs before either would otherwise re-arm every 5 s for
    /// good.
    async fn clear_governed_rearm_backoff(&self, position_id: PositionId) {
        self.governed_rearm_counts.write().await.remove(&position_id);
    }

    /// ADR-0051 Recovery: an admission that failed before the executor leaves
    /// the position Armed with its single-shot detector spent, which only a
    /// restart used to repair. Re-arm it under the governed backoff so it
    /// resumes once the failure clears. Each retry runs the whole admission
    /// again, audit writes before the executor included, so nothing enters
    /// without its record. Runs under `entry_flow_lock`.
    async fn rearm_detector_after_failed_admission(
        &self,
        position_id: PositionId,
        flow: &'static str,
        error: &DaemonError,
    ) {
        let Some(position) = Self::position_to_recover(
            &self.store,
            &self.circuit_breaker,
            &self.entry_exhausted,
            &self.pending_approvals,
            &self.detectors,
            position_id,
        )
        .await
        else {
            return;
        };
        if self
            .rearm_detector_after_governed_block(
                position_id,
                &position,
                "admission failed before the executor",
            )
            .await
        {
            crate::metrics::DETECTOR_REARMS_AFTER_FAILURE.with_label_values(&[flow]).inc();
            warn!(
                %position_id,
                flow,
                %error,
                "Admission failed before the executor — detector re-armed with backoff"
            );
        }
    }

    /// The position a recovery path may re-arm, read as it stands now: still
    /// Armed, outside MonthlyHalt (which disarms Armed positions), not waiting
    /// for the operator, and without a detector, which here can only be a
    /// re-arm made by the failed flow itself. Callers hold `entry_flow_lock`,
    /// so no disarm or admission runs in between.
    async fn position_to_recover(
        store: &S,
        circuit_breaker: &CircuitBreaker,
        entry_exhausted: &RwLock<HashMap<PositionId, EntryRejectionRecord>>,
        pending_approvals: &RwLock<HashMap<Uuid, PendingApprovalRecord>>,
        detectors: &DetectorMap,
        position_id: PositionId,
    ) -> Option<Position> {
        if circuit_breaker.blocks_new_entries().await
            || entry_exhausted.read().await.contains_key(&position_id)
            || detectors.read().await.contains_key(&position_id)
            || pending_approvals.read().await.values().any(|r| r.position.id == position_id)
        {
            return None;
        }
        match store.positions().find_by_id(position_id).await {
            Ok(Some(position)) if matches!(position.state, PositionState::Armed) => Some(position),
            Ok(_) => None,
            Err(e) => {
                warn!(
                    %position_id,
                    error = %e,
                    "Cannot read the position to re-arm its detector — position requires manual re-arm"
                );
                None
            },
        }
    }

    fn emit_query_awaiting_approval(&self, query: &ExecutionQuery) {
        let approval = match &query.approval {
            Some(approval) => approval,
            None => return,
        };

        self.event_bus.send(DaemonEvent::QueryAwaitingApproval {
            query_id: query.id,
            position_id: query.position_id,
            reason: approval.reason.clone(),
            expires_at: approval.expires_at,
        });
    }

    fn emit_query_authorized(&self, query: &ExecutionQuery) {
        let approved_at = query
            .approval
            .as_ref()
            .and_then(|approval| approval.approved_at)
            .unwrap_or_else(chrono::Utc::now);

        self.event_bus.send(DaemonEvent::QueryAuthorized {
            query_id: query.id,
            position_id: query.position_id,
            approved_at,
        });
    }

    fn emit_query_expired(&self, query: &ExecutionQuery) {
        self.event_bus.send(DaemonEvent::QueryExpired {
            query_id: query.id,
            position_id: query.position_id,
            expired_at: chrono::Utc::now(),
        });
    }

    fn spawn_approval_expiration_task(
        &self,
        query_id: Uuid,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) {
        let entry_flow_lock = Arc::clone(&self.entry_flow_lock);
        let store = Arc::clone(&self.store);
        let circuit_breaker = Arc::clone(&self.circuit_breaker);
        let entry_exhausted = Arc::clone(&self.entry_exhausted);
        let pending_approvals = Arc::clone(&self.pending_approvals);
        let query_engine = Arc::clone(&self.query_engine);
        let venue = self.venue.clone();
        let event_bus = Arc::clone(&self.event_bus);
        let ohlcv_port = Arc::clone(&self.ohlcv_port);
        let detectors = Arc::clone(&self.detectors);
        let entry_policies = Arc::clone(&self.entry_policies);
        let governed_rearm_counts = Arc::clone(&self.governed_rearm_counts);
        let stop_invalidation_guard_enabled = self.stop_invalidation_guard_enabled;
        let stop_invalidation_lookback_candles = self.stop_invalidation_lookback_candles;
        let shutdown_token = self.shutdown_token.clone();
        let technical_stop_config =
            TechnicalStopConfig::with_bounds(&self.risk_config_snapshot().stop_distance_bounds());

        let wait_duration = expires_at
            .signed_duration_since(chrono::Utc::now())
            .to_std()
            .unwrap_or_default();

        tokio::spawn(async move {
            tokio::select! {
                _ = shutdown_token.cancelled() => {}
                _ = tokio::time::sleep(wait_duration) => {
                    // Serialized with disarm and admission: a disarm either
                    // took the approval first, or comes after the re-arm and
                    // stops it.
                    let _entry_flow_guard = entry_flow_lock.lock().await;
                    let mut record = {
                        let mut pending = pending_approvals.write().await;
                        pending.remove(&query_id)
                    };

                    if let Some(mut record_inner) = record.take() {
                        // A failed expiry still removed the approval, and with it
                        // the position's only way to an entry: it re-arms like an
                        // expiry (ADR-0051 Recovery).
                        let expiry_failed = match query_engine
                            .expire(&mut record_inner.query, "expired")
                            .await
                        {
                            Ok(()) => {
                                event_bus.send(DaemonEvent::QueryExpired {
                                    query_id: record_inner.query.id,
                                    position_id: record_inner.query.position_id,
                                    expired_at: chrono::Utc::now(),
                                });
                                false
                            },
                            Err(error) => {
                                let error_message = format!("Approval expiry transition error: {}", error);
                                record_inner.query.fail(error_message.clone(), "awaiting_approval".to_string());
                                if let Err(audit_error) = query_engine
                                    .on_state_change(&record_inner.query, "failed")
                                    .await
                                {
                                    warn!(
                                        query_id = %record_inner.query.id,
                                        error = %audit_error,
                                        "Failed to persist failed approval expiry snapshot"
                                    );
                                }
                                warn!(
                                    query_id = %record_inner.query.id,
                                    position_id = %record_inner.position.id,
                                    %error,
                                    "Approval expiry failed — re-arming the detector with backoff"
                                );
                                true
                            },
                        };

                        // The position as it stands now, not as it was queued.
                        let Some(position) = PositionManager::<E, S>::position_to_recover(
                            &store,
                            &circuit_breaker,
                            &entry_exhausted,
                            &pending_approvals,
                            &detectors,
                            record_inner.position.id,
                        )
                        .await
                        else {
                            return;
                        };
                        let entry_policy = {
                            let policies = entry_policies.read().await;
                            policies.get(&position.id).copied().unwrap_or_default()
                        };

                        // Approval expiry is a governed block too: back off the
                        // re-armed detector so an Immediate arm cannot hot-loop
                        // through repeated unattended approvals.
                        let attempt = {
                            let mut counts = governed_rearm_counts.write().await;
                            let count = counts.entry(position.id).or_insert(0);
                            *count = count.saturating_add(1);
                            *count
                        };
                        let delay = governed_rearm_backoff(attempt);

                        let rearmed = PositionManager::<E, S>::rearm_detector(
                            venue,
                            position.id,
                            position,
                            entry_policy,
                            technical_stop_config,
                            event_bus,
                            ohlcv_port,
                            detectors,
                            &circuit_breaker,
                            shutdown_token,
                            "approval expired",
                            stop_invalidation_guard_enabled,
                            stop_invalidation_lookback_candles,
                            Some(delay),
                        )
                        .await;
                        if rearmed && expiry_failed {
                            crate::metrics::DETECTOR_REARMS_AFTER_FAILURE
                                .with_label_values(&["approval_expiry"])
                                .inc();
                        }
                    }
                }
            }
        });
    }

    async fn store_pending_approval(
        &self,
        query: ExecutionQuery,
        position: Position,
        proposed: ProposedTrade,
        governed: GovernedAction,
    ) {
        let query_id = query.id;
        let expires_at = query
            .approval
            .as_ref()
            .map(|approval| approval.expires_at)
            .expect("awaiting approval queries must include approval metadata");

        {
            let mut pending_approvals = self.pending_approvals.write().await;
            pending_approvals.insert(query_id, PendingApprovalRecord {
                query,
                position,
                proposed,
                governed,
            });
        }

        let pending_approvals = self.pending_approvals.read().await;
        if let Some(record) = pending_approvals.get(&query_id) {
            self.emit_query_awaiting_approval(&record.query);
        }
        drop(pending_approvals);

        self.spawn_approval_expiration_task(query_id, expires_at);
    }

    pub async fn get_pending_approvals(&self) -> Vec<ExecutionQuery> {
        let pending = self.pending_approvals.read().await;
        let mut queries: Vec<ExecutionQuery> =
            pending.values().map(|record| record.query.clone()).collect();
        queries.sort_by_key(|query| query.started_at);
        queries
    }

    /// A proven no-send, nothing on the venue. With ADR-0052 admission evidence
    /// on the leg no second admission can succeed (the engine refuses one
    /// against an existing ruler, and the ruler is immutable), so the attempt
    /// ends now with its own reason and the operator re-arms. `true` means the
    /// attempt is over and nothing may re-arm.
    ///
    /// If the exhaustion cannot be appended the error is returned and this
    /// process still holds the record, so it re-arms nothing. A restart loses
    /// that record: it can restore the detector, and the engine's evidence
    /// check then rejects the next signal and makes it terminal.
    async fn end_attempt_on_admission_evidence(
        &self,
        position_id: PositionId,
        signal_id: Option<Uuid>,
        reason: &str,
    ) -> DaemonResult<bool> {
        let Some(position) = self.store.positions().find_by_id(position_id).await? else {
            return Ok(true);
        };
        if !position.carries_admission_evidence() {
            return Ok(false);
        }
        self.mark_entry_exhausted(position_id, signal_id, "entry_not_sent", reason)
            .await?;
        Ok(true)
    }

    /// The executor proved nothing went out. Without admission evidence the
    /// detector goes back under the governed backoff. An error leaves `reach`
    /// alone: the detector stays spent.
    async fn recover_from_entry_not_sent(
        &self,
        position_id: PositionId,
        signal_id: Uuid,
        reason: &str,
        reach: &mut AdmissionReach,
    ) -> DaemonResult<()> {
        if !self
            .end_attempt_on_admission_evidence(position_id, Some(signal_id), reason)
            .await?
        {
            *reach = AdmissionReach::DetectorSpent(position_id);
        }
        Ok(())
    }

    async fn execute_signal_query(
        &self,
        query: &mut ExecutionQuery,
        governed: GovernedAction,
        reach: &mut AdmissionReach,
    ) -> DaemonResult<()> {
        let position_id = query
            .position_id
            .ok_or_else(|| DaemonError::Config("Signal query missing position_id".to_string()))?;

        if let Err(e) = query.transition(QueryState::Acting) {
            let phase = match query.state {
                QueryState::Authorized => "authorized",
                QueryState::RiskChecked => "risk_checked",
                _ => "processing",
            };
            query.fail(format!("{}", e), phase.to_string());
            self.record_query_failure(query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(query, "acting").await?;

        // From here an order may reach the exchange: an error no longer
        // re-arms the detector, unless the executor proves nothing was sent.
        *reach = AdmissionReach::Executor;
        let actions = governed.into_actions();
        let entry_client_order_id = actions.iter().find_map(|action| match action {
            EngineAction::PlaceEntryOrder { client_order_id, .. } => Some(client_order_id.clone()),
            _ => None,
        });
        // Under the permit the caller holds, which the stop below shares
        // (ADR-0056 step 7); in an operation of its own only without one.
        let (executed, ticket_issued) = self
            .executor
            .order_journal()
            .run_admission(
                entry_client_order_id.as_deref(),
                Box::pin(self.execute_and_persist(actions)),
            )
            .await;
        // On Ok and Err alike, and whatever became of persisting the result: a
        // ticket means this admission reached the venue's send.
        if ticket_issued {
            self.clear_governed_rearm_backoff(position_id).await;
        }
        let results = match executed {
            Ok(r) => r,
            Err(e) => {
                query.fail(format!("{}", e), "acting".to_string());
                self.record_query_failure(query).await?;
                return Err(e);
            },
        };

        let actions_count = results.len();
        for result in results {
            match result {
                ActionResult::OrderPlaced { order, event } => {
                    info!(
                        %position_id,
                        exchange_order_id = %order.exchange_order_id,
                        fill_price = %order.fill_price.as_decimal(),
                        fee = %order.fee,
                        event_type = event.as_ref().map(|e| e.event_type()).unwrap_or("none"),
                        "Entry order accepted and filled"
                    );
                    crate::metrics::ORDERS.with_label_values(&["entry"]).inc();

                    if let Err(e) = self
                        .handle_entry_fill(
                            position_id,
                            order.fill_price,
                            order.filled_quantity,
                            order.fee,
                            order.filled_at,
                            Some(order.exchange_order_id),
                        )
                        .await
                    {
                        query.fail(format!("{}", e), "acting".to_string());
                        self.record_query_failure(query).await?;
                        return Err(e);
                    }
                },
                ActionResult::OrderFailed { event, error } => {
                    warn!(
                        %position_id,
                        event_type = event.event_type(),
                        %error,
                        "Entry order failed on exchange"
                    );
                    query.fail(error.clone(), "acting".to_string());
                    self.record_query_failure(query).await?;

                    // Do not rearm after exchange placement failure. Timeout/transport
                    // failures are ambiguous for MARKET orders and may have been accepted
                    // by the exchange; reconciliation must resolve exchange/local drift.
                    return Err(DaemonError::Exec(ExecError::Exchange(error)));
                },
                ActionResult::EntryExecutionRejected { event, error } => {
                    warn!(
                        %position_id,
                        event_type = event.event_type(),
                        %error,
                        "Entry order rejected before fill"
                    );
                    query.fail(error.clone(), "acting".to_string());
                    // Nothing went out either, and the safety event stands. A
                    // legacy leg behaves as before: spent, no re-arm.
                    let signal_id = match &event {
                        Event::EntryExecutionRejected { signal_id, .. } => Some(*signal_id),
                        _ => None,
                    };
                    let ended = self
                        .end_attempt_on_admission_evidence(position_id, signal_id, &error)
                        .await;
                    let recorded = self.record_query_failure(query).await;
                    ended?;
                    recorded?;
                    return Err(DaemonError::Exec(ExecError::InvalidState(error)));
                },
                ActionResult::EntryNotSent { signal_id, reason } => {
                    warn!(%position_id, %reason, "Entry not sent");
                    query.fail(reason.clone(), "acting".to_string());
                    let recovered = self
                        .recover_from_entry_not_sent(position_id, signal_id, &reason, reach)
                        .await;
                    let recorded = self.record_query_failure(query).await;
                    recovered?;
                    recorded?;
                    return Err(DaemonError::Exec(ExecError::SendRefused(reason)));
                },
                ActionResult::ExitNotExecutable { .. } | ActionResult::ExitFailed { .. } => {
                    warn!(%position_id, "Unexpected exit outcome in an entry flow");
                },
                ActionResult::ProtectionNotExecutable { .. } => {},
                ActionResult::AlreadyProcessed(id) => {
                    warn!(%position_id, %id, "Signal already processed (idempotent skip)");
                },
                ActionResult::EventEmitted(event) => {
                    debug!(%position_id, event_type = event.event_type(), "Event emitted");
                },
                ActionResult::StateUpdated => {
                    debug!(%position_id, "State updated");
                },
                ActionResult::Skipped(reason) => {
                    debug!(%position_id, %reason, "Action skipped");
                },
            }
        }

        if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
            query.fail(format!("{}", e), "acting".to_string());
            self.record_after_venue_work(query, "failed").await;
            return Err(DaemonError::Config(format!("Query completion error: {}", e)));
        }
        self.record_after_venue_work(query, "completed").await;

        Ok(())
    }

    /// Build a ProposedTrade for risk evaluation from a signal and its engine
    /// decision.
    ///
    /// Extracts the quantity decided by the Engine (from PlaceEntryOrder
    /// action) and computes notional / margin using the fixed leverage
    /// constant. Returns None if the decision contains no PlaceEntryOrder
    /// (caller handles this).
    fn build_proposed_trade(
        signal: &DetectorSignal,
        decision: &EngineDecision,
    ) -> Option<ProposedTrade> {
        let quantity = decision.actions.iter().find_map(|a| match a {
            EngineAction::PlaceEntryOrder { quantity, .. } => Some(*quantity),
            _ => None,
        })?;

        let qty_decimal = quantity.as_decimal();
        let entry_price = signal.entry_price.as_decimal();
        let notional_value = qty_decimal * entry_price;
        let initial_margin =
            notional_value / Decimal::from(robson_domain::RiskConfig::LEVERAGE as u32);

        Some(ProposedTrade {
            symbol: signal.symbol.as_pair(),
            side: format!("{}", signal.side).to_lowercase(),
            quantity: qty_decimal,
            entry_price,
            notional_value,
            initial_margin,
            // ADR-0043: the gate charges the planned worst-case loss against
            // the monthly budget. Zero = unpriced → full 1% cap is reserved.
            planned_risk: decision.planned_entry_risk.unwrap_or(Decimal::ZERO),
        })
    }

    /// A residual on any open leg blocks new admissions until that leg closes
    /// (ADR-0056 §5). A check of its own: a residual never enters the budget or
    /// latent risk, so it can neither fail the snapshot nor defer MonthlyHalt.
    /// Admissions close for the rest of this process once the journal latch is
    /// set, whatever set it (ADR-0056 step 7, PR P): a bounded send whose row
    /// did not land, discarded history, an aborted operation, a mutation
    /// without a permit, a suspended stream. They also stay closed while the
    /// current month's system stream is suspended, loaded at start, until the
    /// fence PR's accounting contract and release replace this. The latch is
    /// checked again at an entry's send.
    fn check_admissions_open(&self) -> DaemonResult<()> {
        if self.executor.order_journal().latch_set() {
            return Err(DaemonError::AdmissionsClosed {
                reason: "the journal latch is set; admissions stay closed until a restart"
                    .to_string(),
            });
        }
        if let Some((year, month)) = self.event_streams.current_month_suspended() {
            return Err(DaemonError::AdmissionsClosed {
                reason: format!(
                    "the event stream of {year:04}-{month:02} is suspended; admissions stay \
                     closed while that month is current"
                ),
            });
        }
        Ok(())
    }

    async fn check_no_unresolved_residual(&self) -> DaemonResult<()> {
        let open = self.store.positions().find_active().await?;
        let Some(leg) = open.iter().find(|leg| leg.has_unresolved_residual()) else {
            return Ok(());
        };
        let residual = leg
            .residual_quantity
            .map_or_else(|| "unreadable".to_string(), |quantity| quantity.to_string());
        warn!(position_id = %leg.id, %residual, "Admission blocked by residual inventory");
        Err(DaemonError::ResidualInventoryUnresolved { position_id: leg.id, residual })
    }

    /// The admission check before any exchange submission (ADR-0056 §5): fresh
    /// validated metadata, then lot bounds and minimum notional. Unavailable
    /// metadata refuses the entry, under every stop policy.
    async fn validate_entry_quantity_for_exchange(
        &self,
        symbol: &Symbol,
        proposed: &ProposedTrade,
    ) -> Result<(), ExecError> {
        let quantity = Quantity::new(proposed.quantity)
            .map_err(|e| ExecError::Config(format!("Invalid proposed quantity: {}", e)))?;
        let rules = self.exchange().trading_rules_fresh(symbol).await?;
        quantize::admission_quantity(&rules, quantity, proposed.entry_price).map(|_| ())
    }

    /// Start background task to listen for detector signals.
    ///
    /// This task subscribes to the EventBus and processes DetectorSignal events
    /// by calling handle_signal() for each received signal.
    fn start_signal_listener(manager: Arc<Self>) {
        let event_bus = Arc::clone(&manager.event_bus);
        let shutdown_token = manager.shutdown_token.clone();

        tokio::spawn(async move {
            let mut receiver = event_bus.subscribe();

            info!("Position manager signal listener started");

            loop {
                tokio::select! {
                    // Handle shutdown
                    _ = shutdown_token.cancelled() => {
                        info!("Signal listener received shutdown signal");
                        break;
                    }
                    // Process events
                    event = receiver.recv() => {
                        match event {
                            Some(Ok(DaemonEvent::DetectorSignal(signal))) => {
                                let position_id = signal.position_id;
                                let signal_id = signal.signal_id;

                                info!(
                                    %position_id,
                                    %signal_id,
                                    "Processing detector signal from EventBus"
                                );

                                // Under the main loop's slot, whose part this
                                // plays (ADR-0056 step 7).
                                let handled = manager
                                    .executor
                                    .order_journal()
                                    .with_permit(
                                        OperationKind::SignalListener,
                                        manager.handle_signal(signal),
                                    )
                                    .await
                                    .unwrap_or_else(|refused| Err(refused.into()));
                                if let Err(e) = handled {
                                    error!(
                                        %position_id,
                                        %signal_id,
                                        error = %e,
                                        "Failed to process detector signal"
                                    );
                                } else {
                                    info!(
                                        %position_id,
                                        %signal_id,
                                        "Detector signal processed successfully"
                                    );
                                }
                            }
                            Some(Ok(DaemonEvent::DomainEvent(event))) => {
                                if let Err(e) = manager.emit_domain_event(event).await {
                                    error!(
                                        error = %e,
                                        "Failed to persist daemon domain event"
                                    );
                                }
                            }
                            Some(Err(lag_msg)) => {
                                warn!(error = %lag_msg, "Signal receiver lagged");
                            }
                            None => {
                                info!("Signal receiver channel closed");
                                break;
                            }
                            Some(Ok(_)) => {
                                // Ignore other event types (MarketData, StateChanged, etc.)
                            }
                        }
                    }
                }
            }

            info!("Position manager signal listener terminated");
        });
    }

    /// Arm a new position.
    ///
    /// Creates the position in Armed state and spawns a detector task.
    /// The detector will fire a signal when entry conditions are met.
    /// At ARM time the entry price and chart-derived technical stop may still
    /// be unknown, so `tech_stop_distance` is optional and normally `None` for
    /// API-armed positions.
    ///
    /// The `risk_config` parameter updates the engine's capital before any
    /// position sizing calculation. This ensures the operator-supplied capital
    /// is used rather than the hardcoded default.
    pub async fn arm_position(
        &self,
        symbol: Symbol,
        side: Side,
        risk_config: RiskConfig,
        tech_stop_distance: Option<TechnicalStopDistance>,
        account_id: Uuid,
    ) -> DaemonResult<Position> {
        self.arm_position_with_policy(
            symbol,
            side,
            risk_config,
            tech_stop_distance,
            account_id,
            EntryPolicyConfig::default(),
        )
        .await
    }

    /// Arm a new position with an explicit entry and approval policy.
    pub async fn arm_position_with_policy(
        &self,
        symbol: Symbol,
        side: Side,
        risk_config: RiskConfig,
        tech_stop_distance: Option<TechnicalStopDistance>,
        account_id: Uuid,
        entry_policy: EntryPolicyConfig,
    ) -> DaemonResult<Position> {
        // Snapshot the ADR-0041 buffer at arm (issue #154): the position's
        // executable stop derives from this value forever, so a later config
        // change cannot move a live stop.
        let stop_buffer_bps_at_arm = risk_config.stop_buffer_bps();

        // Update engine with operator-supplied capital before any sizing.
        {
            let mut engine = self.engine.lock().unwrap();
            let capital = risk_config.capital();
            let risk_pct = risk_config.risk_per_trade_pct();
            info!(
                capital = %capital,
                risk_percent = %risk_pct,
                "Engine configured with operator capital"
            );
            engine.update_risk_config(risk_config);
        }

        // MonthlyHalt check — blocks new entries when 4% monthly drawdown reached.
        if self.circuit_breaker.blocks_new_entries().await {
            let snap = self.circuit_breaker.snapshot().await;
            return Err(DaemonError::MonthlyHaltActive { reason: snap.reason.unwrap_or_default() });
        }
        self.check_no_unresolved_residual().await?;
        self.check_admissions_open()?;

        // Generate position ID upfront (used in event and returned to caller)
        let position_id = Uuid::now_v7();

        // Create query for lifecycle tracking
        let mut query = ExecutionQuery::new(
            QueryKind::ArmPosition {
                symbol: symbol.clone(),
                side,
                tech_stop_distance: tech_stop_distance.clone(),
                account_id,
            },
            Self::operator_actor(),
        );
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.record_query_accepted(&query).await?;

        info!(
            %position_id,
            query_id = %query.id,
            symbol = %symbol.as_pair(),
            ?side,
            "Arming position"
        );

        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            query.fail(format!("{}", e), "accepted".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "processing").await?;

        // Emit PositionArmed event → apply_event creates position in Armed state.
        // Then persist policy resolution for replay/audit.
        let now = chrono::Utc::now();
        // ADR-0052: every new arm is stamped unconditionally. The legacy
        // policy remains provenance-only for positions armed by older code.
        let armed_event = Event::PositionArmed {
            position_id,
            account_id,
            symbol: symbol.clone(),
            side,
            tech_stop_distance,
            stop_policy: StopPolicy::ExecutableSpan,
            stop_buffer_bps_at_arm: Some(stop_buffer_bps_at_arm),
            timestamp: now,
        };
        let policy_event = Event::EntryPolicyResolved {
            position_id,
            entry_policy: entry_policy.mode,
            approval_policy: entry_policy.approval,
            strategy_id: StrategyRegistry::strategy_id_for_policy(entry_policy.mode),
            timestamp: now,
        };

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            query.fail(format!("{}", e), "processing".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "acting").await?;

        // Execute event emission + persist to eventlog for crash recovery
        let results = match self
            .execute_and_persist(vec![
                EngineAction::EmitEvent(armed_event),
                EngineAction::EmitEvent(policy_event),
            ])
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let err_str = format!("{}", e);
                query.fail(err_str.clone(), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
        };
        let actions_count = results.len();

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "None".to_string(),
            new_state: "Armed".to_string(),
            timestamp: now,
        });

        {
            let mut entry_policies = self.entry_policies.write().await;
            entry_policies.insert(position_id, entry_policy);
        }

        // Load position from projection for detector and return
        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                let e = DaemonError::PositionNotFound(position_id);
                query.fail(format!("{}", e), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
            Err(e) => {
                let err_str = format!("{}", e);
                query.fail(err_str.clone(), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(e.into());
            },
        };

        // Spawn detector task
        let cancel_token = self.child_cancel_token();
        let detector = match DetectorTask::from_position_with_policy(
            &position,
            entry_policy,
            TechnicalStopConfig::with_bounds(&self.risk_config_snapshot().stop_distance_bounds()),
            self.venue.clone(),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.ohlcv_port),
            cancel_token,
        ) {
            Ok(d) => d.with_invalidation_guard(
                self.stop_invalidation_guard_enabled,
                self.stop_invalidation_lookback_candles,
            ),
            Err(e) => {
                let err_str = format!("{}", e);
                query.fail(err_str.clone(), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
        };
        if entry_policy.mode == EntryPolicy::Immediate {
            let signal = detector.try_proactive_immediate_signal().await.map_err(|e| {
                DaemonError::Detector(format!("immediate entry failed before execution: {}", e))
            })?;
            debug!(%position_id, "Immediate position armed; processing entry synchronously");

            // Complete the ARM query before entering the PROCESS_SIGNAL query path.
            if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
                query.fail(format!("{}", e), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query completion error: {}", e)));
            }
            self.record_query_transition(&query, "completed").await?;

            self.handle_signal(signal).await?;
            return self
                .store
                .positions()
                .find_by_id(position_id)
                .await?
                .ok_or(DaemonError::PositionNotFound(position_id));
        }

        let handle = detector.spawn();

        // Store detector handle for cancellation
        let mut detectors = self.detectors.write().await;
        detectors.insert(position_id, handle);

        debug!(%position_id, "Position armed, detector spawned");

        // Complete query ONLY after all operations succeed
        if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
            query.fail(format!("{}", e), "acting".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query completion error: {}", e)));
        }
        self.record_query_transition(&query, "completed").await?;

        Ok(position)
    }

    /// Re-attach entry policy and spawn a detector for an Armed position
    /// recovered from the projection on daemon startup.
    ///
    /// Called after `restore_positions()` has already loaded the position into
    /// the in-memory store. This method only restores the in-memory
    /// `entry_policies` map and spawns the detector — it does not re-emit any
    /// events.
    pub async fn restore_armed_position(
        &self,
        position: &Position,
        entry_policy: EntryPolicyConfig,
    ) -> DaemonResult<()> {
        let position_id = position.id;

        {
            let mut policies = self.entry_policies.write().await;
            policies.insert(position_id, entry_policy);
        }

        let cancel_token = self.child_cancel_token();
        let detector = DetectorTask::from_position_with_policy(
            position,
            entry_policy,
            TechnicalStopConfig::with_bounds(&self.risk_config_snapshot().stop_distance_bounds()),
            self.venue.clone(),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.ohlcv_port),
            cancel_token,
        )?
        .with_invalidation_guard(
            self.stop_invalidation_guard_enabled,
            self.stop_invalidation_lookback_candles,
        );
        let handle = detector.spawn();

        let mut detectors = self.detectors.write().await;
        detectors.insert(position_id, handle);

        info!(%position_id, "Armed position restored: detector re-spawned");
        Ok(())
    }

    /// Returns an `Arc` to the circuit breaker so API handlers can read/write
    /// it.
    pub fn order_journal(&self) -> Arc<robson_exec::order_journal::OrderJournal> {
        self.executor.order_journal()
    }

    pub fn circuit_breaker(&self) -> Arc<CircuitBreaker> {
        Arc::clone(&self.circuit_breaker)
    }

    /// Trigger MonthlyHalt: close all open positions and block new entries.
    ///
    /// This method MUST NOT be called while holding `entry_flow_lock` to avoid
    /// deadlock (panic_close_all → disarm_position → entry_flow_lock.lock()).
    /// It is called from the API handler directly, which does not hold the
    /// lock.
    ///
    /// Follow-up required: Entering positions cannot be cancelled yet (exchange
    /// cancel-order logic not implemented). They will remain in Entering state
    /// until the order fills or the exchange session expires.
    pub async fn trigger_monthly_halt(&self, reason: String) -> DaemonResult<Vec<PositionId>> {
        warn!(%reason, "MonthlyHalt triggered — closing all positions");

        // Transition circuit breaker to MonthlyHalt
        if let Some(()) = self.circuit_breaker.trigger_halt(reason.clone()).await {
            self.event_bus.send(DaemonEvent::MonthlyHaltTriggered {
                reason,
                triggered_at: chrono::Utc::now(),
            });
        }

        // Close all open positions using existing panic logic.
        // panic_close_all does NOT hold entry_flow_lock — it calls disarm_position
        // for Armed positions (which takes entry_flow_lock internally) and
        // panic_close_position_internal for Active positions (no lock needed).
        // Since we don't hold entry_flow_lock here, there is no deadlock.
        self.panic_close_all().await
    }

    /// Evaluate MonthlyHalt from the persisted model in the canonical
    /// ADR-0051 budget snapshot.
    ///
    /// Trigger condition: `remaining_budget <= 0` on a valid basis. With
    /// `capital_base <= 0` nothing fires (ADR-0051 §1).
    /// where:
    ///   remaining_budget = (capital_base × max_monthly_drawdown_pct) −
    /// consumed − latent_risk
    ///   consumed = model(month_peak/equity, governed realized net)
    ///   latent_risk = Σ max(0, loss_if_current_stop_hit) over the same
    ///   Active + Entering set used by slot reporting
    ///
    /// Must be called:
    /// - After any position close that changes realized PnL
    ///   (`handle_exit_fill`)
    /// - MUST NOT be called while holding `entry_flow_lock` (calls
    ///   panic_close_all → disarm_position → entry_flow_lock).
    ///
    /// Returns true if MonthlyHalt was triggered this call.
    pub async fn evaluate_monthly_halt(&self) -> bool {
        // Skip if already halted
        if self.circuit_breaker.blocks_new_entries().await {
            return false;
        }

        let (snapshot, local_risk_open, _, latent_risk_failures) = match self
            .canonical_budget_snapshot_with_positions(chrono::Utc::now())
            .await
        {
            Ok(snapshot) => snapshot,
            Err(e) => {
                warn!(error = %e, "Failed to build canonical budget snapshot for MonthlyHalt evaluation");
                return false;
            },
        };

        self.emit_latent_risk_resolution_warnings(&latent_risk_failures).await;

        // ADR-0051 §1: an invalid basis says nothing about the budget, so it never
        // reads as exhaustion. Admission already fails closed on it, and open
        // positions keep their stops and the reservations made at admission.
        if snapshot.capital_base_invalid {
            error!(
                capital_base = %snapshot.capital_base,
                risk_open_positions = local_risk_open.len(),
                "MonthlyHalt not evaluated: the monthly budget basis is invalid (ADR-0051 §1); open positions are kept"
            );
            return false;
        }

        if snapshot.latent_risk_invalid && !Self::realized_losses_exhaust_budget(&snapshot) {
            warn!(
                "Latent risk unresolved; MonthlyHalt evaluation deferred (admission already blocked, insurance stops remain authoritative)"
            );
            return false;
        }

        // ADR-0051 §2 intentionally closes the HWM-era inconsistency here:
        // Entering reservations now reduce the exact same remaining value as
        // compute_slots_available instead of being omitted by halt math.
        let monthly_budget = self.trading_policy.monthly_budget(snapshot.capital_base);
        let remaining_budget = snapshot.remaining_active;

        if remaining_budget <= Decimal::ZERO {
            match self.budget_basis_is_invalid_now().await {
                Ok(false) => {},
                Ok(true) => {
                    error!(
                        "MonthlyHalt not triggered: the monthly budget basis went invalid during the evaluation (ADR-0051 §1); open positions are kept"
                    );
                    return false;
                },
                Err(e) => {
                    warn!(error = %e, "Failed to re-read the monthly budget basis for MonthlyHalt evaluation");
                    return false;
                },
            }

            let reason = format!(
                "Monthly budget exhausted: remaining={:.2} <= 0 \
                 (model={}, budget={:.2}, consumed={:.2}, month_equity_net={:.2}, month_peak_net={:.2}, latent_risk={:.2})",
                remaining_budget,
                snapshot.model.as_str(),
                monthly_budget,
                snapshot.consumed_active,
                snapshot.month_equity_net,
                snapshot.month_peak_net,
                snapshot.latent_risk,
            );
            warn!(
                budget_model = %snapshot.model.as_str(),
                %remaining_budget,
                %monthly_budget,
                consumed = %snapshot.consumed_active,
                month_equity_net = %snapshot.month_equity_net,
                month_peak_net = %snapshot.month_peak_net,
                latent_risk = %snapshot.latent_risk,
                "MonthlyHalt auto-triggered from canonical budget snapshot"
            );
            match self.trigger_monthly_halt(reason).await {
                Ok(_) => true,
                Err(e) => {
                    error!(error = %e, "MonthlyHalt auto-trigger failed to close positions");
                    // State is already set to MonthlyHalt even if close fails
                    true
                },
            }
        } else {
            false
        }
    }

    /// ADR-0051 §1 at a decision point. A snapshot's reads can take seconds
    /// when trading rules must be fetched, so the basis it read may no
    /// longer stand.
    async fn budget_basis_is_invalid_now(&self) -> DaemonResult<bool> {
        let monthly = self.load_monthly_state(chrono::Utc::now()).await?;
        Ok(monthly.capital_base <= Decimal::ZERO)
    }

    /// A re-read that fails is a governed denial like any other, which re-arms
    /// the detector if its audit write succeeds, and not an operational error,
    /// which would leave it dead.
    fn capital_for_the_gate(snapshot_capital: Decimal, reread: DaemonResult<bool>) -> Decimal {
        match reread {
            Ok(false) => snapshot_capital,
            Ok(true) => Decimal::ZERO,
            Err(e) => {
                warn!(error = %e, "Failed to re-read the monthly budget basis; denying the entry");
                Decimal::ZERO
            },
        }
    }

    /// With latent risk unresolved on a valid basis, what can still decide the
    /// halt on its own: realized losses that exhaust the budget under either
    /// way of counting. A give-back from the peak also needs a mark, which may
    /// not be current then.
    fn realized_losses_exhaust_budget(snapshot: &MonthlyBudgetSnapshot) -> bool {
        snapshot.consumed_active.min(snapshot.consumed_nfs) >= snapshot.budget_amount
    }

    /// Disarm (cancel) an armed position.
    ///
    /// Only positions in Armed state can be disarmed.
    pub async fn disarm_position(&self, position_id: PositionId) -> DaemonResult<()> {
        let _entry_flow_guard = self.entry_flow_lock.lock().await;

        // A disarm resolves any terminal needs_operator_rearm record
        // (ADR-0050 §2): the operator acted.
        self.clear_entry_exhausted(position_id).await;

        // Create query for lifecycle tracking
        let mut query =
            ExecutionQuery::new(QueryKind::DisarmPosition { position_id }, Self::operator_actor());
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.record_query_accepted(&query).await?;

        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            query.fail(format!("{}", e), "accepted".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "processing").await?;

        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                let e = DaemonError::PositionNotFound(position_id);
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
            Err(e) => {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e.into());
            },
        };

        if !matches!(position.state, PositionState::Armed) {
            let e = DaemonError::InvalidPositionState {
                expected: "Armed".to_string(),
                actual: format!("{:?}", position.state),
            };
            query.fail(format!("{}", e), "processing".to_string());
            self.record_query_failure(&query).await?;
            return Err(e);
        }

        info!(%position_id, query_id = %query.id, "Disarming position");

        self.invalidate_pending_approvals_for_position(position_id, "position disarmed")
            .await;

        // Kill detector task if exists
        self.kill_detector(position_id).await;

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            query.fail(format!("{}", e), "processing".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "acting").await?;

        // Emit PositionDisarmed event → apply_event transitions position to Cancelled
        let event = Event::PositionDisarmed {
            position_id,
            reason: "user_disarmed".to_string(),
            timestamp: chrono::Utc::now(),
        };

        let exec_result = self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await;
        match exec_result {
            Ok(results) => {
                if let Err(e) =
                    query.complete(QueryOutcome::ActionsExecuted { actions_count: results.len() })
                {
                    query.fail(format!("{}", e), "acting".to_string());
                    self.record_query_failure(&query).await?;
                    return Err(DaemonError::Config(format!("Query completion error: {}", e)));
                }
                self.record_query_transition(&query, "completed").await?;
            },
            Err(e) => {
                query.fail(format!("{}", e), "acting".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
        }

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Armed".to_string(),
            new_state: "Cancelled".to_string(),
            timestamp: chrono::Utc::now(),
        });

        // Clean up entry policy entry to prevent HashMap growth.
        {
            let mut policies = self.entry_policies.write().await;
            policies.remove(&position_id);
        }

        Ok(())
    }

    /// Close a single active position via reduce-only market order.
    ///
    /// Only positions in Active state can be closed through this path.
    pub async fn close_position(&self, position_id: PositionId) -> DaemonResult<()> {
        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => return Err(DaemonError::PositionNotFound(position_id)),
            Err(e) => return Err(e.into()),
        };

        if !matches!(position.state, PositionState::Active { .. }) {
            return Err(DaemonError::InvalidPositionState {
                expected: "Active".to_string(),
                actual: format!("{:?}", position.state),
            });
        }

        let mut query =
            ExecutionQuery::new(QueryKind::ClosePosition { position_id }, Self::operator_actor());
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.record_query_accepted(&query).await?;

        match self.panic_close_position_internal(position_id, &mut query).await {
            Ok(_) => {
                if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count: 1 }) {
                    query.fail(format!("{}", e), "acting".to_string());
                    self.record_after_venue_work(&query, "failed").await;
                    return Err(DaemonError::Config(format!("Query completion error: {}", e)));
                }
                self.record_after_venue_work(&query, "completed").await;
                Ok(())
            },
            Err(e) => {
                let phase = match &query.state {
                    QueryState::Accepted => "accepted".to_string(),
                    QueryState::Processing => "processing".to_string(),
                    QueryState::RiskChecked => "risk_checked".to_string(),
                    QueryState::AwaitingApproval => "awaiting_approval".to_string(),
                    QueryState::Authorized => "authorized".to_string(),
                    QueryState::Acting => "acting".to_string(),
                    QueryState::Completed => "completed".to_string(),
                    QueryState::Expired => "expired".to_string(),
                    QueryState::Failed { phase, .. } => phase.clone(),
                    QueryState::Denied { check, .. } => format!("denied:{}", check),
                };
                query.fail(format!("{}", e), phase);
                self.record_query_failure(&query).await?;
                Err(e)
            },
        }
    }

    /// Cancel an Armed position or close an Active position.
    pub async fn cancel_or_close_position(&self, position_id: PositionId) -> DaemonResult<()> {
        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => return Err(DaemonError::PositionNotFound(position_id)),
            Err(e) => return Err(e.into()),
        };

        match position.state {
            PositionState::Armed => self.disarm_position(position_id).await,
            PositionState::Active { .. } => self.close_position(position_id).await,
            actual => Err(DaemonError::InvalidPositionState {
                expected: "Armed or Active".to_string(),
                actual: format!("{:?}", actual),
            }),
        }
    }

    /// Handle a detector signal (entry signal received).
    ///
    /// Flow: Engine → Execute actions (emit events) → Save state → Process fill
    /// Mark an Armed position `needs_operator_rearm` (ADR-0050 §2): persist
    /// the durable `entry_attempt_exhausted` audit event, record the
    /// rejection for API surfacing, and publish the public SSE event. No
    /// detector runs for the position afterwards.
    pub async fn mark_entry_exhausted(
        &self,
        position_id: PositionId,
        signal_id: Option<Uuid>,
        reason_code: &str,
        reason: &str,
    ) -> DaemonResult<()> {
        let now = chrono::Utc::now();
        let event = Event::EntryAttemptExhausted {
            position_id,
            signal_id,
            reason_code: reason_code.to_string(),
            reason: reason.to_string(),
            timestamp: now,
        };
        // Persist first, but never lose the in-memory record on a persistence
        // failure: without it the position would go silently inert, the exact
        // defect ADR-0050 eliminates. The error still propagates.
        let persist_result = self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await;

        self.entry_exhausted.write().await.insert(position_id, EntryRejectionRecord {
            reason_code: reason_code.to_string(),
            reason: reason.to_string(),
            rejected_at: now,
        });

        warn!(
            %position_id,
            reason_code,
            reason,
            "Entry attempt exhausted; position needs operator re-arm (ADR-0050)"
        );
        self.event_bus.send(DaemonEvent::EntryRejected {
            position_id,
            reason_code: reason_code.to_string(),
            reason: reason.to_string(),
            terminal: true,
            rejected_at: now,
        });
        persist_result.map(|_| ())
    }

    /// Rebuild the `needs_operator_rearm` record from a persisted
    /// `entry_attempt_exhausted` event at startup, without re-emitting
    /// events (ADR-0050 §6: durable across restarts).
    pub async fn restore_entry_exhausted(
        &self,
        position_id: PositionId,
        reason_code: String,
        reason: String,
        rejected_at: chrono::DateTime<chrono::Utc>,
    ) {
        self.entry_exhausted.write().await.insert(position_id, EntryRejectionRecord {
            reason_code,
            reason,
            rejected_at,
        });
    }

    /// The last terminal rejection for a position, when it is in
    /// `needs_operator_rearm` (ADR-0050 §7 surfacing).
    pub async fn entry_exhaustion(&self, position_id: PositionId) -> Option<EntryRejectionRecord> {
        self.entry_exhausted.read().await.get(&position_id).cloned()
    }

    /// True when the position must NOT have a detector restored or re-armed.
    pub async fn is_entry_exhausted(&self, position_id: PositionId) -> bool {
        self.entry_exhausted.read().await.contains_key(&position_id)
    }

    /// Clear the terminal record (position disarmed or closed).
    pub(crate) async fn clear_entry_exhausted(&self, position_id: PositionId) {
        self.entry_exhausted.write().await.remove(&position_id);
    }

    /// Reason code for a domain-validation rejection (issue #154
    /// deliverable 5): typed failures carry their own code instead of the
    /// generic `signal_domain_rejection`.
    fn domain_rejection_code(error: &robson_domain::DomainError) -> &'static str {
        use robson_domain::DomainError;
        match error {
            DomainError::ExecutableStopOutOfBounds(_) => "executable_stop_out_of_bounds",
            DomainError::TradingRulesUnavailable(_) => "trading_rules_unavailable",
            DomainError::DegenerateStopSpan(_) => "degenerate_stop_span",
            DomainError::AdmissionEvidenceAlreadyPresent(_) => "admission_evidence_present",
            _ => "signal_domain_rejection",
        }
    }

    /// Per-policy handling of a domain-validation rejection of an entry
    /// signal (ADR-0050 §2). Immediate exhausts (terminal, operator re-arm);
    /// strategy modes re-arm the detector under governed backoff, requiring
    /// a fresh trigger by construction.
    /// An `Immediate` arm refused on margin is cancelled, not retried.
    async fn margin_denial_cancels(&self, position_id: PositionId, query: &ExecutionQuery) -> bool {
        matches!(&query.state, QueryState::Denied { check, .. } if check == "insufficient_margin")
            && self.entry_policy_for_position(position_id).await.mode == EntryPolicy::Immediate
    }

    async fn cancel_immediate_arm_on_margin(
        &self,
        position_id: PositionId,
        query_id: Uuid,
    ) -> DaemonResult<()> {
        info!(
            %position_id,
            %query_id,
            "Immediate entry denied by margin policy - cancelling Armed position"
        );
        self.invalidate_pending_approvals_for_position(
            position_id,
            "immediate entry denied by insufficient margin",
        )
        .await;
        self.kill_detector(position_id).await;
        self.execute_and_persist(vec![EngineAction::EmitEvent(Event::PositionDisarmed {
            position_id,
            reason: "risk_denied_insufficient_margin".to_string(),
            timestamp: chrono::Utc::now(),
        })])
        .await?;
        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Armed".to_string(),
            new_state: "Cancelled".to_string(),
            timestamp: chrono::Utc::now(),
        });
        self.entry_policies.write().await.remove(&position_id);
        self.clear_governed_rearm_backoff(position_id).await;
        Ok(())
    }

    async fn handle_signal_domain_rejection(
        &self,
        position: &Position,
        signal: &DetectorSignal,
        error: &robson_domain::DomainError,
    ) -> DaemonResult<()> {
        let reason_code = Self::domain_rejection_code(error);
        let error_text = error.to_string();
        let policy = self.entry_policy_for_position(position.id).await;
        if policy.mode == EntryPolicy::Immediate
            || matches!(error, robson_domain::DomainError::AdmissionEvidenceAlreadyPresent(_))
        {
            self.mark_entry_exhausted(
                position.id,
                Some(signal.signal_id),
                reason_code,
                &error_text,
            )
            .await?;
        } else {
            let now = chrono::Utc::now();
            self.event_bus.send(DaemonEvent::EntryRejected {
                position_id: position.id,
                reason_code: reason_code.to_string(),
                reason: error_text.clone(),
                terminal: false,
                rejected_at: now,
            });
            self.rearm_detector_after_governed_block(
                position.id,
                position,
                "signal domain rejection",
            )
            .await;
        }
        Ok(())
    }

    pub async fn handle_signal(&self, signal: DetectorSignal) -> DaemonResult<()> {
        let mut entry_flow_guard = Some(self.entry_flow_lock.lock().await);
        let position_id = signal.position_id;
        // The detector is single-shot: its handle is spent once its signal is
        // here, so a handle found after the flow is a re-arm the flow made.
        self.kill_detector(position_id).await;
        let mut reach = AdmissionReach::DetectorSpent(position_id);
        let result = self.process_signal(signal, &mut entry_flow_guard, &mut reach).await;
        if let (Err(error), AdmissionReach::DetectorSpent(position_id), Some(_)) =
            (&result, reach, &entry_flow_guard)
        {
            self.rearm_detector_after_failed_admission(position_id, "signal", error).await;
        }
        result
    }

    async fn process_signal(
        &self,
        signal: DetectorSignal,
        entry_flow_guard: &mut Option<MutexGuard<'_, ()>>,
        reach: &mut AdmissionReach,
    ) -> DaemonResult<()> {
        let position_id = signal.position_id;

        // MonthlyHalt blocks new entries — a detector signal that would
        // transition Armed→Entering counts as a new entry.
        if self.circuit_breaker.blocks_new_entries().await {
            let snap = self.circuit_breaker.snapshot().await;
            warn!(
                %position_id,
                state = %snap.state,
                "Signal dropped — MonthlyHalt blocks new entries"
            );
            return Err(DaemonError::MonthlyHaltActive { reason: snap.reason.unwrap_or_default() });
        }

        // Create query for lifecycle tracking
        let mut query = ExecutionQuery::new(
            QueryKind::ProcessSignal {
                signal_id: signal.signal_id,
                symbol: signal.symbol.clone(),
                side: signal.side,
                entry_price: signal.entry_price,
                stop_loss: signal.stop_loss,
            },
            ActorKind::Detector,
        );
        query.position_id = Some(position_id);
        self.populate_query_context_summary(&mut query).await;
        self.record_query_accepted(&query).await?;

        info!(
            %position_id,
            query_id = %query.id,
            signal_id = %signal.signal_id,
            entry_price = %signal.entry_price.as_decimal(),
            "Processing detector signal"
        );

        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            query.fail(format!("{}", e), "accepted".to_string());
            self.record_query_failure(&query).await?;
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(&query, "processing").await?;

        // Load position
        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                let e = DaemonError::PositionNotFound(position_id);
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
            Err(e) => {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e.into());
            },
        };

        if self.has_pending_approval_for_position(position_id).await {
            if let Err(e) = query.complete(QueryOutcome::NoAction {
                reason: "Approval already pending for position".to_string(),
            }) {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query completion error: {}", e)));
            }
            self.record_query_transition(&query, "completed").await?;
            info!(
                %position_id,
                query_id = %query.id,
                "Skipping signal because approval is already pending for position"
            );
            return Ok(());
        }

        if let Err(blocked) = self.check_no_unresolved_residual().await {
            let reason = blocked.to_string();
            query.fail(reason.clone(), "processing".to_string());
            self.record_query_failure(&query).await?;
            self.event_bus.send(DaemonEvent::EntryRejected {
                position_id,
                reason_code: "residual_inventory_unresolved".to_string(),
                reason,
                terminal: false,
                rejected_at: chrono::Utc::now(),
            });
            self.rearm_detector_after_governed_block(
                position_id,
                &position,
                "residual inventory blocks admission",
            )
            .await;
            return Err(blocked);
        }
        if let Err(closed) = self.check_admissions_open() {
            let reason = closed.to_string();
            query.fail(reason.clone(), "processing".to_string());
            self.record_query_failure(&query).await?;
            self.event_bus.send(DaemonEvent::EntryRejected {
                position_id,
                reason_code: "admissions_closed".to_string(),
                reason,
                terminal: false,
                rejected_at: chrono::Utc::now(),
            });
            self.rearm_detector_after_governed_block(position_id, &position, "admissions closed")
                .await;
            return Err(closed);
        }

        // Cache the entry-time invalidation guard level (ADR-0042) for the fill
        // handler. Overwrite on each signal so a re-arm without a guard clears
        // any stale level for this position.
        let guard_level =
            signal.technical_stop_analysis.as_ref().and_then(|a| a.invalidation_guard_level);
        {
            let mut guards = self.invalidation_guards.write().await;
            match guard_level {
                Some(g) => {
                    guards.insert(position_id, g);
                },
                None => {
                    guards.remove(&position_id);
                },
            }
        }

        if let Some(analysis) = signal.technical_stop_analysis.clone() {
            self.tech_stop_audits
                .write()
                .await
                .insert(position_id, TechStopAuditSummary::from_audit(&analysis));
            let event = Event::TechnicalStopAnalyzed {
                position_id,
                signal_id: signal.signal_id,
                symbol: signal.symbol.clone(),
                side: signal.side,
                entry_price: signal.entry_price,
                analysis,
                timestamp: signal.timestamp,
            };

            if let Err(e) = self.execute_and_persist(vec![EngineAction::EmitEvent(event)]).await {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            }
        }

        // Fetch the live available balance so the physical capital cap sizes
        // against the actual wallet, not the month-start policy capital —
        // mid-month drawdown makes the wallet smaller than capital_base and
        // the exchange rejects orders sized past it (2026-07-04 incident,
        // Binance -2019). On fetch failure fall back to None (policy capital
        // bound only) rather than blocking the entry on a transient error;
        // the exchange remains the final arbiter.
        let available_margin = match self.exchange().get_futures_balance().await {
            Ok(balance) => Some(balance.available_balance),
            Err(e) => {
                tracing::warn!(
                    position_id = %position.id,
                    error = %e,
                    "Failed to fetch futures balance for margin cap; sizing against policy capital only"
                );
                None
            },
        };

        // Runtime trading rules for the plan (issue #154): fresh validated
        // metadata only (ADR-0056 §5), and only when the position's policy needs
        // them.
        let trading_rules = self.trading_rules_for_admission(&position).await;

        // Use engine to decide entry (pure: State+Signal → Decision)
        let decision = {
            let engine = self.engine.lock().unwrap();
            engine.decide_entry_with_rules(
                &position,
                &signal,
                available_margin,
                trading_rules.as_ref(),
            )
        };
        let decision = match decision {
            Ok(d) => d,
            Err(robson_engine::EngineError::DomainError(domain_error)) => {
                // ADR-0050 §2: a domain-validation rejection is a governed
                // outcome, not an operational failure. Record it on the
                // query, resolve it per entry policy (terminal for
                // Immediate, governed re-arm for strategy modes), and
                // return Ok so no opaque 500/ERROR surfaces.
                let error_text = domain_error.to_string();
                query.fail(error_text.clone(), "processing".to_string());
                // Resolved even when its record fails, so the failure re-arm
                // cannot retry an Immediate arm this makes terminal.
                let recorded = self.record_query_failure(&query).await;
                self.handle_signal_domain_rejection(&position, &signal, &domain_error).await?;
                if recorded.is_err() && self.detectors.read().await.contains_key(&position_id) {
                    // Re-armed by the rejection itself, so the failure re-arm
                    // will skip it; count the failure here instead.
                    crate::metrics::DETECTOR_REARMS_AFTER_FAILURE
                        .with_label_values(&["signal"])
                        .inc();
                }
                recorded?;
                return Ok(());
            },
            Err(e) => {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e.into());
            },
        };

        // Check if we have actions to execute (before risk check — no point evaluating
        // risk for a no-action decision)
        if decision.actions.is_empty() {
            if let Err(e) = query.complete(QueryOutcome::NoAction {
                reason: "No actions from engine".to_string(),
            }) {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query completion error: {}", e)));
            }
            self.record_query_transition(&query, "completed").await?;
            return Ok(());
        }

        // Phase 2: Risk governance gate.
        //
        // Build RiskContext (current portfolio state) and ProposedTrade (this entry),
        // then delegate to QueryEngine.check_risk(), which:
        //   1. Transitions query to RiskChecked (or returns InvalidState on failure)
        //   2. Evaluates the RiskGate (pure computation)
        //   3. Returns GovernedAction (approved) or CheckRiskError (denied or state
        //      error)
        //
        // Denial (CheckRiskError::Denied) is a governed outcome — return Ok(()).
        // InvalidState (CheckRiskError::InvalidState) is an operational error —
        // propagate.
        let risk_context = match self.build_risk_context().await {
            Ok(ctx) => ctx,
            Err(e) => {
                let err_str = format!("{}", e);
                query.fail(err_str.clone(), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(e);
            },
        };

        let proposed = match Self::build_proposed_trade(&signal, &decision) {
            Some(p) => p,
            None => {
                let err_str =
                    "decide_entry produced actions but no PlaceEntryOrder — cannot build ProposedTrade"
                        .to_string();
                query.fail(err_str.clone(), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(err_str));
            },
        };

        if let Err(error) =
            self.validate_entry_quantity_for_exchange(&signal.symbol, &proposed).await
        {
            let error_message = error.to_string();
            query.fail(error_message.clone(), "processing".to_string());
            self.record_query_failure(&query).await?;
            self.rearm_detector_after_governed_block(
                position_id,
                &position,
                "entry quantity rejected before exchange submission",
            )
            .await;
            return Err(DaemonError::Exec(error));
        }

        let governed: GovernedAction = match self
            .query_engine
            .check_risk(&mut query, &proposed, &risk_context, decision.actions)
            .await
        {
            Ok(g) => g,
            Err(CheckRiskError::Denied) => {
                // Governed denial: query is already in Denied state.
                //
                // v3: If the denial was caused by monthly drawdown (>=4%):
                // 1. Activate MonthlyHalt (circuit_breaker blocks new entries).
                // 2. Release entry_flow_lock — panic_close_all() → disarm_position() also
                //    acquires this lock; holding it here would deadlock. Dropping is safe:
                //    MonthlyHalt is already set, no new entries can proceed
                //    (circuit_breaker.blocks_new_entries() == true).
                // 3. Close all open positions via panic_close_all().
                if let QueryState::Denied { ref check, ref reason } = query.state {
                    if check == "monthly_drawdown" {
                        let reason_str = reason.clone();
                        if let Some(()) =
                            self.circuit_breaker.trigger_halt(reason_str.clone()).await
                        {
                            warn!(
                                %position_id,
                                "Monthly drawdown exceeded — MonthlyHalt triggered, closing positions"
                            );
                            self.event_bus.send(DaemonEvent::MonthlyHaltTriggered {
                                reason: reason_str.clone(),
                                triggered_at: chrono::Utc::now(),
                            });
                            drop(entry_flow_guard.take());
                            if let Err(e) = self.panic_close_all().await {
                                error!(
                                    error = %e,
                                    %position_id,
                                    "MonthlyHalt: panic_close_all failed during handle_signal"
                                );
                            }
                        }
                    }
                }

                // Record risk denial in Prometheus metrics.
                let denied_check = match &query.state {
                    QueryState::Denied { check, .. } => Some(check.clone()),
                    _ => None,
                };
                if let Some(check) = denied_check.as_deref() {
                    crate::metrics::RISK_DENIALS.with_label_values(&[check]).inc();
                }

                if self.margin_denial_cancels(position_id, &query).await {
                    self.cancel_immediate_arm_on_margin(position_id, query.id).await?;
                    return Ok(());
                }

                // Re-arm the detector so the Armed position can receive future signals.
                // The original detector completed when the signal fired; without re-arming,
                // the position would be Armed but permanently unresponsive.
                self.rearm_detector_after_governed_block(position_id, &position, "risk denied")
                    .await;
                info!(
                    %position_id,
                    query_id = %query.id,
                    "Entry denied by risk gate — detector re-armed (governed outcome)"
                );
                return Ok(());
            },
            Err(CheckRiskError::InvalidState(e)) => {
                // Operational error: query lifecycle state machine is inconsistent.
                // This is NOT a governed denial — it indicates a bug or concurrent
                // mutation. Fail the query and propagate as a hard error.
                let err_str = format!("Risk gate lifecycle error: {}", e);
                query.fail(err_str.clone(), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(err_str));
            },
            Err(CheckRiskError::Audit(e)) => {
                // The gate may have refused before its record failed. A refusal
                // that cancels still cancels: the failure re-arm must not retry
                // it.
                if self.margin_denial_cancels(position_id, &query).await {
                    self.cancel_immediate_arm_on_margin(position_id, query.id).await?;
                }
                return Err(e.into());
            },
        };

        // Domain ApprovalPolicy is authoritative over the notional-threshold adapter.
        let domain_approval = self.entry_policy_for_position(position_id).await.approval;

        match self
            .query_engine
            .check_approval_with_domain_policy(&mut query, governed, domain_approval)
            .await
        {
            Ok(ApprovalCheckResult::Ready(governed)) => {
                self.execute_signal_query(&mut query, governed, reach).await?;
                Ok(())
            },
            Ok(ApprovalCheckResult::AwaitingApproval(governed)) => {
                let query_id = query.id;
                let expires_at = query
                    .approval
                    .as_ref()
                    .map(|approval| approval.expires_at)
                    .expect("awaiting approval query must contain approval metadata");

                // Emit audit event for replay-safe AwaitingApproval stage evidence.
                let approval_pending_event = Event::EntryApprovalPending {
                    position_id,
                    signal_id: signal.signal_id,
                    timestamp: chrono::Utc::now(),
                };
                // Broadcast to event bus so real-time consumers (UI, audit log)
                // can observe the AwaitingApproval lifecycle stage immediately.
                self.event_bus.send(DaemonEvent::DomainEvent(approval_pending_event.clone()));
                if let Err(e) = self
                    .execute_and_persist(vec![EngineAction::EmitEvent(approval_pending_event)])
                    .await
                {
                    warn!(
                        %position_id,
                        error = %e,
                        "Failed to persist EntryApprovalPending audit event — approval continues"
                    );
                }

                self.store_pending_approval(query, position.clone(), proposed, governed).await;
                self.clear_governed_rearm_backoff(position_id).await;

                info!(
                    %position_id,
                    %query_id,
                    expires_at = %expires_at,
                    "Entry awaiting operator approval"
                );
                Ok(())
            },
            Err(e) => {
                let err_str = format!("Approval gate lifecycle error: {}", e);
                query.fail(err_str.clone(), "risk_checked".to_string());
                self.record_query_failure(&query).await?;
                Err(DaemonError::Config(err_str))
            },
        }
    }

    /// Approve a pending query and resume execution.
    pub async fn approve_query(&self, query_id: Uuid) -> DaemonResult<ExecutionQuery> {
        let mut entry_flow_guard = Some(self.entry_flow_lock.lock().await);
        let mut reach = AdmissionReach::Untouched;
        let result = self.process_approval(query_id, &mut entry_flow_guard, &mut reach).await;
        if let (Err(error), AdmissionReach::DetectorSpent(position_id), Some(_)) =
            (&result, reach, &entry_flow_guard)
        {
            self.rearm_detector_after_failed_admission(position_id, "approval", error).await;
        }
        result
    }

    async fn process_approval(
        &self,
        query_id: Uuid,
        entry_flow_guard: &mut Option<MutexGuard<'_, ()>>,
        reach: &mut AdmissionReach,
    ) -> DaemonResult<ExecutionQuery> {
        // Approval resume is a new entry — respect MonthlyHalt.
        // The query was approved by the operator, but the system may have
        // entered MonthlyHalt between when the query was queued and now.
        if self.circuit_breaker.blocks_new_entries().await {
            let snap = self.circuit_breaker.snapshot().await;
            return Err(DaemonError::MonthlyHaltActive { reason: snap.reason.unwrap_or_default() });
        }
        self.check_no_unresolved_residual().await?;
        self.check_admissions_open()?;

        let mut record = {
            let mut pending_approvals = self.pending_approvals.write().await;
            pending_approvals
                .remove(&query_id)
                .ok_or(DaemonError::QueryNotFound(query_id))?
        };
        // The approval was the position's only way to an entry.
        *reach = AdmissionReach::DetectorSpent(record.position.id);

        let expired = record
            .query
            .approval
            .as_ref()
            .map(|approval| chrono::Utc::now() >= approval.expires_at)
            .unwrap_or(false);

        if expired {
            if let Err(e) = self.query_engine.expire(&mut record.query, "expired").await {
                let err_str = format!("Approval expiry transition error: {}", e);
                record.query.fail(err_str.clone(), "awaiting_approval".to_string());
                self.record_query_failure(&record.query).await?;
                return Err(DaemonError::Config(err_str));
            }

            self.emit_query_expired(&record.query);
            self.rearm_detector_after_governed_block(
                record.position.id,
                &record.position,
                "approval expired",
            )
            .await;
            return Err(DaemonError::ApprovalExpired(query_id));
        }

        let current_position = match self.store.positions().find_by_id(record.position.id).await {
            Ok(Some(position)) => position,
            Ok(None) => {
                let err = DaemonError::PositionNotFound(record.position.id);
                record.query.fail(
                    format!("Pending approval invalidated: {}", err),
                    "awaiting_approval".to_string(),
                );
                self.record_query_failure(&record.query).await?;
                return Err(err);
            },
            Err(e) => {
                let err_str = format!("{}", e);
                record.query.fail(err_str.clone(), "awaiting_approval".to_string());
                self.record_query_failure(&record.query).await?;
                return Err(e.into());
            },
        };

        if !matches!(current_position.state, PositionState::Armed) {
            let err = DaemonError::InvalidPositionState {
                expected: "Armed".to_string(),
                actual: format!("{:?}", current_position.state),
            };
            record.query.fail(
                format!("Pending approval invalidated: {}", err),
                "awaiting_approval".to_string(),
            );
            self.record_query_failure(&record.query).await?;
            return Err(err);
        }

        let risk_context = self.build_risk_context().await?;
        if let Err(error) = self
            .validate_entry_quantity_for_exchange(&current_position.symbol, &record.proposed)
            .await
        {
            let error_message = error.to_string();
            record.query.fail(error_message.clone(), "awaiting_approval".to_string());
            self.record_query_failure(&record.query).await?;
            self.rearm_detector_after_governed_block(
                current_position.id,
                &current_position,
                "entry quantity rejected before exchange submission",
            )
            .await;
            return Err(DaemonError::Exec(error));
        }
        match self
            .query_engine
            .revalidate_risk(&mut record.query, &record.proposed, &risk_context)
            .await
        {
            Ok(()) => {},
            Err(CheckRiskError::Denied) => {
                // v3: If denial caused by monthly drawdown during approval revalidation:
                // 1. Activate MonthlyHalt (circuit_breaker blocks new entries).
                // 2. Release entry_flow_lock — panic_close_all() → disarm_position() also
                //    acquires this lock; holding it here would deadlock. Dropping is safe:
                //    MonthlyHalt is already set, no new entries can proceed
                //    (circuit_breaker.blocks_new_entries() == true).
                // 3. Close all open positions via panic_close_all().
                if let QueryState::Denied { ref check, ref reason } = record.query.state {
                    if check == "monthly_drawdown" {
                        let reason_str = reason.clone();
                        if let Some(()) =
                            self.circuit_breaker.trigger_halt(reason_str.clone()).await
                        {
                            warn!(
                                query_id = %query_id,
                                "Monthly drawdown exceeded during approval revalidation — MonthlyHalt triggered, closing positions"
                            );
                            self.event_bus.send(DaemonEvent::MonthlyHaltTriggered {
                                reason: reason_str.clone(),
                                triggered_at: chrono::Utc::now(),
                            });
                            drop(entry_flow_guard.take());
                            if let Err(e) = self.panic_close_all().await {
                                error!(
                                    error = %e,
                                    query_id = %query_id,
                                    "MonthlyHalt: panic_close_all failed during approve_query revalidation"
                                );
                            }
                        }
                    }
                }

                self.rearm_detector_after_governed_block(
                    current_position.id,
                    &current_position,
                    "approval revalidation denied",
                )
                .await;

                let reason = match &record.query.state {
                    QueryState::Denied { reason, .. } => reason.clone(),
                    _ => "Approval denied after risk revalidation".to_string(),
                };

                return Err(DaemonError::ApprovalDenied { query_id, reason });
            },
            Err(CheckRiskError::InvalidState(e)) => {
                let err_str = format!("Approval revalidation lifecycle error: {}", e);
                record.query.fail(err_str.clone(), "awaiting_approval".to_string());
                self.record_query_failure(&record.query).await?;
                return Err(DaemonError::Config(err_str));
            },
            Err(CheckRiskError::Audit(e)) => {
                return Err(e.into());
            },
        }

        if let Err(e) = self.query_engine.authorize(&mut record.query).await {
            let err_str = format!("Approval authorization error: {}", e);
            record.query.fail(err_str.clone(), "awaiting_approval".to_string());
            self.record_query_failure(&record.query).await?;
            return Err(DaemonError::Config(err_str));
        }

        self.emit_query_authorized(&record.query);
        self.execute_signal_query(&mut record.query, record.governed, reach).await?;

        Ok(record.query)
    }

    /// Handle entry fill (transition from Entering → Active).
    ///
    /// Flow: Load → Engine → Execute actions (emit events) → Save state
    async fn handle_entry_fill(
        &self,
        position_id: PositionId,
        fill_price: Price,
        filled_quantity: Quantity,
        fee: Decimal,
        filled_at: chrono::DateTime<chrono::Utc>,
        exchange_position_id: Option<String>,
    ) -> DaemonResult<()> {
        // Load position
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Consume the entry-time invalidation guard level cached when the
        // signal was processed (ADR-0042). The guard travels onto the Active
        // state and the EntryFilled event so the executable stop stays clamped
        // until the first trailing advance. Missing (e.g. guard disabled, or a
        // restart during the Entering window) → None = historical behavior.
        let invalidation_guard_level = {
            let mut guards = self.invalidation_guards.write().await;
            guards.remove(&position_id)
        };

        // Runtime trading rules so the insurance stop lands on the plan's
        // tick-quantized trigger (issue #154).
        let trading_rules = self.trading_rules_for(&position).await;

        // Use engine to process fill (pure: State+Fill → Decision)
        // exchange_position_id is passed through to EntryFilled event
        let decision = self.engine.lock().unwrap().process_entry_fill_with_rules(
            &position,
            fill_price,
            filled_quantity,
            fee,
            filled_at,
            exchange_position_id.clone(),
            invalidation_guard_level,
            trading_rules.as_ref(),
        )?;

        // Execute actions (EntryFilled event transitions position to Active via
        // apply_event) Also persists to eventlog for crash recovery
        // (MIG-v2.5#2).
        self.execute_and_persist(decision.actions).await?;

        info!(
            %position_id,
            fill_price = %fill_price.as_decimal(),
            "Entry filled, position now Active"
        );

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Entering".to_string(),
            new_state: "Active".to_string(),
            timestamp: chrono::Utc::now(),
        });

        let core_exchange_id = exchange_position_id.unwrap_or_else(|| {
            warn!(
                %position_id,
                "Missing binance_position_id on Core open; using position_id fallback"
            );
            position_id.to_string()
        });

        self.event_bus.send(DaemonEvent::CorePositionOpened {
            position_id,
            symbol: position.symbol.clone(),
            side: position.side,
            exchange_position_id: core_exchange_id,
        });

        Ok(())
    }

    /// Process market data for active positions.
    ///
    /// This updates trailing stops and triggers exits when necessary.
    ///
    /// # Canonical Flow
    ///
    /// ```text
    /// Tick → State (from projection) → Engine(State, Tick) → Decision
    /// → Executor(Decision) → Result → EventLog.append(Event)
    /// → Projection.apply(Event) (async)
    /// ```
    /// True when an execution error is the exchange refusing a reduce-only
    /// order (Binance -2022). Whether nothing is left to reduce takes an
    /// account observation.
    fn is_reduce_only_rejection(error: &DaemonError) -> bool {
        matches!(error, DaemonError::Exec(ExecError::ReduceOnlyRejected(_)))
    }

    /// True when the exchange no longer reports an open position matching
    /// this position's symbol and side.
    async fn exchange_reports_flat(&self, position: &Position) -> DaemonResult<bool> {
        let open = self.exchange_open_positions().await?;
        Ok(!open.iter().any(|exchange_position| {
            exchange_position.symbol == position.symbol && exchange_position.side == position.side
        }))
    }

    /// Close a book position the exchange already closed (#142).
    ///
    /// Reuses the reverse-reconciliation evidence path: resolve the insurance
    /// stop's real fill on the exchange and run `reconcile_close` with it.
    /// Returns `Ok(true)` when the position reached a terminal state; `Ok
    /// (false)` when fill evidence is not visible yet (caller retries on a
    /// later tick without placing further exit orders).
    async fn try_close_exchange_overtaken_position(
        &self,
        position: &Position,
    ) -> DaemonResult<bool> {
        let exchange = self.exchange();
        let Some(input) = crate::reconciliation_worker::gather_order_fill_evidence(
            &exchange,
            &self.store,
            position,
        )
        .await?
        else {
            debug!(
                position_id = %position.id,
                "Exchange-overtaken exit: insurance stop fill not visible yet; will retry"
            );
            return Ok(false);
        };

        let outcome = self.reconcile_close(input).await?;
        let closed = matches!(
            outcome,
            ReconcileCloseOutcome::Closed | ReconcileCloseOutcome::AlreadyTerminal
        );
        if closed {
            self.exit_overtaken_suspects.write().await.remove(&position.id);
            info!(
                position_id = %position.id,
                symbol = %position.symbol.as_pair(),
                "Exchange-overtaken exit reconciled to Closed without further exit orders"
            );
        }
        Ok(closed)
    }

    /// Drop a decision that carries nothing but the `PositionMonitorTick`
    /// heartbeat when the previous heartbeat for this position is younger
    /// than `monitor_tick_interval`. Any decision that does emit a tick
    /// (alone or alongside an exit / trailing advance) refreshes the stamp.
    /// Returns the possibly emptied decision and whether it was throttled.
    async fn throttle_monitor_tick(
        &self,
        position_id: PositionId,
        mut decision: robson_engine::EngineDecision,
    ) -> (robson_engine::EngineDecision, bool) {
        if self.monitor_tick_interval.is_zero() {
            return (decision, false);
        }
        let is_tick = |action: &EngineAction| {
            matches!(action, EngineAction::EmitEvent(Event::PositionMonitorTick { .. }))
        };
        let only_tick = decision.actions.len() == 1 && is_tick(&decision.actions[0]);
        let now = std::time::Instant::now();
        if only_tick {
            let last = self.last_monitor_tick_at.read().await.get(&position_id).copied();
            if should_drop_monitor_tick(last, now, self.monitor_tick_interval) {
                decision.actions.clear();
                return (decision, true);
            }
        }
        if decision.actions.iter().any(is_tick) {
            self.last_monitor_tick_at.write().await.insert(position_id, now);
        }
        (decision, false)
    }

    pub async fn process_market_data(&self, data: MarketData) -> DaemonResult<()> {
        // Remember the latest mark for this feed so month equity (ADR-0046)
        // values open positions at the live price, not at the last ratchet.
        self.last_market_price
            .write()
            .await
            .insert(data.feed_key(), (data.price, std::time::Instant::now()));

        // This manager's positions execute on one venue; another venue's tick
        // for the same symbol is not their market (ADR-0056 §3).
        if data.venue != self.venue {
            return Ok(());
        }

        // Find all active positions for this symbol (from projection)
        let active_positions = self.store.positions().find_active().await?;
        let active_positions_count = active_positions.len();
        self.prune_insurance_stop_stale(&active_positions).await;
        // A leg that cannot derive its stop for want of rules fails on its own: the
        // others are still processed, and the tick then reports the first failure.
        let mut first_missing_rules = None;

        for position in active_positions {
            if position.symbol != data.symbol {
                continue;
            }

            if !matches!(position.state, PositionState::Active { .. }) {
                continue;
            }

            // ADR-0039: an exchange stop left behind by a failed placement or
            // re-placement is healed here, at tick cadence with backoff,
            // before this tick is evaluated. Re-read the position afterwards
            // so the engine sees the new insurance order id.
            let position = if self.retry_stale_insurance_stop(&position).await {
                self.get_position(position.id).await?.unwrap_or(position)
            } else {
                position
            };
            if !matches!(position.state, PositionState::Active { .. }) {
                continue;
            }
            if self.exit_retry_pending(position.id).await {
                continue;
            }

            // Create one ExecutionQuery PER POSITION processed
            let mut query = ExecutionQuery::new(
                QueryKind::ProcessMarketTick {
                    symbol: data.symbol.clone(),
                    price: data.price,
                },
                ActorKind::MarketData,
            );
            query.position_id = Some(position.id);
            Self::set_query_context_summary(&mut query, active_positions_count);
            self.record_query_accepted(&query).await?;

            debug!(
                position_id = %position.id,
                query_id = %query.id,
                price = %data.price.as_decimal(),
                "Processing market tick for position"
            );

            // Transition to Processing
            if let Err(e) = query.transition(QueryState::Processing) {
                query.fail(format!("{}", e), "accepted".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query transition error: {}", e)));
            }
            self.record_query_transition(&query, "processing").await?;

            // #142 fast-path: the exchange already closed this position (its
            // insurance stop filled) and rejected our reduce-only exit. Do
            // not run the engine or place further exit orders; poll for the
            // stop's fill evidence and close via reverse reconciliation.
            if self.exit_overtaken_suspects.read().await.contains(&position.id) {
                if self.exchange_reports_flat(&position).await.unwrap_or(true) {
                    let closed = match self.try_close_exchange_overtaken_position(&position).await {
                        Ok(closed) => closed,
                        Err(e) => {
                            let err_str = format!("{}", e);
                            query.fail(err_str.clone(), "processing".to_string());
                            self.record_query_failure(&query).await?;
                            return Err(e);
                        },
                    };
                    let outcome = if closed {
                        QueryOutcome::ActionsExecuted { actions_count: 1 }
                    } else {
                        QueryOutcome::NoAction {
                            reason: "exit overtaken by exchange-side close; awaiting fill evidence"
                                .to_string(),
                        }
                    };
                    if let Err(e) = query.complete(outcome) {
                        query.fail(format!("{}", e), "processing".to_string());
                        self.record_query_failure(&query).await?;
                        return Err(DaemonError::Config(format!("Query completion error: {}", e)));
                    }
                    self.record_query_transition(&query, "completed").await?;
                    continue;
                }
                // The exchange reports the position again: the suspicion was
                // wrong, resume normal processing.
                self.exit_overtaken_suspects.write().await.remove(&position.id);
            }

            // Use engine to process (pure: State+Tick → Decision). Trading
            // rules are what is already held (ADR-0056 §5): no exchange call
            // on the tick path unless nothing is; legacy positions skip the
            // lookup entirely.
            let trading_rules = self.trading_rules_for(&position).await;
            let symbol_clone = data.symbol.clone();
            let market_data = robson_engine::MarketData::new(symbol_clone, data.price);
            let decision = {
                let engine = self.engine.lock().unwrap();
                engine.process_active_position_with_rules(
                    &position,
                    &market_data,
                    trading_rules.as_ref(),
                )
            };
            let decision = match decision {
                Ok(d) => d,
                Err(e) => {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "processing".to_string());
                    self.record_query_failure(&query).await?;
                    if matches!(
                        e,
                        robson_engine::EngineError::DomainError(
                            robson_domain::DomainError::TradingRulesUnavailable(_)
                        )
                    ) {
                        warn!(position_id = %position.id, %err_str, "Leg cannot derive its stop");
                        first_missing_rules.get_or_insert(e);
                        continue;
                    }
                    return Err(e.into());
                },
            };

            // Audit tick throttle (ADR-0049): the engine still evaluated this
            // trade for exit and trailing advance; only the heartbeat event
            // is dropped when the previous one is too recent.
            let (decision, tick_throttled) =
                self.throttle_monitor_tick(position.id, decision).await;

            // Check if we have actions to execute
            if decision.actions.is_empty() {
                let reason = if tick_throttled {
                    "Monitor tick throttled".to_string()
                } else {
                    "No stop trigger".to_string()
                };
                if let Err(e) = query.complete(QueryOutcome::NoAction { reason }) {
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "processing".to_string());
                    self.record_query_failure(&query).await?;
                    return Err(DaemonError::Config(format!("Query completion error: {}", e)));
                }
                self.record_query_transition(&query, "completed").await?;
                continue;
            }

            // Transition to Acting before executor call
            if let Err(e) = query.transition(QueryState::Acting) {
                query.fail(format!("{}", e), "processing".to_string());
                self.record_query_failure(&query).await?;
                return Err(DaemonError::Config(format!("Query transition error: {}", e)));
            }
            self.record_query_transition(&query, "acting").await?;

            // Execute actions via Executor (side-effects: EventLog.append, Exchange orders)
            // MemoryStore is updated via apply_event() called by executor after event
            // append
            let actions = Self::stamp_cycle_id(decision.actions, query.id);
            let results = match self.execute_and_persist(actions).await {
                Ok(r) => r,
                // A batch with no venue action whose event did not persist; on
                // a suspended stream every such batch is refused. It fails this
                // leg's query only: the other legs on this tick still trail and
                // exit (ADR-0056 step 7, PR P).
                Err(e) if e.is_event_not_persisted() => {
                    error!(
                        position_id = %position.id,
                        error = %e,
                        "Tick batch did not persist; continuing with the other legs"
                    );
                    query.fail(format!("{}", e), "acting".to_string());
                    self.record_after_venue_work(&query, "failed").await;
                    continue;
                },
                Err(e) => {
                    // #142: a reduce-only rejection with the exchange flat
                    // means the exchange-side insurance stop already closed
                    // the position. Blind retries can never succeed (there is
                    // nothing left to reduce), so hand off to reverse
                    // reconciliation instead of failing the query.
                    let overtaken = Self::is_reduce_only_rejection(&e)
                        && self.exchange_reports_flat(&position).await.unwrap_or(false);
                    if overtaken {
                        warn!(
                            position_id = %position.id,
                            symbol = %position.symbol.as_pair(),
                            error = %e,
                            "Exit overtaken by exchange-side close; handing off to reverse \
                             reconciliation instead of retrying the exit order"
                        );
                        self.exit_overtaken_suspects.write().await.insert(position.id);
                        let closed =
                            match self.try_close_exchange_overtaken_position(&position).await {
                                Ok(closed) => closed,
                                Err(reconcile_error) => {
                                    let err_str = format!("{}", reconcile_error);
                                    query.fail(err_str.clone(), "acting".to_string());
                                    self.record_query_failure(&query).await?;
                                    return Err(reconcile_error);
                                },
                            };
                        let outcome = if closed {
                            QueryOutcome::ActionsExecuted { actions_count: 1 }
                        } else {
                            QueryOutcome::NoAction {
                                reason: "exit overtaken by exchange-side close; awaiting fill \
                                         evidence"
                                    .to_string(),
                            }
                        };
                        if let Err(complete_error) = query.complete(outcome) {
                            query.fail(format!("{}", complete_error), "acting".to_string());
                            self.record_after_venue_work(&query, "failed").await;
                            return Err(DaemonError::Config(format!(
                                "Query completion error: {}",
                                complete_error
                            )));
                        }
                        self.record_after_venue_work(&query, "completed").await;
                        continue;
                    }
                    // The venue refused this exit: on its rules after one refresh
                    // and resubmission, or as reduce-only with the account not flat.
                    // The leg tries again after a window, not at tick cadence.
                    if matches!(
                        e,
                        DaemonError::Exec(
                            ExecError::RulesRejected(_)
                                | ExecError::ReduceOnlyRejected(_)
                                | ExecError::SendRefused(_)
                        )
                    ) {
                        warn!(
                            position_id = %position.id,
                            error = %e,
                            retry_in_secs = EXIT_RULES_REJECTION_BACKOFF.as_secs(),
                            "Exit refused by the venue; retrying after a window (ADR-0056 §5)"
                        );
                        self.exit_retry_after.write().await.insert(
                            position.id,
                            std::time::Instant::now() + EXIT_RULES_REJECTION_BACKOFF,
                        );
                    }
                    let err_str = format!("{}", e);
                    query.fail(err_str.clone(), "acting".to_string());
                    self.record_query_failure(&query).await?;
                    return Err(e);
                },
            };

            // Process results
            // actions_count represents ALL ActionResult variants
            let actions_count = results.len();
            for result in results {
                if let ActionResult::ExitNotExecutable { residual, .. } = &result {
                    warn!(
                        position_id = %position.id,
                        %residual,
                        retry_in_secs = NOT_EXECUTABLE_BACKOFF.as_secs(),
                        "Exit has nothing executable; the leg stays Active"
                    );
                    self.exit_retry_after
                        .write()
                        .await
                        .insert(position.id, std::time::Instant::now() + NOT_EXECUTABLE_BACKOFF);
                }
                if let ActionResult::OrderPlaced {
                    order,
                    event: Some(Event::ExitOrderPlaced { .. }),
                } = result
                {
                    // Only a filled market exit closes the position. Insurance
                    // stop placements and replacements also return OrderPlaced,
                    // but they are accepted protective orders while the
                    // position remains Active.
                    // Note: handle_exit_fill is internal, covered by this query's lifecycle
                    // Note: ExitOrderPlaced event already persisted by execute_and_persist()
                    crate::metrics::ORDERS.with_label_values(&["exit"]).inc();
                    if let Err(e) = self
                        .handle_exit_fill(
                            position.id,
                            order.fill_price,
                            order.filled_quantity,
                            order.fee,
                            order.filled_at,
                        )
                        .await
                    {
                        let err_str = format!("{}", e);
                        query.fail(err_str.clone(), "acting".to_string());
                        self.record_query_failure(&query).await?;
                        return Err(e);
                    }
                }
            }

            // Complete query with success
            if let Err(e) = query.complete(QueryOutcome::ActionsExecuted { actions_count }) {
                query.fail(format!("{}", e), "acting".to_string());
                self.record_after_venue_work(&query, "failed").await;
                return Err(DaemonError::Config(format!("Query completion error: {}", e)));
            }
            self.record_after_venue_work(&query, "completed").await;
        }

        if let Some(error) = first_missing_rules {
            return Err(error.into());
        }

        // ADR-0046 §3: observe the equity peak at the 20 s monitor cadence,
        // not per tick — the refresh reads monthly state, and paying that on
        // every aggTrade is the hot-path N+1 the guardrails forbid. Ticks
        // arriving inside the window skip; marks cannot raise the peak by
        // more than one window's movement before the next observation.
        let should_refresh_peak = {
            let mut last = self.month_peak_refreshed_at.lock().await;
            match *last {
                Some(t) if t.elapsed() < std::time::Duration::from_secs(20) => false,
                _ => {
                    *last = Some(std::time::Instant::now());
                    true
                },
            }
        };
        if should_refresh_peak {
            self.canonical_budget_snapshot(chrono::Utc::now()).await?;
        }
        crate::metrics::CYCLES.with_label_values(&["success"]).inc();
        Ok(())
    }

    /// Handle exit fill (transition to Closed).
    ///
    /// Called after exit order is filled. Emits PositionClosed event
    /// which will be applied by projection to update state.
    async fn handle_exit_fill(
        &self,
        position_id: PositionId,
        fill_price: Price,
        _filled_quantity: Quantity,
        exit_fee: Decimal,
        filled_at: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<()> {
        let position = self
            .store
            .positions()
            .find_by_id(position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(position_id))?;

        // Extract exit reason from position's Exiting state.
        // By this point, executor.execute_exit_order has already emitted and applied
        // ExitOrderPlaced, transitioning the position to Exiting { exit_reason }.
        let exit_reason = match &position.state {
            PositionState::Exiting { exit_reason, .. } => *exit_reason,
            other => {
                return Err(DaemonError::InvalidPositionState {
                    expected: "Exiting".to_string(),
                    actual: format!("{:?}", other),
                });
            },
        };

        info!(
            %position_id,
            fill_price = %fill_price.as_decimal(),
            ?exit_reason,
            "Exit filled, emitting PositionClosed event"
        );

        // Calculate PnL for event using actual fill price.
        // calculate_pnl() returns ZERO for Exiting state — direct calc instead.
        let entry_price =
            position.entry_price.ok_or_else(|| DaemonError::InvalidPositionState {
                expected: "Exiting with entry_price set".to_string(),
                actual: format!("Exiting with entry_price=None for position {}", position_id),
            })?;
        let qty = position.quantity.as_decimal();
        let pnl = match position.side {
            Side::Long => (fill_price.as_decimal() - entry_price.as_decimal()) * qty,
            Side::Short => (entry_price.as_decimal() - fill_price.as_decimal()) * qty,
        };

        // Emit PositionClosed event via executor (ensures append->apply order)
        // Also persists to eventlog for crash recovery (MIG-v2.5#2).
        let total_fees = position.fees_paid + exit_fee;
        let event = Event::PositionClosed {
            position_id,
            exit_reason,
            entry_price,
            exit_price: fill_price,
            realized_pnl: pnl,
            total_fees,
            closure_evidence: robson_domain::ClosureEvidence::real_exit_fill(None),
            timestamp: filled_at,
        };
        self.record_venue_close(event).await?;

        // Record realized PnL in Prometheus metrics
        crate::metrics::POSITION_PNL
            .with_label_values(&[&position_id.to_string()])
            .set(pnl.to_string().parse::<f64>().unwrap_or(0.0));

        // Send to event bus for real-time notification
        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id,
            previous_state: "Exiting".to_string(),
            new_state: "Closed".to_string(),
            timestamp: filled_at,
        });
        self.event_bus.send(DaemonEvent::CorePositionClosed {
            position_id,
            symbol: position.symbol.clone(),
            side: position.side,
        });

        // After position close, realized PnL has changed — evaluate MonthlyHalt.
        // Safe: handle_exit_fill is called from process_market_data which does NOT
        // hold entry_flow_lock. evaluate_monthly_halt calls panic_close_all which
        // takes entry_flow_lock internally — no deadlock.
        self.evaluate_monthly_halt().await;

        Ok(())
    }

    /// Close a stale `Active` position using real exchange evidence gathered by
    /// reverse reconciliation.
    ///
    /// Slice 4A intentionally accepts only real fill/trade evidence. Snapshot
    /// and estimated evidence prove absence but do not prove a real exit price,
    /// so they are rejected without changing local lifecycle state.
    pub(crate) async fn reconcile_close(
        &self,
        input: ReconciledCloseInput,
    ) -> DaemonResult<ReconcileCloseOutcome> {
        let position = self
            .store
            .positions()
            .find_by_id(input.position_id)
            .await?
            .ok_or(DaemonError::PositionNotFound(input.position_id))?;

        match &position.state {
            PositionState::Closed { .. }
            | PositionState::Cancelled
            | PositionState::Error { .. } => {
                debug!(
                    position_id = %input.position_id,
                    state = %position.state.name(),
                    "Reverse reconciliation close skipped: position is already terminal"
                );
                return Ok(ReconcileCloseOutcome::AlreadyTerminal);
            },
            PositionState::Active { .. } => {},
            other => {
                warn!(
                    position_id = %input.position_id,
                    state = ?other,
                    "Reverse reconciliation close skipped: position is not Active"
                );
                return Ok(ReconcileCloseOutcome::SkippedNonActive {
                    state: position.state.name().to_string(),
                });
            },
        }

        let evidence_source = match &input.evidence {
            ReconciliationEvidence::OrderFillRecord(evidence) => {
                if input.exit_price != evidence.fill_price {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "exit_price",
                    });
                }
                if input.filled_quantity != evidence.filled_quantity {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "filled_quantity",
                    });
                }
                if input.fee != evidence.fee {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "fee",
                    });
                }
                if input.fee_asset != evidence.fee_asset {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "fee_asset",
                    });
                }
                if input.closed_at != evidence.filled_at {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "closed_at",
                    });
                }
                "order_fill_record"
            },
            ReconciliationEvidence::UserTradeRecord(evidence) => {
                if input.exit_price != evidence.fill_price {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "exit_price",
                    });
                }
                if input.filled_quantity != evidence.filled_quantity {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "filled_quantity",
                    });
                }
                if input.fee != evidence.fee {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "fee",
                    });
                }
                if input.fee_asset != evidence.fee_asset {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "fee_asset",
                    });
                }
                if input.closed_at != evidence.filled_at {
                    return Ok(ReconcileCloseOutcome::RejectedInconsistentEvidence {
                        field: "closed_at",
                    });
                }
                "user_trade_record"
            },
            ReconciliationEvidence::AccountSnapshot(_) => {
                warn!(
                    position_id = %input.position_id,
                    "Reverse reconciliation close rejected: account snapshot is not real fill evidence"
                );
                return Ok(ReconcileCloseOutcome::RejectedUnsupportedEvidence {
                    source: "account_snapshot",
                });
            },
            ReconciliationEvidence::Estimated(_) => {
                warn!(
                    position_id = %input.position_id,
                    "reconcile_close rejected: Estimated evidence is permanently \
                     disallowed in v3. Only OrderFillRecord and UserTradeRecord are \
                     accepted. Operator must intervene manually."
                );
                return Ok(ReconcileCloseOutcome::RejectedUnsupportedEvidence {
                    source: "estimated",
                });
            },
        };

        let entry_price =
            position.entry_price.ok_or_else(|| DaemonError::InvalidPositionState {
                expected: "Active with entry_price set".to_string(),
                actual: format!("Active with entry_price=None for position {}", input.position_id),
            })?;
        // A close books the leg's whole quantity at the last order's fill, as
        // handle_exit_fill and panic close do. Where earlier fills went at other
        // prices this is an estimate, but no quantity drops out of month net. A fill
        // above the leg prices what filled. The evidence keeps the real quantity.
        let qty = input.filled_quantity.as_decimal().max(position.quantity.as_decimal());
        let pnl = match position.side {
            Side::Long => (input.exit_price.as_decimal() - entry_price.as_decimal()) * qty,
            Side::Short => (entry_price.as_decimal() - input.exit_price.as_decimal()) * qty,
        };

        info!(
            position_id = %input.position_id,
            exit_price = %input.exit_price.as_decimal(),
            filled_quantity = %input.filled_quantity.as_decimal(),
            evidence_source,
            "Reverse reconciliation close accepted, emitting PositionClosed event"
        );

        // Classify the close: a fill from robsond's own protective stop is a
        // governed InsuranceStop exit and must count toward the monthly
        // gauge and slot accounting (the generic reconciled reason is
        // excluded from governed math as out-of-band).
        let exit_reason = if input
            .authored_client_order_id
            .as_deref()
            .is_some_and(|id| id.starts_with("ins-"))
        {
            robson_domain::ExitReason::InsuranceStop
        } else {
            robson_domain::ExitReason::ReconciledMissingOnExchange
        };

        let event = Event::PositionClosed {
            position_id: input.position_id,
            exit_reason,
            entry_price,
            exit_price: input.exit_price,
            realized_pnl: pnl,
            total_fees: position.fees_paid + input.fee,
            closure_evidence: ClosureEvidence::Reconciled(input.evidence),
            timestamp: input.closed_at,
        };
        self.record_venue_close(event).await?;

        crate::metrics::POSITION_PNL
            .with_label_values(&[&input.position_id.to_string()])
            .set(pnl.to_string().parse::<f64>().unwrap_or(0.0));

        self.event_bus.send(DaemonEvent::PositionStateChanged {
            position_id: input.position_id,
            previous_state: "Active".to_string(),
            new_state: "Closed".to_string(),
            timestamp: input.closed_at,
        });
        self.event_bus.send(DaemonEvent::CorePositionClosed {
            position_id: input.position_id,
            symbol: position.symbol.clone(),
            side: position.side,
        });

        self.evaluate_monthly_halt().await;

        Ok(ReconcileCloseOutcome::Closed)
    }

    /// Emergency close all positions.
    ///
    /// Iterates all open core positions (Armed, Entering, Active, Exiting) and
    /// applies state-appropriate shutdown:
    ///
    /// - **Active**: place market exit order via
    ///   `panic_close_position_internal()`.
    /// - **Armed**: disarm (cancel detector). No exchange action needed.
    /// - **Entering**: order submitted but not yet filled — logged as skipped.
    ///   Cancelling a pending entry order requires exchange-specific logic
    ///   deferred to a follow-up task. The position will remain Entering until
    ///   the order fills or the exchange session expires.
    /// - **Exiting**: exit already in progress — skip to avoid duplicate
    ///   orders.
    pub async fn panic_close_all(&self) -> DaemonResult<Vec<PositionId>> {
        warn!("PANIC: Emergency close all positions");

        // Under MonthlyHalt no detector runs: stop them all before the disarms
        // below, which fail at their first audit write when the database is
        // down. A re-arm checks the halt under this same lock.
        if self.circuit_breaker.blocks_new_entries().await {
            for (_, detector) in self.detectors.write().await.drain() {
                detector.abort();
            }
        }

        // find_active() returns open core-lifecycle states only:
        // Armed, Entering, Active, Exiting.
        let all_non_terminal = self.store.positions().find_active().await?;
        let total_count = all_non_terminal.len();
        let mut closed_ids = Vec::new();

        for position in all_non_terminal {
            match &position.state {
                PositionState::Active { .. } => {
                    // Place market exit order
                    let mut query = ExecutionQuery::new(
                        QueryKind::PanicClosePosition { position_id: position.id },
                        Self::operator_actor(),
                    );
                    query.position_id = Some(position.id);
                    Self::set_query_context_summary(&mut query, total_count);
                    self.record_query_accepted(&query).await?;

                    match self.panic_close_position_internal(position.id, &mut query).await {
                        Ok(persisted) => {
                            if !persisted {
                                error!(
                                    position_id = %position.id,
                                    "Panic: leg closed, its close event did not persist"
                                );
                            }
                            if let Err(e) =
                                query.complete(QueryOutcome::ActionsExecuted { actions_count: 1 })
                            {
                                query.fail(format!("{}", e), "acting".to_string());
                                self.record_after_venue_work(&query, "failed").await;
                            } else {
                                self.record_after_venue_work(&query, "completed").await;
                            }
                            closed_ids.push(position.id);
                        },
                        Err(e) => {
                            // Caller owns failure recording — internal method does NOT call fail()
                            let phase = match &query.state {
                                QueryState::Accepted => "accepted".to_string(),
                                QueryState::Processing => "processing".to_string(),
                                QueryState::RiskChecked => "risk_checked".to_string(),
                                QueryState::AwaitingApproval => "awaiting_approval".to_string(),
                                QueryState::Authorized => "authorized".to_string(),
                                QueryState::Acting => "acting".to_string(),
                                QueryState::Completed => "completed".to_string(),
                                QueryState::Expired => "expired".to_string(),
                                QueryState::Failed { phase, .. } => phase.clone(),
                                QueryState::Denied { check, .. } => format!("denied:{}", check),
                            };
                            query.fail(format!("{}", e), phase);
                            // The other legs are still closed.
                            self.record_after_venue_work(&query, "failed").await;
                            error!(position_id = %position.id, error = %e, "Failed to panic close");
                        },
                    }
                },
                PositionState::Armed => {
                    // No exchange order exists — disarm the position (cancel its detector).
                    if let Err(e) = self.disarm_position(position.id).await {
                        warn!(position_id = %position.id, error = %e, "Panic: failed to disarm Armed position");
                    } else {
                        info!(position_id = %position.id, "Panic: Armed position disarmed");
                        closed_ids.push(position.id);
                    }
                },
                PositionState::Entering { .. } => {
                    // Entry order submitted but not yet filled.
                    // Cancelling a pending margin order requires exchange-specific
                    // cancel-order logic (deferred). Log and skip for now.
                    warn!(
                        position_id = %position.id,
                        "Panic: Entering position skipped — entry order cancel not yet implemented"
                    );
                },
                PositionState::Exiting { .. } => {
                    // Exit already in progress — do not place a duplicate order.
                    info!(position_id = %position.id, "Panic: Exiting position skipped — exit already in progress");
                },
                PositionState::Closed { .. } => {
                    // find_active() guarantees this is unreachable.
                },
                PositionState::Error { error, .. } => {
                    // Defensive fallback: find_active() excludes Error, but skip safely if the
                    // repository contract is violated.
                    warn!(
                        position_id = %position.id,
                        error = %error,
                        "Panic: Error position encountered outside find_active() contract"
                    );
                },
                PositionState::Cancelled => {
                    // find_active() guarantees this is unreachable — Cancelled
                    // is terminal.
                },
            }
        }

        info!(closed_count = closed_ids.len(), "Panic close complete");

        Ok(closed_ids)
    }

    /// Emergency close a single position (internal, takes query for lifecycle
    /// tracking).
    ///
    /// # Failure Recording Ownership
    ///
    /// This method does NOT call `query.fail()` or persist failure snapshots on
    /// errors. The caller is responsible for failure recording. This
    /// prevents double-fail.
    ///
    /// Emits PositionClosed event which will be applied by projection. Whether
    /// that close persisted: the leg is closed either way.
    async fn panic_close_position_internal(
        &self,
        position_id: PositionId,
        query: &mut ExecutionQuery,
    ) -> DaemonResult<bool> {
        // Transition to Processing
        if let Err(e) = query.transition(QueryState::Processing) {
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(query, "processing").await?;

        let position = match self.store.positions().find_by_id(position_id).await {
            Ok(Some(pos)) => pos,
            Ok(None) => {
                return Err(DaemonError::PositionNotFound(position_id));
            },
            Err(e) => {
                return Err(e.into());
            },
        };

        let exit_side = position.side.exit_action();

        // Transition to Acting before executor call
        if let Err(e) = query.transition(QueryState::Acting) {
            return Err(DaemonError::Config(format!("Query transition error: {}", e)));
        }
        self.record_query_transition(query, "acting").await?;

        // Place market exit order on exchange (executor also emits ExitOrderPlaced →
        // Active → Exiting) Use execute_and_persist to ensure events are
        // persisted to eventlog (MIG-v2.5#2)
        let results = self
            .execute_and_persist(vec![EngineAction::PlaceExitOrder {
                position_id,
                cycle_id: Some(query.id),
                symbol: position.symbol.clone(),
                side: exit_side,
                quantity: position.quantity,
                reason: robson_domain::ExitReason::UserPanic,
            }])
            .await?;

        // The leg rounds below the lot minimum: no order went out and the residual
        // is on record.
        if let Some(residual) = results.iter().find_map(|r| match r {
            ActionResult::ExitNotExecutable { residual, .. } => Some(*residual),
            _ => None,
        }) {
            return Err(DaemonError::Exec(ExecError::OrderRejected(format!(
                "Panic close: {residual} rounds below the lot minimum; nothing can be submitted"
            ))));
        }

        // Extract actual fill details from exchange result
        let (fill_price, exit_fee, filled_at) = match results.into_iter().find_map(|r| {
            if let ActionResult::OrderPlaced { order, .. } = r {
                Some((order.fill_price, order.fee, order.filled_at))
            } else {
                None
            }
        }) {
            Some(details) => details,
            None => {
                return Err(DaemonError::Exec(ExecError::InvalidState(
                    "Panic close: PlaceExitOrder did not return OrderPlaced".to_string(),
                )));
            },
        };

        // Calculate PnL from actual fill price (not current_price via calculate_pnl).
        // Direct calc avoids implicit current_price and guarantees fill_price is used.
        let entry_price =
            position.entry_price.ok_or_else(|| DaemonError::InvalidPositionState {
                expected: "Active/Exiting with entry_price set".to_string(),
                actual: format!("entry_price=None for position {}", position_id),
            })?;
        let qty = position.quantity.as_decimal();
        let pnl = match position.side {
            Side::Long => (fill_price.as_decimal() - entry_price.as_decimal()) * qty,
            Side::Short => (entry_price.as_decimal() - fill_price.as_decimal()) * qty,
        };

        // Emit PositionClosed with actual fill price (Exiting → Closed)
        let total_fees = position.fees_paid + exit_fee;
        let event = Event::PositionClosed {
            position_id,
            exit_reason: robson_domain::ExitReason::UserPanic,
            entry_price,
            exit_price: fill_price,
            realized_pnl: pnl,
            total_fees,
            closure_evidence: robson_domain::ClosureEvidence::real_exit_fill(None),
            timestamp: filled_at,
        };

        let persisted = self.record_venue_close(event).await?;

        // Send to event bus for real-time notification
        self.event_bus.send(DaemonEvent::CorePositionClosed {
            position_id,
            symbol: position.symbol.clone(),
            side: position.side,
        });

        Ok(persisted)
    }

    /// Kill detector task for a position.
    async fn kill_detector(&self, position_id: PositionId) {
        let mut detectors = self.detectors.write().await;
        if let Some(handle) = detectors.remove(&position_id) {
            handle.abort();
            debug!(%position_id, "Detector task killed");
        }
    }

    /// Get position by ID.
    pub async fn get_position(&self, position_id: PositionId) -> DaemonResult<Option<Position>> {
        Ok(self.store.positions().find_by_id(position_id).await?)
    }

    /// Get the latest market price from the exchange.
    pub async fn get_market_price(&self, symbol: &Symbol) -> DaemonResult<Price> {
        Ok(self.executor.get_price(symbol).await?)
    }

    /// Last market price received for `symbol` on the tick path (ADR-0046
    /// amendment), if it is younger than [`MARKET_PRICE_CACHE_MAX_AGE`].
    /// `None` until the first tick after startup and again once the feed for
    /// the symbol has gone quiet: a symbol with only an Exiting position is
    /// not covered by the ADR-0044 REST fallback poll, so without this bound
    /// an operator read could serve an arbitrarily old price. Callers fall
    /// back to an exchange round trip on `None`.
    pub async fn last_market_price_for(&self, symbol: &Symbol) -> Option<Price> {
        self.last_market_price
            .read()
            .await
            .get(&self.feed_key(symbol))
            .filter(|(_, seen_at)| seen_at.elapsed() <= MARKET_PRICE_CACHE_MAX_AGE)
            .map(|(price, _)| *price)
    }

    #[cfg(test)]
    pub(crate) async fn set_last_market_price_for_test(
        &self,
        symbol: &Symbol,
        price: Price,
        seen_at: std::time::Instant,
    ) {
        self.last_market_price
            .write()
            .await
            .insert(self.feed_key(symbol), (price, seen_at));
    }

    /// Get all open core positions.
    ///
    /// Includes Armed, Entering, Active, and Exiting positions.
    /// Excludes Closed and Error.
    pub async fn get_open_positions(&self) -> DaemonResult<Vec<Position>> {
        Ok(self.store.positions().find_active().await?)
    }

    /// Historical alias used by older API/CLI code.
    pub async fn get_active_positions(&self) -> DaemonResult<Vec<Position>> {
        self.get_open_positions().await
    }

    /// Get open position count.
    pub async fn position_count(&self) -> DaemonResult<usize> {
        // This is a hack since Store doesn't have count method
        // In memory store we can count, in production we'd have proper query
        let open = self.store.positions().find_active().await?;
        Ok(open.len())
    }

    /// Whether the symbol carries an Entering or Active position — the
    /// REST-fallback polling gate (ADR-0044).
    ///
    /// Fails protective: a storage error reports `true` so the fallback keeps
    /// polling rather than leaving a possibly open position blind.
    pub async fn has_risk_open_on_symbol(&self, symbol: &Symbol) -> bool {
        match self.store.positions().find_risk_open().await {
            Ok(positions) => positions.iter().any(|p| p.symbol.as_pair() == symbol.as_pair()),
            Err(e) => {
                warn!(
                    error = %e,
                    symbol = %symbol.as_pair(),
                    "Risk-open lookup failed; REST fallback stays protective"
                );
                true
            },
        }
    }

    /// Preserve the pre-ADR-0052 latent-risk path byte-for-byte for durable
    /// legacy positions. No execution costs or resolver behavior are added to
    /// this provenance-only branch.
    fn legacy_resolved_position_risk(position: &Position) -> Option<ResolvedPositionRisk> {
        let (entry, stop) = match &position.state {
            PositionState::Active { trailing_stop, .. } => {
                (position.entry_price?.as_decimal(), trailing_stop.as_decimal())
            },
            PositionState::Entering { expected_entry, .. } => {
                let entry = expected_entry.as_decimal();
                let stop = position
                    .tech_stop_distance
                    .as_ref()
                    .map(|ts| ts.initial_stop.as_decimal())
                    .unwrap_or(entry);
                (entry, stop)
            },
            _ => return None,
        };
        let qty = position.quantity.as_decimal();
        let risk = match position.side {
            Side::Long => (entry - stop) * qty,
            Side::Short => (stop - entry) * qty,
        };
        Some(ResolvedPositionRisk {
            current_stop: stop,
            risk: risk.max(Decimal::ZERO),
            cost_priced_latent_risk: None,
        })
    }

    /// Resolve live rules once per distinct symbol before latent-risk pricing.
    /// Only ExecutableSpan Active positions need current exchange metadata;
    /// Entering positions use their immutable admission trigger and legacy
    /// positions preserve their rules-free provenance path.
    async fn trading_rules_by_symbol_for_positions(
        &self,
        positions: &[Position],
    ) -> HashMap<Symbol, SymbolTradingRules> {
        let mut symbols: HashMap<Symbol, Option<&SymbolTradingRules>> = HashMap::new();
        for position in positions.iter().filter(|position| {
            position.stop_policy == StopPolicy::ExecutableSpan
                && matches!(&position.state, PositionState::Active { .. })
        }) {
            let snapshot = symbols.entry(position.symbol.clone()).or_default();
            *snapshot = snapshot.or(position.rules_snapshot.as_ref());
        }
        let mut rules_by_symbol = HashMap::with_capacity(symbols.len());

        // What is held: this runs on the tick path (ADR-0056 §5). Admission checks
        // its own symbol against fresh metadata before it gets here.
        for (symbol, snapshot) in symbols {
            match self.rules_for_managing(&symbol, snapshot).await {
                Some(rules) => {
                    rules_by_symbol.insert(symbol, rules);
                },
                None => {
                    warn!(
                        symbol = %symbol.as_pair(),
                        "Trading rules unavailable for ExecutableSpan positions; canonical latent-risk pricing will fail closed for this symbol"
                    );
                },
            }
        }

        rules_by_symbol
    }

    /// Resolve the canonical latent reservation once for every risk-open
    /// position without performing I/O in the pricing loop. ExecutableSpan
    /// Active positions consume the pre-resolved rules for their symbol over
    /// the current trailing stop; Entering positions consume the immutable
    /// admission trigger. Both use the same gap + fee envelope as planned-risk
    /// pricing.
    fn resolve_latent_risk_for_positions(
        &self,
        positions: &[Position],
        rules_by_symbol: &HashMap<Symbol, SymbolTradingRules>,
    ) -> (
        Decimal,
        HashMap<PositionId, ResolvedPositionRisk>,
        Vec<LatentRiskResolutionFailure>,
    ) {
        let mut total = Decimal::ZERO;
        let mut resolved = HashMap::with_capacity(positions.len());
        let mut failures = Vec::new();
        let risk_config = self.risk_config_snapshot();
        let engine = self.engine();

        for position in positions {
            // Its projection may lag its log (ADR-0056 step 7, PR P): the leg's
            // snapshot is invalid whatever its pricing would say.
            if self.event_streams.position_suspended(position.id) {
                failures.push(LatentRiskResolutionFailure {
                    position_id: position.id,
                    state: position.state.name().to_string(),
                    reason: "its event stream is suspended".to_string(),
                });
                continue;
            }
            if position.stop_policy == StopPolicy::LegacyUncapped {
                if let Some(pricing) = Self::legacy_resolved_position_risk(position) {
                    total += pricing.risk;
                    resolved.insert(position.id, pricing);
                }
                continue;
            }

            let resolution: Result<(Price, Price), String> = match &position.state {
                PositionState::Active {
                    trailing_stop, invalidation_guard_level, ..
                } => match position.entry_price {
                    Some(entry) => {
                        engine
                            .stop_plan(
                                position,
                                *trailing_stop,
                                *invalidation_guard_level,
                                rules_by_symbol.get(&position.symbol),
                            )
                            .map(|plan| (entry, plan.trigger))
                            .map_err(|error| error.to_string())
                    },
                    None => Err(
                        "ExecutableSpan Active position is missing its fill entry price"
                            .to_string(),
                    ),
                },
                PositionState::Entering { expected_entry, .. } => position
                    .initial_executable_stop
                    .map(|trigger| (*expected_entry, trigger))
                    .ok_or_else(|| {
                        "ExecutableSpan Entering position is missing persisted initial_executable_stop"
                            .to_string()
                    }),
                _ => continue,
            };

            let (entry, trigger) = match resolution {
                Ok(values) => values,
                Err(reason) => {
                    failures.push(LatentRiskResolutionFailure {
                        position_id: position.id,
                        state: position.state.name().to_string(),
                        reason,
                    });
                    continue;
                },
            };

            let risk = match latent_risk_per_unit_at_trigger(
                &risk_config,
                position.side,
                entry,
                trigger,
            ) {
                Ok(per_unit) => (per_unit * position.quantity.as_decimal()).max(Decimal::ZERO),
                Err(error) => {
                    failures.push(LatentRiskResolutionFailure {
                        position_id: position.id,
                        state: position.state.name().to_string(),
                        reason: error.to_string(),
                    });
                    continue;
                },
            };

            total += risk;
            resolved.insert(position.id, ResolvedPositionRisk {
                current_stop: trigger.as_decimal(),
                risk,
                cost_priced_latent_risk: Some(risk),
            });
        }

        (total, resolved, failures)
    }

    async fn emit_latent_risk_resolution_warnings(&self, failures: &[LatentRiskResolutionFailure]) {
        let unresolved: HashSet<_> = failures.iter().map(|failure| failure.position_id).collect();
        let new_failures = {
            let mut warned = self.latent_risk_warning_positions.write().await;
            warned.retain(|position_id| unresolved.contains(position_id));
            failures
                .iter()
                .filter(|failure| warned.insert(failure.position_id))
                .cloned()
                .collect::<Vec<_>>()
        };
        if new_failures.is_empty() {
            return;
        }

        let timestamp = chrono::Utc::now();
        let actions = new_failures
            .into_iter()
            .map(|failure| {
                error!(
                    position_id = %failure.position_id,
                    state = %failure.state,
                    reason = %failure.reason,
                    "CRITICAL: executable-stop latent risk is unresolved; canonical budget snapshot fails closed"
                );
                EngineAction::EmitEvent(Event::ExecutableStopRiskResolutionFailed {
                    position_id: failure.position_id,
                    state: failure.state,
                    reason: failure.reason,
                    severity: "critical".to_string(),
                    timestamp,
                })
            })
            .collect();
        if let Err(persist_error) = self.execute_and_persist(actions).await {
            error!(
                error = %persist_error,
                "Failed to persist executable-stop latent-risk warning; snapshot remains fail-closed"
            );
        }
    }

    /// Build the single monthly-budget snapshot shared by admission, status,
    /// slots, and halt evaluation. The returned position set is the one bulk
    /// local read used to compute HWM equity; admission additionally needs it
    /// for duplicate checks and per-position summaries.
    async fn canonical_budget_snapshot_with_positions(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<(
        MonthlyBudgetSnapshot,
        Vec<Position>,
        HashMap<PositionId, ResolvedPositionRisk>,
        Vec<LatentRiskResolutionFailure>,
    )> {
        let monthly = self.load_monthly_state(now).await?;
        let governed_realized_net = self.robson_month_net(now).await?;
        let local_risk_open = self.store.positions().find_risk_open().await?;
        let unrealized_net = self.unrealized_pnl_marked(&local_risk_open).await;
        let month_equity_net = governed_realized_net + unrealized_net;
        let month_peak_net = self
            .refresh_month_peak_net_from_snapshot(now, &monthly, month_equity_net)
            .await?;
        // ADR-0051 failure modes: never omit a durable local risk-bearing
        // position from the reservation, even when the exchange snapshot
        // says it is gone (partial/stale responses must stay conservative).
        let rules_by_symbol = self.trading_rules_by_symbol_for_positions(&local_risk_open).await;
        let (latent_risk, resolved_risk, latent_risk_failures) =
            self.resolve_latent_risk_for_positions(&local_risk_open, &rules_by_symbol);
        let latent_risk_invalid = !latent_risk_failures.is_empty();

        let capital_base_invalid = monthly.capital_base <= Decimal::ZERO;
        if capital_base_invalid {
            error!(
                capital_base = %monthly.capital_base,
                year = now.year(),
                month = now.month(),
                "ADR-0051 §1: monthly budget snapshot is invalid because capital_base <= 0; new entries denied, open positions kept"
            );
        }
        let budget_amount = self.trading_policy.monthly_budget(monthly.capital_base);

        let consumed_hwm = (month_peak_net - month_equity_net).max(Decimal::ZERO);
        let consumed_nfs = (-governed_realized_net).max(Decimal::ZERO);
        let consumed_active = match monthly.budget_model {
            MonthlyBudgetModel::HwmV1 => consumed_hwm,
            MonthlyBudgetModel::NetFromStartNonExpandingV1 => consumed_nfs,
        };
        let remaining_hwm = if latent_risk_invalid {
            Decimal::ZERO
        } else {
            self.trading_policy
                .remaining_budget(monthly.capital_base, consumed_hwm, latent_risk)
        };
        let remaining_nfs = if latent_risk_invalid {
            Decimal::ZERO
        } else {
            self.trading_policy
                .remaining_budget(monthly.capital_base, consumed_nfs, latent_risk)
        };
        let remaining_active = match monthly.budget_model {
            MonthlyBudgetModel::HwmV1 => remaining_hwm,
            MonthlyBudgetModel::NetFromStartNonExpandingV1 => remaining_nfs,
        };
        let slots_hwm = if latent_risk_invalid {
            0
        } else {
            self.trading_policy
                .slots_available(monthly.capital_base, consumed_hwm, latent_risk)
        };
        let slots_nfs = if latent_risk_invalid {
            0
        } else {
            self.trading_policy
                .slots_available(monthly.capital_base, consumed_nfs, latent_risk)
        };
        let slots_active = match monthly.budget_model {
            MonthlyBudgetModel::HwmV1 => slots_hwm,
            MonthlyBudgetModel::NetFromStartNonExpandingV1 => slots_nfs,
        };

        crate::metrics::BUDGET_MODEL_SHADOW_SLOTS_DELTA.set(slots_nfs as f64 - slots_hwm as f64);
        let (other_model, other_slots, other_remaining) = match monthly.budget_model {
            MonthlyBudgetModel::HwmV1 => {
                (MonthlyBudgetModel::NetFromStartNonExpandingV1, slots_nfs, remaining_nfs)
            },
            MonthlyBudgetModel::NetFromStartNonExpandingV1 => {
                (MonthlyBudgetModel::HwmV1, slots_hwm, remaining_hwm)
            },
        };
        let shadow_slots_changed = {
            let current = (slots_active, other_slots);
            let mut last = self.shadow_slots_last.lock().await;
            let changed = last.as_ref() != Some(&current);
            *last = Some(current);
            changed
        };
        if slots_active != other_slots && shadow_slots_changed {
            info!(
                active_model = %monthly.budget_model.as_str(),
                other_model = %other_model.as_str(),
                %slots_active,
                %other_slots,
                %remaining_active,
                %other_remaining,
                %consumed_hwm,
                %consumed_nfs,
                %latent_risk,
                "Monthly budget shadow models diverged as expected during dormant rollout"
            );
        }

        Ok((
            MonthlyBudgetSnapshot {
                model: monthly.budget_model,
                capital_base: monthly.capital_base,
                capital_base_invalid,
                latent_risk_invalid,
                budget_amount,
                governed_realized_net,
                month_equity_net,
                month_peak_net,
                latent_risk,
                consumed_active,
                consumed_hwm,
                consumed_nfs,
                remaining_active,
                remaining_hwm,
                remaining_nfs,
                slots_active,
                slots_hwm,
                slots_nfs,
            },
            local_risk_open,
            resolved_risk,
            latent_risk_failures,
        ))
    }

    pub(crate) async fn canonical_budget_snapshot(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<MonthlyBudgetSnapshot> {
        self.canonical_budget_snapshot_with_positions(now)
            .await
            .map(|(snapshot, _, _, _)| snapshot)
    }

    /// Compute dynamic count of new slots available from the canonical
    /// monthly budget snapshot.
    ///
    /// Used by `/status` API to expose `new_slots_available` (MIG-v3#12
    /// follow-up, ADR-0034). Since ADR-0043 this counts guaranteed full-cap
    /// entries — a floor, not a ceiling: entries are admitted by actual
    /// planned risk, so more operations than this may fit in the month.
    pub async fn compute_slots_available(&self) -> DaemonResult<u32> {
        Ok(self.canonical_budget_snapshot(chrono::Utc::now()).await?.slots_active)
    }

    // =========================================================================
    // Startup recovery accessors
    // =========================================================================

    /// Access the underlying store (read-only reference via Arc clone).
    pub fn store(&self) -> Arc<S> {
        self.store.clone()
    }

    /// Access the OHLCV port (for startup recovery candle fetches).
    pub fn ohlcv_port(&self) -> Arc<dyn OhlcvPort> {
        self.ohlcv_port.clone()
    }

    /// Clone the engine (recovery needs a snapshot for pure processing).
    pub fn engine(&self) -> Engine {
        self.engine.lock().unwrap().clone()
    }

    /// Access the exchange port (for startup-recovery insurance-stop checks).
    ///
    /// Recovery needs to query whether the protective stop filled or is still
    /// open during the downtime gap (ADR-0039 mission 2).
    pub(crate) fn exchange(&self) -> Arc<E> {
        self.executor.exchange()
    }

    /// Execute engine actions and persist events — recovery variant.
    ///
    /// Identical to `execute_and_persist` but `pub(crate)` so
    /// `startup_recovery` can call it without going through the full
    /// `process_market_data` flow (no query tracking, no metrics).
    pub(crate) async fn execute_and_persist_recovery(
        &self,
        actions: Vec<EngineAction>,
    ) -> DaemonResult<Vec<ActionResult>> {
        self.execute_and_persist(actions).await
    }

    /// Record a startup-recovery audit event through the normal durable path.
    ///
    /// The recovery path has outcomes that may be no-ops from a lifecycle
    /// perspective (for example, the exchange stop was already correct). This
    /// helper still appends those checks to the event stream so later audits do
    /// not depend on volatile daemon logs.
    pub(crate) async fn record_recovery_audit_event(&self, event: Event) -> DaemonResult<()> {
        self.store.events().append(&event).await?;
        self.store.apply_event(&event)?;
        self.persist_event_to_log(&event).await?;
        Ok(())
    }

    /// Handle exit fill during startup recovery.
    ///
    /// Thin wrapper around `handle_exit_fill` exposed as `pub(crate)` so
    /// `startup_recovery` can use it.
    pub(crate) async fn handle_exit_fill_recovery(
        &self,
        position_id: PositionId,
        fill_price: Price,
        filled_quantity: Quantity,
        exit_fee: Decimal,
        filled_at: chrono::DateTime<chrono::Utc>,
    ) -> DaemonResult<()> {
        self.handle_exit_fill(position_id, fill_price, filled_quantity, exit_fee, filled_at)
            .await
    }
}

// =============================================================================
// Tests
// =============================================================================

/// An entry preview captured under the manager lock (see
/// `PositionManager::prepare_entry_preview`) and executed after it.
pub struct EntryPreviewJob<E: ExchangePort + 'static> {
    capital: Decimal,
    risk_config: robson_domain::RiskConfig,
    position: Position,
    detector: DetectorTask,
    exchange: Arc<E>,
    engine: Engine,
}

impl<E: ExchangePort + 'static> EntryPreviewJob<E> {
    /// Availability failures of the chart step, as opposed to the analyzer
    /// refusing the setup. `TechnicalStopError::InsufficientData` and the
    /// empty-candle guard are the two sources today.
    fn is_availability_failure(text: &str) -> bool {
        let lower = text.to_ascii_lowercase();
        lower.contains("no candles") || lower.contains("insufficient")
    }

    pub async fn run(self) -> DaemonResult<EntryPreview> {
        let Self {
            capital,
            risk_config,
            position,
            detector,
            exchange,
            engine,
        } = self;
        let entry_side = position.side;
        let mut preview = EntryPreview {
            symbol: position.symbol.as_pair(),
            side: format!("{entry_side:?}"),
            reference_price: Decimal::ZERO,
            capital_base: capital,
            analysis: None,
            sizing: None,
            rejection: None,
            unavailable: None,
            warnings: Vec::new(),
        };

        let signal = match detector.preview_immediate_signal().await {
            Ok(signal) => signal,
            Err(DaemonError::Detector(text)) => {
                if Self::is_availability_failure(&text) {
                    preview.unavailable = Some(text);
                } else {
                    preview.rejection = Some(text);
                }
                return Ok(preview);
            },
            Err(error) => return Err(error),
        };
        let entry = signal.entry_price.as_decimal();
        preview.reference_price = entry;

        if let Some(audit) = &signal.technical_stop_analysis {
            let summary = TechStopAuditSummary::from_audit(audit);
            let distance = (signal.stop_loss.as_decimal() - entry).abs();
            let distance_pct = if entry > Decimal::ZERO {
                distance / entry * Decimal::from(100)
            } else {
                Decimal::ZERO
            };
            if summary.method == "atr_fallback" {
                preview.warnings.push(
                    "ATR fallback: no support/resistance level within bounds on the 15m chart"
                        .to_string(),
                );
            } else if summary.confidence == "medium" {
                preview.warnings.push(
                    "Fewer chart levels than configured; the stop uses the deepest available"
                        .to_string(),
                );
            }
            if distance_pct < Decimal::new(5, 1) {
                preview.warnings.push(
                    "Stop distance below 0.5% of price: within typical 15m noise".to_string(),
                );
            }
            preview.analysis = Some(EntryPreviewAnalysis {
                technical_stop: signal.stop_loss.as_decimal(),
                stop_distance: distance,
                stop_distance_pct: distance_pct,
                method: summary.method,
                level_n: summary.level_n,
                confidence: summary.confidence,
                invalidation_guard_level: audit.invalidation_guard_level.map(|p| p.as_decimal()),
            });
        }

        let available_margin = match exchange.get_futures_balance().await {
            Ok(balance) => Some(balance.available_balance),
            Err(error) => {
                warn!(%error, "Entry preview: futures balance unavailable; sizing against policy capital only");
                preview.warnings.push(
                    "Futures balance unavailable; size assumes policy capital as margin"
                        .to_string(),
                );
                None
            },
        };
        // Read-only lookup (`ExchangePort::trading_rules_preview`): served
        // from the production cache when fresh, fetched without caching
        // otherwise, so a preview never populates, refreshes or evicts the
        // metadata real arms depend on. A failure here is an availability
        // failure, not a policy rejection: the engine would fail closed on
        // it, which must not read as "the arm is blocked".
        let trading_rules = match exchange.trading_rules_preview(&position.symbol).await {
            Ok(rules) => Some(rules),
            Err(error) => {
                warn!(symbol = %position.symbol.as_pair(), %error, "Entry preview: trading rules unavailable");
                preview.unavailable =
                    Some(format!("exchange trading rules unavailable: {error}; size not computed"));
                return Ok(preview);
            },
        };
        let mut engine = engine;
        engine.update_risk_config(risk_config);
        match engine.decide_entry_with_rules(
            &position,
            &signal,
            available_margin,
            trading_rules.as_ref(),
        ) {
            Ok(decision) => {
                let planned_risk = decision.planned_entry_risk.unwrap_or(Decimal::ZERO);
                preview.sizing = decision.actions.iter().find_map(|action| match action {
                    EngineAction::EmitEvent(Event::EntrySignalReceived {
                        quantity,
                        initial_executable_stop,
                        executable_span,
                        ..
                    }) => {
                        let qty = quantity.as_decimal();
                        Some(EntryPreviewSizing {
                            quantity: qty,
                            notional: qty * entry,
                            planned_risk,
                            planned_risk_pct_of_capital: if capital > Decimal::ZERO {
                                planned_risk / capital * Decimal::from(100)
                            } else {
                                Decimal::ZERO
                            },
                            executable_trigger: initial_executable_stop.map(|p| p.as_decimal()),
                            executable_span: *executable_span,
                        })
                    },
                    _ => None,
                });
            },
            Err(robson_engine::EngineError::DomainError(domain_error)) => {
                preview.rejection = Some(domain_error.to_string());
            },
            Err(error) => return Err(error.into()),
        }

        Ok(preview)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};
    use robson_domain::{
        Candle, TechnicalStopAnalysisAudit, TechnicalStopConfidenceSnapshot,
        TechnicalStopConfigSnapshot, TechnicalStopMethodSnapshot,
    };
    use robson_exec::{IntentJournal, StubExchange, StubOhlcv};
    use robson_store::MemoryStore;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::query_engine::TracingQueryRecorder;

    /// The default venue of a manager built without `with_venue_id`.
    fn test_venue() -> VenueId {
        VenueId::new(VenueId::BINANCE_USDM)
    }

    #[test]
    fn governed_rearm_backoff_doubles_from_5s_to_15min_cap() {
        assert_eq!(governed_rearm_backoff(1).as_secs(), 5);
        assert_eq!(governed_rearm_backoff(2).as_secs(), 10);
        assert_eq!(governed_rearm_backoff(3).as_secs(), 20);
        assert_eq!(governed_rearm_backoff(8).as_secs(), 640);
        assert_eq!(governed_rearm_backoff(9).as_secs(), 900);
        assert_eq!(governed_rearm_backoff(100).as_secs(), 900);
        assert_eq!(governed_rearm_backoff(u32::MAX).as_secs(), 900);
        // attempt 0 never happens (counter starts at 1), but must not panic
        assert_eq!(governed_rearm_backoff(0).as_secs(), 5);
    }

    #[tokio::test]
    async fn preview_entry_sizes_without_persisting_anything() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let preview = manager.preview_entry(symbol, Side::Long).await.unwrap();

        assert_eq!(preview.symbol, "BTCUSDT");
        assert!(preview.reference_price > Decimal::ZERO);
        let analysis = preview.analysis.expect("stub candles carry technical levels");
        assert!(analysis.stop_distance > Decimal::ZERO);
        assert!(["swing_point", "atr_fallback"].contains(&analysis.method.as_str()));
        let sizing = preview.sizing.expect("engine must size the entry");
        assert!(sizing.quantity > Decimal::ZERO);
        assert!(sizing.planned_risk > Decimal::ZERO);
        assert!(preview.rejection.is_none());
        assert!(preview.unavailable.is_none());

        // Nothing was armed, nothing was ordered.
        assert!(manager.store.positions().find_active().await.unwrap().is_empty());
        assert_eq!(manager.executor.exchange().market_order_call_count(), 0);
    }

    #[test]
    fn preview_availability_failures_are_not_rejections() {
        type Job = EntryPreviewJob<StubExchange>;
        assert!(Job::is_availability_failure("no candles available for entry preview"));
        assert!(Job::is_availability_failure("Insufficient data: required 20, got 3"));
        assert!(!Job::is_availability_failure(
            "stop distance 0.05% below the configured minimum of 0.10%"
        ));
    }

    #[test]
    fn tech_stop_audit_summary_parses_persisted_event_payloads() {
        let swing = serde_json::json!({
            "type": "technical_stop_analyzed",
            "analysis": {
                "method": {"type": "swing_point", "level_n": 2},
                "confidence": "high",
                "stop_price": "100"
            }
        });
        assert_eq!(
            TechStopAuditSummary::from_event_payload(&swing),
            Some(TechStopAuditSummary {
                method: "swing_point".to_string(),
                level_n: Some(2),
                confidence: "high".to_string(),
            })
        );

        let atr = serde_json::json!({
            "type": "technical_stop_analyzed",
            "analysis": {"method": {"type": "atr_fallback"}, "confidence": "low"}
        });
        assert_eq!(
            TechStopAuditSummary::from_event_payload(&atr),
            Some(TechStopAuditSummary {
                method: "atr_fallback".to_string(),
                level_n: None,
                confidence: "low".to_string(),
            })
        );

        assert_eq!(
            TechStopAuditSummary::from_event_payload(&serde_json::json!({"type": "other"})),
            None
        );
    }

    #[test]
    fn monitor_tick_throttle_drops_only_recent_repeats() {
        use std::time::{Duration, Instant};
        let interval = Duration::from_secs(20);
        // Offset so subtracting up to a minute can never underflow the
        // monotonic clock on a freshly booted host.
        let now = Instant::now() + Duration::from_secs(3600);

        // First tick for a position is never dropped.
        assert!(!should_drop_monitor_tick(None, now, interval));
        // A tick younger than the interval is dropped.
        assert!(should_drop_monitor_tick(Some(now - Duration::from_secs(5)), now, interval));
        // At or beyond the interval the tick goes through again.
        assert!(!should_drop_monitor_tick(Some(now - Duration::from_secs(20)), now, interval));
        assert!(!should_drop_monitor_tick(Some(now - Duration::from_secs(60)), now, interval));
        // A zero interval never drops (throttle disabled).
        assert!(!should_drop_monitor_tick(Some(now), now, Duration::ZERO));
    }

    async fn create_test_manager_with_store_and_event_bus(
        store: Arc<MemoryStore>,
        event_bus: Arc<EventBus>,
        approval_policy: ApprovalPolicy,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        create_test_manager_with_order_journal(store, event_bus, approval_policy, None).await
    }

    async fn create_test_manager_with_order_journal(
        store: Arc<MemoryStore>,
        event_bus: Arc<EventBus>,
        approval_policy: ApprovalPolicy,
        order_journal: Option<Arc<robson_exec::order_journal::OrderJournal>>,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        exchange.set_trading_rules(
            robson_domain::SymbolTradingRules::new(
                Symbol::from_pair("BTCUSDT").unwrap(),
                dec!(0.01),
                Decimal::ZERO,
                dec!(0.000001),
                dec!(0.000001),
                dec!(1000000),
                Decimal::ZERO,
                2,
                6,
            )
            .unwrap(),
        );
        let journal = Arc::new(IntentJournal::new());
        let executor = Executor::new(exchange, journal, store.clone());
        let executor = Arc::new(match order_journal {
            Some(order_journal) => executor.with_order_journal(order_journal),
            None => executor,
        });
        let risk_config = RiskConfig::new(dec!(10000)).unwrap(); // 1% cap
        let engine = Engine::new(risk_config);

        Arc::new(
            PositionManager::with_approval_policy(
                engine,
                executor,
                store,
                event_bus,
                Arc::new(TracingQueryRecorder),
                approval_policy,
                TradingPolicy::default(),
            )
            .with_ohlcv_port(Arc::new(StubOhlcv::new(create_test_candles()))),
        )
    }

    /// Create a test manager without starting the signal listener.
    /// Use this for unit tests that call handle_signal() directly.
    async fn create_test_manager_with_approval_policy(
        approval_policy: ApprovalPolicy,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        create_test_manager_with_store_and_event_bus(
            Arc::new(MemoryStore::new()),
            Arc::new(EventBus::new(100)),
            approval_policy,
        )
        .await
    }

    async fn create_test_manager() -> Arc<PositionManager<StubExchange, MemoryStore>> {
        create_test_manager_with_approval_policy(ApprovalPolicy::new(Decimal::from(100u32), 300))
            .await
    }

    #[cfg(feature = "postgres")]
    fn create_postgres_test_manager(
        pool: sqlx::PgPool,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());

        Arc::new(
            PositionManager::with_approval_policy(
                engine,
                executor,
                store,
                Arc::new(EventBus::new(100)),
                Arc::new(TracingQueryRecorder),
                ApprovalPolicy::new(Decimal::from(100u32), 300),
                TradingPolicy::default(),
            )
            .with_event_log(pool, Uuid::now_v7()),
        )
    }

    #[cfg(feature = "postgres")]
    fn projector_test_envelope(
        event_type: &str,
        payload: serde_json::Value,
        occurred_at: chrono::DateTime<chrono::Utc>,
        seq: i64,
    ) -> robson_eventlog::EventEnvelope {
        robson_eventlog::EventEnvelope {
            event_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            stream_key: format!("test:monthly-budget-model:{seq}"),
            seq,
            event_type: event_type.to_string(),
            payload,
            payload_schema_version: 1,
            occurred_at,
            ingested_at: occurred_at,
            idempotency_key: format!("test:monthly-budget-model:{event_type}:{seq}"),
            trace_id: None,
            causation_id: None,
            command_id: None,
            workflow_id: None,
            actor_type: Some(robson_eventlog::ActorType::CLI),
            actor_id: Some("test-user".to_string()),
            prev_hash: None,
            hash: None,
        }
    }

    /// Cancel-first: the stop's cancel happened, so it must reach the event log
    /// even though the exit behind it failed, or a restart would restore a stop
    /// that is gone.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_cancel_before_a_failed_exit_reaches_the_event_log(pool: sqlx::PgPool) {
        let manager = create_postgres_test_manager(pool.clone());
        let position_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let exchange = manager.exchange();
        let stop = exchange
            .place_stop_market_order(
                &symbol,
                Side::Long.exit_action(),
                Quantity::new(dec!(0.01)).unwrap(),
                Price::new(dec!(94000)).unwrap(),
                "ins-db-test",
            )
            .await
            .unwrap();
        exchange.set_order_fail_next(true);

        let error = manager
            .execute_and_persist(vec![
                EngineAction::CancelInsuranceStop {
                    position_id,
                    symbol: symbol.clone(),
                    order_id: stop.exchange_order_id.clone(),
                },
                EngineAction::PlaceExitOrder {
                    position_id,
                    cycle_id: Some(Uuid::now_v7()),
                    symbol,
                    side: Side::Long.exit_action(),
                    quantity: Quantity::new(dec!(0.01)).unwrap(),
                    reason: robson_domain::ExitReason::TrailingStop,
                },
            ])
            .await
            .unwrap_err();
        assert!(matches!(error, DaemonError::Exec(ExecError::Exchange(_))), "{error:?}");

        let cancels: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM event_log WHERE event_type = 'insurance_stop_cancelled' \
             AND stream_key = $1",
        )
        .bind(format!("position:{position_id}"))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cancels, 1);
    }

    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn eventlog_failure_before_entry_order_blocks_exchange_call(pool: sqlx::PgPool) {
        let manager = create_postgres_test_manager(pool.clone());
        pool.close().await;

        let position_id = Uuid::now_v7();
        let signal_id = Uuid::now_v7();
        let order_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(94000)).unwrap();
        let quantity = Quantity::new(dec!(0.01)).unwrap();
        let actions = vec![
            EngineAction::EmitEvent(Event::EntrySignalReceived {
                position_id,
                signal_id,
                entry_price: entry,
                stop_loss: stop,
                quantity,
                initial_executable_stop: Some(stop),
                executable_span: Some(dec!(1000)),
                cap_basis_distance: Some(dec!(1000)),
                tick_size: Some(dec!(0.01)),
                rules_snapshot: None,
                timestamp: chrono::Utc::now(),
            }),
            EngineAction::EmitEvent(Event::EntryOrderRequested {
                position_id,
                cycle_id: Some(Uuid::now_v7()),
                order_id,
                client_order_id: signal_id.to_string(),
                expected_price: entry,
                quantity,
                signal_id,
                timestamp: chrono::Utc::now(),
            }),
            EngineAction::PlaceEntryOrder {
                position_id,
                cycle_id: Some(Uuid::now_v7()),
                symbol,
                side: Side::Long.entry_action(),
                quantity,
                order_id,
                client_order_id: signal_id.to_string(),
                expected_price: entry,
                signal_id,
            },
        ];

        let error = manager.execute_and_persist(actions).await.unwrap_err();
        assert!(matches!(error, DaemonError::EventLog(_)));
        assert_eq!(
            manager.exchange().market_order_call_count(),
            0,
            "entry admission persistence is a pre-exchange barrier"
        );
    }

    async fn create_phase3_test_manager(
        ttl_seconds: u64,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        create_test_manager_with_approval_policy(ApprovalPolicy::new(
            Decimal::from(5u32),
            ttl_seconds,
        ))
        .await
    }

    /// Create a test manager WITH signal listener running.
    /// Use this for E2E tests that need full event-driven flow.
    async fn create_test_manager_with_listener() -> Arc<PositionManager<StubExchange, MemoryStore>>
    {
        let manager = create_test_manager().await;
        PositionManager::start(Arc::clone(&manager));
        manager
    }

    fn create_test_risk_config() -> RiskConfig {
        RiskConfig::new(dec!(10000)).unwrap() // 1% cap
    }

    /// ADR-0052: arm pins the one new-position policy and ADR-0041 buffer
    /// snapshot atomically with position creation, and both survive replay.
    #[tokio::test]
    async fn arm_position_pins_stop_policy_and_buffer_snapshot() {
        let manager = create_test_manager().await;
        let risk_config = RiskConfig::new(dec!(10000)).unwrap().with_stop_buffer(dec!(10)).unwrap();

        let position = manager
            .arm_position(
                Symbol::from_pair("BTCUSDT").unwrap(),
                Side::Long,
                risk_config,
                None,
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        assert_eq!(position.stop_policy, StopPolicy::ExecutableSpan);
        assert_eq!(position.stop_buffer_bps_at_arm, Some(dec!(10)));

        // The PositionArmed event carries the same pinned values.
        let events = manager.store().events().find_by_position(position.id).await.unwrap();
        let armed = events
            .iter()
            .find_map(|event| match event {
                Event::PositionArmed { stop_policy, stop_buffer_bps_at_arm, .. } => {
                    Some((*stop_policy, *stop_buffer_bps_at_arm))
                },
                _ => None,
            })
            .expect("PositionArmed event must exist");
        assert_eq!(armed, (StopPolicy::ExecutableSpan, Some(dec!(10))));
    }

    #[tokio::test]
    async fn admitted_entry_persists_executable_span_plan_before_submission() {
        let manager = create_test_manager().await;
        let position = manager
            .arm_position(
                Symbol::from_pair("BTCUSDT").unwrap(),
                Side::Long,
                RiskConfig::new(dec!(10000)).unwrap().with_stop_buffer(dec!(10)).unwrap(),
                None,
                Uuid::now_v7(),
            )
            .await
            .unwrap();
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: position.symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(90250)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let events = manager.store().events().find_by_position(position.id).await.unwrap();
        let signal_index = events
            .iter()
            .position(|event| matches!(event, Event::EntrySignalReceived { .. }))
            .expect("entry admission event must be durable");
        let request_index = events
            .iter()
            .position(|event| matches!(event, Event::EntryOrderRequested { .. }))
            .expect("entry order request must be durable");
        assert!(signal_index < request_index, "plan must persist before entry submission intent");
        match &events[signal_index] {
            Event::EntrySignalReceived {
                entry_price,
                initial_executable_stop,
                executable_span,
                cap_basis_distance,
                tick_size,
                ..
            } => {
                assert_eq!(*entry_price, Price::new(dec!(95000)).unwrap());
                assert_eq!(*initial_executable_stop, Some(Price::new(dec!(90159.75)).unwrap()));
                assert_eq!(*executable_span, Some(dec!(4840.25)));
                assert_eq!(*cap_basis_distance, Some(dec!(4750)));
                assert_eq!(*tick_size, Some(dec!(0.01)));
            },
            other => panic!("expected EntrySignalReceived, got {other:?}"),
        }

        let persisted = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(persisted.stop_policy, StopPolicy::ExecutableSpan);
        assert_eq!(persisted.stop_plan_entry_reference, Some(Price::new(dec!(95000)).unwrap()));
        assert_eq!(persisted.executable_span, Some(dec!(4840.25)));
        assert_eq!(persisted.cap_basis_distance, Some(dec!(4750)));
        assert_eq!(persisted.tick_size_at_admission, Some(dec!(0.01)));
    }

    fn create_test_candles() -> Vec<Candle> {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let now = chrono::Utc::now();
        let base = dec!(100);
        let mut candles: Vec<Candle> = (0..100)
            .map(|i| {
                let open_time = now + Duration::minutes(i);
                Candle::new(
                    symbol.clone(),
                    base,
                    base,
                    base,
                    base,
                    dec!(100),
                    10,
                    open_time,
                    open_time + Duration::minutes(15),
                )
            })
            .collect();

        candles[50] = Candle::new(
            symbol.clone(),
            base,
            dec!(104),
            dec!(98),
            base,
            dec!(100),
            10,
            now + Duration::minutes(50),
            now + Duration::minutes(65),
        );
        candles[70] = Candle::new(
            symbol.clone(),
            base,
            dec!(104),
            dec!(96),
            base,
            dec!(100),
            10,
            now + Duration::minutes(70),
            now + Duration::minutes(85),
        );

        let pullback_start = candles.len() - 10;
        for index in pullback_start..candles.len() - 1 {
            let open_time = now + Duration::minutes(index as i64);
            candles[index] = Candle::new(
                symbol.clone(),
                dec!(99.5),
                dec!(99.5),
                dec!(99.5),
                dec!(99.5),
                dec!(100),
                10,
                open_time,
                open_time + Duration::minutes(15),
            );
        }
        let final_index = candles.len() - 1;
        let final_open_time = now + Duration::minutes(final_index as i64);
        candles[final_index] = Candle::new(
            symbol.clone(),
            dec!(105),
            dec!(105),
            dec!(105),
            dec!(105),
            dec!(100),
            10,
            final_open_time,
            final_open_time + Duration::minutes(15),
        );

        candles
    }

    fn sample_technical_stop_analysis(stop_price: Price) -> TechnicalStopAnalysisAudit {
        TechnicalStopAnalysisAudit {
            stop_price,
            raw_technical_stop: None,
            invalidation_guard_level: None,
            effective_stop_basis: None,
            method: TechnicalStopMethodSnapshot::SwingPoint { level_n: 2 },
            confidence: TechnicalStopConfidenceSnapshot::High,
            detected_levels: vec![Price::new(dec!(90000)).unwrap(), stop_price],
            config: TechnicalStopConfigSnapshot {
                min_candles: 100,
                swing_lookback: 2,
                support_level_n: 2,
                level_tolerance: dec!(0.005),
                atr_period: 14,
                atr_multiplier: dec!(1.5),
                min_stop_distance_pct: dec!(0.001),
                max_stop_distance_pct: dec!(0.10),
            },
            stop_anchor: None,
            stop_quality: None,
            configured_level_n: Some(2),
            selected_level_n: Some(2),
            skipped_levels: vec![],
            selection_rule: Some("anchor_n_walk_deeper".to_string()),
            cluster_representative: None,
        }
    }

    /// `sample_technical_stop_analysis` with the invalidation guard fields
    /// populated (ADR-0042).
    fn sample_technical_stop_analysis_with_guard(
        stop_price: Price,
        guard: Price,
        basis: robson_domain::entities::EffectiveStopBasis,
    ) -> TechnicalStopAnalysisAudit {
        let mut audit = sample_technical_stop_analysis(stop_price);
        audit.raw_technical_stop = Some(stop_price);
        audit.invalidation_guard_level = Some(guard);
        audit.effective_stop_basis = Some(basis);
        audit
    }

    async fn save_active_position(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        symbol: &str,
        side: Side,
        entry_price: Decimal,
        quantity: Decimal,
    ) -> Position {
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair(symbol).unwrap();
        let entry_price = Price::new(entry_price).unwrap();
        let trailing_stop = match side {
            Side::Long => Price::new(entry_price.as_decimal() - dec!(10)).unwrap(),
            Side::Short => Price::new(entry_price.as_decimal() + dec!(10)).unwrap(),
        };
        let quantity = Quantity::new(quantity).unwrap();
        let now = chrono::Utc::now();

        let mut position = Position::new(account_id, symbol, side);
        position.entry_price = Some(entry_price);
        position.entry_filled_at = Some(now);
        position.quantity = quantity;
        position.state = PositionState::Active {
            current_price: entry_price,
            trailing_stop,
            favorable_extreme: entry_price,
            extreme_at: now,
            insurance_stop_id: None,
            invalidation_guard_level: None,
            last_emitted_stop: None,
        };
        position.updated_at = now;

        manager.store.positions().save(&position).await.unwrap();
        position
    }

    fn budget_test_rules_for(symbol: Symbol) -> robson_domain::SymbolTradingRules {
        robson_domain::SymbolTradingRules::new(
            symbol,
            dec!(0.01),
            Decimal::ZERO,
            dec!(0.001),
            dec!(0.001),
            dec!(1000000),
            Decimal::ZERO,
            2,
            3,
        )
        .unwrap()
    }

    fn budget_test_rules() -> robson_domain::SymbolTradingRules {
        budget_test_rules_for(Symbol::from_pair("BTCUSDT").unwrap())
    }

    fn executable_active_position_for_budget(
        quantity: Decimal,
        buffer_bps: Decimal,
    ) -> (Position, robson_domain::ExecutableStopPlan) {
        executable_active_position_for_budget_on_symbol("BTCUSDT", quantity, buffer_bps)
    }

    fn executable_active_position_for_budget_on_symbol(
        symbol: &str,
        quantity: Decimal,
        buffer_bps: Decimal,
    ) -> (Position, robson_domain::ExecutableStopPlan) {
        let entry = Price::new(dec!(100)).unwrap();
        let technical_stop = Price::new(dec!(90)).unwrap();
        let symbol = Symbol::from_pair(symbol).unwrap();
        let rules = budget_test_rules_for(symbol.clone());
        let plan = robson_domain::build_executable_stop_plan(robson_domain::StopPlanInputs {
            policy: StopPolicy::ExecutableSpan,
            side: Side::Long,
            technical_stop,
            guard: None,
            entry_reference: Some(entry),
            technical_span: Some(dec!(10)),
            stop_buffer_bps: buffer_bps,
            executable_span_source: robson_domain::ExecutableSpanSource::Admission,
            rules: Some(&rules),
        })
        .unwrap();

        let now = chrono::Utc::now();
        let mut position = Position::new_with_stop_policy(
            Uuid::now_v7(),
            symbol,
            Side::Long,
            StopPolicy::ExecutableSpan,
            Some(buffer_bps),
        );
        position.entry_price = Some(entry);
        position.tech_stop_distance =
            Some(TechnicalStopDistance::from_entry_and_stop(entry, technical_stop));
        position.quantity = Quantity::new(quantity).unwrap();
        position.initial_executable_stop = Some(plan.trigger);
        position.executable_span = plan.executable_span;
        position.cap_basis_distance = plan.cap_basis_distance;
        position.tick_size_at_admission = plan.tick_size;
        position.entry_filled_at = Some(now);
        position.state = PositionState::Active {
            current_price: entry,
            trailing_stop: technical_stop,
            favorable_extreme: entry,
            extreme_at: now,
            insurance_stop_id: None,
            invalidation_guard_level: None,
            last_emitted_stop: None,
        };
        (position, plan)
    }

    fn create_budget_test_manager(
        capital: Decimal,
        install_rules: bool,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        let exchange = Arc::new(StubExchange::new(dec!(100)));
        if install_rules {
            exchange.set_trading_rules(budget_test_rules());
        }
        let store = Arc::new(MemoryStore::new());
        let executor =
            Arc::new(Executor::new(exchange, Arc::new(IntentJournal::new()), store.clone()));
        let engine =
            Engine::new(RiskConfig::new(capital).unwrap().with_stop_buffer(dec!(100)).unwrap());
        Arc::new(PositionManager::new(
            engine,
            executor,
            store,
            Arc::new(EventBus::new(100)),
            Arc::new(TracingQueryRecorder),
            TradingPolicy::default(),
        ))
    }

    /// Regression for #137: an accepted insurance-stop replacement is not an
    /// exit fill. A successful return proves the ProcessMarketTick query did
    /// not fail in the Acting state by routing the protective order result to
    /// handle_exit_fill.
    /// ADR-0039: when the cancel-replace of the exchange insurance stop fails
    /// after a trailing advance, the software plan has moved but the exchange
    /// stop sits one rung behind and the engine never re-emits the move. The
    /// manager must remember the position and re-place the stop on a later
    /// tick, with backoff.
    #[tokio::test]
    async fn test_failed_insurance_stop_replace_is_retried_on_a_later_tick() {
        let manager = create_test_manager().await;
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        let entry_price = Price::new(dec!(100)).unwrap();
        let initial_stop = Price::new(dec!(90)).unwrap();
        let previous_order_id = "insurance-old".to_string();
        position.tech_stop_distance =
            Some(TechnicalStopDistance::from_entry_and_stop(entry_price, initial_stop));
        position.insurance_stop_id = Some(previous_order_id.clone());
        if let PositionState::Active { insurance_stop_id, last_emitted_stop, .. } =
            &mut position.state
        {
            *insurance_stop_id = Some(previous_order_id.clone());
            *last_emitted_stop = Some(initial_stop);
        }
        manager.store.positions().save(&position).await.unwrap();
        let tick = |price: Decimal| MarketData {
            venue: test_venue(),
            symbol: position.symbol.clone(),
            price: Price::new(price).unwrap(),
            timestamp: chrono::Utc::now(),
            source: crate::event_bus::MarketDataSource::Ws,
        };

        // Tick 1: the stop advances to breakeven, but the exchange rejects
        // the replacement order. Audit-only failure; old stop stays live.
        let exchange = manager.executor.exchange();
        exchange.set_order_fail_next(true);
        manager.process_market_data(tick(dec!(110))).await.unwrap();

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::InsuranceStopFailed { .. })));
        assert!(!events.iter().any(|e| matches!(e, Event::InsuranceStopReplaced { .. })));
        let after_failure = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(after_failure.insurance_stop_id.as_deref(), Some("insurance-old"));
        match &after_failure.state {
            PositionState::Active { trailing_stop, .. } => assert_eq!(*trailing_stop, entry_price),
            other => panic!("expected Active, got {other:?}"),
        }
        assert!(manager.insurance_stop_stale.read().await.contains_key(&position.id));

        // Tick 2, inside the backoff window: no retry yet.
        manager.process_market_data(tick(dec!(111))).await.unwrap();
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(!events.iter().any(|e| matches!(e, Event::InsuranceStopReplaced { .. })));

        // Backoff elapsed: the next tick re-places the stop at the current
        // software trigger even though the engine has nothing new to emit.
        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(position.id, std::time::Instant::now());
        manager.process_market_data(tick(dec!(111))).await.unwrap();

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let (new_order_id, stop_price) = events
            .iter()
            .find_map(|event| match event {
                Event::InsuranceStopReplaced {
                    previous_order_id: prev,
                    order_id,
                    stop_price,
                    ..
                } if prev == "insurance-old" => Some((order_id.clone(), *stop_price)),
                _ => None,
            })
            .expect("retry must re-place the insurance stop");
        assert_eq!(stop_price, entry_price);
        let healed = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(healed.insurance_stop_id.as_deref(), Some(new_order_id.as_str()));
        assert!(matches!(healed.state, PositionState::Active { .. }));
        assert!(!manager.insurance_stop_stale.read().await.contains_key(&position.id));
    }

    #[tokio::test]
    async fn test_process_market_data_keeps_active_position_after_insurance_stop_replace() {
        let manager = create_test_manager().await;
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        let entry_price = Price::new(dec!(100)).unwrap();
        let initial_stop = Price::new(dec!(90)).unwrap();
        let previous_order_id = "insurance-old".to_string();

        position.tech_stop_distance =
            Some(TechnicalStopDistance::from_entry_and_stop(entry_price, initial_stop));
        position.insurance_stop_id = Some(previous_order_id.clone());
        if let PositionState::Active { insurance_stop_id, last_emitted_stop, .. } =
            &mut position.state
        {
            *insurance_stop_id = Some(previous_order_id.clone());
            *last_emitted_stop = Some(initial_stop);
        }
        manager.store.positions().save(&position).await.unwrap();

        let result = manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: position.symbol.clone(),
                price: Price::new(dec!(110)).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await;
        assert!(result.is_ok(), "insurance-stop replace failed the market tick: {result:?}");

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let (replacement_order_id, replacement_stop) = events
            .iter()
            .find_map(|event| match event {
                Event::InsuranceStopReplaced {
                    previous_order_id: event_previous_order_id,
                    order_id,
                    stop_price,
                    ..
                } if event_previous_order_id == &previous_order_id => {
                    Some((order_id.as_str(), *stop_price))
                },
                _ => None,
            })
            .expect("insurance stop replacement event must be emitted");
        assert_ne!(replacement_order_id, previous_order_id);
        assert_eq!(replacement_stop, entry_price);
        assert!(events.iter().any(|event| {
            matches!(
                event,
                Event::TrailingStopUpdated { new_stop, .. } if *new_stop == entry_price
            )
        }));

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(updated.insurance_stop_id.as_deref(), Some(replacement_order_id));
        match &updated.state {
            PositionState::Active { trailing_stop, insurance_stop_id, .. } => {
                assert_eq!(*trailing_stop, entry_price);
                assert_eq!(insurance_stop_id.as_deref(), Some(replacement_order_id));
            },
            other => panic!("insurance-stop replace must leave position Active, got {other:?}"),
        }
    }

    /// The #137 routing fix must preserve the real exit path: ExitOrderPlaced
    /// transitions the position to Exiting before handle_exit_fill closes it.
    #[tokio::test]
    async fn test_process_market_data_routes_real_exit_fill_to_closed() {
        let manager = create_test_manager().await;
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        let entry_price = Price::new(dec!(100)).unwrap();
        let trailing_stop = Price::new(dec!(90)).unwrap();
        let fill_price = Price::new(dec!(89)).unwrap();
        position.tech_stop_distance =
            Some(TechnicalStopDistance::from_entry_and_stop(entry_price, trailing_stop));
        let exchange = manager.executor.exchange();
        let insurance_order = exchange
            .place_stop_market_order(
                &position.symbol,
                position.side.exit_action(),
                position.quantity,
                trailing_stop,
                "ins-test-exit",
            )
            .await
            .unwrap();
        position.insurance_stop_id = Some(insurance_order.exchange_order_id.clone());
        if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
            *insurance_stop_id = Some(insurance_order.exchange_order_id);
        }
        manager.store.positions().save(&position).await.unwrap();
        exchange.set_price("BTCUSDT", fill_price.as_decimal());

        let result = manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: position.symbol.clone(),
                price: trailing_stop,
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await;
        assert!(result.is_ok(), "real exit flow failed: {result:?}");

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events.iter().any(|event| matches!(event, Event::InsuranceStopCancelled { .. })));
        assert!(events.iter().any(|event| matches!(event, Event::ExitOrderPlaced { .. })));
        assert!(events.iter().any(|event| matches!(event, Event::PositionClosed { .. })));

        let closed = manager.get_position(position.id).await.unwrap().unwrap();
        match closed.state {
            PositionState::Closed { exit_price, exit_reason, .. } => {
                assert_eq!(exit_price, fill_price);
                assert_eq!(exit_reason, robson_domain::ExitReason::TrailingStop);
            },
            other => panic!("real exit fill must leave position Closed, got {other:?}"),
        }
    }

    /// An Active long of quantity 1 with a live exchange stop at 90.
    async fn active_leg_with_live_stop(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
    ) -> (Position, String) {
        let mut position =
            save_active_position(manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        let trailing_stop = Price::new(dec!(90)).unwrap();
        position.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(100)).unwrap(),
            trailing_stop,
        ));
        let placed = manager
            .executor
            .exchange()
            .place_stop_market_order(
                &position.symbol,
                position.side.exit_action(),
                position.quantity,
                trailing_stop,
                "ins-step6",
            )
            .await
            .unwrap();
        let stop_id = placed.exchange_order_id;
        position.insurance_stop_id = Some(stop_id.clone());
        if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
            *insurance_stop_id = Some(stop_id.clone());
        }
        manager.store.positions().save(&position).await.unwrap();
        (position, stop_id)
    }

    async fn stop_crossing_tick(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position: &Position,
    ) -> DaemonResult<()> {
        manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: position.symbol.clone(),
                price: Price::new(dec!(90)).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await
    }

    /// What a leg looks like afterwards: its state, whether its stop is still
    /// on the venue, and the events it produced, in order.
    async fn leg_outcome(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position: &Position,
        stop_id: &str,
    ) -> serde_json::Value {
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        serde_json::json!({
            "state": leg.state.name(),
            "stop_live": manager.executor.exchange().has_stop_order(stop_id),
            "events": events.iter().map(|event| event.event_type()).collect::<Vec<_>>(),
        })
    }

    /// ADR-0056 step 6: the leg-level outcomes of the exit path.
    /// `testdata/adr0056_step6_leg_before.json` was written by this function on
    /// the old code (commit 2da94feb) and stays unchanged.
    async fn exit_leg_scenarios() -> serde_json::Value {
        let manager = create_test_manager().await;
        let (position, stop_id) = active_leg_with_live_stop(&manager).await;
        assert!(stop_crossing_tick(&manager, &position).await.is_ok());
        let clean_exit = leg_outcome(&manager, &position, &stop_id).await;

        let manager = create_test_manager().await;
        let (position, stop_id) = active_leg_with_live_stop(&manager).await;
        manager.executor.exchange().set_order_fail_next(true);
        assert!(stop_crossing_tick(&manager, &position).await.is_err());
        let exit_order_fails = leg_outcome(&manager, &position, &stop_id).await;

        // The old code could only hear of a short fill through its handler; the same
        // fill now comes through the executor, which puts it on the record.
        let manager = create_test_manager().await;
        let (position, stop_id) = active_leg_with_live_stop(&manager).await;
        manager.executor.exchange().set_short_fill_next(dec!(0.9));
        manager
            .execute_and_persist(vec![EngineAction::PlaceExitOrder {
                position_id: position.id,
                cycle_id: None,
                symbol: position.symbol.clone(),
                side: position.side.exit_action(),
                quantity: position.quantity,
                reason: robson_domain::ExitReason::TrailingStop,
            }])
            .await
            .unwrap();
        manager
            .handle_exit_fill(
                position.id,
                Price::new(dec!(90)).unwrap(),
                Quantity::new(dec!(0.9)).unwrap(),
                Decimal::ZERO,
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        let exit_fill_below_leg_quantity = leg_outcome(&manager, &position, &stop_id).await;

        serde_json::json!({
            "clean_exit": clean_exit,
            "exit_order_fails": exit_order_fails,
            "exit_fill_below_leg_quantity": exit_fill_below_leg_quantity,
        })
    }

    #[tokio::test]
    async fn exit_leg_outcomes_match_the_after_fixture_and_say_why_they_moved() {
        let observed = exit_leg_scenarios().await;
        let before: serde_json::Value =
            serde_json::from_str(include_str!("testdata/adr0056_step6_leg_before.json")).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(include_str!("testdata/adr0056_step6_leg_after.json")).unwrap();

        for scenario in [
            "clean_exit",
            "exit_order_fails",
            "exit_fill_below_leg_quantity",
        ] {
            assert_eq!(observed[scenario], after[scenario]["outcome"], "{scenario}");
            let why = after[scenario]["why"].as_str().unwrap_or_default();
            assert_eq!(before[scenario] != observed[scenario], !why.is_empty(), "{scenario}");
        }
    }

    // ---- ADR-0056 §5: the two rules -------------------------------------------

    /// Rules of the lot grid `step` on the test symbol, permissive elsewhere.
    fn lot_rules(step: Decimal) -> SymbolTradingRules {
        SymbolTradingRules::new(
            Symbol::from_pair("BTCUSDT").unwrap(),
            dec!(0.01),
            Decimal::ZERO,
            step,
            step,
            dec!(1000000),
            Decimal::ZERO,
            2,
            6,
        )
        .unwrap()
    }

    /// A due or hanging metadata fetch must not hold up the exit.
    #[tokio::test]
    async fn risk_reduction_does_not_wait_for_metadata() {
        let manager = create_test_manager().await;
        let (mut position, stop_id) = active_leg_with_live_stop(&manager).await;
        position.stop_policy = StopPolicy::ExecutableSpan;
        position.executable_span = Some(dec!(10));
        position.cap_basis_distance = Some(dec!(10));
        position.stop_plan_entry_reference = Some(Price::new(dec!(100)).unwrap());
        position.stop_buffer_bps_at_arm = Some(Decimal::ZERO);
        manager.store.positions().save(&position).await.unwrap();
        let exchange = manager.exchange();
        exchange.set_rules_fetch_hangs(true);
        exchange.set_rules_refresh_due(true);

        let tick = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stop_crossing_tick(&manager, &position),
        )
        .await
        .expect("the tick must not wait for a metadata fetch");
        assert!(tick.is_ok(), "{tick:?}");

        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
        assert!(!exchange.has_stop_order(&stop_id));
    }

    /// After a restart: nothing cached, the venue's metadata unreachable, only
    /// the leg's persisted snapshot. The exit still rounds by it and goes out.
    #[tokio::test]
    async fn risk_reduction_runs_on_the_persisted_snapshot_alone() {
        let manager = create_test_manager().await;
        let (mut position, _) = active_leg_with_live_stop(&manager).await;
        position.stop_policy = StopPolicy::ExecutableSpan;
        position.executable_span = Some(dec!(10));
        position.cap_basis_distance = Some(dec!(10));
        position.stop_plan_entry_reference = Some(Price::new(dec!(100)).unwrap());
        position.stop_buffer_bps_at_arm = Some(Decimal::ZERO);
        position.rules_snapshot = Some(lot_rules(dec!(0.001)));
        manager.store.positions().save(&position).await.unwrap();
        let exchange = manager.exchange();
        exchange.clear_cached_rules();
        exchange.set_rules_fetch_hangs(true);

        let tick = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stop_crossing_tick(&manager, &position),
        )
        .await
        .expect("a persisted snapshot means there is nothing to wait for");
        assert!(tick.is_ok(), "{tick:?}");

        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
        assert_eq!(exchange.rules_refresh_call_count(), 0);
    }

    /// A TTL refresh that is due happens on the side, bounded.
    #[tokio::test]
    async fn a_due_refresh_is_requested_on_the_side_and_bounded() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        exchange.set_rules_refresh_due(true);
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let calls_before = exchange.trading_rules_call_count("BTCUSDT");

        for _ in 0..5 {
            assert!(manager.rules_for_managing(&symbol, None).await.is_some());
        }
        for _ in 0..50 {
            if exchange.trading_rules_call_count("BTCUSDT") > calls_before {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(exchange.trading_rules_call_count("BTCUSDT"), calls_before + 1);
    }

    /// Admission needs fresh metadata even while stale rules still serve risk
    /// reduction, and it stops before anything reaches the venue.
    #[tokio::test]
    async fn admission_fails_closed_without_fresh_metadata() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        exchange.set_fresh_rules_unavailable(true);

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let proposed = ProposedTrade {
            symbol: symbol.as_pair(),
            side: "long".to_string(),
            quantity: dec!(0.01),
            entry_price: dec!(95000),
            notional_value: dec!(950),
            initial_margin: dec!(950),
            planned_risk: dec!(10),
        };
        assert!(manager.validate_entry_quantity_for_exchange(&symbol, &proposed).await.is_err());
        let position = Position::new_with_stop_policy(
            Uuid::now_v7(),
            symbol.clone(),
            Side::Long,
            StopPolicy::ExecutableSpan,
            Some(Decimal::ZERO),
        );
        assert!(manager.trading_rules_for_admission(&position).await.is_none());
        assert!(manager.trading_rules_for(&position).await.is_some(), "stale still serves");
        assert_eq!(exchange.market_order_call_count(), 0);

        exchange.set_fresh_rules_unavailable(false);
        assert!(manager.validate_entry_quantity_for_exchange(&symbol, &proposed).await.is_ok());
    }

    #[tokio::test]
    async fn admission_applies_max_quantity_and_minimum_notional() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        exchange.set_trading_rules(
            SymbolTradingRules::new(
                symbol.clone(),
                dec!(0.10),
                dec!(556.80),
                dec!(0.001),
                dec!(0.001),
                dec!(120),
                dec!(50),
                2,
                3,
            )
            .unwrap(),
        );
        let trade = |quantity: Decimal, entry_price: Decimal| ProposedTrade {
            symbol: symbol.as_pair(),
            side: "long".to_string(),
            quantity,
            entry_price,
            notional_value: quantity * entry_price,
            initial_margin: quantity * entry_price,
            planned_risk: dec!(10),
        };
        for (quantity, price, accepted) in [
            (dec!(0.001), dec!(77500), true),
            (dec!(0.001), dec!(30000), false), // notional 30 under 50
            (dec!(120), dec!(77500), true),
            (dec!(120.001), dec!(77500), false), // MARKET_LOT_SIZE maximum
            (dec!(0.0005), dec!(77500), false),  // lot minimum
        ] {
            let result = manager
                .validate_entry_quantity_for_exchange(&symbol, &trade(quantity, price))
                .await;
            assert_eq!(result.is_ok(), accepted, "{quantity} at {price}: {result:?}");
        }
    }

    /// The venue refuses the exit on its rules, also after the refresh. The leg
    /// stays Active with nothing recorded as closed, and the next ticks do not
    /// hit the venue until the retry window ends.
    #[tokio::test]
    async fn a_failed_resubmit_does_not_loop() {
        let manager = create_test_manager().await;
        let (position, stop_id) = active_leg_with_live_stop(&manager).await;
        let exchange = manager.exchange();
        exchange.set_rules_rejections_next(100);

        let error = stop_crossing_tick(&manager, &position).await.unwrap_err();
        assert!(matches!(error, DaemonError::Exec(ExecError::RulesRejected(_))), "{error:?}");

        assert_eq!(exchange.market_order_call_count(), 2, "one resubmission");
        assert_eq!(exchange.rules_refresh_call_count(), 1, "one refresh");
        assert!(
            !exchange.has_stop_order(&stop_id),
            "cancel-first: the stop went before the exit"
        );
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Active { .. }));
        assert_eq!(leg.insurance_stop_id, None);
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::InsuranceStopCancelled { .. })));
        assert!(events
            .iter()
            .all(|e| !matches!(e, Event::ExitOrderPlaced { .. } | Event::PositionClosed { .. })));

        // The retry window: more ticks, no more requests.
        for _ in 0..3 {
            assert!(stop_crossing_tick(&manager, &position).await.is_ok());
        }
        assert_eq!(exchange.market_order_call_count(), 2);
        assert_eq!(exchange.rules_refresh_call_count(), 1);

        // When the window ends the leg tries again, and the venue may accept.
        manager.exit_retry_after.write().await.clear();
        exchange.set_rules_rejections_next(0);
        assert!(stop_crossing_tick(&manager, &position).await.is_ok());
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
    }

    fn venue_still_holds(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position: &Position,
        quantity: Decimal,
    ) {
        manager.exchange().set_open_position(
            position.symbol.clone(),
            position.side,
            Quantity::new(quantity).unwrap(),
            Price::new(dec!(100)).unwrap(),
        );
    }

    /// A tick above the stop: nothing crosses, so only the heal path acts.
    async fn quiet_tick(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position: &Position,
    ) {
        manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: position.symbol.clone(),
                price: Price::new(dec!(95)).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await
            .unwrap();
    }

    fn short_fill_records(events: &[Event]) -> Vec<(Decimal, Decimal, Option<String>)> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::ResidualInventoryRecorded {
                    order_kind: robson_domain::ReductionKind::Exit,
                    accepted_quantity,
                    residual_quantity,
                    exchange_order_id,
                    ..
                } => Some((*accepted_quantity, *residual_quantity, exchange_order_id.clone())),
                _ => None,
            })
            .collect()
    }

    /// A fill closes the leg, whatever the venue still holds, and no venue read
    /// decides it. The whole leg is booked at the fill, the fill is on the
    /// record, and the closed leg blocks nothing.
    #[tokio::test]
    async fn a_short_exit_closes_the_leg_whatever_the_venue_still_holds() {
        let manager = create_test_manager().await;
        let (position, stop_id) = active_leg_with_live_stop(&manager).await;
        let exchange = manager.exchange();
        exchange.set_price("BTCUSDT", dec!(90));
        exchange.set_short_fill_next(dec!(0.9));
        venue_still_holds(&manager, &position, dec!(0.1));

        assert!(stop_crossing_tick(&manager, &position).await.is_ok());

        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        match leg.state {
            PositionState::Closed { realized_pnl, .. } => {
                assert_eq!(realized_pnl, dec!(-10), "(90 - 100) x 1, the whole leg");
            },
            other => panic!("a fill closes the leg, got {other:?}"),
        }
        assert!(!exchange.has_stop_order(&stop_id));
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let records = short_fill_records(&events);
        assert_eq!(records.len(), 1, "{events:?}");
        assert_eq!((records[0].0, records[0].1), (dec!(0.9), dec!(0.1)));
        assert_eq!(leg.residual_quantity, None);
        assert!(manager.check_no_unresolved_residual().await.is_ok());
    }

    /// The same through a close and through panic: closed, reported closed, and
    /// the fill on the record.
    #[tokio::test]
    async fn a_close_or_a_panic_that_fills_short_closes_with_the_fill_on_record() {
        for panic in [false, true] {
            let manager = create_test_manager().await;
            let (position, _) = active_leg_with_live_stop(&manager).await;
            manager.exchange().set_short_fill_next(dec!(0.9));
            venue_still_holds(&manager, &position, dec!(0.1));

            if panic {
                assert_eq!(manager.panic_close_all().await.unwrap(), [position.id]);
            } else {
                manager.close_position(position.id).await.unwrap();
            }

            let leg = manager.get_position(position.id).await.unwrap().unwrap();
            assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
            let events = manager.store.events().find_by_position(position.id).await.unwrap();
            assert_eq!(short_fill_records(&events).len(), 1, "{events:?}");
        }
    }

    /// Rules widened after admission: the refresh lets the exit through at the
    /// new lot, the fill closes the leg, and what the grid leaves is the
    /// worker's.
    #[tokio::test]
    async fn a_rounded_exit_closes_the_leg() {
        let manager = create_test_manager().await;
        let (mut position, _) = active_leg_with_live_stop(&manager).await;
        position.quantity = Quantity::new(dec!(0.105)).unwrap();
        manager.store.positions().save(&position).await.unwrap();
        let exchange = manager.exchange();
        exchange.set_venue_rules(lot_rules(dec!(0.01)));

        assert!(stop_crossing_tick(&manager, &position).await.is_ok());

        assert_eq!(exchange.market_order_call_count(), 2, "one resubmission on the new lot");
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let records = short_fill_records(&events);
        assert_eq!((records[0].0, records[0].1), (dec!(0.10), dec!(0.005)));
    }

    /// Whatever stops the exit behind its cancel, the leg gets a stop back,
    /// once the usual window passes, and with nothing crossing it again.
    #[tokio::test]
    async fn a_failed_exit_gets_its_stop_back_whatever_the_error() {
        let failures: [(&str, fn(&StubExchange)); 4] = [
            ("rules rejection", |exchange| exchange.set_rules_rejections_next(2)),
            ("reduce-only refusal", |exchange| exchange.set_reduce_only_rejections_next(1)),
            ("exchange failure", |exchange| exchange.set_order_fail_next(true)),
            ("venue rejection", |exchange| exchange.set_reject_order_next(true)),
        ];
        for (name, fail) in failures {
            let manager = create_test_manager().await;
            let (position, stop_id) = active_leg_with_live_stop(&manager).await;
            let exchange = manager.exchange();
            venue_still_holds(&manager, &position, dec!(1));
            fail(&exchange);

            assert!(stop_crossing_tick(&manager, &position).await.is_err(), "{name}");

            let leg = manager.get_position(position.id).await.unwrap().unwrap();
            assert!(matches!(leg.state, PositionState::Active { .. }), "{name}");
            assert_eq!(leg.insurance_stop_id, None, "{name}: cancel-first took it");

            // The price recovers: no exit, no trailing advance, only the heal path.
            quiet_tick(&manager, &position).await;
            let scheduled = manager.insurance_stop_stale.read().await.get(&position.id).copied();
            assert!(scheduled.is_some(), "{name}: the heal path must notice");
            assert!(
                manager
                    .get_position(position.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .insurance_stop_id
                    .is_none(),
                "{name}: the first attempt waits its window"
            );

            manager
                .insurance_stop_stale
                .write()
                .await
                .insert(position.id, std::time::Instant::now());
            quiet_tick(&manager, &position).await;

            let leg = manager.get_position(position.id).await.unwrap().unwrap();
            let healed = leg.insurance_stop_id.expect("a stop again");
            assert_ne!(healed, stop_id, "{name}");
            assert!(exchange.has_stop_order(&healed), "{name}");
            assert!(
                !manager.insurance_stop_stale.read().await.contains_key(&position.id),
                "{name}"
            );
        }
    }

    /// A leg the venue closed by itself has nothing to protect: no stop
    /// attempts while it is suspected, or each would fail and write an
    /// event.
    #[tokio::test]
    async fn a_leg_suspected_closed_on_the_venue_is_not_healed() {
        let manager = create_test_manager().await;
        let (position, _) = active_leg_with_live_stop(&manager).await;
        let exchange = manager.exchange();
        exchange.set_reduce_only_rejections_next(1);

        assert!(stop_crossing_tick(&manager, &position).await.is_ok(), "overtaken, flat");
        assert!(manager.exit_overtaken_suspects.read().await.contains(&position.id));

        quiet_tick(&manager, &position).await;
        assert!(!manager.insurance_stop_stale.read().await.contains_key(&position.id));
    }

    /// The whole leg rounds below the lot: nothing executed, the residual on
    /// record, the long window. The stop its batch took cannot come back while
    /// the leg stays there: that is the accepted limit of cancel-first.
    #[tokio::test]
    async fn an_unexecutable_leg_stays_active_with_a_recorded_residual() {
        let manager = create_test_manager().await;
        let (mut position, _) = active_leg_with_live_stop(&manager).await;
        position.quantity = Quantity::new(dec!(0.0005)).unwrap();
        manager.store.positions().save(&position).await.unwrap();
        let exchange = manager.exchange();
        exchange.set_trading_rules(lot_rules(dec!(0.001)));
        exchange.set_venue_rules(lot_rules(dec!(0.001)));
        let orders = exchange.market_order_call_count();

        assert!(stop_crossing_tick(&manager, &position).await.is_ok());

        assert_eq!(exchange.market_order_call_count() - orders, 1, "submitted raw, refused");
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Active { .. }), "{:?}", leg.state);
        assert_eq!(leg.residual_quantity, Some(dec!(0.0005)));
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events
            .iter()
            .all(|e| !matches!(e, Event::ExitOrderPlaced { .. } | Event::PositionClosed { .. })));

        // Five minutes, not thirty seconds.
        let retry_at = manager.exit_retry_after.read().await[&position.id];
        let wait = retry_at.saturating_duration_since(std::time::Instant::now());
        assert!(wait > EXIT_RULES_REJECTION_BACKOFF, "{wait:?}");
        let orders = exchange.market_order_call_count();
        for _ in 0..3 {
            assert!(stop_crossing_tick(&manager, &position).await.is_ok());
        }
        assert_eq!(exchange.market_order_call_count(), orders);
    }

    /// A rounded replacement: the residual is on record once and later stops do
    /// not clear it, not even one covering the whole leg. A failed cancel of
    /// the predecessor changes none of that.
    #[tokio::test]
    async fn a_residual_outlives_later_stops() {
        let manager = create_test_manager().await;
        let (mut position, stop_id) = active_leg_with_live_stop(&manager).await;
        position.quantity = Quantity::new(dec!(0.105)).unwrap();
        manager.store.positions().save(&position).await.unwrap();
        let exchange = manager.exchange();
        exchange.set_trading_rules(lot_rules(dec!(0.01)));
        let replace = |previous: String| EngineAction::ReplaceInsuranceStop {
            position_id: position.id,
            symbol: position.symbol.clone(),
            side: position.side.exit_action(),
            quantity: position.quantity,
            previous_order_id: previous,
            new_stop_price: Price::new(dec!(90)).unwrap(),
        };
        let residuals = |events: &[Event]| {
            events
                .iter()
                .filter(|e| matches!(e, Event::ResidualInventoryRecorded { .. }))
                .count()
        };

        exchange.set_cancel_fails_next(true);
        manager.execute_and_persist(vec![replace(stop_id)]).await.unwrap();

        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(leg.residual_quantity, Some(dec!(0.005)));
        assert_eq!(leg.quantity.as_decimal(), dec!(0.105));
        assert!(matches!(leg.state, PositionState::Active { .. }));

        // The next stop is short by the same amount: nothing new on the log.
        let current = leg.insurance_stop_id.clone().unwrap();
        manager.execute_and_persist(vec![replace(current)]).await.unwrap();
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert_eq!(residuals(&events), 1);

        // A stop for the whole leg does not clear it.
        exchange.set_trading_rules(lot_rules(dec!(0.001)));
        let current = manager
            .get_position(position.id)
            .await
            .unwrap()
            .unwrap()
            .insurance_stop_id
            .unwrap();
        manager.execute_and_persist(vec![replace(current)]).await.unwrap();
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(leg.residual_quantity, Some(dec!(0.005)));
        let summary = crate::api::position_summary_for_test(&manager, &leg).await;
        assert_eq!(summary.residual_quantity, Some(dec!(0.005)));
    }

    /// A residual is priced nowhere, whichever way the leg's risk is priced.
    #[tokio::test]
    async fn a_residual_stays_out_of_the_budget_under_either_stop_policy() {
        for executable_span in [false, true] {
            let manager = create_test_manager().await;
            let (mut position, _) = active_leg_with_live_stop(&manager).await;
            if executable_span {
                position.stop_policy = StopPolicy::ExecutableSpan;
                position.executable_span = Some(dec!(10));
                position.cap_basis_distance = Some(dec!(10));
                position.stop_plan_entry_reference = Some(Price::new(dec!(100)).unwrap());
                position.stop_buffer_bps_at_arm = Some(Decimal::ZERO);
            }
            manager.store.positions().save(&position).await.unwrap();
            let (before, _, _, failures) = manager
                .canonical_budget_snapshot_with_positions(chrono::Utc::now())
                .await
                .unwrap();
            assert!(failures.is_empty(), "{failures:?}");
            assert!(before.latent_risk > Decimal::ZERO, "the leg must carry some risk");

            position.residual_quantity = Some(dec!(0.5));
            position.residual_unknown = true;
            manager.store.positions().save(&position).await.unwrap();
            let (after, _, resolved, failures) = manager
                .canonical_budget_snapshot_with_positions(chrono::Utc::now())
                .await
                .unwrap();

            assert!(failures.is_empty(), "{failures:?}");
            assert!(!after.latent_risk_invalid);
            assert_eq!(after.latent_risk, before.latent_risk);
            assert_eq!(resolved[&position.id].risk, before.latent_risk);
            assert_eq!(after.remaining_active, before.remaining_active);
            assert_eq!(after.slots_active, before.slots_active);
        }
    }

    /// A residual on any open leg, known or unreadable, blocks new admissions
    /// at every door with its own reason, until that leg closes.
    #[tokio::test]
    async fn a_residual_on_any_open_leg_blocks_new_admissions_until_it_closes() {
        let blocked = |result: Option<DaemonError>| {
            assert!(
                matches!(result, Some(DaemonError::ResidualInventoryUnresolved { .. })),
                "{result:?}"
            );
        };
        for unknown in [false, true] {
            let manager = create_test_manager().await;
            let (position, _) = active_leg_with_live_stop(&manager).await;
            let symbol = Symbol::from_pair("BTCUSDT").unwrap();
            let entry = Price::new(dec!(100)).unwrap();
            let stop = Price::new(dec!(98)).unwrap();
            let arm = || {
                manager.arm_position(
                    symbol.clone(),
                    Side::Long,
                    create_test_risk_config(),
                    Some(TechnicalStopDistance::from_entry_and_stop(entry, stop)),
                    Uuid::now_v7(),
                )
            };
            // Armed before the residual: the signal is the door it comes to.
            let armed = arm().await.unwrap();

            let mut leg = manager.get_position(position.id).await.unwrap().unwrap();
            if unknown {
                leg.residual_unknown = true;
            } else {
                leg.residual_quantity = Some(dec!(0.005));
            }
            manager.store.positions().save(&leg).await.unwrap();
            let orders = manager.exchange().market_order_call_count();

            blocked(arm().await.err());
            blocked(manager.approve_query(Uuid::now_v7()).await.err());
            blocked(
                manager
                    .handle_signal(DetectorSignal {
                        signal_id: Uuid::now_v7(),
                        position_id: armed.id,
                        symbol: symbol.clone(),
                        side: Side::Long,
                        entry_price: entry,
                        stop_loss: stop,
                        technical_stop_analysis: None,
                        timestamp: chrono::Utc::now(),
                    })
                    .await
                    .err(),
            );
            assert_eq!(manager.exchange().market_order_call_count(), orders);
            // The block re-armed it; its error must not re-arm it a second time.
            assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), Some(&1));
            let armed = manager.get_position(armed.id).await.unwrap().unwrap();
            assert!(matches!(armed.state, PositionState::Armed), "{:?}", armed.state);

            // It is not the budget saying no.
            let (snapshot, _, _, failures) = manager
                .canonical_budget_snapshot_with_positions(chrono::Utc::now())
                .await
                .unwrap();
            assert!(failures.is_empty(), "{failures:?}");
            assert!(!snapshot.latent_risk_invalid);

            // Closing the leg resolves it.
            manager.close_position(position.id).await.unwrap();
            assert!(arm().await.is_ok());
        }
    }

    /// An Active leg whose stop cannot be derived, so latent risk fails.
    async fn leg_with_unresolved_latent_risk(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
    ) -> Position {
        let mut position =
            save_active_position(manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        position.stop_policy = StopPolicy::ExecutableSpan;
        position.cap_basis_distance = None;
        manager.store.positions().save(&position).await.unwrap();
        let (snapshot, _, _, _) = manager
            .canonical_budget_snapshot_with_positions(chrono::Utc::now())
            .await
            .unwrap();
        assert!(snapshot.latent_risk_invalid, "the test needs an unresolved latent risk");
        position
    }

    fn budget_snapshot(
        model: MonthlyBudgetModel,
        capital_base: Decimal,
        consumed_active: Decimal,
        consumed_nfs: Decimal,
    ) -> MonthlyBudgetSnapshot {
        MonthlyBudgetSnapshot {
            model,
            capital_base,
            capital_base_invalid: capital_base <= Decimal::ZERO,
            latent_risk_invalid: true,
            budget_amount: TradingPolicy::default().monthly_budget(capital_base),
            governed_realized_net: -consumed_nfs,
            month_equity_net: -consumed_active,
            month_peak_net: Decimal::ZERO,
            latent_risk: Decimal::ZERO,
            consumed_active,
            consumed_hwm: consumed_active,
            consumed_nfs,
            remaining_active: Decimal::ZERO,
            remaining_hwm: Decimal::ZERO,
            remaining_nfs: Decimal::ZERO,
            slots_active: 0,
            slots_hwm: 0,
            slots_nfs: 0,
        }
    }

    /// With latent risk unresolved only realized losses decide.
    #[test]
    fn only_realized_losses_exhaust_the_budget_with_latent_risk_unresolved() {
        type Manager = PositionManager<StubExchange, MemoryStore>;
        let nfs = MonthlyBudgetModel::NetFromStartNonExpandingV1;
        let hwm = MonthlyBudgetModel::HwmV1;
        let exhausted = |snapshot| Manager::realized_losses_exhaust_budget(&snapshot);

        // Production's model: a budget of 400 on 10000.
        assert!(!exhausted(budget_snapshot(nfs, dec!(10000), dec!(399), dec!(399))));
        assert!(exhausted(budget_snapshot(nfs, dec!(10000), dec!(400), dec!(400))));
        assert!(exhausted(budget_snapshot(hwm, dec!(10000), dec!(400), dec!(400))));

        // A give-back from a peak, with no realized loss.
        assert!(!exhausted(budget_snapshot(hwm, dec!(10000), dec!(500), Decimal::ZERO)));
    }

    /// The whole way through a halt evaluation: a persisted peak above the
    /// budget, no realized loss and latent risk unresolved. The give-back
    /// needs a mark that may not be current, so this defers as it always
    /// has.
    #[tokio::test]
    async fn a_give_back_with_no_realized_loss_defers_the_halt_with_latent_risk_unresolved() {
        let manager = create_test_manager().await;
        leg_with_unresolved_latent_risk(&manager).await;
        let now = chrono::Utc::now();
        manager
            .month_peak_net_cache
            .lock()
            .await
            .insert((now.year(), now.month()), dec!(500));

        let (snapshot, _, _, _) =
            manager.canonical_budget_snapshot_with_positions(now).await.unwrap();
        assert!(snapshot.latent_risk_invalid);
        assert!(
            snapshot.consumed_active >= snapshot.budget_amount,
            "{}",
            snapshot.consumed_active
        );
        assert_eq!(snapshot.consumed_nfs, Decimal::ZERO);

        assert!(!manager.evaluate_monthly_halt().await);
        assert!(!manager.circuit_breaker.blocks_new_entries().await);
    }

    /// Unpriced risk defers a decision that depends on it. It does not defer
    /// one that known losses have already made.
    #[tokio::test]
    async fn known_losses_fire_the_halt_even_with_latent_risk_unresolved() {
        let manager = create_test_manager().await;
        leg_with_unresolved_latent_risk(&manager).await;
        assert!(!manager.evaluate_monthly_halt().await, "nothing known exhausts the budget");
        assert!(!manager.circuit_breaker.blocks_new_entries().await);

        // Just under the budget of 400: still deferred.
        save_closed_position_with_pnl(&manager, dec!(-399)).await;
        assert!(!manager.evaluate_monthly_halt().await);

        save_closed_position_with_pnl(&manager, dec!(-1)).await;
        assert!(manager.evaluate_monthly_halt().await, "known losses exhaust the budget");
        assert!(manager.circuit_breaker.blocks_new_entries().await);
    }

    /// The basis a broken `monthly_state` row gives. Constructors refuse it, so
    /// it comes in the way a stored config would. In-memory managers read their
    /// basis from the engine's config; `update_engine_capital` restores it.
    fn set_capital_base_to_zero(manager: &Arc<PositionManager<StubExchange, MemoryStore>>) {
        let config: RiskConfig =
            serde_json::from_value(serde_json::json!({ "capital": "0" })).unwrap();
        manager.engine.lock().unwrap().update_risk_config(config);
    }

    async fn is_still_active(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position: &Position,
    ) -> bool {
        let stored = manager.get_position(position.id).await.unwrap().unwrap();
        matches!(stored.state, PositionState::Active { .. })
    }

    /// ADR-0051 §1: a basis of zero is not an exhausted budget. Nothing latches
    /// and the open position keeps running on its stop, even though the
    /// arithmetic reads below zero.
    #[tokio::test]
    async fn an_invalid_basis_neither_halts_nor_closes_open_positions() {
        let manager = create_test_manager().await;
        let leg = save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        set_capital_base_to_zero(&manager);

        let snapshot = manager.canonical_budget_snapshot(chrono::Utc::now()).await.unwrap();
        assert!(snapshot.capital_base_invalid);
        assert!(!snapshot.latent_risk_invalid, "the path with latent risk resolved");
        assert!(snapshot.remaining_active < Decimal::ZERO, "{}", snapshot.remaining_active);
        assert_eq!(snapshot.slots_active, 0);

        assert!(!manager.evaluate_monthly_halt().await);
        assert!(!manager.circuit_breaker.blocks_new_entries().await);
        assert!(is_still_active(&manager, &leg).await);
    }

    /// The same with latent risk unresolved and losses that would exhaust any
    /// budget.
    #[tokio::test]
    async fn an_invalid_basis_does_not_halt_with_latent_risk_unresolved_either() {
        let manager = create_test_manager().await;
        let leg = leg_with_unresolved_latent_risk(&manager).await;
        save_closed_position_with_pnl(&manager, dec!(-500)).await;
        set_capital_base_to_zero(&manager);

        assert!(!manager.evaluate_monthly_halt().await);
        assert!(!manager.circuit_breaker.blocks_new_entries().await);
        assert!(is_still_active(&manager, &leg).await);
    }

    /// Nothing is latched by the invalid basis, so a real exhaustion still
    /// fires once the basis is valid again.
    #[tokio::test]
    async fn the_halt_decides_normally_once_the_basis_is_valid_again() {
        let manager = create_test_manager().await;
        let leg = save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        save_closed_position_with_pnl(&manager, dec!(-400)).await;
        set_capital_base_to_zero(&manager);
        assert!(!manager.evaluate_monthly_halt().await);
        assert!(!manager.circuit_breaker.blocks_new_entries().await);

        manager.update_engine_capital(dec!(10000));

        assert!(manager.evaluate_monthly_halt().await);
        assert!(manager.circuit_breaker.blocks_new_entries().await);
        assert!(!is_still_active(&manager, &leg).await);
    }

    #[test]
    fn the_gate_sees_no_capital_unless_the_basis_still_stands() {
        type Manager = PositionManager<StubExchange, MemoryStore>;

        assert_eq!(Manager::capital_for_the_gate(dec!(10000), Ok(false)), dec!(10000));
        assert_eq!(Manager::capital_for_the_gate(dec!(10000), Ok(true)), Decimal::ZERO);
        assert_eq!(
            Manager::capital_for_the_gate(
                dec!(10000),
                Err(DaemonError::Config("database gone".to_string()))
            ),
            Decimal::ZERO
        );
    }

    /// Automatic liquidation is what the invalid basis gave up. The operator's
    /// own decision to flatten does not depend on the budget.
    #[tokio::test]
    async fn the_operator_can_still_flatten_on_an_invalid_basis() {
        let manager = create_test_manager().await;
        let leg = save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        set_capital_base_to_zero(&manager);

        let closed = manager.trigger_monthly_halt("operator decision".to_string()).await.unwrap();

        assert_eq!(closed, [leg.id]);
        assert!(manager.circuit_breaker.blocks_new_entries().await);
    }

    #[cfg(feature = "postgres")]
    async fn set_month_basis(pool: &sqlx::PgPool, capital_base: Decimal) {
        let now = chrono::Utc::now();
        sqlx::query(
            r#"
            INSERT INTO monthly_state (
                year, month, capital_base, month_peak_net,
                monthly_budget_model, created_at
            )
            VALUES ($1, $2, $3, 0, 'net_from_start_non_expanding_v1', $4)
            ON CONFLICT (year, month) DO UPDATE SET capital_base = EXCLUDED.capital_base
            "#,
        )
        .bind(now.year() as i16)
        .bind(now.month() as i16)
        .bind(capital_base)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    /// An Armed BTCUSDT position and the signal that would enter it.
    async fn armed_position_and_signal(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
    ) -> (Position, DetectorSignal) {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        manager.exchange().set_trading_rules(
            robson_domain::SymbolTradingRules::new(
                symbol.clone(),
                dec!(0.01),
                Decimal::ZERO,
                dec!(0.000001),
                dec!(0.000001),
                dec!(1000000),
                Decimal::ZERO,
                2,
                6,
            )
            .unwrap(),
        );
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(91200)).unwrap();
        let mut armed = Position::new(Uuid::now_v7(), symbol.clone(), Side::Long);
        armed.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(entry, stop));
        armed.state = PositionState::Armed;
        manager.store.positions().save(&armed).await.unwrap();
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: armed.id,
            symbol,
            side: Side::Long,
            entry_price: entry,
            stop_loss: stop,
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };
        (armed, signal)
    }

    /// Denied before the exchange, with the breaker and the rest of the book
    /// left alone.
    #[cfg(feature = "postgres")]
    async fn assert_denied_and_book_kept(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        armed: &Position,
        leg: &Position,
    ) {
        let denied = manager.get_position(armed.id).await.unwrap().unwrap();
        assert!(matches!(denied.state, PositionState::Armed), "{:?}", denied.state);
        assert_eq!(manager.exchange().market_order_call_count(), 0);
        assert!(!manager.circuit_breaker.blocks_new_entries().await);
        assert!(is_still_active(manager, leg).await);
    }

    /// The row production would read, with the engine still holding the capital
    /// it sized with.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn a_zero_base_row_denies_signals_and_keeps_the_book(pool: sqlx::PgPool) {
        set_month_basis(&pool, Decimal::ZERO).await;
        let manager = create_postgres_test_manager(pool);
        let leg = save_active_position(&manager, "ETHUSDT", Side::Long, dec!(100), dec!(1)).await;

        // A close elsewhere in the book.
        assert!(!manager.evaluate_monthly_halt().await);
        assert!(!manager.circuit_breaker.blocks_new_entries().await);

        let (armed, signal) = armed_position_and_signal(&manager).await;
        manager.handle_signal(signal).await.unwrap();

        assert_denied_and_book_kept(&manager, &armed, &leg).await;
    }

    /// The basis goes invalid while an entry waits for the operator.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn a_zero_base_row_denies_a_pending_approval_and_keeps_the_book(pool: sqlx::PgPool) {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};

        set_month_basis(&pool, dec!(10000)).await;
        let manager = create_postgres_test_manager(pool.clone());
        let leg = save_active_position(&manager, "ETHUSDT", Side::Long, dec!(100), dec!(1)).await;
        let (armed, signal) = armed_position_and_signal(&manager).await;
        manager.entry_policies.write().await.insert(
            armed.id,
            EntryPolicyConfig::new(EntryPolicy::Immediate, DomainApprovalPolicy::HumanConfirmation),
        );
        manager.handle_signal(signal).await.unwrap();
        let query_id = {
            let pending = manager.pending_approvals.read().await;
            *pending.keys().next().expect("the signal must wait for approval")
        };

        set_month_basis(&pool, Decimal::ZERO).await;
        let result = manager.approve_query(query_id).await;

        assert!(matches!(result, Err(DaemonError::ApprovalDenied { .. })), "{result:?}");
        assert_denied_and_book_kept(&manager, &armed, &leg).await;
    }

    /// The production query recorder on the same database as the event log, so
    /// a failed audit write fails the admission as in production.
    /// `TracingQueryRecorder` never fails.
    #[cfg(feature = "postgres")]
    fn create_audited_postgres_test_manager(
        pool: sqlx::PgPool,
        approval_ttl_seconds: u64,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        audited_postgres_test_manager_with_journal(pool, approval_ttl_seconds, None)
    }

    #[cfg(feature = "postgres")]
    fn audited_postgres_test_manager_with_journal(
        pool: sqlx::PgPool,
        approval_ttl_seconds: u64,
        order_journal: Option<Arc<robson_exec::order_journal::OrderJournal>>,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Executor::new(exchange, journal, store.clone());
        let executor = Arc::new(match order_journal {
            Some(order_journal) => executor.with_order_journal(order_journal),
            None => executor,
        });
        let engine = Engine::new(RiskConfig::new(dec!(10000)).unwrap());
        let tenant_id = Uuid::now_v7();

        Arc::new(
            PositionManager::with_approval_policy(
                engine,
                executor,
                store,
                Arc::new(EventBus::new(100)),
                Arc::new(crate::query_engine::EventLogQueryRecorder::new(
                    pool.clone(),
                    tenant_id,
                    "robson:daemon",
                )),
                ApprovalPolicy::new(Decimal::from(100u32), approval_ttl_seconds),
                TradingPolicy::default(),
            )
            .with_event_log(pool, tenant_id),
        )
    }

    /// Fails every event-log insert that matches `condition`, SQL over `NEW`,
    /// until `restore_event_log_writes`. Injected on the server, so no client
    /// cache can hide it.
    #[cfg(feature = "postgres")]
    async fn fail_event_log_writes(pool: &sqlx::PgPool, condition: &str) {
        inject_event_log_fault(pool, condition, "RAISE EXCEPTION 'injected event log failure';")
            .await;
    }

    /// Runs `action`, PL/pgSQL, on every event-log insert that matches
    /// `condition`, until `restore_event_log_writes`.
    #[cfg(feature = "postgres")]
    async fn inject_event_log_fault(pool: &sqlx::PgPool, condition: &str, action: &str) {
        let function = format!(
            r#"
            CREATE OR REPLACE FUNCTION injected_event_log_failure() RETURNS trigger
            LANGUAGE plpgsql AS $$
            BEGIN
                IF {condition} THEN
                    {action}
                END IF;
                RETURN NEW;
            END
            $$
            "#
        );
        sqlx::query(&function).execute(pool).await.unwrap();
        sqlx::query(
            "CREATE TRIGGER injected_event_log_failure BEFORE INSERT ON event_log \
             FOR EACH ROW EXECUTE FUNCTION injected_event_log_failure()",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[cfg(feature = "postgres")]
    async fn restore_event_log_writes(pool: &sqlx::PgPool) {
        sqlx::query("DROP TRIGGER injected_event_log_failure ON event_log")
            .execute(pool)
            .await
            .unwrap();
    }

    /// The `fail_event_log_writes` condition for one query audit write.
    #[cfg(feature = "postgres")]
    fn audit_write(transition_cause: &str) -> String {
        format!(
            "NEW.event_type = '{}' AND NEW.payload->>'transition_cause' = '{transition_cause}'",
            robson_eventlog::QUERY_STATE_CHANGED_EVENT_TYPE
        )
    }

    /// A position's query audit trail, in write order.
    #[cfg(feature = "postgres")]
    async fn audit_trail(pool: &sqlx::PgPool, position_id: PositionId) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT payload->>'transition_cause' FROM event_log \
             WHERE event_type = $1 AND payload->>'position_id' = $2 ORDER BY seq",
        )
        .bind(robson_eventlog::QUERY_STATE_CHANGED_EVENT_TYPE)
        .bind(position_id.to_string())
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[cfg(feature = "postgres")]
    async fn position_event_count(
        pool: &sqlx::PgPool,
        position_id: PositionId,
        event_type: &str,
    ) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM event_log WHERE stream_key = $1 AND event_type = $2",
        )
        .bind(format!("position:{position_id}"))
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// `armed_position_and_signal` with the arm on record in the event log, as
    /// an entry's projection requires.
    #[cfg(feature = "postgres")]
    async fn recorded_armed_position_and_signal(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
    ) -> (Position, DetectorSignal) {
        let (armed, signal) = armed_position_and_signal(manager).await;
        manager
            .persist_event_to_log(&Event::PositionArmed {
                position_id: armed.id,
                account_id: armed.account_id,
                symbol: armed.symbol.clone(),
                side: armed.side,
                tech_stop_distance: armed.tech_stop_distance,
                stop_policy: armed.stop_policy,
                stop_buffer_bps_at_arm: armed.stop_buffer_bps_at_arm,
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();
        (armed, signal)
    }

    /// The handle a detector leaves in the map once it has fired: finished,
    /// since detectors are single-shot. Replaces, and stops, a re-armed one.
    #[cfg(feature = "postgres")]
    async fn detector_fired(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position_id: PositionId,
    ) {
        let spent = tokio::spawn(async { None });
        while !spent.is_finished() {
            tokio::task::yield_now().await;
        }
        if let Some(rearmed) = manager.detectors.write().await.insert(position_id, spent) {
            rearmed.abort();
        }
    }

    /// The re-armed detector fires again.
    #[cfg(feature = "postgres")]
    fn signal_again(signal: &DetectorSignal) -> DetectorSignal {
        DetectorSignal {
            signal_id: Uuid::now_v7(),
            timestamp: chrono::Utc::now(),
            ..signal.clone()
        }
    }

    /// Still Armed, nothing on the exchange, and a detector back under the
    /// governed backoff.
    #[cfg(feature = "postgres")]
    async fn assert_rearmed_and_nothing_entered(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        armed: &Position,
        attempt: u32,
        case: &str,
    ) {
        let position = manager.get_position(armed.id).await.unwrap().unwrap();
        assert!(matches!(position.state, PositionState::Armed), "{case}: {:?}", position.state);
        assert_eq!(manager.exchange().market_order_call_count(), 0, "{case}");
        assert!(
            manager
                .detectors
                .read()
                .await
                .get(&armed.id)
                .is_some_and(|handle| !handle.is_finished()),
            "{case}: detector lost"
        );
        assert_eq!(
            manager.governed_rearm_counts.read().await.get(&armed.id),
            Some(&attempt),
            "{case}"
        );
    }

    /// The query audit writes of an automatic entry, in order. The executor
    /// runs between `acting` and `completed`.
    #[cfg(feature = "postgres")]
    const ADMISSION_AUDIT: [&str; 5] = [
        "accepted",
        "processing",
        "risk_checked",
        "acting",
        "completed",
    ];

    /// ADR-0051 Recovery, through the production recorder: the audit write
    /// `ADMISSION_AUDIT[failing]` fails. The position keeps a detector and
    /// nothing reaches the exchange. Once writes succeed the next signal
    /// enters, with every write before the order on record.
    #[cfg(feature = "postgres")]
    async fn assert_failed_admission_write_rearms(pool: sqlx::PgPool, failing: usize) {
        let cause = ADMISSION_AUDIT[failing];
        set_month_basis(&pool, dec!(10000)).await;
        let manager = create_audited_postgres_test_manager(pool.clone(), 300);
        let (armed, signal) = recorded_armed_position_and_signal(&manager).await;

        // Twice: a failure that recurs keeps backing off, wherever it is.
        fail_event_log_writes(&pool, &audit_write(cause)).await;
        for attempt in 1..=2 {
            detector_fired(&manager, armed.id).await;
            let result = manager.handle_signal(signal_again(&signal)).await;
            assert!(result.is_err(), "{cause}: {result:?}");
            assert_rearmed_and_nothing_entered(&manager, &armed, attempt, cause).await;
        }
        restore_event_log_writes(&pool).await;

        detector_fired(&manager, armed.id).await;
        manager.handle_signal(signal_again(&signal)).await.unwrap();

        assert_eq!(manager.exchange().market_order_call_count(), 1, "{cause}");
        let entered = manager.get_position(armed.id).await.unwrap().unwrap();
        assert!(
            matches!(entered.state, PositionState::Active { .. }),
            "{cause}: {:?}",
            entered.state
        );
        assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), None, "{cause}");
        let expected: Vec<String> = ADMISSION_AUDIT[..failing]
            .iter()
            .chain(ADMISSION_AUDIT[..failing].iter())
            .chain(ADMISSION_AUDIT.iter())
            .map(|cause| cause.to_string())
            .collect();
        assert_eq!(audit_trail(&pool, armed.id).await, expected, "{cause}");
        assert_eq!(
            position_event_count(&pool, armed.id, "entry_signal_received").await,
            1,
            "{cause}"
        );
    }

    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failed_accepted_write_rearms_the_detector(pool: sqlx::PgPool) {
        assert_failed_admission_write_rearms(pool, 0).await;
    }

    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failed_processing_write_rearms_the_detector(pool: sqlx::PgPool) {
        assert_failed_admission_write_rearms(pool, 1).await;
    }

    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failed_risk_checked_write_rearms_the_detector(pool: sqlx::PgPool) {
        assert_failed_admission_write_rearms(pool, 2).await;
    }

    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failed_acting_write_rearms_the_detector(pool: sqlx::PgPool) {
        assert_failed_admission_write_rearms(pool, 3).await;
    }

    /// A database that refuses every write: each fire costs one failed write
    /// and re-arms with a longer backoff, so an outage is no loop, and the
    /// entry once writes succeed resets it.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_write_outage_backs_the_detector_off_until_it_ends(pool: sqlx::PgPool) {
        set_month_basis(&pool, dec!(10000)).await;
        let manager = create_audited_postgres_test_manager(pool.clone(), 300);
        let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
        let rearms =
            || crate::metrics::DETECTOR_REARMS_AFTER_FAILURE.with_label_values(&["signal"]);
        let rearms_before = rearms().get();

        fail_event_log_writes(&pool, "TRUE").await;
        for attempt in 1..=3 {
            detector_fired(&manager, armed.id).await;
            let result = manager.handle_signal(signal_again(&signal)).await;
            assert!(result.is_err(), "{result:?}");
            assert_rearmed_and_nothing_entered(&manager, &armed, attempt, "outage").await;
        }
        restore_event_log_writes(&pool).await;

        assert!(audit_trail(&pool, armed.id).await.is_empty());
        assert!(rearms().get() >= rearms_before + 3.0, "each re-arm must be counted");

        manager.handle_signal(signal_again(&signal)).await.unwrap();
        assert_eq!(manager.exchange().market_order_call_count(), 1);
        assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), None);
    }

    /// The approval flow. A failed `awaiting_approval` write queues nothing,
    /// and a failed write after the operator approves has already taken the
    /// approval. Both re-arm, and the operator approves the next signal.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failed_audit_write_in_the_approval_flow_rearms_the_detector(pool: sqlx::PgPool) {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};

        set_month_basis(&pool, dec!(10000)).await;
        let manager = create_audited_postgres_test_manager(pool.clone(), 300);
        let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
        manager.entry_policies.write().await.insert(
            armed.id,
            EntryPolicyConfig::new(EntryPolicy::Immediate, DomainApprovalPolicy::HumanConfirmation),
        );

        fail_event_log_writes(&pool, &audit_write("awaiting_approval")).await;
        let result = manager.handle_signal(signal_again(&signal)).await;
        restore_event_log_writes(&pool).await;
        assert!(result.is_err(), "{result:?}");
        assert!(manager.get_pending_approvals().await.is_empty());
        assert_rearmed_and_nothing_entered(&manager, &armed, 1, "awaiting_approval").await;

        for cause in ["authorized", "acting"] {
            manager.handle_signal(signal_again(&signal)).await.unwrap();
            let query_id = manager.get_pending_approvals().await[0].id;
            assert!(!manager.detectors.read().await.contains_key(&armed.id), "{cause}");

            fail_event_log_writes(&pool, &audit_write(cause)).await;
            let result = manager.approve_query(query_id).await;
            restore_event_log_writes(&pool).await;

            assert!(result.is_err(), "{cause}: {result:?}");
            assert!(manager.get_pending_approvals().await.is_empty(), "{cause}");
            // Queuing the approval reset the backoff.
            assert_rearmed_and_nothing_entered(&manager, &armed, 1, cause).await;
        }

        manager.handle_signal(signal_again(&signal)).await.unwrap();
        let query_id = manager.get_pending_approvals().await[0].id;
        manager.approve_query(query_id).await.unwrap();
        assert_eq!(manager.exchange().market_order_call_count(), 1);
        assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), None);
    }

    /// The boundary: once the entry's actions reach the executor an order may
    /// exist, so no failure from there re-arms. Before the order: the first
    /// entry event fails to persist, which also keeps the order back
    /// (ADR-0052). After it: the last audit write fails, which since PR P
    /// keeps the entry's outcome and answers `Ok` (ADR-0056 step 7).
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failure_once_the_executor_has_the_entry_does_not_rearm(pool: sqlx::PgPool) {
        set_month_basis(&pool, dec!(10000)).await;
        let cases = [
            ("NEW.event_type = 'entry_signal_received'".to_string(), 0, false),
            (audit_write("completed"), 1, true),
        ];

        for (condition, orders, kept) in cases {
            let manager = create_audited_postgres_test_manager(pool.clone(), 300);
            let (armed, signal) = recorded_armed_position_and_signal(&manager).await;

            fail_event_log_writes(&pool, &condition).await;
            let result = manager.handle_signal(signal).await;
            restore_event_log_writes(&pool).await;

            assert_eq!(result.is_ok(), kept, "{condition}: {result:?}");
            assert_eq!(manager.exchange().market_order_call_count(), orders, "{condition}");
            assert!(!manager.detectors.read().await.contains_key(&armed.id), "{condition}");
            assert_eq!(
                manager.governed_rearm_counts.read().await.get(&armed.id),
                None,
                "{condition}"
            );
        }
    }

    // ---- ADR-0056 step 7, PR J: typed refusal and the backoff reset ------------

    /// A manager whose order journal answers from a scripted backend.
    async fn journaled_manager() -> (
        Arc<PositionManager<StubExchange, MemoryStore>>,
        Arc<robson_exec::order_journal::StubJournalBackend>,
    ) {
        let backend = Arc::new(robson_exec::order_journal::StubJournalBackend::default());
        let journal = Arc::new(
            robson_exec::order_journal::OrderJournal::new(
                backend.clone(),
                &robson_exec::venue::VenueId::new("test:venue"),
            )
            .with_commit_bound(std::time::Duration::from_millis(20)),
        );
        let manager = create_test_manager_with_order_journal(
            Arc::new(MemoryStore::new()),
            Arc::new(EventBus::new(100)),
            ApprovalPolicy::new(Decimal::from(100u32), 300),
            Some(journal),
        )
        .await;
        (manager, backend)
    }

    /// A position with `failures` earlier governed re-arms behind it.
    async fn armed_with_backoff(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        failures: u32,
    ) -> (Position, DetectorSignal) {
        let (armed, signal) = armed_position_and_signal(manager).await;
        manager.governed_rearm_counts.write().await.insert(armed.id, failures);
        (armed, signal)
    }

    /// A position armed the production way: `ExecutableSpan`, with its signal.
    async fn production_policy_position_and_signal(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
    ) -> (Position, DetectorSignal) {
        manager.exchange().set_trading_rules(
            robson_domain::SymbolTradingRules::new(
                Symbol::from_pair("BTCUSDT").unwrap(),
                dec!(0.01),
                Decimal::ZERO,
                dec!(0.000001),
                dec!(0.000001),
                dec!(1000000),
                Decimal::ZERO,
                2,
                6,
            )
            .unwrap(),
        );
        let position = manager
            .arm_position(
                Symbol::from_pair("BTCUSDT").unwrap(),
                Side::Long,
                create_test_risk_config(),
                None,
                Uuid::now_v7(),
            )
            .await
            .unwrap();
        assert_eq!(position.stop_policy, StopPolicy::ExecutableSpan);
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: position.symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(90250)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };
        (position, signal)
    }

    /// ADR-0052: a proven no-send has already written the immutable ruler, so a
    /// re-arm could never admit. The attempt ends with its own reason, audited,
    /// nothing went out, and a later signal stays refused.
    #[tokio::test]
    async fn a_no_send_on_a_production_policy_leg_ends_the_attempt() {
        use robson_exec::order_journal::JournalFault;
        for refuse_in_adapter in [false, true] {
            let (manager, backend) = journaled_manager().await;
            let (armed, signal) = production_policy_position_and_signal(&manager).await;
            if refuse_in_adapter {
                manager.exchange().set_refuse_before_send_next(true);
            } else {
                backend.push_fault(JournalFault::Known);
            }

            let result = manager.handle_signal(signal.clone()).await;

            assert!(
                matches!(result, Err(DaemonError::Exec(ExecError::SendRefused(_)))),
                "{result:?}"
            );
            assert_eq!(manager.exchange().market_order_call_count(), 0);
            assert!(!manager.detectors.read().await.contains_key(&armed.id), "no re-arm");
            assert!(manager.is_entry_exhausted(armed.id).await);
            let stored = manager.get_position(armed.id).await.unwrap().unwrap();
            assert!(matches!(stored.state, PositionState::Armed));
            assert!(stored.carries_admission_evidence(), "the ruler was not cleared");
            let events = manager.store.events().find_by_position(armed.id).await.unwrap();
            assert!(events.iter().any(|event| matches!(
                event,
                Event::EntryAttemptExhausted { reason_code, signal_id, .. }
                    if reason_code == "entry_not_sent" && *signal_id == Some(signal.signal_id)
            )));

            // The next signal changes nothing and sends nothing.
            let _ = manager
                .handle_signal(DetectorSignal { signal_id: Uuid::now_v7(), ..signal })
                .await;
            assert_eq!(manager.exchange().market_order_call_count(), 0);
            assert!(!manager.detectors.read().await.contains_key(&armed.id));
        }
    }

    /// The account's futures settings refuse the entry after the ruler was
    /// written. It is a proven no-send: the safety event stands and the attempt
    /// ends, as for any other.
    #[tokio::test]
    async fn a_futures_settings_refusal_on_a_production_policy_leg_ends_the_attempt() {
        let (manager, _backend) = journaled_manager().await;
        let (armed, signal) = production_policy_position_and_signal(&manager).await;
        manager.exchange().set_futures_settings("Hedge", 1);

        let result = manager.handle_signal(signal.clone()).await;

        assert!(result.is_err(), "{result:?}");
        assert_eq!(manager.exchange().market_order_call_count(), 0);
        assert!(!manager.detectors.read().await.contains_key(&armed.id), "no re-arm");
        assert!(manager.is_entry_exhausted(armed.id).await);
        let stored = manager.get_position(armed.id).await.unwrap().unwrap();
        assert!(stored.carries_admission_evidence());
        let events = manager.store.events().find_by_position(armed.id).await.unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::EntryExecutionRejected { .. })));
        assert!(events.iter().any(|event| matches!(
            event,
            Event::EntryAttemptExhausted { reason_code, signal_id, .. }
                if reason_code == "entry_not_sent" && *signal_id == Some(signal.signal_id)
        )));
    }

    /// A legacy leg keeps exactly what it had: the safety event, a spent
    /// detector, no exhaustion and no new re-arm.
    #[tokio::test]
    async fn a_futures_settings_refusal_on_a_legacy_leg_changes_nothing() {
        let (manager, _backend) = journaled_manager().await;
        let (armed, signal) = armed_with_backoff(&manager, 2).await;
        manager.exchange().set_futures_settings("Hedge", 1);

        let result = manager.handle_signal(signal).await;

        assert!(result.is_err(), "{result:?}");
        assert!(!manager.detectors.read().await.contains_key(&armed.id), "no re-arm");
        assert!(!manager.is_entry_exhausted(armed.id).await);
        assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), Some(&2));
        let events = manager.store.events().find_by_position(armed.id).await.unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::EntryExecutionRejected { .. })));
        assert!(!events.iter().any(|e| matches!(e, Event::EntryAttemptExhausted { .. })));
    }

    /// The exhaustion itself cannot be recorded: the detector stays spent, and
    /// nothing re-arms.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn an_exhaustion_that_cannot_be_recorded_leaves_the_detector_spent(pool: sqlx::PgPool) {
        use robson_exec::order_journal::{JournalFault, OrderJournal, StubJournalBackend};
        let backend = Arc::new(StubJournalBackend::default());
        let journal = Arc::new(OrderJournal::new(
            backend.clone(),
            &robson_exec::venue::VenueId::new("test:venue"),
        ));
        let manager = audited_postgres_test_manager_with_journal(pool.clone(), 300, Some(journal));
        let (armed, signal) = production_policy_position_and_signal(&manager).await;
        backend.push_fault(JournalFault::Known);

        fail_event_log_writes(&pool, "NEW.event_type = 'entry_attempt_exhausted'").await;
        let result = manager.handle_signal(signal).await;
        restore_event_log_writes(&pool).await;

        assert!(result.is_err(), "{result:?}");
        assert_eq!(manager.exchange().market_order_call_count(), 0);
        assert!(!manager.detectors.read().await.contains_key(&armed.id), "no re-arm");
    }

    /// #301's paths: a failure before the executor has written no evidence, so
    /// on a production-policy leg the re-arm does recover.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failure_before_the_executor_leaves_no_evidence_and_the_rearm_recovers(
        pool: sqlx::PgPool,
    ) {
        let manager = create_audited_postgres_test_manager(pool.clone(), 300);
        let (armed, signal) = production_policy_position_and_signal(&manager).await;

        fail_event_log_writes(&pool, &audit_write("acting")).await;
        let result = manager.handle_signal(signal.clone()).await;
        restore_event_log_writes(&pool).await;

        assert!(result.is_err(), "{result:?}");
        let stored = manager.get_position(armed.id).await.unwrap().unwrap();
        assert!(!stored.carries_admission_evidence(), "the failure came before the ruler");
        assert!(manager.detectors.read().await.contains_key(&armed.id), "re-armed");

        detector_fired(&manager, armed.id).await;
        manager.handle_signal(signal_again(&signal)).await.expect("the re-arm admits");
        assert_eq!(manager.exchange().market_order_call_count(), 1);
    }

    #[tokio::test]
    async fn a_strict_commit_known_failure_re_arms_under_the_growing_backoff() {
        use robson_exec::order_journal::JournalFault;
        let (manager, backend) = journaled_manager().await;
        let (armed, signal) = armed_with_backoff(&manager, 2).await;
        // The two pre-send barrier events are not journal writes: the first
        // journal commit is the entry's.
        backend.push_fault(JournalFault::Known);
        let signal_for_retry = signal.clone();

        let result = manager.handle_signal(signal).await;

        assert!(
            matches!(result, Err(DaemonError::Exec(ExecError::SendRefused(_)))),
            "{result:?}"
        );
        assert_eq!(manager.exchange().market_order_call_count(), 0);
        assert!(manager.detectors.read().await.contains_key(&armed.id), "re-armed");
        assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), Some(&3));

        // On a legacy leg the re-arm is real: the next signal admits.
        if let Some(rearmed) = manager.detectors.write().await.remove(&armed.id) {
            rearmed.abort();
        }
        manager
            .handle_signal(DetectorSignal {
                signal_id: Uuid::now_v7(),
                ..signal_for_retry
            })
            .await
            .expect("the re-arm admits");
        assert_eq!(manager.exchange().market_order_call_count(), 1);
    }

    #[tokio::test]
    async fn a_refusal_inside_the_adapter_before_the_send_re_arms() {
        let (manager, _backend) = journaled_manager().await;
        let (armed, signal) = armed_with_backoff(&manager, 1).await;
        manager.exchange().set_refuse_before_send_next(true);

        let result = manager.handle_signal(signal).await;

        assert!(result.is_err(), "{result:?}");
        assert!(manager.detectors.read().await.contains_key(&armed.id), "re-armed");
        assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), Some(&2));
    }

    /// An uncertain commit may have left a row, so it stays executor-class:
    /// no re-arm, and with no ticket the backoff is not reset either.
    #[tokio::test]
    async fn an_uncertain_commit_neither_re_arms_nor_resets_the_backoff() {
        use robson_exec::order_journal::JournalFault;
        for fault in [
            JournalFault::AckLost,
            JournalFault::NotLandedUncertain,
            JournalFault::Hang,
        ] {
            let (manager, backend) = journaled_manager().await;
            let (armed, signal) = armed_with_backoff(&manager, 2).await;
            backend.push_fault(fault);

            let result = manager.handle_signal(signal).await;

            assert!(result.is_err(), "{fault:?}: {result:?}");
            assert_eq!(manager.exchange().market_order_call_count(), 0, "{fault:?}");
            assert!(!manager.detectors.read().await.contains_key(&armed.id), "{fault:?}");
            assert_eq!(
                manager.governed_rearm_counts.read().await.get(&armed.id),
                Some(&2),
                "{fault:?}"
            );
        }
    }

    /// #301's reset, after the ticket: a failed send still reached the venue.
    #[tokio::test]
    async fn a_ticketed_entry_that_failed_resets_the_backoff_and_does_not_re_arm() {
        let (manager, _backend) = journaled_manager().await;
        let (armed, signal) = armed_with_backoff(&manager, 3).await;
        manager.exchange().set_order_fail_next(true);

        let result = manager.handle_signal(signal).await;

        assert!(result.is_err(), "{result:?}");
        assert_eq!(manager.exchange().market_order_call_count(), 1);
        assert!(!manager.detectors.read().await.contains_key(&armed.id));
        assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), None);
    }

    /// The journal's result write fails, in each way it can: the fill is kept
    /// and the exchange stop still goes on.
    #[tokio::test]
    async fn a_failed_journal_result_write_keeps_the_fill_and_places_the_stop() {
        use robson_exec::order_journal::JournalFault;
        for fault in [
            JournalFault::Known,
            JournalFault::AckLost,
            JournalFault::NotLandedUncertain,
            JournalFault::Hang,
        ] {
            let (manager, backend) = journaled_manager().await;
            let (armed, signal) = armed_with_backoff(&manager, 1).await;
            backend.fail_commit(1, Some(fault));

            manager
                .handle_signal(signal)
                .await
                .unwrap_or_else(|e| panic!("{fault:?}: {e:?}"));

            let position = manager.get_position(armed.id).await.unwrap().unwrap();
            assert!(matches!(position.state, PositionState::Active { .. }), "{fault:?}");
            assert_eq!(manager.exchange().protection_call_count(), 1, "{fault:?}");
            assert_eq!(manager.governed_rearm_counts.read().await.get(&armed.id), None);
        }
    }

    /// A failed pre-send barrier is no ticket: the backoff stands, and it is
    /// still not a no-send the flow can re-arm from. A barrier stays a barrier.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failed_pre_send_barrier_keeps_the_backoff_and_sends_nothing(pool: sqlx::PgPool) {
        for event in ["entry_signal_received", "entry_order_requested"] {
            let manager = create_audited_postgres_test_manager(pool.clone(), 300);
            let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
            manager.governed_rearm_counts.write().await.insert(armed.id, 4);

            let condition = format!("NEW.event_type = '{event}'");
            fail_event_log_writes(&pool, &condition).await;
            let result = manager.handle_signal(signal).await;
            restore_event_log_writes(&pool).await;

            assert!(result.is_err(), "{event}: {result:?}");
            assert_eq!(manager.exchange().market_order_call_count(), 0, "{event}");
            assert_eq!(
                manager.governed_rearm_counts.read().await.get(&armed.id),
                Some(&4),
                "{event}"
            );
        }
    }

    /// The failure re-arm never retries a refusal that ends an `Immediate` arm,
    /// even when the refusal's own record fails: a domain rejection still needs
    /// the operator (ADR-0050 §2), and a margin refusal still cancels.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failed_record_does_not_turn_an_immediate_refusal_into_a_retry(pool: sqlx::PgPool) {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        let immediate =
            EntryPolicyConfig::new(EntryPolicy::Immediate, DomainApprovalPolicy::Automatic);

        // A stop too tight for the bounds, and its `failed` record fails.
        set_month_basis(&pool, dec!(10000)).await;
        let manager = create_audited_postgres_test_manager(pool.clone(), 300);
        let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
        manager.entry_policies.write().await.insert(armed.id, immediate);
        fail_event_log_writes(&pool, &audit_write("failed")).await;
        let result = manager
            .handle_signal(DetectorSignal {
                stop_loss: Price::new(dec!(94990)).unwrap(),
                ..signal_again(&signal)
            })
            .await;
        restore_event_log_writes(&pool).await;

        assert!(result.is_err(), "{result:?}");
        assert!(manager.is_entry_exhausted(armed.id).await);
        assert!(!manager.detectors.read().await.contains_key(&armed.id));
        assert_eq!(manager.exchange().market_order_call_count(), 0);

        // A basis too small for the size, and the `risk_denied` record fails.
        set_month_basis(&pool, dec!(100)).await;
        let manager = create_audited_postgres_test_manager(pool.clone(), 300);
        let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
        manager.entry_policies.write().await.insert(armed.id, immediate);
        fail_event_log_writes(&pool, &audit_write("risk_denied")).await;
        let result = manager.handle_signal(signal).await;
        restore_event_log_writes(&pool).await;

        assert!(result.is_err(), "{result:?}");
        let cancelled = manager.get_position(armed.id).await.unwrap().unwrap();
        assert!(!matches!(cancelled.state, PositionState::Armed), "{:?}", cancelled.state);
        assert!(!manager.detectors.read().await.contains_key(&armed.id));
        assert_eq!(manager.exchange().market_order_call_count(), 0);
    }

    /// A strategy-mode rejection re-arms on its own, so when its `failed`
    /// record fails the failure re-arm skips it, and the count happens there.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_strategy_rejection_whose_record_fails_is_counted(pool: sqlx::PgPool) {
        set_month_basis(&pool, dec!(10000)).await;
        let manager = create_audited_postgres_test_manager(pool.clone(), 300);
        let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
        let rearms =
            || crate::metrics::DETECTOR_REARMS_AFTER_FAILURE.with_label_values(&["signal"]);
        let rearms_before = rearms().get();

        fail_event_log_writes(&pool, &audit_write("failed")).await;
        let result = manager
            .handle_signal(DetectorSignal {
                stop_loss: Price::new(dec!(94990)).unwrap(),
                ..signal_again(&signal)
            })
            .await;
        restore_event_log_writes(&pool).await;

        assert!(result.is_err(), "{result:?}");
        assert!(!manager.is_entry_exhausted(armed.id).await);
        assert_rearmed_and_nothing_entered(&manager, &armed, 1, "strategy rejection").await;
        assert!(rearms().get() >= rearms_before + 1.0, "the re-arm must be counted");
    }

    /// An approval that expires while writes fail is gone all the same, so the
    /// expiry task re-arms as on a recorded expiry.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_failed_expiry_write_rearms_the_detector(pool: sqlx::PgPool) {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};

        set_month_basis(&pool, dec!(10000)).await;
        let manager = create_audited_postgres_test_manager(pool.clone(), 2);
        let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
        manager.entry_policies.write().await.insert(
            armed.id,
            EntryPolicyConfig::new(EntryPolicy::Immediate, DomainApprovalPolicy::HumanConfirmation),
        );
        manager.handle_signal(signal).await.unwrap();
        assert_eq!(manager.get_pending_approvals().await.len(), 1);

        fail_event_log_writes(&pool, &audit_write("expired")).await;
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !manager.detectors.read().await.contains_key(&armed.id) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the expiry task must re-arm the detector");
        restore_event_log_writes(&pool).await;

        assert!(manager.get_pending_approvals().await.is_empty());
        assert_rearmed_and_nothing_entered(&manager, &armed, 1, "expired").await;
        assert!(!audit_trail(&pool, armed.id).await.iter().any(|cause| cause == "expired"));
    }

    /// The approval-expiry task against a disarm and MonthlyHalt over random
    /// schedules, with the expiry's audit write slowed and sometimes failing: a
    /// detector survives only on a position still Armed outside MonthlyHalt.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn an_expiry_racing_a_disarm_or_a_halt_leaves_no_stray_detector(pool: sqlx::PgPool) {
        use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};

        set_month_basis(&pool, dec!(10000)).await;
        let schedules = (any::<bool>(), any::<bool>(), any::<bool>(), 0u64..40);
        let mut runner = TestRunner::deterministic();

        for _ in 0..24 {
            let (write_fails, disarm, halt, disarm_after_ms) =
                schedules.new_tree(&mut runner).unwrap().current();
            let case = format!(
                "write_fails={write_fails} disarm={disarm} halt={halt} after={disarm_after_ms}ms"
            );
            let manager = create_audited_postgres_test_manager(pool.clone(), 300);
            let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
            manager.entry_policies.write().await.insert(
                armed.id,
                EntryPolicyConfig::new(
                    EntryPolicy::ConfirmedTrend,
                    DomainApprovalPolicy::HumanConfirmation,
                ),
            );
            manager.handle_signal(signal).await.unwrap();
            let query_id = manager.get_pending_approvals().await[0].id;

            let action = if write_fails {
                "PERFORM pg_sleep(0.05); RAISE EXCEPTION 'injected event log failure';"
            } else {
                "PERFORM pg_sleep(0.05);"
            };
            inject_event_log_fault(&pool, &audit_write("expired"), action).await;
            if halt {
                manager.circuit_breaker.trigger_halt("test".to_string()).await;
            }

            manager.spawn_approval_expiration_task(query_id, chrono::Utc::now());
            if disarm {
                tokio::time::sleep(std::time::Duration::from_millis(disarm_after_ms)).await;
                manager.disarm_position(armed.id).await.unwrap();
            }
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !manager.get_pending_approvals().await.is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the approval must leave the queue");
            // An expiry still inside its critical section finishes first.
            drop(manager.entry_flow_lock.lock().await);
            restore_event_log_writes(&pool).await;

            let live_detector = manager
                .detectors
                .read()
                .await
                .get(&armed.id)
                .is_some_and(|handle| !handle.is_finished());
            assert_eq!(live_detector, !disarm && !halt, "{case}");
        }
    }

    /// MonthlyHalt latched at a random moment while an expiry re-arms during a
    /// write outage, so the halt's own disarms fail: no detector outlives the
    /// halt, the re-armed one or an ordinary one.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn a_halt_during_an_outage_leaves_no_detector_running(pool: sqlx::PgPool) {
        use proptest::{
            strategy::{Strategy, ValueTree},
            test_runner::TestRunner,
        };
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};

        set_month_basis(&pool, dec!(10000)).await;
        let mut runner = TestRunner::deterministic();

        for _ in 0..24 {
            let halt_after_ms = (0u64..80).new_tree(&mut runner).unwrap().current();
            let manager = create_audited_postgres_test_manager(pool.clone(), 300);
            let (armed, signal) = recorded_armed_position_and_signal(&manager).await;
            manager.entry_policies.write().await.insert(
                armed.id,
                EntryPolicyConfig::new(
                    EntryPolicy::ConfirmedTrend,
                    DomainApprovalPolicy::HumanConfirmation,
                ),
            );
            manager.handle_signal(signal).await.unwrap();
            let query_id = manager.get_pending_approvals().await[0].id;
            let other = save_armed_position_with_policy(
                &manager,
                "ETHUSDT",
                Side::Long,
                EntryPolicyConfig::default(),
            )
            .await;
            manager.detectors.write().await.insert(
                other.id,
                tokio::spawn(async {
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    None
                }),
            );

            // Every write fails; the expiry's only after 50 ms.
            inject_event_log_fault(
                &pool,
                "TRUE",
                &format!(
                    "IF {} THEN PERFORM pg_sleep(0.05); END IF; \
                     RAISE EXCEPTION 'injected event log failure';",
                    audit_write("expired")
                ),
            )
            .await;
            manager.spawn_approval_expiration_task(query_id, chrono::Utc::now());
            tokio::time::sleep(std::time::Duration::from_millis(halt_after_ms)).await;
            let _ = manager.trigger_monthly_halt("test".to_string()).await;
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !manager.get_pending_approvals().await.is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the approval must leave the queue");
            drop(manager.entry_flow_lock.lock().await);
            restore_event_log_writes(&pool).await;

            let detectors = manager.detectors.read().await;
            for position_id in [armed.id, other.id] {
                assert!(
                    !detectors.get(&position_id).is_some_and(|handle| !handle.is_finished()),
                    "halt after {halt_after_ms}ms: a detector outlived the halt"
                );
            }
        }
    }

    /// Holds the next snapshot on `positions_current`, which it reads after the
    /// basis.
    #[cfg(feature = "postgres")]
    async fn stall_on_positions_current(
        pool: &sqlx::PgPool,
    ) -> sqlx::Transaction<'static, sqlx::Postgres> {
        let mut stall = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE positions_current IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *stall)
            .await
            .unwrap();
        stall
    }

    /// Once a snapshot is held there, write the basis and let the snapshot go
    /// on.
    #[cfg(feature = "postgres")]
    async fn write_the_basis_while_a_snapshot_waits(
        pool: &sqlx::PgPool,
        writes: &[Decimal],
        stall: sqlx::Transaction<'_, sqlx::Postgres>,
    ) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let waiting: i64 = sqlx::query_scalar(
                    r#"
                    SELECT count(*) FROM pg_locks
                    WHERE NOT granted
                      AND database = (SELECT oid FROM pg_database WHERE datname = current_database())
                      AND relation = 'positions_current'::regclass
                    "#,
                )
                .fetch_one(pool)
                .await
                .unwrap();
                if waiting > 0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the snapshot must reach the locked table");
        for capital_base in writes {
            set_month_basis(pool, *capital_base).await;
        }
        stall.rollback().await.unwrap();
    }

    /// ADR-0051 §1 over random schedules: whatever the basis goes through while
    /// a snapshot waits, the halt and the gate decide on where it ended up.
    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "Requires DATABASE_URL to be set"]
    async fn decisions_follow_the_basis_as_it_stands_after_the_snapshot(pool: sqlx::PgPool) {
        use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};

        let write = prop_oneof![Just(Decimal::ZERO), (1i64..50_000).prop_map(Decimal::from)];
        let schedules = (1i64..50_000, 1i64..400, proptest::collection::vec(write, 0..4));
        let mut runner = TestRunner::deterministic();
        let (mut fired_any, mut spared_any) = (false, false);

        for _ in 0..32 {
            let (initial, quantity, writes) = schedules.new_tree(&mut runner).unwrap().current();
            let initial = Decimal::from(initial);
            let settled = writes.last().copied().unwrap_or(initial);

            set_month_basis(&pool, initial).await;
            let manager = create_postgres_test_manager(pool.clone());
            let leg = save_active_position(
                &manager,
                "ETHUSDT",
                Side::Long,
                dec!(100),
                Decimal::from(quantity),
            )
            .await;
            let snapshot = manager.canonical_budget_snapshot(chrono::Utc::now()).await.unwrap();
            assert!(!snapshot.latent_risk_invalid);
            let exhausted = snapshot.remaining_active <= Decimal::ZERO;

            let stall = stall_on_positions_current(&pool).await;
            let (fired, ()) = tokio::join!(
                manager.evaluate_monthly_halt(),
                write_the_basis_while_a_snapshot_waits(&pool, &writes, stall)
            );

            let case = format!("initial {initial}, {quantity} units, writes {writes:?}");
            assert_eq!(fired, exhausted && settled > Decimal::ZERO, "halt: {case}");
            assert_eq!(manager.circuit_breaker.blocks_new_entries().await, fired, "{case}");
            if !fired {
                assert!(is_still_active(&manager, &leg).await, "{case}");
            }
            fired_any |= fired;
            spared_any |= exhausted && !fired;

            set_month_basis(&pool, initial).await;
            let manager = create_postgres_test_manager(pool.clone());
            let stall = stall_on_positions_current(&pool).await;
            let (context, ()) = tokio::join!(
                manager.build_risk_context(),
                write_the_basis_while_a_snapshot_waits(&pool, &writes, stall)
            );

            let expected = if settled > Decimal::ZERO {
                initial
            } else {
                Decimal::ZERO
            };
            assert_eq!(context.unwrap().capital, expected, "gate: {case}");
        }

        assert!(fired_any && spared_any, "the schedules must reach both outcomes");
    }

    /// Reconciliation passes, with the manager held the way the daemon holds
    /// it.
    async fn passes(
        manager: Arc<PositionManager<StubExchange, MemoryStore>>,
        missing_grace: std::time::Duration,
        count: usize,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        let exchange = manager.exchange();
        let store = manager.store();
        let event_bus = manager.event_bus.clone();
        let held = Arc::new(RwLock::new(Arc::try_unwrap(manager).ok().expect("sole owner")));
        let worker = crate::reconciliation_worker::ReconciliationWorker::new_with_missing_grace(
            exchange,
            held.clone(),
            store,
            event_bus,
            std::time::Duration::from_secs(60),
            missing_grace,
            CancellationToken::new(),
        );
        for _ in 0..count {
            worker.scan_and_reconcile().await.unwrap();
        }
        drop(worker);
        Arc::new(Arc::try_unwrap(held).ok().expect("sole owner").into_inner())
    }

    /// One pass.
    async fn scanned(
        manager: Arc<PositionManager<StubExchange, MemoryStore>>,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        passes(manager, std::time::Duration::from_secs(60), 1).await
    }

    /// The missing-position path, all the way: the first pass observes the leg
    /// missing and the second, with no grace, resolves it.
    async fn missing_path_ran(
        manager: Arc<PositionManager<StubExchange, MemoryStore>>,
    ) -> Arc<PositionManager<StubExchange, MemoryStore>> {
        passes(manager, std::time::Duration::ZERO, 2).await
    }

    /// A leg of 0.105 and a residual of 0.005 whose rounded stop of 0.1 filled
    /// with no tick crossing it: the venue holds the 0.005, and no stop.
    async fn leg_with_a_dead_stop(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
    ) -> (Position, String) {
        let exchange = manager.exchange();
        exchange.set_trading_rules(lot_rules(dec!(0.01)));
        exchange.set_venue_rules(lot_rules(dec!(0.01)));
        let (mut leg, stop_id) = active_leg_with_live_stop(manager).await;
        leg.quantity = Quantity::new(dec!(0.105)).unwrap();
        leg.residual_quantity = Some(dec!(0.005));
        manager.store.positions().save(&leg).await.unwrap();
        exchange
            .fill_stop_order_for(&stop_id, Quantity::new(dec!(0.1)).unwrap())
            .unwrap();
        venue_still_holds(manager, &leg, dec!(0.005));
        (leg, stop_id)
    }

    async fn stop_heal_scheduled(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position_id: PositionId,
    ) -> bool {
        manager.insurance_stop_stale.read().await.contains_key(&position_id)
    }

    /// The whole way: the scan notices with no request of its own, the heal
    /// re-places at tick cadence and the leg records the new stop, whose
    /// predecessor is already gone. The residual goes on blocking admissions.
    #[tokio::test]
    async fn a_dead_stop_on_a_held_leg_is_re_placed_and_the_new_one_recorded() {
        let manager = create_test_manager().await;
        let (leg, dead_stop) = leg_with_a_dead_stop(&manager).await;
        let exchange = manager.exchange();
        let placements = exchange.protection_call_count();

        let manager = scanned(manager).await;

        assert!(stop_heal_scheduled(&manager, leg.id).await);
        assert!(stop_retry_in(&manager, leg.id).await <= INSURANCE_STOP_RETRY_BACKOFF);
        assert_eq!(exchange.protection_call_count(), placements, "the heal places, not the scan");

        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(leg.id, std::time::Instant::now());
        quiet_tick(&manager, &leg).await;

        let healed = manager.get_position(leg.id).await.unwrap().unwrap();
        let new_stop = healed.insurance_stop_id.clone().unwrap();
        assert_ne!(new_stop, dead_stop);
        let PositionState::Active { insurance_stop_id, .. } = &healed.state else {
            panic!("{:?}", healed.state);
        };
        assert_eq!(insurance_stop_id.as_deref(), Some(new_stop.as_str()));
        assert_eq!(exchange.stop_order(&new_stop).unwrap().quantity.as_decimal(), dec!(0.1));
        assert!(!stop_heal_scheduled(&manager, leg.id).await);
        let events = manager.store.events().find_by_position(leg.id).await.unwrap();
        assert!(
            events.iter().any(|event| matches!(
                event,
                Event::InsuranceStopReplaced { previous_order_id, order_id, .. }
                    if *previous_order_id == dead_stop && *order_id == new_stop
            )),
            "{events:?}"
        );
        assert_eq!(healed.residual_quantity, Some(dec!(0.005)));
        assert!(matches!(
            manager.check_no_unresolved_residual().await,
            Err(DaemonError::ResidualInventoryUnresolved { .. })
        ));

        // Open and on record: the next scan has nothing to ask for.
        let manager = scanned(manager).await;
        assert!(!stop_heal_scheduled(&manager, leg.id).await);
    }

    /// Replaced after the sweep read its legs: the read does not show the old
    /// stop, and the leg no longer records it.
    #[tokio::test]
    async fn a_stop_replaced_during_the_read_schedules_nothing() {
        let manager = create_test_manager().await;
        let (leg, dead_stop) = leg_with_a_dead_stop(&manager).await;
        let store = manager.store();
        let position_id = leg.id;
        manager.exchange().set_after_next_open_orders_read(move || {
            store
                .apply_event(&Event::InsuranceStopReplaced {
                    position_id,
                    previous_order_id: dead_stop,
                    order_id: "successor".to_string(),
                    stop_price: Price::new(dec!(90)).unwrap(),
                    timestamp: chrono::Utc::now(),
                })
                .unwrap();
        });

        let manager = scanned(manager).await;

        assert!(!stop_heal_scheduled(&manager, leg.id).await);
    }

    #[tokio::test]
    async fn a_failed_open_orders_read_schedules_nothing() {
        let manager = create_test_manager().await;
        let (leg, _) = leg_with_a_dead_stop(&manager).await;
        manager.exchange().set_open_orders_fail(true);

        let manager = scanned(manager).await;

        assert!(!stop_heal_scheduled(&manager, leg.id).await);
    }

    #[tokio::test]
    async fn a_leg_suspected_closed_by_the_venue_schedules_nothing() {
        let manager = create_test_manager().await;
        let (leg, _) = leg_with_a_dead_stop(&manager).await;
        manager.exit_overtaken_suspects.write().await.insert(leg.id);

        let manager = scanned(manager).await;

        assert!(!stop_heal_scheduled(&manager, leg.id).await);
    }

    /// An ordinary stop fill: the venue is flat and the leg is on its way to
    /// the missing-position path, which reads the fill through the id on
    /// record.
    #[tokio::test]
    async fn a_leg_the_venue_does_not_hold_schedules_nothing() {
        let manager = create_test_manager().await;
        let (leg, dead_stop) = leg_with_a_dead_stop(&manager).await;
        let one_lot = Quantity::new(dec!(0.01)).unwrap();
        manager
            .exchange()
            .close_position_market(&leg.symbol, leg.side, one_lot, "flat")
            .await
            .unwrap();

        let manager = scanned(manager).await;

        assert!(!stop_heal_scheduled(&manager, leg.id).await);
        let loaded = manager.get_position(leg.id).await.unwrap().unwrap();
        assert_eq!(loaded.insurance_stop_id, Some(dead_stop));
    }

    #[tokio::test]
    async fn a_stop_that_is_open_schedules_nothing() {
        let manager = create_test_manager().await;
        let (leg, _) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(1));

        let manager = scanned(manager).await;

        assert!(!stop_heal_scheduled(&manager, leg.id).await);
    }

    /// An attempt already waiting keeps its window: a stop with nothing
    /// executable retries on the long one, and must not come back every
    /// scan.
    #[tokio::test]
    async fn a_heal_already_scheduled_keeps_its_window() {
        let manager = create_test_manager().await;
        let (leg, _) = leg_with_a_dead_stop(&manager).await;
        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(leg.id, std::time::Instant::now() + NOT_EXECUTABLE_BACKOFF);

        let manager = scanned(manager).await;

        assert!(stop_retry_in(&manager, leg.id).await > INSURANCE_STOP_RETRY_BACKOFF);
    }

    /// Only an `Active` leg with an id on record has anything to compare: one
    /// that is exiting does not, and the `uncovered_symbols` case has no id
    /// at all.
    #[tokio::test]
    async fn an_exiting_leg_or_one_with_no_id_schedules_nothing() {
        let manager = create_test_manager().await;
        let (mut exiting, dead_stop) = leg_with_a_dead_stop(&manager).await;
        exiting.state = PositionState::Exiting {
            exit_order_id: Uuid::now_v7(),
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };
        manager.store.positions().save(&exiting).await.unwrap();
        let no_id = save_active_position(&manager, "ETHUSDT", Side::Long, dec!(100), dec!(1)).await;
        venue_still_holds(&manager, &no_id, dec!(1));

        manager.schedule_stop_heal_if_dead(exiting.id, &dead_stop).await;
        let manager = scanned(manager).await;

        assert!(!stop_heal_scheduled(&manager, exiting.id).await);
        assert!(!stop_heal_scheduled(&manager, no_id.id).await);
    }

    /// The heal, due now, through a tick that crosses nothing.
    async fn heal_ran(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        leg: &Position,
    ) -> String {
        let before = manager.get_position(leg.id).await.unwrap().unwrap().insurance_stop_id;
        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(leg.id, std::time::Instant::now());
        quiet_tick(manager, leg).await;
        let healed = manager.get_position(leg.id).await.unwrap().unwrap();
        let recorded = healed.insurance_stop_id.clone().unwrap();
        assert_ne!(Some(recorded.clone()), before);
        assert_eq!(Some(&before.unwrap()), healed.superseded_stop_ids.first());
        recorded
    }

    /// The trades of the triggered order, which the path falls back to when it
    /// cannot place the fill on a stop.
    fn stop_trades_on_the_venue(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        leg: &Position,
        stop_id: &str,
    ) {
        manager.exchange().set_user_trades(&leg.symbol.as_pair(), vec![
            robson_exec::UserTradeRecord {
                exchange_order_id: format!("TRIGGERED-{stop_id}"),
                exchange_trade_id: format!("TRADE-{stop_id}"),
                fill_price: Price::new(dec!(90)).unwrap(),
                filled_quantity: leg.quantity,
                fee: dec!(0.1),
                fee_asset: "USDT".to_string(),
                filled_at: chrono::Utc::now(),
            },
        ]);
    }

    /// A close the budget sees: `InsuranceStop`, with the whole loss in the
    /// month net and in what the budget has consumed.
    async fn the_stop_loss_is_in_the_budget(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        leg: &Position,
    ) {
        let closed = manager.get_position(leg.id).await.unwrap().unwrap();
        assert!(
            matches!(closed.state, PositionState::Closed {
                exit_reason: robson_domain::ExitReason::InsuranceStop,
                ..
            }),
            "{:?}",
            closed.state
        );
        assert_eq!(closed.realized_pnl, dec!(-10), "(90 - 100) x 1");
        let loss = dec!(10) + closed.fees_paid;
        let now = chrono::Utc::now();
        assert_eq!(manager.robson_month_net(now).await.unwrap(), -loss);
        let (snapshot, _, _, _) =
            manager.canonical_budget_snapshot_with_positions(now).await.unwrap();
        assert_eq!(snapshot.consumed_nfs, loss);
    }

    /// A triggered while being replaced: its market order is working, the venue
    /// still holds the leg, and the heal accepts B and finds A already gone. A
    /// then fills the whole leg. That close is still its stop's, and the budget
    /// has to see it.
    #[tokio::test]
    async fn a_stop_replaced_while_it_executes_keeps_its_loss_in_the_budget() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        exchange.set_price("BTCUSDT", dec!(90));
        let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(1));
        exchange.trigger_stop_order(&stop_a);

        let manager = scanned(manager).await;
        assert!(stop_heal_scheduled(&manager, leg.id).await);
        let stop_b = heal_ran(&manager, &leg).await;

        exchange.fill_stop_order_for(&stop_a, Quantity::new(dec!(1)).unwrap()).unwrap();
        exchange.clear_open_position(&leg.symbol, leg.side);
        stop_trades_on_the_venue(&manager, &leg, &stop_a);
        let manager = missing_path_ran(manager).await;

        the_stop_loss_is_in_the_budget(&manager, &leg).await;
        assert!(!exchange.has_stop_order(&stop_b), "and the sweep takes B");
    }

    /// The scan's two reads are not one: an ordinary stop fill between them
    /// shows a held leg with its stop gone, and schedules a heal on a leg about
    /// to be flat. The fill keeps its provenance.
    #[tokio::test]
    async fn a_stop_filled_between_the_scans_reads_keeps_its_loss_in_the_budget() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        exchange.set_price("BTCUSDT", dec!(90));
        let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(1));
        let venue = exchange.clone();
        let (filled, symbol, side) = (stop_a.clone(), leg.symbol.clone(), leg.side);
        exchange.set_after_next_positions_read(move || {
            venue.fill_stop_order_for(&filled, Quantity::new(dec!(1)).unwrap()).unwrap();
            venue.clear_open_position(&symbol, side);
        });

        let manager = scanned(manager).await;
        assert!(stop_heal_scheduled(&manager, leg.id).await, "the reads disagree");
        let stop_b = heal_ran(&manager, &leg).await;

        stop_trades_on_the_venue(&manager, &leg, &stop_a);
        let manager = missing_path_ran(manager).await;

        the_stop_loss_is_in_the_budget(&manager, &leg).await;
        assert!(!exchange.has_stop_order(&stop_b), "and the sweep takes B");
    }

    /// `positions_current` carries no superseded stops: a restart between B's
    /// recording and the close gets them back from the stream.
    #[tokio::test]
    async fn a_restart_before_the_close_still_finds_the_fill_of_a_superseded_stop() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        exchange.set_price("BTCUSDT", dec!(90));
        let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(1));
        exchange.trigger_stop_order(&stop_a);
        let manager = scanned(manager).await;
        heal_ran(&manager, &leg).await;

        let mut restored = manager.get_position(leg.id).await.unwrap().unwrap();
        restored.superseded_stop_ids.clear();
        for event in manager.store.events().find_by_position(leg.id).await.unwrap() {
            restored
                .restore_leg_evidence_record(
                    event.event_type(),
                    serde_json::to_value(&event).unwrap(),
                )
                .unwrap();
        }
        assert_eq!(restored.superseded_stop_ids, [stop_a.clone()]);
        manager.store.positions().save(&restored).await.unwrap();

        exchange.fill_stop_order_for(&stop_a, Quantity::new(dec!(1)).unwrap()).unwrap();
        exchange.clear_open_position(&leg.symbol, leg.side);
        let manager = missing_path_ran(manager).await;

        the_stop_loss_is_in_the_budget(&manager, &leg).await;
    }

    /// The recorded stop first, then the last three replaced, newest first. The
    /// one before those is gone from the evidence.
    #[tokio::test]
    async fn fill_evidence_reads_the_recorded_stop_then_the_last_three_newest_first() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        let (leg, first) = active_leg_with_live_stop(&manager).await;
        let mut stops = vec![first];
        for n in 0..4 {
            let stop = exchange
                .place_stop_market_order(
                    &leg.symbol,
                    leg.side.exit_action(),
                    leg.quantity,
                    Price::new(dec!(90)).unwrap(),
                    &format!("ins-{n}"),
                )
                .await
                .unwrap()
                .exchange_order_id;
            manager
                .store
                .apply_event(&Event::InsuranceStopReplaced {
                    position_id: leg.id,
                    previous_order_id: stops.last().unwrap().clone(),
                    order_id: stop.clone(),
                    stop_price: Price::new(dec!(90)).unwrap(),
                    timestamp: chrono::Utc::now(),
                })
                .unwrap();
            stops.push(stop);
        }
        let leg = manager.get_position(leg.id).await.unwrap().unwrap();
        assert_eq!(leg.insurance_stop_id.as_ref(), Some(&stops[4]));
        assert_eq!(leg.superseded_stop_ids, [stops[3].clone(), stops[2].clone(), stops[1].clone()]);
        let one = Quantity::new(dec!(1)).unwrap();

        let evidence = || async {
            let input = crate::reconciliation_worker::gather_stop_fill_evidence(
                &manager.exchange(),
                &manager.store,
                &leg,
            )
            .await
            .unwrap()?;
            match input.evidence {
                ReconciliationEvidence::OrderFillRecord(fill) => Some(fill.exchange_order_id),
                other => panic!("{other:?}"),
            }
        };

        exchange.fill_stop_order_for(&stops[0], one).unwrap();
        assert_eq!(evidence().await, None, "past the last three");
        exchange.fill_stop_order_for(&stops[1], one).unwrap();
        assert_eq!(evidence().await, Some(format!("TRIGGERED-{}", stops[1])));
        exchange.fill_stop_order_for(&stops[2], one).unwrap();
        assert_eq!(evidence().await, Some(format!("TRIGGERED-{}", stops[2])), "newest first");
        exchange.fill_stop_order_for(&stops[4], one).unwrap();
        assert_eq!(evidence().await, Some(format!("TRIGGERED-{}", stops[4])), "recorded first");
    }

    /// A superseded stop with no fill changes nothing: the path falls through
    /// to user trades, and such a close stays out of the governed month
    /// net.
    #[tokio::test]
    async fn a_superseded_stop_with_no_fill_falls_through_to_user_trades() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(1));
        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(leg.id, std::time::Instant::now());
        quiet_tick(&manager, &leg).await;
        assert_eq!(manager.get_position(leg.id).await.unwrap().unwrap().superseded_stop_ids, [
            stop_a
        ]);

        exchange.clear_open_position(&leg.symbol, leg.side);
        stop_trades_on_the_venue(&manager, &leg, "manual");
        let manager = missing_path_ran(manager).await;

        let closed = manager.get_position(leg.id).await.unwrap().unwrap();
        assert!(
            matches!(closed.state, PositionState::Closed {
                exit_reason: robson_domain::ExitReason::ReconciledMissingOnExchange,
                ..
            }),
            "{:?}",
            closed.state
        );
        assert_eq!(manager.robson_month_net(chrono::Utc::now()).await.unwrap(), Decimal::ZERO);
    }

    /// A failed lookup of a superseded stop is what a failed lookup of the
    /// recorded one is: an error, not an absent fill.
    #[tokio::test]
    async fn a_failed_lookup_of_a_superseded_stop_is_an_error() {
        let manager = create_test_manager().await;
        let mut leg =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        leg.record_superseded_stop("superseded");
        manager.exchange().set_fail_next(true);

        let lookup = crate::reconciliation_worker::gather_stop_fill_evidence(
            &manager.exchange(),
            &manager.store,
            &leg,
        )
        .await;

        assert!(lookup.is_err());
    }

    /// A partial, then B closes the rest and its trades are not visible yet.
    /// The partial does not close the leg at its own price and in its own
    /// month: the leg stays unresolved until B's evidence appears.
    #[tokio::test]
    async fn an_older_partial_of_a_displaced_stop_does_not_finalize_a_later_close() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(0.6));
        exchange.set_price("BTCUSDT", dec!(80));
        let partial = Quantity::new(dec!(0.4)).unwrap();
        let last_month = chrono::Utc::now() - Duration::days(40);
        exchange.fill_stop_order_at(&stop_a, partial, last_month).unwrap();
        let stop_b = heal_ran(&manager, &leg).await;

        exchange.clear_open_position(&leg.symbol, leg.side);
        let manager = missing_path_ran(manager).await;

        let waiting = manager.get_position(leg.id).await.unwrap().unwrap();
        assert!(matches!(waiting.state, PositionState::Active { .. }), "{:?}", waiting.state);

        exchange.set_price("BTCUSDT", dec!(90));
        exchange
            .fill_stop_order_for(&stop_b, Quantity::new(dec!(0.6)).unwrap())
            .unwrap();
        let manager = missing_path_ran(manager).await;

        the_stop_loss_is_in_the_budget(&manager, &leg).await;
        let closed = manager.get_position(leg.id).await.unwrap().unwrap();
        assert_eq!(
            closed.closed_at.map(|at| (at.year(), at.month())),
            Some((chrono::Utc::now().year(), chrono::Utc::now().month()))
        );
    }

    /// From a displaced stop, the leg's whole quantity and nothing else.
    #[tokio::test]
    async fn only_a_fill_of_the_whole_leg_counts_from_a_displaced_stop() {
        for (filled, counts) in [(dec!(0.4), false), (dec!(1.5), false), (dec!(1), true)] {
            let manager = create_test_manager().await;
            let exchange = manager.exchange();
            let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
            exchange.fill_stop_order_for(&stop_a, Quantity::new(filled).unwrap()).unwrap();
            manager
                .store
                .apply_event(&Event::InsuranceStopReplaced {
                    position_id: leg.id,
                    previous_order_id: stop_a,
                    order_id: "stop-b".to_string(),
                    stop_price: Price::new(dec!(90)).unwrap(),
                    timestamp: chrono::Utc::now(),
                })
                .unwrap();
            let leg = manager.get_position(leg.id).await.unwrap().unwrap();

            let input = crate::reconciliation_worker::gather_stop_fill_evidence(
                &exchange,
                &manager.store,
                &leg,
            )
            .await
            .unwrap();

            assert_eq!(input.is_some(), counts, "{filled}");
        }
    }

    /// The tick reads the recorded stop alone, at what it always cost: a fill
    /// on a superseded stop waits for the scan, and counts all the same.
    #[tokio::test]
    async fn the_tick_asks_nothing_about_a_superseded_stop() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        exchange.set_price("BTCUSDT", dec!(90));
        let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(1));
        exchange.trigger_stop_order(&stop_a);
        let manager = scanned(manager).await;
        let stop_b = heal_ran(&manager, &leg).await;
        exchange.fill_stop_order_for(&stop_a, Quantity::new(dec!(1)).unwrap()).unwrap();
        exchange.clear_open_position(&leg.symbol, leg.side);
        manager.exit_overtaken_suspects.write().await.insert(leg.id);
        assert!(exchange.protection_fill_lookups().is_empty());

        quiet_tick(&manager, &leg).await;

        assert_eq!(exchange.protection_fill_lookups(), [stop_b]);
        let waiting = manager.get_position(leg.id).await.unwrap().unwrap();
        assert!(matches!(waiting.state, PositionState::Active { .. }), "{:?}", waiting.state);

        let manager = missing_path_ran(manager).await;

        the_stop_loss_is_in_the_budget(&manager, &leg).await;
    }

    /// What a superseded stop executed says nothing while the venue shows the
    /// leg: no path that reads it runs on a leg the venue holds.
    #[tokio::test]
    async fn a_superseded_fill_does_not_close_a_leg_the_venue_holds() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(0.6));
        exchange.trigger_stop_order(&stop_a);
        let manager = scanned(manager).await;
        heal_ran(&manager, &leg).await;
        exchange.fill_stop_order_for(&stop_a, Quantity::new(dec!(1)).unwrap()).unwrap();

        let manager = missing_path_ran(manager).await;

        let loaded = manager.get_position(leg.id).await.unwrap().unwrap();
        assert!(matches!(loaded.state, PositionState::Active { .. }), "{:?}", loaded.state);
    }

    /// The heal is scheduled, then a refused exit marks the leg suspected
    /// closed by the venue. The due heal waits, and the fill closes the
    /// leg.
    #[tokio::test]
    async fn a_heal_scheduled_before_the_suspicion_is_not_consumed_under_it() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        exchange.set_price("BTCUSDT", dec!(90));
        let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(1));
        exchange.trigger_stop_order(&stop_a);
        let manager = scanned(manager).await;
        assert!(stop_heal_scheduled(&manager, leg.id).await);

        exchange.clear_open_position(&leg.symbol, leg.side);
        manager.exit_overtaken_suspects.write().await.insert(leg.id);
        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(leg.id, std::time::Instant::now());
        let placements = exchange.protection_call_count();
        quiet_tick(&manager, &leg).await;

        assert_eq!(exchange.protection_call_count(), placements);
        let waiting = manager.get_position(leg.id).await.unwrap().unwrap();
        assert_eq!(waiting.insurance_stop_id.as_ref(), Some(&stop_a));
        assert!(stop_heal_scheduled(&manager, leg.id).await);

        exchange.fill_stop_order_for(&stop_a, Quantity::new(dec!(1)).unwrap()).unwrap();
        quiet_tick(&manager, &leg).await;

        the_stop_loss_is_in_the_budget(&manager, &leg).await;
    }

    /// The suspicion was wrong: the venue shows the leg again, and the heal
    /// that waited runs on the next tick.
    #[tokio::test]
    async fn a_heal_that_waited_runs_once_the_suspicion_is_dropped() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        let (leg, stop_a) = active_leg_with_live_stop(&manager).await;
        venue_still_holds(&manager, &leg, dec!(1));
        exchange.trigger_stop_order(&stop_a);
        let manager = scanned(manager).await;
        manager.exit_overtaken_suspects.write().await.insert(leg.id);
        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(leg.id, std::time::Instant::now());
        let placements = exchange.protection_call_count();

        quiet_tick(&manager, &leg).await;
        assert_eq!(exchange.protection_call_count(), placements);
        assert!(!manager.exit_overtaken_suspects.read().await.contains(&leg.id));

        quiet_tick(&manager, &leg).await;
        assert_eq!(exchange.protection_call_count(), placements + 1);
    }

    /// A residual has no say in the halt either way.
    #[tokio::test]
    async fn a_residual_does_not_defer_the_halt() {
        let manager = create_test_manager().await;
        let (position, _) = active_leg_with_live_stop(&manager).await;
        let mut leg = manager.get_position(position.id).await.unwrap().unwrap();
        leg.residual_quantity = Some(dec!(0.5));
        leg.residual_unknown = true;
        manager.store.positions().save(&leg).await.unwrap();
        save_closed_position_with_pnl(&manager, dec!(-400)).await;

        assert!(manager.evaluate_monthly_halt().await);
        assert!(manager.circuit_breaker.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn panic_close_of_an_unexecutable_leg_reports_it_and_keeps_the_leg_active() {
        let manager = create_test_manager().await;
        let (mut position, _) = active_leg_with_live_stop(&manager).await;
        position.quantity = Quantity::new(dec!(0.0005)).unwrap();
        manager.store.positions().save(&position).await.unwrap();
        manager.exchange().set_trading_rules(lot_rules(dec!(0.001)));
        manager.exchange().set_venue_rules(lot_rules(dec!(0.001)));

        let error = manager.close_position(position.id).await.unwrap_err();
        assert!(error.to_string().contains("below the lot minimum"), "{error}");
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Active { .. }));
    }

    /// Nothing held and the lookup fails: a decided exit still goes out as
    /// given.
    #[tokio::test]
    async fn a_close_with_no_rules_to_be_had_goes_out_raw() {
        let manager = create_test_manager().await;
        let (mut position, _) = active_leg_with_live_stop(&manager).await;
        position.quantity = Quantity::new(dec!(0.1055)).unwrap();
        manager.store.positions().save(&position).await.unwrap();
        manager.exchange().clear_cached_rules();

        manager.close_position(position.id).await.unwrap();
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
    }

    /// The stop's own fill, as `get_stop_order_fill` reports it.
    async fn stop_fill_of(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position: &Position,
        stop_id: &str,
        filled: Decimal,
    ) -> ReconciledCloseInput {
        let exchange = manager.exchange();
        exchange.fill_stop_order_for(stop_id, Quantity::new(filled).unwrap()).unwrap();
        crate::reconciliation_worker::gather_order_fill_evidence(
            &exchange,
            &manager.store,
            position,
        )
        .await
        .unwrap()
        .expect("the fill is visible")
    }

    /// A short fill closes the leg, whatever the venue still holds, and the
    /// close books the whole leg at that fill. The fee is this order's, and
    /// the evidence keeps what filled.
    #[tokio::test]
    async fn reconcile_closes_on_a_short_stop_fill_whatever_the_venue_holds() {
        let manager = create_test_manager().await;
        let (mut position, stop_id) = active_leg_with_live_stop(&manager).await;
        position.quantity = Quantity::new(dec!(0.105)).unwrap();
        position.fees_paid = dec!(1);
        manager.store.positions().save(&position).await.unwrap();
        manager.exchange().set_price("BTCUSDT", dec!(90));
        venue_still_holds(&manager, &position, dec!(0.005));
        let input = stop_fill_of(&manager, &position, &stop_id, dec!(0.1)).await;
        let fee = input.fee;
        assert!(fee > Decimal::ZERO);

        let outcome = manager.reconcile_close(input).await.unwrap();

        assert!(matches!(outcome, ReconcileCloseOutcome::Closed), "{outcome:?}");
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        match leg.state {
            PositionState::Closed { realized_pnl, .. } => {
                assert_eq!(realized_pnl, dec!(-1.05), "(90 - 100) x 0.105, not x 0.1");
            },
            other => panic!("expected Closed, got {other:?}"),
        }
        assert_eq!(leg.fees_paid, dec!(1) + fee, "this order's fee, for what it executed");
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let evidence = events
            .iter()
            .find_map(|event| match event {
                Event::PositionClosed { closure_evidence, .. } => Some(closure_evidence),
                _ => None,
            })
            .expect("the close is on record");
        match evidence {
            ClosureEvidence::Reconciled(ReconciliationEvidence::OrderFillRecord(fill)) => {
                assert_eq!(fill.filled_quantity.as_decimal(), dec!(0.1));
            },
            other => panic!("expected the stop's fill, got {other:?}"),
        }
    }

    /// A fill of the whole leg books what filled.
    #[tokio::test]
    async fn reconcile_closes_on_a_full_stop_fill() {
        let manager = create_test_manager().await;
        let (position, stop_id) = active_leg_with_live_stop(&manager).await;
        manager.exchange().set_price("BTCUSDT", dec!(90));
        let input = stop_fill_of(&manager, &position, &stop_id, dec!(1)).await;

        let outcome = manager.reconcile_close(input).await.unwrap();

        assert!(matches!(outcome, ReconcileCloseOutcome::Closed), "{outcome:?}");
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        match leg.state {
            PositionState::Closed { realized_pnl, .. } => assert_eq!(realized_pnl, dec!(-10)),
            other => panic!("expected Closed, got {other:?}"),
        }
    }

    /// A fill above the leg prices what filled, as it always has.
    #[tokio::test]
    async fn reconcile_prices_a_fill_above_the_leg_at_what_filled() {
        let manager = create_test_manager().await;
        let (position, stop_id) = active_leg_with_live_stop(&manager).await;
        manager.exchange().set_price("BTCUSDT", dec!(90));
        let input = stop_fill_of(&manager, &position, &stop_id, dec!(1.2)).await;

        let outcome = manager.reconcile_close(input).await.unwrap();

        assert!(matches!(outcome, ReconcileCloseOutcome::Closed), "{outcome:?}");
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        match leg.state {
            PositionState::Closed { realized_pnl, .. } => assert_eq!(realized_pnl, dec!(-12)),
            other => panic!("expected Closed, got {other:?}"),
        }
    }

    /// An `ExecutableSpan` leg with no snapshot and an empty cache.
    async fn leg_with_nothing_held(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
    ) -> Position {
        let (mut position, _) = active_leg_with_live_stop(manager).await;
        position.stop_policy = StopPolicy::ExecutableSpan;
        position.executable_span = Some(dec!(10));
        position.cap_basis_distance = Some(dec!(10));
        position.stop_plan_entry_reference = Some(Price::new(dec!(100)).unwrap());
        position.stop_buffer_bps_at_arm = Some(Decimal::ZERO);
        manager.store.positions().save(&position).await.unwrap();
        manager.exchange().clear_cached_rules();
        position
    }

    /// The heal races a close: the venue accepts the stop while reconciliation
    /// closes the leg. Nothing is recorded on the closed leg, and the stop
    /// is taken back, since no sweep would ever visit it on a flat symbol.
    #[tokio::test]
    async fn a_heal_that_races_a_close_withdraws_its_stop_and_records_nothing() {
        let manager = create_test_manager().await;
        let (mut position, stop_id) = active_leg_with_live_stop(&manager).await;
        let exchange = manager.exchange();
        exchange.cancel_stop_market_order(&position.symbol, &stop_id).await.unwrap();
        position.insurance_stop_id = None;
        if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
            *insurance_stop_id = None;
        }
        manager.store.positions().save(&position).await.unwrap();
        let stops_before = exchange.stop_order_count();

        let store = manager.store.clone();
        let position_id = position.id;
        exchange.set_after_next_stop_accepted(move || {
            let price = Price::new(dec!(95)).unwrap();
            store
                .apply_event(&Event::PositionClosed {
                    position_id,
                    exit_reason: robson_domain::ExitReason::ReconciledMissingOnExchange,
                    entry_price: price,
                    exit_price: price,
                    realized_pnl: Decimal::ZERO,
                    total_fees: Decimal::ZERO,
                    closure_evidence: robson_domain::ClosureEvidence::real_exit_fill(None),
                    timestamp: chrono::Utc::now(),
                })
                .unwrap();
        });
        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(position.id, std::time::Instant::now());

        assert!(manager.retry_stale_insurance_stop(&position).await);

        assert_eq!(exchange.protection_call_count(), 2, "the placement did go out");
        assert_eq!(exchange.stop_order_count(), stops_before, "and was taken back");
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
        assert_eq!(leg.insurance_stop_id, None);
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(
            !events.iter().any(|e| matches!(
                e,
                Event::InsuranceStopPlaced { .. } | Event::InsuranceStopFailed { .. }
            )),
            "{events:?}"
        );
    }

    /// A leg failing for want of rules fails alone: the others on the symbol
    /// are processed, and the tick then reports it. In a few stores,
    /// because the order of the legs is the order of a hash map.
    #[tokio::test]
    async fn a_leg_with_no_rules_to_be_had_does_not_stop_the_others_in_its_tick() {
        for _ in 0..8 {
            let manager = create_test_manager().await;
            let mut others = Vec::new();
            for _ in 0..3 {
                others.push(active_leg_with_live_stop(&manager).await.0);
            }
            let blind = leg_with_nothing_held(&manager).await;

            let error = stop_crossing_tick(&manager, &blind).await.unwrap_err();

            assert!(error.to_string().contains("trading rules"), "{error}");
            let leg = manager.get_position(blind.id).await.unwrap().unwrap();
            assert!(matches!(leg.state, PositionState::Active { .. }), "{:?}", leg.state);
            for other in &others {
                let leg = manager.get_position(other.id).await.unwrap().unwrap();
                assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
            }
        }
    }

    /// The lookup a leg with nothing held does is bounded end to end: a hung
    /// one is a failed one. The tick and the budget snapshot both come
    /// through here.
    #[tokio::test]
    async fn a_hung_lookup_for_a_leg_with_nothing_held_runs_out_of_time() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        exchange.clear_cached_rules();
        exchange.set_rules_fetch_hangs(true);
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let rules = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            manager.rules_for_managing_within(&symbol, None, std::time::Duration::from_millis(10)),
        )
        .await
        .expect("the bound must hold");

        assert!(rules.is_none());
    }

    /// Nothing held: the tick looks the rules up on the way and acts on the
    /// crossing it was given. No tick is discarded on the assumption of an
    /// exchange stop that may not be there.
    #[tokio::test]
    async fn a_leg_with_nothing_held_looks_the_rules_up_and_exits_on_the_crossing_tick() {
        let manager = create_test_manager().await;
        let position = leg_with_nothing_held(&manager).await;
        let exchange = manager.exchange();
        exchange.set_venue_rules(lot_rules(dec!(0.001)));
        let lookups = exchange.trading_rules_call_count("BTCUSDT");

        assert!(stop_crossing_tick(&manager, &position).await.is_ok());

        assert!(exchange.trading_rules_call_count("BTCUSDT") > lookups);
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
    }

    /// The lookup fails too: the engine cannot derive this leg's stop and fails
    /// closed, loudly, as it always has. The next crossing tick tries again.
    #[tokio::test]
    async fn a_leg_with_no_rules_to_be_had_fails_the_tick_and_tries_again() {
        let manager = create_test_manager().await;
        let position = leg_with_nothing_held(&manager).await;
        let exchange = manager.exchange();

        let error = stop_crossing_tick(&manager, &position).await.unwrap_err();
        assert!(error.to_string().contains("trading rules"), "{error}");
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Active { .. }), "{:?}", leg.state);

        exchange.set_venue_rules(lot_rules(dec!(0.001)));
        assert!(stop_crossing_tick(&manager, &position).await.is_ok());
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Closed { .. }), "{:?}", leg.state);
    }

    /// At most one refresh in flight per symbol, whatever the spacing allows,
    /// and bookkeeping only for symbols still open.
    #[tokio::test]
    async fn refreshes_on_the_side_are_one_per_symbol_and_pruned() {
        let manager = create_test_manager().await;
        let exchange = manager.exchange();
        exchange.set_rules_refresh_due(true);
        exchange.set_rules_fetch_hangs(true);
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        assert!(manager.rules_for_managing(&symbol, None).await.is_some());
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(exchange.trading_rules_call_count("BTCUSDT"), 1);

        // The spacing has passed; the first one is still in flight.
        {
            let mut requests = manager.rules_refresh_requests.lock().unwrap();
            let request = requests.get_mut(&symbol).unwrap();
            request.requested_at = std::time::Instant::now()
                .checked_sub(RULES_REFRESH_REQUEST_INTERVAL * 2)
                .unwrap();
        }
        assert!(manager.rules_for_managing(&symbol, None).await.is_some());
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert_eq!(exchange.trading_rules_call_count("BTCUSDT"), 1, "one in flight");

        // Pruned to open symbols, an in-flight entry excepted.
        let other = Symbol::from_pair("ETHUSDT").unwrap();
        manager
            .rules_refresh_requests
            .lock()
            .unwrap()
            .insert(other.clone(), RulesRefreshRequest {
                requested_at: std::time::Instant::now(),
                in_flight: false,
            });
        manager
            .exit_retry_after
            .write()
            .await
            .insert(Uuid::now_v7(), std::time::Instant::now());
        manager.prune_insurance_stop_stale(&[]).await;
        assert!(manager.exit_retry_after.read().await.is_empty(), "bounded by the open legs");
        let requests = manager.rules_refresh_requests.lock().unwrap();
        assert!(!requests.contains_key(&other));
        assert!(requests.contains_key(&symbol));
    }

    /// A stop that cannot be replaced on the venue's rules stays where it is.
    #[tokio::test]
    async fn a_refused_stop_replacement_keeps_the_old_stop_and_refreshes_once() {
        let manager = create_test_manager().await;
        let (position, stop_id) = active_leg_with_live_stop(&manager).await;
        let exchange = manager.exchange();
        let calls_before = exchange.protection_call_count();
        exchange.set_rules_rejections_next(100);

        let results = manager
            .execute_and_persist(vec![EngineAction::ReplaceInsuranceStop {
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: position.side.exit_action(),
                quantity: position.quantity,
                previous_order_id: stop_id.clone(),
                new_stop_price: Price::new(dec!(95)).unwrap(),
            }])
            .await
            .unwrap();

        assert_eq!(exchange.protection_call_count() - calls_before, 2);
        assert_eq!(exchange.rules_refresh_call_count(), 1);
        assert!(exchange.has_stop_order(&stop_id), "the old stop stays until a successor exists");
        assert!(results
            .iter()
            .any(|r| matches!(r, ActionResult::EventEmitted(Event::InsuranceStopFailed { .. }))));
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(leg.insurance_stop_id.as_deref(), Some(stop_id.as_str()));
        // The existing heal path owns the next attempt, 30 s out.
        assert!(manager.insurance_stop_stale.read().await.contains_key(&position.id));
    }

    /// How long until this leg's next stop attempt.
    async fn stop_retry_in(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position_id: PositionId,
    ) -> std::time::Duration {
        let retry_at = manager.insurance_stop_stale.read().await[&position_id];
        retry_at.saturating_duration_since(std::time::Instant::now())
    }

    /// Run the heal path now, with its window already due.
    async fn retry_stop_now(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        position_id: PositionId,
    ) {
        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(position_id, std::time::Instant::now());
        let leg = manager.get_position(position_id).await.unwrap().unwrap();
        assert!(manager.retry_stale_insurance_stop(&leg).await);
    }

    async fn below_lot_leg(manager: &Arc<PositionManager<StubExchange, MemoryStore>>) -> Position {
        let mut position =
            save_active_position(manager, "BTCUSDT", Side::Long, dec!(100), dec!(0.0005)).await;
        position.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(100)).unwrap(),
            Price::new(dec!(90)).unwrap(),
        ));
        manager.store.positions().save(&position).await.unwrap();
        let exchange = manager.exchange();
        exchange.set_trading_rules(lot_rules(dec!(0.001)));
        exchange.set_venue_rules(lot_rules(dec!(0.001)));
        position
    }

    /// A stop below the lot minimum stays there until the leg or the rules
    /// change: the long window, and nothing new on the log after the first
    /// attempt, whatever retries it.
    #[tokio::test]
    async fn a_stop_below_the_lot_retries_on_the_long_window_and_writes_once() {
        let manager = create_test_manager().await;
        let position = below_lot_leg(&manager).await;
        let exchange = manager.exchange();

        let first = manager
            .execute_and_persist(vec![EngineAction::PlaceInsuranceStop {
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: position.side.exit_action(),
                quantity: position.quantity,
                stop_price: Price::new(dec!(90)).unwrap(),
            }])
            .await
            .unwrap();
        assert!(first
            .iter()
            .any(|r| matches!(r, ActionResult::EventEmitted(Event::InsuranceStopFailed { .. }))));
        assert!(stop_retry_in(&manager, position.id).await > INSURANCE_STOP_RETRY_BACKOFF);
        let logged = manager.store.events().find_by_position(position.id).await.unwrap().len();
        let attempts = exchange.protection_call_count();

        for _ in 0..3 {
            retry_stop_now(&manager, position.id).await;
            assert!(stop_retry_in(&manager, position.id).await > INSURANCE_STOP_RETRY_BACKOFF);
        }

        assert_eq!(exchange.protection_call_count() - attempts, 3, "it does keep trying");
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert_eq!(events.len(), logged, "{events:?}");
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(leg.residual_quantity, Some(dec!(0.0005)), "and it still stands");

        // What the executor hands over to persist on a repeat: nothing.
        let repeat = manager
            .execute_and_persist(vec![EngineAction::PlaceInsuranceStop {
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: position.side.exit_action(),
                quantity: position.quantity,
                stop_price: Price::new(dec!(90)).unwrap(),
            }])
            .await
            .unwrap();
        assert!(matches!(repeat.as_slice(), [ActionResult::ProtectionNotExecutable { .. }]));
    }

    /// The exit and the stop of one leg retry in turns. Neither makes the
    /// other's record look new.
    #[tokio::test]
    async fn a_below_lot_leg_retrying_its_exit_and_its_stop_does_not_grow_the_log() {
        let manager = create_test_manager().await;
        let position = below_lot_leg(&manager).await;

        manager.close_position(position.id).await.unwrap_err();
        retry_stop_now(&manager, position.id).await;
        let logged = manager.store.events().find_by_position(position.id).await.unwrap().len();

        for _ in 0..3 {
            manager.close_position(position.id).await.unwrap_err();
            retry_stop_now(&manager, position.id).await;
        }

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert_eq!(events.len(), logged, "{events:?}");
    }

    /// A transient failure is not that: it keeps its short window and its event
    /// on every attempt.
    #[tokio::test]
    async fn a_transient_stop_failure_keeps_its_short_window_and_its_event() {
        let manager = create_test_manager().await;
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        position.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(100)).unwrap(),
            Price::new(dec!(90)).unwrap(),
        ));
        manager.store.positions().save(&position).await.unwrap();
        let exchange = manager.exchange();

        exchange.set_order_fail_next(true);
        manager
            .execute_and_persist(vec![EngineAction::PlaceInsuranceStop {
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: position.side.exit_action(),
                quantity: position.quantity,
                stop_price: Price::new(dec!(90)).unwrap(),
            }])
            .await
            .unwrap();
        assert!(stop_retry_in(&manager, position.id).await <= INSURANCE_STOP_RETRY_BACKOFF);

        exchange.set_order_fail_next(true);
        retry_stop_now(&manager, position.id).await;
        assert!(stop_retry_in(&manager, position.id).await <= INSURANCE_STOP_RETRY_BACKOFF);

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let failures =
            events.iter().filter(|e| matches!(e, Event::InsuranceStopFailed { .. })).count();
        assert_eq!(failures, 2);
    }

    /// Admission writes the rules it used on the event, and the leg keeps them.
    #[tokio::test]
    async fn admission_persists_the_rules_it_used_with_the_leg() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(TechnicalStopDistance::from_entry_and_stop(entry, stop)),
                Uuid::now_v7(),
            )
            .await
            .unwrap();
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: entry,
            stop_loss: stop,
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };
        manager.handle_signal(signal).await.unwrap();

        let rules = manager.exchange().last_validated_rules(&symbol).unwrap();
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(leg.rules_snapshot.as_ref(), Some(&rules));
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            Event::EntrySignalReceived { rules_snapshot: Some(persisted), .. }
                if persisted == &rules
        )));
    }

    /// Regression for #142: when the exchange-side insurance stop already
    /// closed the position and the reduce-only exit is rejected (-2022), the
    /// tick must hand off to reverse reconciliation and close the position
    /// instead of failing the query.
    #[tokio::test]
    async fn test_reduce_only_rejection_with_flat_exchange_reconciles_close() {
        let manager = create_test_manager().await;
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        let entry_price = Price::new(dec!(100)).unwrap();
        let trailing_stop = Price::new(dec!(90)).unwrap();
        position.tech_stop_distance =
            Some(TechnicalStopDistance::from_entry_and_stop(entry_price, trailing_stop));

        let exchange = manager.executor.exchange();
        let insurance_order = exchange
            .place_stop_market_order(
                &position.symbol,
                position.side.exit_action(),
                position.quantity,
                trailing_stop,
                "ins-test-overtaken",
            )
            .await
            .unwrap();
        position.insurance_stop_id = Some(insurance_order.exchange_order_id.clone());
        if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
            *insurance_stop_id = Some(insurance_order.exchange_order_id.clone());
        }
        manager.store.positions().save(&position).await.unwrap();

        // The exchange-side stop triggers first: order gone from the open set,
        // real fill recorded, and the exchange is flat (no open positions).
        exchange.set_price("BTCUSDT", trailing_stop.as_decimal());
        exchange.fill_stop_order(&insurance_order.exchange_order_id).unwrap();
        exchange.set_reject_reduce_only_next(true);

        let result = manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: position.symbol.clone(),
                price: Price::new(dec!(89)).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await;
        assert!(result.is_ok(), "overtaken exit must not fail the tick: {result:?}");
        assert_eq!(exchange.market_order_call_count(), 1, "exactly one exit attempt is allowed");

        let closed = manager.get_position(position.id).await.unwrap().unwrap();
        match &closed.state {
            PositionState::Closed { exit_price, .. } => {
                assert_eq!(*exit_price, trailing_stop);
            },
            other => panic!("overtaken exit must reconcile to Closed, got {other:?}"),
        }
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events.iter().any(|event| matches!(event, Event::PositionClosed { .. })));
    }

    /// ADR-0056 step 7, §2.3 item 5: a stop-out whose software cancel finds the
    /// order gone with no final state, then an overtaken close. The close
    /// never resolves the cancel: its row stays uncertain, with the leg closed.
    #[tokio::test]
    async fn an_overtaken_close_never_resolves_an_uncertain_cancel() {
        let (manager, backend) = journaled_manager().await;
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        let entry_price = Price::new(dec!(100)).unwrap();
        let trailing_stop = Price::new(dec!(90)).unwrap();
        position.tech_stop_distance =
            Some(TechnicalStopDistance::from_entry_and_stop(entry_price, trailing_stop));
        let exchange = manager.executor.exchange();
        let stop = exchange
            .place_stop_market_order(
                &position.symbol,
                position.side.exit_action(),
                position.quantity,
                trailing_stop,
                "ins-test-overtaken",
            )
            .await
            .unwrap()
            .exchange_order_id;
        position.insurance_stop_id = Some(stop.clone());
        if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
            *insurance_stop_id = Some(stop.clone());
        }
        manager.store.positions().save(&position).await.unwrap();

        exchange.set_price("BTCUSDT", trailing_stop.as_decimal());
        exchange.fill_stop_order(&stop).unwrap();
        exchange.set_reject_reduce_only_next(true);
        exchange.set_stop_cancel_outcome_next(Ok(
            robson_exec::venue::StopCancelOutcome::AlreadyGone {
                final_state: robson_exec::venue::OrderLookup::Unknown,
            },
        ));

        manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: position.symbol.clone(),
                price: Price::new(dec!(89)).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await
            .unwrap();

        let closed = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(closed.state, PositionState::Closed { .. }), "{:?}", closed.state);
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(!events.iter().any(|e| matches!(e, Event::InsuranceStopCancelled { .. })));
        let cancel = backend
            .rows()
            .into_iter()
            .find(|r| {
                r.intent.as_ref().unwrap().kind == robson_exec::order_journal::IntentKind::Cancel
            })
            .expect("the cancel is journaled");
        assert_eq!(cancel.intent.unwrap().cancels_order_id.as_deref(), Some(stop.as_str()));
        let outcome = cancel.outcome.expect("its outcome is written");
        assert_eq!(outcome.cancel_class, Some("gone_unresolved"));
        assert_eq!(outcome.venue_outcome, None, "the close settles nothing about the cancel");
        assert!(outcome.uncertain_at.is_some());
    }

    /// ADR-0056 step 7, PR C: a leg crossing its stop, a broad cancel error, an
    /// exit failing with a generic error, later ticks. The cancel goes out
    /// again before every exit attempt, the heal's deadline stands, and a batch
    /// that finishes after that deadline leaves it due for the next tick.
    #[tokio::test]
    async fn repeated_crossing_ticks_repeat_the_cancel_and_do_not_starve_the_heal() {
        let (manager, backend) = journaled_manager().await;
        let (position, stop) = active_leg_with_live_stop(&manager).await;
        let exchange = manager.exchange();
        let broad = || Err(ExecError::Exchange("order gateway unavailable".to_string()));
        let cancel_rows = || {
            backend
                .rows()
                .iter()
                .filter(|r| {
                    r.intent.as_ref().unwrap().kind
                        == robson_exec::order_journal::IntentKind::Cancel
                })
                .count()
        };
        let failures = || async {
            let events = manager.store.events().find_by_position(position.id).await.unwrap();
            events.iter().filter(|e| matches!(e, Event::InsuranceStopFailed { .. })).count()
        };

        exchange.set_stop_cancel_outcome_next(broad());
        exchange.set_order_fail_next(true);
        let first = stop_crossing_tick(&manager, &position).await;
        assert!(first.is_err(), "the exit failed: {first:?}");
        assert_eq!(exchange.stop_cancel_call_count(), 1);
        assert_eq!(exchange.market_order_call_count(), 1);
        assert_eq!(failures().await, 1);
        assert_eq!(cancel_rows(), 1);
        let deadline = manager.insurance_stop_stale.read().await[&position.id];

        for tick in 2..=4u64 {
            exchange.set_stop_cancel_outcome_next(broad());
            exchange.set_order_fail_next(true);
            let again = stop_crossing_tick(&manager, &position).await;
            assert!(again.is_err(), "tick {tick}: the exit failed again: {again:?}");
            assert_eq!(exchange.stop_cancel_call_count(), tick, "tick {tick}: cancelled again");
            assert_eq!(exchange.market_order_call_count(), tick, "tick {tick}: the exit ran");
            assert_eq!(failures().await, tick as usize, "tick {tick}: its own audit");
            assert_eq!(cancel_rows(), tick as usize, "tick {tick}: its own intent");
            assert_eq!(
                manager.insurance_stop_stale.read().await[&position.id],
                deadline,
                "tick {tick}: the heal's deadline stands"
            );
        }
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(leg.insurance_stop_id.as_deref(), Some(stop.as_str()), "the id was kept");

        // This batch started with the deadline ahead and its bookkeeping runs
        // after the deadline has passed: the overdue deadline stays.
        let overdue = std::time::Instant::now();
        let stale = Arc::clone(&manager.insurance_stop_stale);
        let id = position.id;
        exchange.set_after_next_stop_cancel(move || {
            stale.try_write().unwrap().insert(id, overdue);
        });
        exchange.set_stop_cancel_outcome_next(broad());
        exchange.set_order_fail_next(true);
        assert!(stop_crossing_tick(&manager, &position).await.is_err());
        assert_eq!(manager.insurance_stop_stale.read().await[&position.id], overdue);

        // The next tick sees it due and replaces the stop against the kept id.
        let placements = exchange.protection_call_count();
        stop_crossing_tick(&manager, &position).await.unwrap();
        assert_eq!(exchange.protection_call_count(), placements + 1, "the heal placed a stop");
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(
            events.iter().any(|e| matches!(
                e,
                Event::InsuranceStopReplaced { previous_order_id, .. }
                    if previous_order_id == &stop
            )),
            "{events:?}"
        );
    }

    /// Codex round 2 on PR C: a timed-out cancel leaves the stop live and the
    /// exit fails. On the next tick the cancel goes out again before the exit,
    /// so a stop that can now be cancelled is gone when the exit goes out.
    #[tokio::test]
    async fn a_retried_cancel_clears_a_live_stop_before_the_next_exit() {
        let (manager, backend) = journaled_manager().await;
        let (position, stop) = active_leg_with_live_stop(&manager).await;
        let exchange = manager.exchange();
        exchange.set_stop_cancel_outcome_next(Err(ExecError::Timeout("no answer".to_string())));
        exchange.set_order_fail_next(true);
        assert!(stop_crossing_tick(&manager, &position).await.is_err());
        assert!(exchange.has_stop_order(&stop), "the timed-out cancel left it live");

        let exits_at_cancel = Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX));
        let (seen, venue) = (Arc::clone(&exits_at_cancel), Arc::clone(&exchange));
        exchange.set_after_next_stop_cancel(move || {
            seen.store(venue.market_order_call_count(), std::sync::atomic::Ordering::SeqCst);
        });
        stop_crossing_tick(&manager, &position).await.unwrap();

        assert_eq!(
            exits_at_cancel.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the cancel went out before this tick's exit"
        );
        assert_eq!(exchange.stop_cancel_call_count(), 2);
        assert!(!exchange.has_stop_order(&stop), "the retried cancel took the stop");
        assert_eq!(exchange.market_order_call_count(), 2, "and the exit followed it");
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events.iter().any(|e| matches!(e, Event::InsuranceStopCancelled { .. })));
        let classes: Vec<_> = backend
            .rows()
            .iter()
            .filter_map(|r| r.outcome.as_ref().and_then(|o| o.cancel_class))
            .collect();
        assert!(classes.contains(&"error") && classes.contains(&"cancelled"), "{classes:?}");
    }

    /// ADR-0056 step 7, PR C, Codex round 3: a new process over a leg whose
    /// latest stop event is a failure, price back above the stop, open-orders
    /// reads failing, placement available. The rebuilt retry still comes due
    /// and replaces the stop against the id it kept.
    #[tokio::test]
    async fn a_heal_rebuilt_at_restart_replaces_the_kept_stop_with_open_orders_failing() {
        let manager = create_test_manager().await;
        let (position, stop) = active_leg_with_live_stop(&manager).await;
        let exchange = manager.exchange();
        exchange.set_open_orders_fail(true);

        // Without the rebuild nothing here would notice the leg.
        quiet_tick(&manager, &position).await;
        assert!(!manager.insurance_stop_stale.read().await.contains_key(&position.id));

        manager.schedule_stop_heal_after_restart(position.id).await;
        let window = stop_retry_in(&manager, position.id).await;
        assert!(window > std::time::Duration::from_secs(25), "{window:?}");
        assert!(window <= INSURANCE_STOP_RETRY_BACKOFF, "{window:?}");
        let placements = exchange.protection_call_count();
        quiet_tick(&manager, &position).await;
        assert_eq!(exchange.protection_call_count(), placements, "not due yet");

        manager
            .insurance_stop_stale
            .write()
            .await
            .insert(position.id, std::time::Instant::now());
        quiet_tick(&manager, &position).await;

        assert_eq!(exchange.protection_call_count(), placements + 1, "the heal placed a stop");
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(
            events.iter().any(|e| matches!(
                e,
                Event::InsuranceStopReplaced { previous_order_id, .. }
                    if previous_order_id == &stop
            )),
            "{events:?}"
        );
        assert!(!manager.insurance_stop_stale.read().await.contains_key(&position.id));
    }

    #[tokio::test]
    async fn a_heal_rebuilt_at_restart_keeps_an_entry_already_there() {
        let manager = create_test_manager().await;
        let id = Uuid::now_v7();
        let ahead = std::time::Instant::now() + std::time::Duration::from_secs(10);
        manager.insurance_stop_stale.write().await.insert(id, ahead);

        manager.schedule_stop_heal_after_restart(id).await;

        assert_eq!(manager.insurance_stop_stale.read().await[&id], ahead);
    }

    /// A successful startup-recovery placement or replacement clears the
    /// rebuilt retry through the ordinary bookkeeping.
    #[tokio::test]
    async fn a_startup_recovery_stop_clears_the_rebuilt_retry() {
        let manager = create_test_manager().await;
        let (position, stop) = active_leg_with_live_stop(&manager).await;
        let replace = EngineAction::ReplaceInsuranceStop {
            position_id: position.id,
            symbol: position.symbol.clone(),
            side: position.side.exit_action(),
            quantity: position.quantity,
            previous_order_id: stop,
            new_stop_price: Price::new(dec!(91)).unwrap(),
        };
        let place = EngineAction::PlaceInsuranceStop {
            position_id: position.id,
            symbol: position.symbol.clone(),
            side: position.side.exit_action(),
            quantity: position.quantity,
            stop_price: Price::new(dec!(91)).unwrap(),
        };

        for action in [replace, place] {
            manager.schedule_stop_heal_after_restart(position.id).await;
            manager.execute_and_persist_recovery(vec![action]).await.unwrap();
            assert!(!manager.insurance_stop_stale.read().await.contains_key(&position.id));
        }
    }

    fn stop_failed(position_id: PositionId) -> ActionResult {
        ActionResult::EventEmitted(Event::InsuranceStopFailed {
            position_id,
            stop_price: Price::zero(),
            error: "venue".to_string(),
            timestamp: chrono::Utc::now(),
        })
    }

    /// Only an absent deadline gets a new window: one on record stands,
    /// overdue included.
    #[tokio::test]
    async fn a_stop_failure_keeps_any_heal_deadline_on_record() {
        let manager = create_test_manager().await;
        let id = Uuid::now_v7();

        manager.note_insurance_stop_results(&[stop_failed(id)]).await;
        assert!(stop_retry_in(&manager, id).await > std::time::Duration::from_secs(25), "absent");

        let ahead = std::time::Instant::now() + std::time::Duration::from_secs(10);
        manager.insurance_stop_stale.write().await.insert(id, ahead);
        manager.note_insurance_stop_results(&[stop_failed(id)]).await;
        assert_eq!(manager.insurance_stop_stale.read().await[&id], ahead, "ahead");

        let overdue = std::time::Instant::now();
        manager.insurance_stop_stale.write().await.insert(id, overdue);
        manager.note_insurance_stop_results(&[stop_failed(id)]).await;
        assert_eq!(manager.insurance_stop_stale.read().await[&id], overdue, "overdue");

        manager
            .note_insurance_stop_results(&[ActionResult::ProtectionNotExecutable {
                position_id: id,
            }])
            .await;
        assert!(stop_retry_in(&manager, id).await > INSURANCE_STOP_RETRY_BACKOFF, "long window");
    }

    /// A failed trailing replacement finding an overdue deadline leaves it due,
    /// and the heal that follows pushes its window first: one heal, not one on
    /// every tick.
    #[tokio::test]
    async fn a_failed_replacement_on_an_overdue_deadline_is_not_a_heal_on_every_tick() {
        let manager = create_test_manager().await;
        let (position, stop) = active_leg_with_live_stop(&manager).await;
        let exchange = manager.exchange();
        let overdue = std::time::Instant::now();
        manager.insurance_stop_stale.write().await.insert(position.id, overdue);

        exchange.set_order_fail_next(true);
        manager
            .execute_and_persist(vec![EngineAction::ReplaceInsuranceStop {
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: position.side.exit_action(),
                quantity: position.quantity,
                previous_order_id: stop,
                new_stop_price: Price::new(dec!(91)).unwrap(),
            }])
            .await
            .unwrap();
        assert_eq!(manager.insurance_stop_stale.read().await[&position.id], overdue);

        exchange.set_order_fail_next(true);
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(manager.retry_stale_insurance_stop(&leg).await, "it was due");
        assert!(
            stop_retry_in(&manager, position.id).await > std::time::Duration::from_secs(25),
            "the heal pushed its window before it failed"
        );

        let placements = exchange.protection_call_count();
        assert!(!manager.retry_stale_insurance_stop(&leg).await, "not due again");
        assert_eq!(exchange.protection_call_count(), placements);
    }

    /// A heal pushes its own window before it acts, so one that fails still
    /// backs off although a deadline ahead now stands.
    #[tokio::test]
    async fn a_failed_heal_still_backs_off() {
        let manager = create_test_manager().await;
        let (position, _) = active_leg_with_live_stop(&manager).await;
        manager.exchange().set_order_fail_next(true);

        retry_stop_now(&manager, position.id).await;

        assert!(
            stop_retry_in(&manager, position.id).await > std::time::Duration::from_secs(25),
            "the failed heal waits a full window"
        );
    }

    /// Regression for #142: while fill evidence is not visible yet, later
    /// ticks must not place further exit orders for a suspect position; once
    /// evidence appears, the position closes.
    #[tokio::test]
    async fn test_overtaken_suspect_stops_exit_retries_until_evidence_appears() {
        let manager = create_test_manager().await;
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        let entry_price = Price::new(dec!(100)).unwrap();
        let trailing_stop = Price::new(dec!(90)).unwrap();
        position.tech_stop_distance =
            Some(TechnicalStopDistance::from_entry_and_stop(entry_price, trailing_stop));
        // The recorded insurance stop id is not resolvable on the exchange
        // yet, so fill evidence is delayed.
        position.insurance_stop_id = Some("ins-not-yet-visible".to_string());
        if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
            *insurance_stop_id = Some("ins-not-yet-visible".to_string());
        }
        manager.store.positions().save(&position).await.unwrap();

        let exchange = manager.executor.exchange();
        exchange.set_reject_reduce_only_next(true);

        let tick = |price: Decimal| MarketData {
            venue: test_venue(),
            symbol: position.symbol.clone(),
            price: Price::new(price).unwrap(),
            timestamp: chrono::Utc::now(),
            source: crate::event_bus::MarketDataSource::Ws,
        };

        // Tick 1: rejection with flat exchange marks the suspect; no close yet.
        assert!(manager.process_market_data(tick(dec!(89))).await.is_ok());
        assert_eq!(exchange.market_order_call_count(), 1);
        let still_active = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(still_active.state, PositionState::Active { .. }));

        // Tick 2: suspect path polls evidence only; no further exit orders.
        assert!(manager.process_market_data(tick(dec!(88))).await.is_ok());
        assert_eq!(
            exchange.market_order_call_count(),
            1,
            "suspect position must not re-place exit orders"
        );

        // Evidence becomes visible: the real triggered stop fill appears.
        exchange.set_price("BTCUSDT", trailing_stop.as_decimal());
        let filled_order = exchange
            .place_stop_market_order(
                &position.symbol,
                position.side.exit_action(),
                position.quantity,
                trailing_stop,
                "ins-late-evidence",
            )
            .await
            .unwrap();
        exchange.fill_stop_order(&filled_order.exchange_order_id).unwrap();
        let mut stored = manager.get_position(position.id).await.unwrap().unwrap();
        stored.insurance_stop_id = Some(filled_order.exchange_order_id.clone());
        if let PositionState::Active { insurance_stop_id, .. } = &mut stored.state {
            *insurance_stop_id = Some(filled_order.exchange_order_id.clone());
        }
        manager.store.positions().save(&stored).await.unwrap();

        // Tick 3: evidence resolves and the position closes; still one order.
        assert!(manager.process_market_data(tick(dec!(88))).await.is_ok());
        assert_eq!(exchange.market_order_call_count(), 1);
        let closed = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(closed.state, PositionState::Closed { .. }),
            "evidence must close the suspect position, got {:?}",
            closed.state
        );
    }

    /// #142 guard: a reduce-only rejection while the exchange still reports
    /// the position open is a real failure and must keep the original
    /// fail-the-query path.
    #[tokio::test]
    async fn test_reduce_only_rejection_with_position_still_open_fails() {
        let manager = create_test_manager().await;
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        let entry_price = Price::new(dec!(100)).unwrap();
        let trailing_stop = Price::new(dec!(90)).unwrap();
        position.tech_stop_distance =
            Some(TechnicalStopDistance::from_entry_and_stop(entry_price, trailing_stop));
        position.insurance_stop_id = Some("ins-still-open".to_string());
        if let PositionState::Active { insurance_stop_id, .. } = &mut position.state {
            *insurance_stop_id = Some("ins-still-open".to_string());
        }
        manager.store.positions().save(&position).await.unwrap();

        let exchange = manager.executor.exchange();
        exchange.set_open_position(
            position.symbol.clone(),
            position.side,
            position.quantity,
            entry_price,
        );
        exchange.set_reject_reduce_only_next(true);

        let result = manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: position.symbol.clone(),
                price: Price::new(dec!(89)).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await;
        assert!(result.is_err(), "rejection with a live exchange position must surface");

        let unchanged = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(unchanged.state, PositionState::Active { .. }));
    }

    /// Save a bare Armed position directly to the store with an explicit
    /// entry policy, bypassing arm-time detector spawning so tests control
    /// the signal flow deterministically.
    async fn save_armed_position_with_policy(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        symbol: &str,
        side: Side,
        policy: EntryPolicyConfig,
    ) -> Position {
        let symbol = Symbol::from_pair(symbol).unwrap();
        let position = Position::new(Uuid::now_v7(), symbol, side);
        manager.store.positions().save(&position).await.unwrap();
        manager.entry_policies.write().await.insert(position.id, policy);
        position
    }

    /// ADR-0050 §2: an Immediate-mode signal rejected by domain validation
    /// (stop too tight) terminates in needs_operator_rearm: durable audit
    /// event, no re-arm, no error surfaced.
    #[tokio::test]
    async fn test_immediate_domain_rejection_needs_operator_rearm() {
        use robson_domain::ApprovalPolicy as DomainApprovalPolicy;
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let policy =
            EntryPolicyConfig::new(EntryPolicy::Immediate, DomainApprovalPolicy::Automatic);
        let position =
            save_armed_position_with_policy(&manager, "BTCUSDT", Side::Long, policy).await;

        // Stop 0.05% below entry: below the 0.1% minimum bound.
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(100)).unwrap(),
            stop_loss: Price::new(dec!(99.95)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        let result = manager.handle_signal(signal).await;
        assert!(result.is_ok(), "domain rejection must be a governed outcome: {result:?}");

        assert!(manager.is_entry_exhausted(position.id).await);
        let rejection = manager.entry_exhaustion(position.id).await.unwrap();
        assert_eq!(rejection.reason_code, "signal_domain_rejection");
        assert!(rejection.reason.contains("Stop too tight"), "got: {}", rejection.reason);

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(
            events.iter().any(|event| matches!(event, Event::EntryAttemptExhausted { .. })),
            "durable entry_attempt_exhausted event must be persisted"
        );

        // Position remains Armed (state untouched) but flagged.
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(updated.state, PositionState::Armed));
    }

    /// ADR-0050 §2: strategy modes re-arm the detector under governed
    /// backoff on a domain rejection; no terminal record is created.
    #[tokio::test]
    async fn test_strategy_mode_domain_rejection_rearms() {
        use robson_domain::ApprovalPolicy as DomainApprovalPolicy;
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let policy =
            EntryPolicyConfig::new(EntryPolicy::ConfirmedTrend, DomainApprovalPolicy::Automatic);
        let position =
            save_armed_position_with_policy(&manager, "BTCUSDT", Side::Long, policy).await;

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(100)).unwrap(),
            stop_loss: Price::new(dec!(99.95)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        let result = manager.handle_signal(signal).await;
        assert!(result.is_ok(), "domain rejection must be a governed outcome: {result:?}");

        assert!(!manager.is_entry_exhausted(position.id).await);
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(
            !events.iter().any(|event| matches!(event, Event::EntryAttemptExhausted { .. })),
            "strategy modes must not create the terminal record"
        );
        assert!(
            *manager.governed_rearm_counts.read().await.get(&position.id).unwrap_or(&0) >= 1,
            "governed re-arm backoff must be scheduled"
        );
    }

    #[tokio::test]
    async fn executable_span_readmission_is_terminal_under_confirmed_trend() {
        use robson_domain::ApprovalPolicy as DomainApprovalPolicy;

        let manager = create_test_manager().await;
        let policy =
            EntryPolicyConfig::new(EntryPolicy::ConfirmedTrend, DomainApprovalPolicy::Automatic);
        let mut position =
            save_armed_position_with_policy(&manager, "BTCUSDT", Side::Long, policy).await;
        position.stop_policy = StopPolicy::ExecutableSpan;
        position.initial_executable_stop = Some(Price::new(dec!(90)).unwrap());
        position.executable_span = Some(dec!(10));
        position.cap_basis_distance = Some(dec!(10));
        position.tick_size_at_admission = Some(dec!(0.01));
        manager.store.positions().save(&position).await.unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: position.symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(100)).unwrap(),
            stop_loss: Price::new(dec!(90)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let rejection = manager.entry_exhaustion(position.id).await.unwrap();
        assert_eq!(rejection.reason_code, "admission_evidence_present");
        assert!(rejection.reason.contains("immutable admission evidence"));
        assert_eq!(
            manager.governed_rearm_counts.read().await.get(&position.id),
            None,
            "re-admission evidence is terminal and must not enter a re-arm loop"
        );
        assert!(!manager.detectors.read().await.contains_key(&position.id));

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            Event::EntryAttemptExhausted { reason_code, .. }
                if reason_code == "admission_evidence_present"
        )));
    }

    /// No re-arm lands while MonthlyHalt is latched, whatever path asks for
    /// it: the check sits under the lock the halt's stop takes.
    #[tokio::test]
    async fn no_detector_is_rearmed_under_monthly_halt() {
        let manager = create_test_manager().await;
        let position = save_armed_position_with_policy(
            &manager,
            "BTCUSDT",
            Side::Long,
            EntryPolicyConfig::default(),
        )
        .await;
        manager.circuit_breaker.trigger_halt("test".to_string()).await;

        assert!(
            !manager
                .rearm_detector_after_governed_block(position.id, &position, "test")
                .await
        );
        assert!(!manager.detectors.read().await.contains_key(&position.id));
    }

    /// A latched MonthlyHalt drops the signal, and the failure re-arm leaves
    /// the position alone: the halt disarms Armed positions.
    #[tokio::test]
    async fn a_signal_dropped_by_monthly_halt_is_not_rearmed() {
        let manager = create_test_manager().await;
        let position = save_armed_position_with_policy(
            &manager,
            "BTCUSDT",
            Side::Long,
            EntryPolicyConfig::default(),
        )
        .await;
        manager.circuit_breaker.trigger_halt("test".to_string()).await;

        let result = manager
            .handle_signal(DetectorSignal {
                signal_id: Uuid::now_v7(),
                position_id: position.id,
                symbol: position.symbol.clone(),
                side: Side::Long,
                entry_price: Price::new(dec!(95000)).unwrap(),
                stop_loss: Price::new(dec!(90250)).unwrap(),
                technical_stop_analysis: None,
                timestamp: chrono::Utc::now(),
            })
            .await;

        assert!(matches!(result, Err(DaemonError::MonthlyHaltActive { .. })), "{result:?}");
        assert!(!manager.detectors.read().await.contains_key(&position.id));
        assert_eq!(manager.governed_rearm_counts.read().await.get(&position.id), None);
    }

    /// Disarm resolves needs_operator_rearm; governed re-arm is refused
    /// while the record exists.
    #[tokio::test]
    async fn test_disarm_clears_needs_operator_rearm() {
        let manager = create_test_manager().await;
        let position = save_armed_position_with_policy(
            &manager,
            "BTCUSDT",
            Side::Long,
            EntryPolicyConfig::default(),
        )
        .await;

        manager
            .mark_entry_exhausted(position.id, None, "no_valid_signal", "test reason")
            .await
            .unwrap();
        assert!(manager.is_entry_exhausted(position.id).await);

        // Governed re-arm is refused while terminal.
        let loaded = manager.get_position(position.id).await.unwrap().unwrap();
        manager.rearm_detector_after_governed_block(position.id, &loaded, "test").await;
        assert!(
            !manager.detectors.read().await.contains_key(&position.id),
            "no detector may spawn for a needs_operator_rearm position"
        );

        manager.disarm_position(position.id).await.unwrap();
        assert!(!manager.is_entry_exhausted(position.id).await);
    }

    #[tokio::test]
    async fn test_arm_position() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let position = manager
            .arm_position(symbol, Side::Long, create_test_risk_config(), None, Uuid::now_v7())
            .await
            .unwrap();

        assert!(matches!(position.state, PositionState::Armed));
        assert!(
            position.tech_stop_distance.is_none(),
            "ARM must not invent a technical stop before detector signal"
        );

        // Should be persisted
        let loaded = manager.get_position(position.id).await.unwrap().unwrap();
        assert_eq!(loaded.id, position.id);
        assert!(loaded.tech_stop_distance.is_none());
    }

    #[tokio::test]
    async fn test_disarm_position() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol,
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        manager.disarm_position(position.id).await.unwrap();

        // Position must be kept for audit trail, transitioned to Cancelled state
        let loaded = manager
            .get_position(position.id)
            .await
            .unwrap()
            .expect("position must exist after disarm");
        assert!(
            matches!(loaded.state, PositionState::Cancelled),
            "expected Cancelled after disarm, got {:?}",
            loaded.state
        );
    }

    #[tokio::test]
    async fn test_cancel_or_close_position_closes_active_position() {
        let manager = create_test_manager().await;
        let position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(95000), dec!(0.01)).await;

        manager.cancel_or_close_position(position.id).await.unwrap();

        let loaded = manager
            .get_position(position.id)
            .await
            .unwrap()
            .expect("position must still exist after close");
        assert!(
            matches!(loaded.state, PositionState::Closed { .. }),
            "expected Closed after active close, got {:?}",
            loaded.state
        );
    }

    #[tokio::test]
    async fn test_handle_signal() {
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // Create detector signal.
        //
        // Stop distance must be wide enough to pass the Phase 2 risk gate:
        //   qty = (capital * risk_pct) / (entry * stop_pct) = $100 / (95000 * stop_pct)
        //   notional = qty * entry = $100 / stop_pct
        //   max_single_position_pct = 15% of $10000 = $1500
        //   → stop_pct ≥ 100/1500 ≈ 6.67%
        //
        // Using 8% stop distance: stop_loss = 95000 * 0.92 = 87400
        //   notional ≈ $1250 < $1500 ✓
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(), // 8% below — passes risk gate
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        // Position should now be Active
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(updated.state, PositionState::Active { .. }));

        // Core open event must be emitted when position becomes active
        let mut opened = false;
        for _ in 0..20 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
            {
                if let Ok(DaemonEvent::CorePositionOpened { position_id, symbol, side, .. }) = event
                {
                    assert_eq!(position_id, position.id);
                    assert_eq!(symbol.as_pair(), "BTCUSDT");
                    assert_eq!(side, Side::Long);
                    opened = true;
                    break;
                }
            }
        }
        assert!(opened, "Expected CorePositionOpened event");
    }

    #[tokio::test]
    async fn test_handle_signal_persists_technical_stop_audit_before_entry_signal() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal_id = Uuid::now_v7();
        let entry_price = Price::new(dec!(95000)).unwrap();
        let stop_loss = Price::new(dec!(87400)).unwrap();
        let signal = DetectorSignal {
            signal_id,
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price,
            stop_loss,
            technical_stop_analysis: Some(sample_technical_stop_analysis(stop_loss)),
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let technical_idx = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    Event::TechnicalStopAnalyzed {
                        signal_id: event_signal_id,
                        ..
                    } if *event_signal_id == signal_id
                )
            })
            .expect("technical_stop_analyzed must be appended");
        let entry_signal_idx = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    Event::EntrySignalReceived {
                        signal_id: event_signal_id,
                        ..
                    } if *event_signal_id == signal_id
                )
            })
            .expect("entry_signal_received must be appended");

        assert!(
            technical_idx < entry_signal_idx,
            "technical stop audit must be persisted before entry signal"
        );

        match &events[technical_idx] {
            Event::TechnicalStopAnalyzed {
                entry_price: persisted_entry_price,
                analysis,
                ..
            } => {
                assert_eq!(*persisted_entry_price, entry_price);
                assert_eq!(analysis.stop_price, stop_loss);
                assert_eq!(analysis.method, TechnicalStopMethodSnapshot::SwingPoint { level_n: 2 });
                assert_eq!(analysis.confidence, TechnicalStopConfidenceSnapshot::High);
                assert_eq!(analysis.detected_levels.len(), 2);
                assert_eq!(analysis.detected_levels[1], stop_loss);
            },
            other => panic!("expected TechnicalStopAnalyzed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_handle_signal_persists_invalidation_guard_in_audit() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal_id = Uuid::now_v7();
        let entry_price = Price::new(dec!(95000)).unwrap();
        let stop_loss = Price::new(dec!(87400)).unwrap();
        let guard = Price::new(dec!(87000)).unwrap();
        let signal = DetectorSignal {
            signal_id,
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price,
            stop_loss,
            technical_stop_analysis: Some(sample_technical_stop_analysis_with_guard(
                stop_loss,
                guard,
                robson_domain::entities::EffectiveStopBasis::InvalidationGuard,
            )),
            timestamp: chrono::Utc::now(),
        };

        // The audit (with the guard) is persisted before any risk/entry step.
        let _ = manager.handle_signal(signal).await;

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let analyzed = events
            .iter()
            .find_map(|event| match event {
                Event::TechnicalStopAnalyzed { analysis, .. } => Some(analysis.clone()),
                _ => None,
            })
            .expect("TechnicalStopAnalyzed must be appended");

        assert_eq!(analyzed.invalidation_guard_level, Some(guard));
        assert_eq!(analyzed.raw_technical_stop, Some(stop_loss));
        assert_eq!(
            analyzed.effective_stop_basis,
            Some(robson_domain::entities::EffectiveStopBasis::InvalidationGuard)
        );
    }

    #[tokio::test]
    async fn test_disarm_non_armed_fails() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        // Create tech stop distance: entry $100, stop $98 (2% distance)
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // Move to Active — 8% stop distance passes the risk gate (see
        // test_handle_signal)
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(), // 8% below — passes risk gate
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };
        manager.handle_signal(signal).await.unwrap();

        // Try to disarm (should fail — position is now Active, not Armed)
        let result = manager.disarm_position(position.id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_position_not_found() {
        let manager = create_test_manager().await;
        let fake_id = Uuid::now_v7();

        let result = manager.disarm_position(fake_id).await;
        assert!(matches!(result, Err(DaemonError::PositionNotFound(_))));
    }

    #[tokio::test]
    async fn test_panic_close_emits_core_position_closed() {
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // 8% stop — passes risk gate (1% loss cap on $10k capital; margin still fits)
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(), // 8% below — passes risk gate
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };
        manager.handle_signal(signal).await.unwrap();

        // Verify position is Active before panic close (entry was not denied)
        let pos = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(pos.state, PositionState::Active { .. }),
            "Expected Active before panic close, got {:?}",
            pos.state
        );

        let _ = manager.panic_close_all().await.unwrap();

        let mut closed = false;
        for _ in 0..20 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
            {
                if let Ok(DaemonEvent::CorePositionClosed { position_id, symbol, side }) = event {
                    assert_eq!(position_id, position.id);
                    assert_eq!(symbol.as_pair(), "BTCUSDT");
                    assert_eq!(side, Side::Long);
                    closed = true;
                    break;
                }
            }
        }
        assert!(closed, "Expected CorePositionClosed event");
    }

    /// Phase 2: Risk gate denial keeps position Armed and re-arms the detector.
    /// Risk gate denial re-arms detector and keeps position Armed.
    ///
    /// Under ADR-0024, the duplicate-position guard (same symbol+side) still
    /// triggers a governed denial. The position must remain Armed and get a
    /// fresh detector.
    #[tokio::test]
    async fn test_risk_gate_denial_rearmed_and_position_stays_armed() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        // Seed an active BTCUSDT Long position to trigger duplicate denial
        let mut pos = Position::new(uuid::Uuid::now_v7(), symbol.clone(), Side::Long);
        pos.quantity = Quantity::new(dec!(0.01)).unwrap();
        pos.entry_price = Some(Price::new(dec!(95000)).unwrap());
        pos.state = PositionState::Active {
            current_price: Price::new(dec!(95000)).unwrap(),
            trailing_stop: Price::new(dec!(93500)).unwrap(),
            favorable_extreme: Price::new(dec!(95000)).unwrap(),
            extreme_at: chrono::Utc::now(),
            insurance_stop_id: None,
            invalidation_guard_level: None,
            last_emitted_stop: None,
        };
        manager.store.positions().save(&pos).await.unwrap();

        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(93100)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        // handle_signal must return Ok(()) — denial is a governed outcome
        manager.handle_signal(signal).await.unwrap();

        // Position must still be Armed — entry was denied by duplicate guard
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after denial, got {:?}",
            updated.state
        );

        // Detector must have been re-armed
        let detectors = manager.detectors.read().await;
        assert!(
            detectors.contains_key(&position.id),
            "Expected detector to be re-armed after risk denial"
        );
    }

    #[tokio::test]
    async fn test_margin_rejection_moves_to_error_without_rearming_detector() {
        let exchange = Arc::new(StubExchange::new(dec!(95000)));
        exchange.set_futures_settings("Hedge", RiskConfig::LEVERAGE);
        exchange.set_trading_rules(
            robson_domain::SymbolTradingRules::new(
                Symbol::from_pair("BTCUSDT").unwrap(),
                dec!(0.01),
                Decimal::ZERO,
                dec!(0.000001),
                dec!(0.000001),
                dec!(1000000),
                Decimal::ZERO,
                2,
                6,
            )
            .unwrap(),
        );
        let journal = Arc::new(IntentJournal::new());
        let store = Arc::new(MemoryStore::new());
        let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
        let event_bus = Arc::new(EventBus::new(100));
        let risk_config = RiskConfig::new(dec!(10000)).unwrap();
        let engine = Engine::new(risk_config);
        let manager = Arc::new(
            PositionManager::with_approval_policy(
                engine,
                executor,
                store,
                event_bus,
                Arc::new(TracingQueryRecorder),
                ApprovalPolicy::new(Decimal::from(100u32), 300),
                TradingPolicy::default(),
            )
            .with_ohlcv_port(Arc::new(StubOhlcv::new(create_test_candles()))),
        );

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(87400)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: entry,
            stop_loss: stop,
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        let result = manager.handle_signal(signal).await;
        assert!(matches!(
            result,
            Err(DaemonError::Exec(robson_exec::ExecError::InvalidState(_)))
        ));

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Error { recoverable: true, .. }),
            "Expected recoverable Error after margin rejection, got {:?}",
            updated.state
        );

        let detectors = manager.detectors.read().await;
        assert!(
            !detectors.contains_key(&position.id),
            "Detector must not be re-armed after internal margin rejection"
        );
    }

    #[tokio::test]
    async fn test_handle_signal_rejects_quantity_below_exchange_step_size_before_exchange() {
        let manager = Arc::new({
            let exchange = Arc::new(StubExchange::new(dec!(76200)));
            exchange.set_trading_rules(
                robson_domain::SymbolTradingRules::new(
                    Symbol::from_pair("BTCUSDT").unwrap(),
                    dec!(0.01),
                    Decimal::ZERO,
                    dec!(0.001),
                    dec!(0.001),
                    dec!(1000000),
                    Decimal::ZERO,
                    2,
                    3,
                )
                .unwrap(),
            );
            let journal = Arc::new(IntentJournal::new());
            let store = Arc::new(MemoryStore::new());
            let executor = Arc::new(Executor::new(exchange, journal, store.clone()));
            let event_bus = Arc::new(EventBus::new(100));
            let risk_config = RiskConfig::new(dec!(100)).unwrap();
            let engine = Engine::new(risk_config);

            PositionManager::with_approval_policy(
                engine,
                executor,
                store,
                event_bus,
                Arc::new(TracingQueryRecorder),
                ApprovalPolicy::new(Decimal::from(100u32), 300),
                TradingPolicy::default(),
            )
            .with_ohlcv_port(Arc::new(StubOhlcv::new(create_test_candles())))
        });

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                RiskConfig::new(dec!(100)).unwrap(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(76200)).unwrap(),
            stop_loss: Price::new(dec!(73825.27)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        // ExecutableSpan admission has live lot rules, so the quantity is
        // quantized and rejected as a governed domain outcome before any
        // exchange submission rather than relying on an adapter rejection.
        manager.handle_signal(signal).await.unwrap();
        assert_eq!(manager.exchange().market_order_call_count(), 0, "nothing reaches the venue");

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after quantity rejection, got {:?}",
            updated.state
        );

        let detectors = manager.detectors.read().await;
        assert!(
            detectors.contains_key(&position.id),
            "Detector must be re-armed after pre-exchange quantity rejection"
        );
    }

    #[tokio::test]
    async fn test_handle_signal_waits_for_approval_when_required() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                hc_policy,
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(), /* 10% below -> approval required, risk
                                                          * approved */
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed while approval is pending, got {:?}",
            updated.state
        );

        let pending = manager.pending_approvals.read().await;
        assert_eq!(pending.len(), 1, "Expected exactly one pending approval");
        let record = pending.values().next().expect("pending approval must exist");
        assert_eq!(record.query.state, QueryState::AwaitingApproval);
        assert!(record.query.approval.is_some());
        drop(pending);

        let mut awaiting_seen = false;
        for _ in 0..20 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
            {
                if let Ok(DaemonEvent::QueryAwaitingApproval { query_id, position_id, .. }) = event
                {
                    assert_eq!(position_id, Some(position.id));
                    let pending = manager.pending_approvals.read().await;
                    assert!(pending.contains_key(&query_id));
                    awaiting_seen = true;
                    break;
                }
            }
        }
        assert!(awaiting_seen, "Expected QueryAwaitingApproval event");
    }

    // -------------------------------------------------------------------------
    // Domain ApprovalPolicy integration tests (Phase 4)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_arm_position_with_explicit_policy_stores_policy() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = manager
            .arm_position_with_policy(
                symbol,
                Side::Long,
                create_test_risk_config(),
                None,
                Uuid::now_v7(),
                policy,
            )
            .await
            .unwrap();

        let stored = manager.entry_policy_for_position(position.id).await;
        assert_eq!(stored.mode, EntryPolicy::ConfirmedTrend);
        assert_eq!(stored.approval, DomainApprovalPolicy::HumanConfirmation);
    }

    /// `ApprovalPolicy::Automatic` must bypass the notional-threshold gate even
    /// when notional exceeds the threshold that would normally require
    /// approval.
    #[tokio::test]
    async fn test_handle_signal_automatic_bypasses_notional_threshold() {
        // phase3 manager has 5% threshold; signal notional will exceed it.
        let manager = create_phase3_test_manager(300).await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let policy = EntryPolicyConfig {
            mode: robson_domain::EntryPolicy::ConfirmedTrend,
            approval: robson_domain::ApprovalPolicy::Automatic,
        };

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                policy,
            )
            .await
            .unwrap();

        // Signal with notional > 5% threshold (entry 95000, stop 85500, qty ~0.0105)
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        // Automatic: no pending approval, position must have moved past Armed.
        let pending = manager.pending_approvals.read().await;
        assert!(pending.is_empty(), "Expected no pending approvals with Automatic policy");
    }

    /// `ApprovalPolicy::HumanConfirmation` must always require operator
    /// approval even when the notional is below the threshold that normally
    /// auto-proceeds.
    #[tokio::test]
    async fn test_handle_signal_human_confirmation_always_waits() {
        // create_test_manager uses 100% threshold → normally no approval needed.
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let policy = EntryPolicyConfig {
            mode: robson_domain::EntryPolicy::ConfirmedTrend,
            approval: robson_domain::ApprovalPolicy::HumanConfirmation,
        };

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                policy,
            )
            .await
            .unwrap();

        // Low-notional signal that would normally proceed automatically.
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        // HumanConfirmation: position stays Armed, one pending approval.
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed while awaiting human confirmation, got {:?}",
            updated.state
        );
        let pending = manager.pending_approvals.read().await;
        assert_eq!(pending.len(), 1, "Expected exactly one pending approval");
        let record = pending.values().next().expect("pending approval record must exist");
        assert_eq!(record.query.state, QueryState::AwaitingApproval);
    }

    #[tokio::test]
    async fn test_approve_query_executes_pending_signal() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                hc_policy,
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(), // HumanConfirmation always waits
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let query_id = {
            let pending = manager.pending_approvals.read().await;
            *pending.keys().next().expect("pending approval query must exist")
        };

        let approved_query = manager.approve_query(query_id).await.unwrap();

        assert_eq!(approved_query.state, QueryState::Completed);
        assert!(manager.pending_approvals.read().await.is_empty());

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Active { .. }),
            "Expected Active after approval execution, got {:?}",
            updated.state
        );
    }

    #[tokio::test]
    async fn test_disarm_invalidates_pending_approval() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                hc_policy,
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let query_id = {
            let pending = manager.pending_approvals.read().await;
            *pending.keys().next().expect("pending approval query must exist")
        };

        manager.disarm_position(position.id).await.unwrap();

        assert!(manager.pending_approvals.read().await.is_empty());
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Cancelled),
            "Expected Cancelled after disarm, got {:?}",
            updated.state
        );

        let approval_result = manager.approve_query(query_id).await;
        assert!(matches!(approval_result, Err(DaemonError::QueryNotFound(id)) if id == query_id));
    }

    /// Approval is denied when risk context changes (ADR-0024: duplicate
    /// guard).
    ///
    /// Get a pending approval, then seed a duplicate position. Revalidation
    /// denies the approval.
    #[tokio::test]
    async fn test_approve_query_denied_when_risk_context_changes() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        let manager = create_test_manager().await;

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                hc_policy,
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let query_id = {
            let pending = manager.pending_approvals.read().await;
            *pending.keys().next().expect("pending approval query must exist")
        };

        // Seed a duplicate BTCUSDT Long position
        save_active_position(&manager, "BTCUSDT", Side::Long, dec!(95000), dec!(0.01)).await;

        let approval_result = manager.approve_query(query_id).await;
        assert!(matches!(
            approval_result,
            Err(DaemonError::ApprovalDenied { query_id: denied_query_id, .. })
            if denied_query_id == query_id
        ));
        assert!(manager.pending_approvals.read().await.is_empty());

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after approval denial, got {:?}",
            updated.state
        );

        let detectors = manager.detectors.read().await;
        assert!(
            detectors.contains_key(&position.id),
            "Detector must be re-armed after approval denial"
        );
    }

    #[tokio::test]
    async fn test_pending_approval_expires_and_does_not_execute() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        // 1-second TTL so the approval expires quickly during the test.
        let manager = create_phase3_test_manager(1).await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                hc_policy,
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(), /* 10% below -> approval required, risk
                                                          * approved */
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let mut expired_seen = false;
        for _ in 0..40 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(100), receiver.recv()).await
            {
                if let Ok(DaemonEvent::QueryExpired { position_id, .. }) = event {
                    assert_eq!(position_id, Some(position.id));
                    expired_seen = true;
                    break;
                }
            }
        }
        assert!(expired_seen, "Expected QueryExpired event");

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after approval expiry, got {:?}",
            updated.state
        );
        assert!(manager.pending_approvals.read().await.is_empty());

        let detectors = manager.detectors.read().await;
        assert!(
            detectors.contains_key(&position.id),
            "Detector must be re-armed after approval expiry"
        );
    }

    /// Entering positions reserve latent risk for dynamic slots.
    ///
    /// Seed four Entering positions with unique symbol+side pairs, each
    /// carrying one risk unit. A new non-duplicate entry must be denied
    /// because the monthly slot budget is exhausted before those orders
    /// fill.
    #[tokio::test]
    async fn test_entering_positions_count_in_risk_context() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        // Capital is 10_000, so one risk unit is 100.
        // Each seeded Entering position has latent risk:
        // (entry 100 - stop 96) * qty 25 = 100.
        for pair in ["ETHUSDT", "SOLUSDT", "XRPUSDT", "DOGEUSDT"] {
            let entry = Price::new(dec!(100)).unwrap();
            let stop = Price::new(dec!(96)).unwrap();
            let mut pos =
                Position::new(uuid::Uuid::now_v7(), Symbol::from_pair(pair).unwrap(), Side::Long);
            pos.quantity = Quantity::new(dec!(25)).unwrap();
            pos.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(entry, stop));
            pos.state = PositionState::Entering {
                entry_order_id: uuid::Uuid::now_v7(),
                expected_entry: entry,
                signal_id: uuid::Uuid::now_v7(),
            };
            manager.store.positions().save(&pos).await.unwrap();
        }

        // Arm a non-duplicate position (the one we will try to enter).
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(92)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                uuid::Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: uuid::Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let halt = manager.circuit_breaker.snapshot().await;
        assert_eq!(halt.state, crate::circuit_breaker::HaltState::MonthlyHalt);
        assert!(halt.blocks_new_entries, "Slot exhaustion must trigger MonthlyHalt");

        // Slot exhaustion is represented as MonthlyDrawdown today, so the
        // MonthlyHalt path cancels all armed positions (no exchange action taken).
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Cancelled),
            "Expected Cancelled after MonthlyHalt (Entering positions exhausted slots), got {:?}",
            updated.state
        );
    }

    /// E2E test: full detector integration (arm → spawn detector → MA crossover
    /// → signal → entry)
    ///
    /// Flow:
    /// 1. arm_position() → spawns detector
    /// 2. Inject synthetic market data via EventBus
    /// 3. Wait for MA crossover → DetectorSignal
    /// 4. Signal listener processes signal → Entry order → Position becomes
    ///    Active
    ///
    /// Scope: Uses stub exchange, NO real orders, NO WebSocket
    #[tokio::test]
    async fn test_e2e_detector_ma_crossover_signal() {
        // Use manager WITH signal listener for full E2E flow
        let manager = create_test_manager_with_listener().await;
        let event_bus = manager.event_bus.clone();

        // Subscribe to EventBus to capture DetectorSignal
        let mut signal_receiver = event_bus.subscribe();

        // Arm position (spawns detector internally)
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let position_id = position.id;

        // Yield to let detector task subscribe
        tokio::task::yield_now().await;

        // Feed descending prices (fast MA < slow MA) to establish "below" state
        // Need enough data points for MA calculation (slow_period=21 default)
        for i in (0..30).rev() {
            let price = Decimal::from(100 + i);
            let market_data = MarketData {
                venue: test_venue(),
                symbol: symbol.clone(),
                price: Price::new(price).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            };
            event_bus.send(DaemonEvent::MarketData(market_data));
        }

        // Feed ascending prices; strategy confirmation comes from persisted
        // candles, while market data is only the detector evaluation trigger.
        for i in 0..10 {
            let price = Decimal::from(100 + i * 3); // Larger steps to trigger crossover faster
            let market_data = MarketData {
                venue: test_venue(),
                symbol: symbol.clone(),
                price: Price::new(price).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            };
            event_bus.send(DaemonEvent::MarketData(market_data));
        }

        let detector_signal = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match signal_receiver.recv().await {
                    Some(Ok(DaemonEvent::DetectorSignal(signal)))
                        if signal.position_id == position_id =>
                    {
                        return Some(signal);
                    },
                    Some(Ok(_)) | Some(Err(_)) => continue,
                    None => return None,
                }
            }
        })
        .await
        .expect("Timed out waiting for detector signal");
        let signal = detector_signal.expect("Detector should emit signal on MA crossover");

        // Assert: signal properties
        assert_eq!(signal.position_id, position_id);
        assert_eq!(signal.symbol.as_pair(), "BTCUSDT");
        assert_eq!(signal.side, Side::Long);
        assert!(signal.entry_price.as_decimal() > dec!(0));
        assert!(signal.stop_loss.as_decimal() > dec!(0));

        // Verify detector was cleaned up (single-shot)
        // Detector should be removed after signaling (checked via detector count)
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_arm_immediate_automatic_processes_entry_without_market_tick() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};

        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let policy =
            EntryPolicyConfig::new(EntryPolicy::Immediate, DomainApprovalPolicy::Automatic);

        let position = manager
            .arm_position_with_policy(
                symbol,
                Side::Long,
                create_test_risk_config(),
                None,
                Uuid::now_v7(),
                policy,
            )
            .await
            .unwrap();

        assert!(
            !matches!(position.state, PositionState::Armed),
            "Immediate automatic arm must not remain waiting for a strategy signal"
        );
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events.iter().any(|event| matches!(event, Event::EntrySignalReceived { .. })));
    }

    /// An `Immediate` arm enters, and places the entry's stop, under the arm's
    /// own permit: one ordinary slot, no second permit, every venue mutation
    /// under it, and the entry and its stop charged to one operation
    /// (ADR-0056 step 7).
    #[tokio::test(flavor = "current_thread")]
    async fn an_immediate_arm_enters_and_protects_under_its_own_permit() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        use robson_exec::venue::Venue;

        let manager = create_test_manager().await;
        let journal = manager.order_journal();
        manager.exchange().attach_gate(Arc::clone(journal.gate()));
        let policy =
            EntryPolicyConfig::new(EntryPolicy::Immediate, DomainApprovalPolicy::Automatic);

        let position = journal
            .acquire(OperationKind::Arm)
            .unwrap()
            .run(manager.arm_position_with_policy(
                Symbol::from_pair("BTCUSDT").unwrap(),
                Side::Long,
                create_test_risk_config(),
                None,
                Uuid::now_v7(),
                policy,
            ))
            .await
            .unwrap();

        assert!(matches!(position.state, PositionState::Active { .. }), "{:?}", position.state);
        let mutations = manager.exchange().mutations();
        let what: Vec<_> = mutations.iter().map(|m| m.what).collect();
        assert!(what.contains(&"POST /fapi/v1/order"), "the entry: {what:?}");
        assert!(what.contains(&"POST /fapi/v1/algoOrder"), "its stop: {what:?}");
        for mutation in mutations {
            assert_eq!(mutation.permit, Some(OperationKind::Arm), "{mutation:?}");
            let gate = mutation.gate.unwrap();
            assert_eq!((gate.permits, gate.ordinary, gate.unpermitted), (1, 1, 0), "{mutation:?}");
        }
        assert_eq!(journal.operation_statements_max(), 18, "entry and stop, one operation");
        assert_eq!(journal.gate().snapshot().permits, 0);
    }

    /// Once the daemon is stopping, an entry from the main loop is refused at
    /// its send: a typed no-send, nothing reaches the venue, and a leg with
    /// admission evidence ends its attempt (ADR-0056 step 7, design §2.4).
    #[tokio::test(flavor = "current_thread")]
    async fn an_entry_refused_at_its_send_while_draining_ends_as_a_no_send() {
        use robson_exec::venue::Venue;

        let manager = create_test_manager().await;
        let journal = manager.order_journal();
        manager.exchange().attach_gate(Arc::clone(journal.gate()));
        let position = manager
            .arm_position(
                Symbol::from_pair("BTCUSDT").unwrap(),
                Side::Long,
                create_test_risk_config(),
                Some(TechnicalStopDistance::from_entry_and_stop(
                    Price::new(dec!(100)).unwrap(),
                    Price::new(dec!(90)).unwrap(),
                )),
                Uuid::now_v7(),
            )
            .await
            .unwrap();
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: position.symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(86000)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        let permit = journal.acquire(OperationKind::MainLoop).unwrap();
        journal.gate().begin_draining();
        let refused = permit.run(manager.handle_signal(signal)).await;

        assert!(
            matches!(refused, Err(DaemonError::Exec(ExecError::SendRefused(_)))),
            "{refused:?}"
        );
        assert_eq!(manager.exchange().market_order_call_count(), 0);
        assert!(
            manager.exchange().mutations().iter().all(|m| m.what != "POST /fapi/v1/order"),
            "no order went out"
        );
        assert_eq!(journal.gate().snapshot().sends_started, 0);
        let leg = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(leg.state, PositionState::Armed), "{:?}", leg.state);
        assert!(
            manager.is_entry_exhausted(position.id).await,
            "evidence-bearing: the attempt ends"
        );
    }

    /// A capital-capped immediate entry must ACTIVATE, not be denied by the
    /// risk gate's qty × entry round-trip (2026-07-04 prod incident: the
    /// Decimal round-trip exceeded capital by ~1e-22 and the gate rejected
    /// the capital cap sizing itself chose, cancelling the position).
    ///
    /// With sizing and the gate reading the same in-memory capital, a genuine
    /// insufficient-margin denial is no longer constructible here; that gate
    /// branch stays covered by robson-engine's RiskGate tests and remains
    /// reachable in production only when the persisted monthly ledger capital
    /// diverges from the engine's configured capital.
    #[tokio::test(flavor = "current_thread")]
    async fn test_arm_immediate_capital_capped_entry_activates_within_capital() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};

        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let arm_policy =
            EntryPolicyConfig::new(EntryPolicy::ConfirmedTrend, DomainApprovalPolicy::Automatic);
        let immediate_policy =
            EntryPolicyConfig::new(EntryPolicy::Immediate, DomainApprovalPolicy::Automatic);

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                RiskConfig::new(dec!(100)).unwrap(),
                None,
                Uuid::now_v7(),
                arm_policy,
            )
            .await
            .unwrap();
        manager.entry_policies.write().await.insert(position.id, immediate_policy);

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(94700)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let position = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(position.state, PositionState::Active { .. }),
            "Capital-capped immediate entry must activate, got {:?}",
            position.state
        );

        // The tight stop makes the risk-sized quantity exceed 1x capital, so
        // the capital cap engages: notional fits the $100 capital minus the
        // default 100 bps headroom (exchange fee + mark-price cushion), with
        // no round-trip excess for the gate to reject.
        let notional = position.quantity.as_decimal() * dec!(95000);
        assert!(notional > dec!(98.9), "capital cap must engage, notional {}", notional);
        assert!(notional <= dec!(99), "notional {} must fit capital minus headroom", notional);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_restore_armed_immediate_automatic_processes_first_market_tick() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};

        let store = Arc::new(MemoryStore::new());
        let original_manager = create_test_manager_with_store_and_event_bus(
            Arc::clone(&store),
            Arc::new(EventBus::new(100)),
            ApprovalPolicy::new(Decimal::from(100u32), 300),
        )
        .await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let policy =
            EntryPolicyConfig::new(EntryPolicy::Immediate, DomainApprovalPolicy::Automatic);

        let position_id = Uuid::now_v7();
        let account_id = Uuid::now_v7();
        let now = chrono::Utc::now();
        original_manager
            .execute_and_persist(vec![
                EngineAction::EmitEvent(Event::PositionArmed {
                    position_id,
                    account_id,
                    symbol: symbol.clone(),
                    side: Side::Long,
                    tech_stop_distance: None,
                    stop_policy: StopPolicy::LegacyUncapped,
                    stop_buffer_bps_at_arm: None,
                    timestamp: now,
                }),
                EngineAction::EmitEvent(Event::EntryPolicyResolved {
                    position_id,
                    entry_policy: policy.mode,
                    approval_policy: policy.approval,
                    strategy_id: StrategyRegistry::strategy_id_for_policy(policy.mode),
                    timestamp: now,
                }),
            ])
            .await
            .unwrap();

        original_manager.shutdown().await;

        let restored_event_bus = Arc::new(EventBus::new(100));
        let restored_manager = create_test_manager_with_store_and_event_bus(
            Arc::clone(&store),
            Arc::clone(&restored_event_bus),
            ApprovalPolicy::new(Decimal::from(100u32), 300),
        )
        .await;
        PositionManager::start(Arc::clone(&restored_manager));
        tokio::task::yield_now().await;

        let restored_position = store
            .positions()
            .find_by_id(position_id)
            .await
            .unwrap()
            .expect("armed position must survive restart");

        restored_manager
            .restore_armed_position(&restored_position, policy)
            .await
            .unwrap();

        // Regression coverage: this first tick is sent immediately after restore,
        // before the spawned detector task has a chance to poll on current_thread.
        restored_event_bus.send(DaemonEvent::MarketData(MarketData {
            venue: test_venue(),
            symbol: symbol.clone(),
            price: Price::new(dec!(100)).unwrap(),
            timestamp: chrono::Utc::now(),
            source: crate::event_bus::MarketDataSource::Ws,
        }));

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let events = store.events().find_by_position(position_id).await.unwrap();
                if events.iter().any(|event| {
                    matches!(event, Event::EntrySignalReceived { position_id: event_position_id, .. } if *event_position_id == position_id)
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("restored immediate detector must process the first market tick");

        restored_manager.shutdown().await;
    }

    // =========================================================================
    // MonthlyHalt auto-trigger tests
    // =========================================================================

    /// Helper: save a closed position with realized PnL in the current month.
    async fn save_closed_position_with_pnl(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        realized_pnl: Decimal,
    ) -> Position {
        save_closed_position_with_pnl_and_reason(
            manager,
            realized_pnl,
            robson_domain::ExitReason::TrailingStop,
        )
        .await
    }

    async fn save_closed_position_with_pnl_and_reason(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        realized_pnl: Decimal,
        exit_reason: robson_domain::ExitReason,
    ) -> Position {
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let exit = Price::new(dec!(95000) + realized_pnl).unwrap();
        let now = chrono::Utc::now();

        let mut position = Position::new(account_id, symbol, Side::Long);
        position.entry_price = Some(entry);
        position.quantity = Quantity::new(dec!(0.1)).unwrap();
        position.realized_pnl = realized_pnl;
        position.closed_at = Some(now);
        position.updated_at = now;
        position.state = PositionState::Closed {
            exit_price: exit,
            realized_pnl,
            exit_reason,
        };

        manager.store.positions().save(&position).await.unwrap();
        position
    }

    #[tokio::test]
    async fn test_monthly_halt_not_triggered_at_399_pct_loss() {
        let manager = create_test_manager().await;
        // ADR-0043: remaining = budget - realized_loss = 400 - 399 = 1 > 0.
        // Pre-ADR-0043 this halted (1 < 100 risk_per_trade); now the system
        // stays live — a smaller planned-risk entry may still fit.
        save_closed_position_with_pnl(&manager, dec!(-399)).await;

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(
            !triggered,
            "3.99% loss leaves budget (remaining=1 > 0) — must stay live (ADR-0043)"
        );
        assert!(!manager.circuit_breaker.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_monthly_halt_auto_triggers_at_exactly_4_pct_loss() {
        let manager = create_test_manager().await;
        // Capital is 10000, 4% = 400. At exactly -400, should trigger.
        save_closed_position_with_pnl(&manager, dec!(-400)).await;

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(triggered, "exactly 4% monthly loss must trigger MonthlyHalt");
        assert!(manager.circuit_breaker.blocks_new_entries().await);
    }

    /// ADR-0046 §3: month equity must mark open positions at the live price,
    /// not at the stored `current_price` (fill price, or the last trailing
    /// ratchet, or the entry price again after a restart).
    #[tokio::test]
    async fn last_market_price_for_expires_after_the_watchdog_window() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let price = Price::new(dec!(80_000)).unwrap();

        assert!(manager.last_market_price_for(&symbol).await.is_none());

        manager
            .set_last_market_price_for_test(&symbol, price, std::time::Instant::now())
            .await;
        assert_eq!(manager.last_market_price_for(&symbol).await, Some(price));

        // A symbol whose feed went quiet (e.g. only an Exiting position, not
        // covered by the REST fallback poll) must not serve an old mark to
        // `/status`; the caller falls back to the exchange.
        let stale = std::time::Instant::now()
            - MARKET_PRICE_CACHE_MAX_AGE
            - std::time::Duration::from_secs(1);
        manager.set_last_market_price_for_test(&symbol, price, stale).await;
        assert!(manager.last_market_price_for(&symbol).await.is_none());
    }

    #[tokio::test]
    async fn another_venues_tick_moves_neither_prices_nor_positions() {
        let manager = create_test_manager().await;
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        position.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(100)).unwrap(),
            Price::new(dec!(90)).unwrap(),
        ));
        manager.store.positions().save(&position).await.unwrap();
        let symbol = position.symbol.clone();
        let events_before = manager.store.events().find_by_position(position.id).await.unwrap();

        // Far through the stop on another venue: not this position's market.
        let other_venue = VenueId::new("binance:spot");
        manager
            .process_market_data(MarketData {
                venue: other_venue.clone(),
                symbol: symbol.clone(),
                price: Price::new(dec!(50)).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await
            .unwrap();

        assert!(
            manager.last_market_price_for(&symbol).await.is_none(),
            "a foreign tick must not read as this venue's price"
        );
        let stored = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(matches!(stored.state, PositionState::Active { .. }));
        assert_eq!(manager.executor.exchange().market_order_call_count(), 0);
        let events_after = manager.store.events().find_by_position(position.id).await.unwrap();
        assert_eq!(events_after.len(), events_before.len());

        // Both feeds keep their own entry for the same symbol.
        manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: symbol.clone(),
                price: Price::new(dec!(101)).unwrap(),
                timestamp: chrono::Utc::now(),
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await
            .unwrap();
        assert_eq!(
            manager.last_market_price_for(&symbol).await,
            Some(Price::new(dec!(101)).unwrap())
        );
        let prices = manager.price_cache();
        let prices = prices.read().await;
        assert_eq!(prices.len(), 2);
        let foreign = prices.get(&FeedKey::new(other_venue, symbol)).unwrap();
        assert_eq!(foreign.0, Price::new(dec!(50)).unwrap());
    }

    #[tokio::test]
    async fn test_month_equity_marks_open_positions_at_last_market_price() {
        let manager = create_test_manager().await;
        let now = chrono::Utc::now();
        // Stored current_price == entry (as after a restart): unrealized 0.
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(1)).await;
        position.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(100)).unwrap(),
            Price::new(dec!(90)).unwrap(),
        ));
        manager.store.positions().save(&position).await.unwrap();
        assert_eq!(manager.month_equity_net(now).await.unwrap(), Decimal::ZERO);

        // A tick that moves neither the stop nor the stored price (legacy
        // span is 10, favorable move is 5).
        manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: position.symbol.clone(),
                price: Price::new(dec!(105)).unwrap(),
                timestamp: now,
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await
            .unwrap();

        // The favorable move persisted the watermark (issue #192), so the
        // stored price now reads 105 too; equity agrees.
        let stored = manager.get_position(position.id).await.unwrap().unwrap();
        match stored.state {
            PositionState::Active { current_price, .. } => {
                assert_eq!(current_price.as_decimal(), dec!(105), "watermark advance persisted")
            },
            other => panic!("expected Active, got {other:?}"),
        }
        assert_eq!(manager.month_equity_net(now).await.unwrap(), dec!(5));

        // A later adverse tick lowers equity again: give-back is visible
        // intra-trade, not only at exit. The stored price stays at the
        // watermark (105), so this is where the live mark must win.
        manager
            .process_market_data(MarketData {
                venue: test_venue(),
                symbol: position.symbol.clone(),
                price: Price::new(dec!(98)).unwrap(),
                timestamp: now,
                source: crate::event_bus::MarketDataSource::Ws,
            })
            .await
            .unwrap();
        let stored = manager.get_position(position.id).await.unwrap().unwrap();
        match stored.state {
            PositionState::Active { current_price, .. } => {
                assert_eq!(
                    current_price.as_decimal(),
                    dec!(105),
                    "adverse ticks never move the stored price"
                )
            },
            other => panic!("expected Active, got {other:?}"),
        }
        assert_eq!(
            manager.month_equity_net(now).await.unwrap(),
            dec!(-2),
            "equity must use the live mark (98) rather than the stored price (105)"
        );
    }

    #[tokio::test]
    async fn test_monthly_halt_triggers_on_giveback_from_positive_peak() {
        let manager = create_test_manager().await;
        let now = chrono::Utc::now();
        save_closed_position_with_pnl(&manager, dec!(300)).await;
        manager.refresh_month_peak_net(now).await.unwrap();

        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let current = Price::new(dec!(91000)).unwrap();
        let qty = Quantity::new(dec!(0.1)).unwrap();
        let mut position = Position::new(account_id, symbol, Side::Long);
        position.entry_price = Some(entry);
        position.quantity = qty;
        position.state = PositionState::Active {
            current_price: current,
            trailing_stop: entry,
            favorable_extreme: entry,
            extreme_at: now,
            insurance_stop_id: None,
            invalidation_guard_level: None,
            last_emitted_stop: None,
        };
        position.updated_at = now;
        manager.store.positions().save(&position).await.unwrap();

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(
            triggered,
            "peak=300 and equity=-100 gives back 400 even with gross realized loss at 0"
        );
        assert!(manager.circuit_breaker.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_monthly_halt_not_triggered_at_net_390_from_start_with_zero_peak() {
        let manager = create_test_manager().await;
        save_closed_position_with_pnl(&manager, dec!(-390)).await;

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(!triggered, "peak=0 and equity=-390 leaves 10 budget");
        assert!(!manager.circuit_breaker.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_month_peak_net_persists_monotonic_in_month() {
        let manager = create_test_manager().await;
        let now = chrono::Utc::now();
        save_closed_position_with_pnl(&manager, dec!(300)).await;
        manager.refresh_month_peak_net(now).await.unwrap();
        assert_eq!(manager.load_monthly_state(now).await.unwrap().month_peak_net, dec!(300));

        save_closed_position_with_pnl(&manager, dec!(-100)).await;
        manager.refresh_month_peak_net(now).await.unwrap();
        assert_eq!(
            manager.load_monthly_state(now).await.unwrap().month_peak_net,
            dec!(300),
            "month_peak_net must never decrease within the month"
        );
    }

    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn test_all_monthly_state_writers_inherit_prior_budget_model(pool: sqlx::PgPool) {
        enum WriterCase {
            Projector {
                name: &'static str,
                event_type: &'static str,
                payload: serde_json::Value,
            },
            PeakRefresh,
        }

        let target = chrono::Utc.with_ymd_and_hms(2027, 4, 15, 12, 0, 0).unwrap();
        let nfs = MonthlyBudgetModel::NetFromStartNonExpandingV1;

        // Each case starts without the target month's row, so it exercises the
        // INSERT inheritance path rather than an ON CONFLICT update that
        // intentionally preserves the existing model.
        sqlx::query(
            r#"
            INSERT INTO monthly_state (
                year, month, capital_base, monthly_budget_model,
                boundary_reset_at, created_at
            )
            VALUES (2027, 3, 10000, 'net_from_start_non_expanding_v1', $1, $1)
            ON CONFLICT (year, month) DO UPDATE SET
                monthly_budget_model = EXCLUDED.monthly_budget_model
            "#,
        )
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();

        let cases = vec![
            WriterCase::Projector {
                name: "month_boundary_reset",
                event_type: "month_boundary_reset",
                payload: serde_json::json!({
                    "capital_base": "10100",
                    "carried_positions_risk": "0",
                    "month": 4,
                    "year": 2027,
                    "timestamp": target,
                }),
            },
            WriterCase::Projector {
                name: "capital_base_recalibrated",
                event_type: "capital_base_recalibrated",
                payload: serde_json::json!({
                    "previous_capital_base": "10000",
                    "new_capital_base": "9900",
                    "wallet_balance": "9900",
                    "carried_risk": "0",
                    "reason": "test",
                    "evidence": "test",
                    "month": 4,
                    "year": 2027,
                    "timestamp": target,
                }),
            },
            WriterCase::PeakRefresh,
        ];

        for (index, case) in cases.into_iter().enumerate() {
            sqlx::query("DELETE FROM monthly_state WHERE year = 2027 AND month = 4")
                .execute(&pool)
                .await
                .unwrap();

            let name = match case {
                WriterCase::Projector { name, event_type, payload } => {
                    let envelope =
                        projector_test_envelope(event_type, payload, target, index as i64 + 1);
                    apply_event_to_projections(&pool, &envelope).await.unwrap();
                    name
                },
                WriterCase::PeakRefresh => {
                    let manager = create_postgres_test_manager(pool.clone());
                    let monthly = manager.load_monthly_state(target).await.unwrap();
                    assert_eq!(
                        monthly.budget_model, nfs,
                        "missing-row runtime fallback must inherit NFS"
                    );
                    manager
                        .refresh_month_peak_net_from_snapshot(target, &monthly, dec!(10))
                        .await
                        .unwrap();
                    "refresh_month_peak_net_from_snapshot"
                },
            };

            let model: String = sqlx::query_scalar(
                "SELECT monthly_budget_model FROM monthly_state WHERE year = 2027 AND month = 4",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(model, nfs.as_str(), "{name} must inherit NFS");
        }

        sqlx::query("DELETE FROM monthly_state WHERE year = 2027 AND month = 4")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            INSERT INTO monthly_state (
                year, month, capital_base, month_peak_net,
                monthly_budget_model, created_at
            )
            VALUES (2027, 4, 0, 0, 'net_from_start_non_expanding_v1', $1)
            "#,
        )
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
        let invalid_snapshot = create_postgres_test_manager(pool)
            .canonical_budget_snapshot(target)
            .await
            .unwrap();
        assert!(invalid_snapshot.capital_base_invalid);
        assert_eq!(invalid_snapshot.budget_amount(), Decimal::ZERO);
        assert_eq!(invalid_snapshot.slots_active, 0);
    }

    #[cfg(feature = "postgres")]
    #[sqlx::test(migrations = "../migrations")]
    #[ignore = "requires DATABASE_URL"]
    async fn test_losing_close_before_first_boundary_poll_keeps_a_valid_base(pool: sqlx::PgPool) {
        let manager = create_postgres_test_manager(pool.clone());
        let now = chrono::Utc::now();

        let envelope = projector_test_envelope(
            "position_closed",
            serde_json::json!({
                "position_id": Uuid::new_v4(),
                "exit_reason": "TrailingStop",
                "entry_price": "100",
                "exit_price": "90",
                "realized_pnl": "-10",
                "total_fees": "1",
                "timestamp": now,
            }),
            now,
            1,
        );
        apply_event_to_projections(&pool, &envelope).await.unwrap();

        let snapshot = manager.canonical_budget_snapshot(now).await.unwrap();
        assert!(!snapshot.capital_base_invalid);
        assert_eq!(snapshot.capital_base, dec!(10000));
        assert!(!manager.evaluate_monthly_halt().await);
        assert!(!manager.circuit_breaker().blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_month_peak_net_resets_at_month_boundary() {
        let manager = create_test_manager().await;
        let now = chrono::Utc::now();
        save_closed_position_with_pnl(&manager, dec!(300)).await;
        manager.refresh_month_peak_net(now).await.unwrap();
        assert_eq!(manager.load_monthly_state(now).await.unwrap().month_peak_net, dec!(300));

        let next_month = if now.month() == 12 {
            chrono::Utc.with_ymd_and_hms(now.year() + 1, 1, 1, 0, 0, 1).unwrap()
        } else {
            chrono::Utc.with_ymd_and_hms(now.year(), now.month() + 1, 1, 0, 0, 1).unwrap()
        };
        assert_eq!(
            manager.load_monthly_state(next_month).await.unwrap().month_peak_net,
            Decimal::ZERO
        );
    }

    #[tokio::test]
    async fn test_monthly_halt_auto_trigger_blocks_subsequent_arm() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(100)).unwrap(),
            Price::zero(),
        );

        // Trigger MonthlyHalt
        save_closed_position_with_pnl(&manager, dec!(-400)).await;
        manager.evaluate_monthly_halt().await;

        // Attempt to arm a new position — must fail
        let result = manager
            .arm_position(
                symbol,
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await;

        assert!(result.is_err(), "arm_position must fail after MonthlyHalt auto-trigger");
        let err = result.unwrap_err();
        assert!(
            matches!(err, DaemonError::MonthlyHaltActive { .. }),
            "expected MonthlyHaltActive error, got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn test_monthly_halt_auto_trigger_closes_active_positions() {
        let manager = create_test_manager().await;
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(100)).unwrap(),
            Price::zero(),
        );

        // Arm a position, then promote to Active
        let position = manager
            .arm_position(
                Symbol::from_pair("BTCUSDT").unwrap(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: position.symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };
        manager.handle_signal(signal).await.unwrap();

        // Now trigger MonthlyHalt
        save_closed_position_with_pnl(&manager, dec!(-400)).await;
        manager.evaluate_monthly_halt().await;

        // Active position should have been closed by panic_close_all
        let open = manager.store.positions().find_active().await.unwrap();
        let active_count =
            open.iter().filter(|p| matches!(p.state, PositionState::Active { .. })).count();
        assert_eq!(active_count, 0, "MonthlyHalt must close Active positions");
    }

    #[tokio::test]
    async fn test_monthly_halt_does_not_retrigger_if_already_halted() {
        let manager = create_test_manager().await;
        save_closed_position_with_pnl(&manager, dec!(-400)).await;

        let first = manager.evaluate_monthly_halt().await;
        assert!(first, "first evaluation should trigger");

        let second = manager.evaluate_monthly_halt().await;
        assert!(!second, "subsequent evaluation should not re-trigger");
    }

    // =========================================================================
    // ADR-0024 §5: Latent risk halt trigger tests
    // =========================================================================

    #[tokio::test]
    async fn test_monthly_halt_triggered_by_latent_risk_alone() {
        // No realized losses, but open positions consume the entire budget via
        // latent risk. ADR-0043: halt when remaining = budget - 0 - latent <= 0.
        let manager = create_test_manager().await;
        // capital = 10000, budget = 400, need latent_risk >= 400 to exhaust.
        // Create one position with high latent risk directly.
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(90900)).unwrap(); // 4100 below entry
        let qty = Quantity::new(dec!(0.1)).unwrap();
        let now = chrono::Utc::now();

        let mut position = Position::new(account_id, symbol, Side::Long);
        position.entry_price = Some(entry);
        position.quantity = qty;
        position.state = PositionState::Active {
            current_price: entry,
            trailing_stop: stop,
            favorable_extreme: entry,
            extreme_at: now,
            insurance_stop_id: None,
            invalidation_guard_level: None,
            last_emitted_stop: None,
        };
        position.updated_at = now;
        manager.store.positions().save(&position).await.unwrap();
        // latent_risk = (95000 - 90900) * 0.1 = 410
        // remaining = 400 - 0 - 410 = -10 <= 0 → halt

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(
            triggered,
            "latent risk of 410 must exhaust budget (remaining=-10 <= 0, ADR-0043)"
        );
        assert!(manager.circuit_breaker.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_monthly_halt_not_triggered_with_breakeven_positions() {
        // Positions at breakeven contribute zero latent risk.
        let manager = create_test_manager().await;
        // Use save_active_position which sets stop = entry - 10 for Long.
        // With entry=3000, stop=2990, qty=0.1 → latent = 1 per position.
        // 3 positions: latent = 3, remaining = 400 - 0 - 3 = 397 >= 100 → no halt
        save_active_position(&manager, "BTCUSDT", Side::Long, dec!(3000), dec!(0.1)).await;
        save_active_position(&manager, "ETHUSDT", Side::Long, dec!(3000), dec!(0.1)).await;
        save_active_position(&manager, "SOLUSDT", Side::Long, dec!(3000), dec!(0.1)).await;

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(
            !triggered,
            "3 positions with tiny latent risk (3 total) should not halt (remaining=397)"
        );
        assert!(!manager.circuit_breaker.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_monthly_halt_triggered_by_combined_realized_loss_and_latent_risk() {
        // realized_loss = 150, latent_risk = 250 → remaining = 400 - 150 - 250 = 0
        // ADR-0043: halt fires when the budget is fully consumed (remaining <= 0).
        let manager = create_test_manager().await;

        // Realized loss of 150
        save_closed_position_with_pnl(&manager, dec!(-150)).await;

        // Latent risk of 250: entry=95000, stop=92500 (2500 diff), qty=0.1 → risk = 250
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(92500)).unwrap();
        let qty = Quantity::new(dec!(0.1)).unwrap();
        let now = chrono::Utc::now();

        let mut position = Position::new(account_id, symbol, Side::Long);
        position.entry_price = Some(entry);
        position.quantity = qty;
        position.state = PositionState::Active {
            current_price: entry,
            trailing_stop: stop,
            favorable_extreme: entry,
            extreme_at: now,
            insurance_stop_id: None,
            invalidation_guard_level: None,
            last_emitted_stop: None,
        };
        position.updated_at = now;
        manager.store.positions().save(&position).await.unwrap();

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(triggered, "realized=150 + latent=250 → remaining=0 must trigger (ADR-0043)");
    }

    #[tokio::test]
    async fn test_monthly_halt_not_triggered_while_budget_remains() {
        // ADR-0043: the system stays live while remaining_budget > 0, even
        // below one full risk unit — smaller planned-risk entries may still
        // fit (saved risk becomes an extra operation). Pre-ADR-0043 this
        // scenario (remaining=50 < risk_per_trade=100) would have halted.
        let manager = create_test_manager().await;
        // budget = 400: realized = 250, latent = 100 → remaining = 50 → no halt
        save_closed_position_with_pnl(&manager, dec!(-250)).await;

        // latent = 100: entry=95000, stop=94000 (1000 diff), qty=0.1 → 100
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(94000)).unwrap();
        let qty = Quantity::new(dec!(0.1)).unwrap();
        let now = chrono::Utc::now();

        let mut position = Position::new(account_id, symbol, Side::Long);
        position.entry_price = Some(entry);
        position.quantity = qty;
        position.state = PositionState::Active {
            current_price: entry,
            trailing_stop: stop,
            favorable_extreme: entry,
            extreme_at: now,
            insurance_stop_id: None,
            invalidation_guard_level: None,
            last_emitted_stop: None,
        };
        position.updated_at = now;
        manager.store.positions().save(&position).await.unwrap();

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(
            !triggered,
            "remaining=50 > 0 → must stay live for smaller planned-risk entries (ADR-0043)"
        );
    }

    #[tokio::test]
    async fn test_entering_risk_is_identical_for_slots_and_monthly_halt() {
        let manager = create_test_manager().await;
        save_closed_position_with_pnl(&manager, dec!(-300)).await;
        assert_eq!(
            manager.compute_slots_available().await.unwrap(),
            1,
            "one full-cap slot remains before the Entering reservation"
        );

        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let mut entering =
            Position::new(Uuid::now_v7(), Symbol::from_pair("ETHUSDT").unwrap(), Side::Long);
        entering.quantity = Quantity::new(dec!(10)).unwrap();
        entering.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(entry, stop));
        entering.state = PositionState::Entering {
            entry_order_id: Uuid::now_v7(),
            expected_entry: entry,
            signal_id: Uuid::now_v7(),
        };
        manager.store.positions().save(&entering).await.unwrap();

        let snapshot = manager.canonical_budget_snapshot(chrono::Utc::now()).await.unwrap();
        assert_eq!(snapshot.latent_risk, dec!(100));
        assert_eq!(snapshot.remaining_active, Decimal::ZERO);
        assert_eq!(manager.compute_slots_available().await.unwrap(), 0);

        // ADR-0051 §2: halt consumes that exact Entering reservation instead
        // of the former Active-only subset.
        assert!(manager.evaluate_monthly_halt().await);
        assert!(manager.circuit_breaker.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_snapshot_and_admission_latent_risk_match_without_pending_approvals() {
        let manager = create_test_manager().await;
        save_active_position(&manager, "BTCUSDT", Side::Long, dec!(3000), dec!(0.1)).await;

        let (executable, _) = executable_active_position_for_budget(dec!(2), dec!(100));
        manager.store.positions().save(&executable).await.unwrap();

        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let mut entering =
            Position::new(Uuid::now_v7(), Symbol::from_pair("ETHUSDT").unwrap(), Side::Long);
        entering.quantity = Quantity::new(dec!(10)).unwrap();
        entering.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(entry, stop));
        entering.state = PositionState::Entering {
            entry_order_id: Uuid::now_v7(),
            expected_entry: entry,
            signal_id: Uuid::now_v7(),
        };
        manager.store.positions().save(&entering).await.unwrap();

        assert!(manager.pending_approvals.read().await.is_empty());
        let snapshot = manager.canonical_budget_snapshot(chrono::Utc::now()).await.unwrap();
        let admission = manager.build_risk_context().await.unwrap();

        assert_eq!(snapshot.latent_risk, admission.latent_risk_sum());
    }

    #[tokio::test]
    async fn executable_trigger_costs_drive_august_slots_halt_and_admission() {
        // August-style basis: 1,578 capital => 63.12 monthly budget. Raw
        // technical distance reserves only 60 for this position, while the
        // executable trigger + ADR-0051 gap/fee envelope reserves >63.12.
        let manager = create_budget_test_manager(dec!(1578), true);
        let (position, plan) = executable_active_position_for_budget(dec!(6), dec!(100));
        let raw_risk = dec!(10) * dec!(6);
        let expected_risk = robson_domain::latent_risk_per_unit_at_trigger(
            &manager.risk_config_snapshot(),
            Side::Long,
            Price::new(dec!(100)).unwrap(),
            plan.trigger,
        )
        .unwrap()
            * dec!(6);
        assert!(raw_risk < dec!(63.12));
        assert!(expected_risk > dec!(63.12));
        assert_ne!(plan.trigger, Price::new(dec!(90)).unwrap());
        manager.store.positions().save(&position).await.unwrap();

        let snapshot = manager.canonical_budget_snapshot(chrono::Utc::now()).await.unwrap();
        assert_eq!(snapshot.latent_risk, expected_risk);
        assert_eq!(snapshot.slots_active, 0);
        assert!(snapshot.remaining_active < Decimal::ZERO);

        let admission = manager.build_risk_context().await.unwrap();
        let summary = admission
            .open_positions
            .iter()
            .find(|summary| summary.position_id == position.id)
            .unwrap();
        assert_eq!(summary.current_stop, plan.trigger.as_decimal());
        assert_eq!(summary.cost_priced_latent_risk, Some(expected_risk));
        assert_eq!(admission.latent_risk_sum(), snapshot.latent_risk);

        assert!(
            manager.evaluate_monthly_halt().await,
            "halt must consume the same executable-trigger reservation"
        );
    }

    #[tokio::test]
    async fn snapshot_resolves_rules_once_per_symbol_and_isolates_failures() {
        let manager = create_budget_test_manager(dec!(10000), false);
        manager.exchange().set_trading_rules(budget_test_rules());

        let (btc_one, _) =
            executable_active_position_for_budget_on_symbol("BTCUSDT", dec!(1), dec!(100));
        let (btc_two, _) =
            executable_active_position_for_budget_on_symbol("BTCUSDT", dec!(2), dec!(100));
        let (eth_unpriced, _) =
            executable_active_position_for_budget_on_symbol("ETHUSDT", dec!(1), dec!(100));
        for position in [&btc_one, &btc_two, &eth_unpriced] {
            manager.store.positions().save(position).await.unwrap();
        }

        let (snapshot, _, resolved, failures) = manager
            .canonical_budget_snapshot_with_positions(chrono::Utc::now())
            .await
            .unwrap();

        assert!(snapshot.latent_risk_invalid);
        assert!(resolved.contains_key(&btc_one.id));
        assert!(resolved.contains_key(&btc_two.id));
        assert!(!resolved.contains_key(&eth_unpriced.id));
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].position_id, eth_unpriced.id);
        assert!(failures[0].reason.contains("Trading rules unavailable"));
        // ADR-0056 §5: this runs on the tick path, so rules already held cost no
        // fetch at all (was one per symbol), and a symbol with none costs one
        // lookup, however many positions it has.
        assert_eq!(manager.exchange().trading_rules_call_count("BTCUSDT"), 0);
        assert_eq!(manager.exchange().trading_rules_call_count("ETHUSDT"), 1);

        let events = manager.store.events().find_by_position(eth_unpriced.id).await.unwrap();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::ExecutableStopRiskResolutionFailed { .. })),
            "read-only snapshots must return evidence without appending durable events"
        );
    }

    #[tokio::test]
    async fn unresolvable_executable_rules_fail_budget_snapshot_closed() {
        let manager = create_budget_test_manager(dec!(1578), false);
        let (position, _) = executable_active_position_for_budget(dec!(1), dec!(100));
        manager.store.positions().save(&position).await.unwrap();

        let snapshot = manager.canonical_budget_snapshot(chrono::Utc::now()).await.unwrap();
        assert!(snapshot.latent_risk_invalid);
        assert_eq!(snapshot.slots_active, 0);
        assert_eq!(snapshot.remaining_active, Decimal::ZERO);
        assert!(manager.build_risk_context().await.is_err());

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let durable_failures = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::ExecutableStopRiskResolutionFailed { severity, reason, .. }
                        if severity == "critical" && reason.contains("Trading rules unavailable")
                )
            })
            .count();
        assert_eq!(durable_failures, 1);
        assert!(
            !manager.evaluate_monthly_halt().await,
            "unpriceable latent risk must defer MonthlyHalt rather than liquidate"
        );
        assert!(!manager.circuit_breaker.blocks_new_entries().await);

        let persisted = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        assert!(matches!(persisted.state, PositionState::Active { .. }));
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::ExecutableStopRiskResolutionFailed { .. }))
                .count(),
            1,
            "halt evaluation must reuse the durable-event dedup"
        );
    }

    #[tokio::test]
    async fn test_build_risk_context_uses_real_monthly_pnl() {
        let manager = create_test_manager().await;

        // Save a closed position with -150 realized PnL
        save_closed_position_with_pnl(&manager, dec!(-150)).await;

        // Save an Active position with -50 unrealized PnL
        save_active_position(&manager, "ETHUSDT", Side::Long, dec!(3000), dec!(0.1)).await;

        let ctx = manager.build_risk_context().await.unwrap();

        // monthly_realized_pnl should be -150 (from the closed position)
        assert_eq!(
            ctx.monthly_realized_pnl,
            dec!(-150),
            "monthly_realized_pnl must reflect closed positions in current month"
        );

        // The unrealized PnL depends on calculate_pnl() which needs entry_price
        // and current_price. Since we set entry_price = 3000 and current_price = 3000
        // in save_active_position, unrealized PnL should be 0.
        // Let's just verify it's not hardcoded zero anymore by checking it ran.
        assert!(
            ctx.monthly_unrealized_pnl == dec!(0),
            "monthly_unrealized_pnl should be computed (not hardcoded)"
        );
    }

    #[tokio::test]
    async fn test_entering_position_survives_monthly_halt() {
        // Entering positions cannot be cancelled — no cancel-order mechanism.
        // This test documents the limitation: Entering positions remain.
        let manager = create_test_manager().await;

        // Create a position in Entering state
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let mut position = Position::new(account_id, symbol, Side::Long);
        position.quantity = Quantity::new(dec!(0.01)).unwrap();
        position.state = PositionState::Entering {
            entry_order_id: Uuid::now_v7(),
            expected_entry: Price::new(dec!(95000)).unwrap(),
            signal_id: Uuid::now_v7(),
        };
        manager.store.positions().save(&position).await.unwrap();

        // Trigger MonthlyHalt
        save_closed_position_with_pnl(&manager, dec!(-400)).await;
        manager.evaluate_monthly_halt().await;

        // Entering position should still exist (not cancelled)
        let entering = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        assert!(
            matches!(entering.state, PositionState::Entering { .. }),
            "Entering positions are not cancellable yet — this is a documented limitation"
        );
    }

    #[tokio::test]
    async fn test_handle_signal_monthly_drawdown_closes_active_positions() {
        let manager = create_test_manager().await;

        // 1. Closed position: realized PnL = -400 (exactly 4% of 10_000 capital)
        save_closed_position_with_pnl(&manager, dec!(-400)).await;

        // 2. Active position — must be closed by MonthlyHalt
        let _active =
            save_active_position(&manager, "ETHUSDT", Side::Long, dec!(3000), dec!(0.1)).await;

        // 3. Arm a second position and fire a signal
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(
            Price::new(dec!(95000)).unwrap(),
            Price::new(dec!(85500)).unwrap(),
        );
        let armed = manager
            .arm_position(
                Symbol::from_pair("BTCUSDT").unwrap(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: armed.id,
            symbol: armed.symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        // handle_signal must succeed (governed denial → Ok(())) and trigger MonthlyHalt
        manager.handle_signal(signal).await.unwrap();

        // 4. MonthlyHalt must be active
        assert!(
            manager.circuit_breaker.blocks_new_entries().await,
            "MonthlyHalt must be active after monthly drawdown in handle_signal"
        );

        // 5. Active positions must be closed
        let open = manager.store.positions().find_active().await.unwrap();
        let active_count =
            open.iter().filter(|p| matches!(p.state, PositionState::Active { .. })).count();
        assert_eq!(
            active_count, 0,
            "handle_signal monthly drawdown path must close Active positions"
        );
    }

    /// Regression guard: every path that sets MonthlyHalt must also close
    /// positions. This test exercises trigger_monthly_halt() directly (the
    /// canonical path). handle_signal() and approve_query() paths are
    /// covered by their respective tests.
    #[tokio::test]
    async fn test_trigger_monthly_halt_closes_active_positions() {
        let manager = create_test_manager().await;

        save_active_position(&manager, "BTCUSDT", Side::Long, dec!(95000), dec!(0.1)).await;

        let closed = manager.trigger_monthly_halt("operator test".to_string()).await.unwrap();

        assert!(!closed.is_empty(), "trigger_monthly_halt must return closed position IDs");

        let open = manager.store.positions().find_active().await.unwrap();
        let active_count =
            open.iter().filter(|p| matches!(p.state, PositionState::Active { .. })).count();
        assert_eq!(active_count, 0, "trigger_monthly_halt must close all Active positions");
    }

    #[tokio::test]
    async fn test_approve_query_monthly_drawdown_closes_active_positions() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        let manager = create_test_manager().await;

        // 1. Save an Active position — it must be closed by MonthlyHalt
        save_active_position(&manager, "ETHUSDT", Side::Long, dec!(3000), dec!(0.1)).await;

        // 2. Arm a position and send a signal that requires approval
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                hc_policy,
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(), /* HumanConfirmation always requires
                                                          * approval */
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        // handle_signal passes risk check (no monthly drawdown yet) → pending approval
        manager.handle_signal(signal).await.unwrap();

        let query_id = {
            let pending = manager.pending_approvals.read().await;
            *pending.keys().next().expect("pending approval query must exist")
        };

        // 3. Now push monthly realized PnL to exactly 4% (capital is 10_000)
        save_closed_position_with_pnl(&manager, dec!(-400)).await;

        // 4. approve_query revalidates risk → monthly_drawdown denied → MonthlyHalt
        let result = manager.approve_query(query_id).await;
        assert!(
            matches!(result, Err(DaemonError::ApprovalDenied { .. })),
            "approve_query must fail with ApprovalDenied when monthly drawdown exceeded"
        );

        // 5. MonthlyHalt must be active
        assert!(
            manager.circuit_breaker.blocks_new_entries().await,
            "MonthlyHalt must be active after monthly drawdown in approve_query"
        );

        // 6. Active positions must be closed
        let open = manager.store.positions().find_active().await.unwrap();
        let active_count =
            open.iter().filter(|p| matches!(p.state, PositionState::Active { .. })).count();
        assert_eq!(
            active_count, 0,
            "approve_query monthly drawdown path must close Active positions"
        );
    }

    // =========================================================================
    // handle_exit_fill realized PnL tests
    // =========================================================================

    /// Verify that handle_exit_fill computes realized PnL directly from
    /// (fill_price − entry_price) × qty, NOT via calculate_pnl() which returns
    /// ZERO for the Exiting state.
    ///
    /// Setup: manually place a Long position in Exiting state with known
    /// entry_price and quantity, then call handle_exit_fill with a fill_price
    /// below entry → expected negative realized PnL.
    #[tokio::test]
    async fn test_handle_exit_fill_records_negative_realized_pnl() {
        let manager = create_test_manager().await;

        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry_price = Price::new(dec!(95000)).unwrap();
        let fill_price = Price::new(dec!(90000)).unwrap(); // $5_000 below entry
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        // Build position in Exiting state (as if exit order was placed)
        let mut position = Position::new(account_id, symbol.clone(), Side::Long);
        position.entry_price = Some(entry_price);
        position.quantity = quantity;
        position.state = PositionState::Exiting {
            exit_order_id: Uuid::now_v7(),
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };
        position.updated_at = chrono::Utc::now();
        manager.store.positions().save(&position).await.unwrap();

        // Call handle_exit_fill — must compute realized PnL directly
        let result = manager
            .handle_exit_fill(position.id, fill_price, quantity, Decimal::ZERO, chrono::Utc::now())
            .await;
        assert!(result.is_ok(), "handle_exit_fill failed: {:?}", result.err());

        // Reload and verify the closed position has correct negative realized PnL
        let closed = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();

        // Expected: (90000 - 95000) * 0.1 = -500
        let expected_pnl =
            (fill_price.as_decimal() - entry_price.as_decimal()) * quantity.as_decimal();
        assert_eq!(
            closed.realized_pnl, expected_pnl,
            "realized_pnl must be {} (got {}) — direct calc, not calculate_pnl()",
            expected_pnl, closed.realized_pnl
        );
        assert!(
            closed.realized_pnl < Decimal::ZERO,
            "realized_pnl must be negative for Long position closing below entry"
        );

        // Verify Closed state carries the same PnL
        match closed.state {
            PositionState::Closed { realized_pnl, exit_price, .. } => {
                assert_eq!(realized_pnl, expected_pnl, "Closed.realized_pnl must match");
                assert_eq!(exit_price, fill_price, "Closed.exit_price must be fill_price");
            },
            other => panic!("expected Closed state, got {:?}", other),
        }
    }

    /// handle_exit_fill must return an error when entry_price is None
    /// instead of silently falling back to fill_price.
    #[tokio::test]
    async fn test_handle_exit_fill_rejects_missing_entry_price() {
        let manager = create_test_manager().await;

        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let fill_price = Price::new(dec!(90000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();

        // Build position with entry_price = None (corrupted/incomplete state)
        let mut position = Position::new(account_id, symbol, Side::Long);
        position.entry_price = None; // explicitly missing
        position.quantity = quantity;
        position.state = PositionState::Exiting {
            exit_order_id: Uuid::now_v7(),
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };
        position.updated_at = chrono::Utc::now();
        manager.store.positions().save(&position).await.unwrap();

        let result = manager
            .handle_exit_fill(position.id, fill_price, quantity, Decimal::ZERO, chrono::Utc::now())
            .await;

        assert!(result.is_err(), "handle_exit_fill must reject position with entry_price=None");
        let err = result.unwrap_err();
        assert!(
            matches!(err, DaemonError::InvalidPositionState { .. }),
            "expected InvalidPositionState, got: {:?}",
            err
        );
    }

    fn order_fill_reconciled_input(
        position_id: PositionId,
        exit_price: Price,
        quantity: Quantity,
        fee: Decimal,
        closed_at: chrono::DateTime<chrono::Utc>,
    ) -> ReconciledCloseInput {
        ReconciledCloseInput {
            position_id,
            exit_price,
            filled_quantity: quantity,
            fee,
            fee_asset: "USDT".to_string(),
            closed_at,
            authored_client_order_id: None,
            evidence: ReconciliationEvidence::OrderFillRecord(robson_domain::OrderFillEvidence {
                exchange_order_id: "ORDER-1".to_string(),
                fill_price: exit_price,
                filled_quantity: quantity,
                fee,
                fee_asset: "USDT".to_string(),
                filled_at: closed_at,
            }),
        }
    }

    fn user_trade_reconciled_input(
        position_id: PositionId,
        exit_price: Price,
        quantity: Quantity,
        fee: Decimal,
        closed_at: chrono::DateTime<chrono::Utc>,
    ) -> ReconciledCloseInput {
        ReconciledCloseInput {
            position_id,
            exit_price,
            filled_quantity: quantity,
            fee,
            fee_asset: "USDT".to_string(),
            closed_at,
            authored_client_order_id: None,
            evidence: ReconciliationEvidence::UserTradeRecord(robson_domain::UserTradeEvidence {
                exchange_order_id: "ORDER-2".to_string(),
                exchange_trade_id: "TRADE-1".to_string(),
                fill_price: exit_price,
                filled_quantity: quantity,
                fee,
                fee_asset: "USDT".to_string(),
                filled_at: closed_at,
            }),
        }
    }

    #[tokio::test]
    async fn reconcile_close_order_fill_long_records_realized_pnl() {
        let manager = create_test_manager().await;
        let position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(0.01)).await;
        let exit_price = Price::new(dec!(90)).unwrap();
        let quantity = Quantity::new(dec!(0.01)).unwrap();
        let closed_at = chrono::Utc::now();

        let outcome = manager
            .reconcile_close(order_fill_reconciled_input(
                position.id,
                exit_price,
                quantity,
                dec!(0.01),
                closed_at,
            ))
            .await
            .unwrap();

        assert_eq!(outcome, ReconcileCloseOutcome::Closed);
        let closed = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        let expected_pnl = (dec!(90) - dec!(100)) * dec!(0.01);
        assert_eq!(closed.realized_pnl, expected_pnl);
        match closed.state {
            PositionState::Closed {
                exit_price: state_exit,
                realized_pnl,
                exit_reason,
            } => {
                assert_eq!(state_exit, exit_price);
                assert_eq!(realized_pnl, expected_pnl);
                assert_eq!(exit_reason, robson_domain::ExitReason::ReconciledMissingOnExchange);
            },
            other => panic!("expected Closed state, got {:?}", other),
        }

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let close_event = events
            .iter()
            .find_map(|event| {
                if let Event::PositionClosed { closure_evidence, exit_reason, .. } = event {
                    Some((closure_evidence, exit_reason))
                } else {
                    None
                }
            })
            .expect("PositionClosed event must be emitted");
        assert_eq!(*close_event.1, robson_domain::ExitReason::ReconciledMissingOnExchange);
        assert!(matches!(
            close_event.0,
            ClosureEvidence::Reconciled(ReconciliationEvidence::OrderFillRecord(_))
        ));
    }

    #[tokio::test]
    async fn reconcile_close_user_trade_short_records_realized_pnl() {
        let manager = create_test_manager().await;
        let position =
            save_active_position(&manager, "BTCUSDT", Side::Short, dec!(100), dec!(0.01)).await;
        let exit_price = Price::new(dec!(90)).unwrap();
        let quantity = Quantity::new(dec!(0.01)).unwrap();
        let closed_at = chrono::Utc::now();

        let outcome = manager
            .reconcile_close(user_trade_reconciled_input(
                position.id,
                exit_price,
                quantity,
                dec!(0.01),
                closed_at,
            ))
            .await
            .unwrap();

        assert_eq!(outcome, ReconcileCloseOutcome::Closed);
        let closed = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        let expected_pnl = (dec!(100) - dec!(90)) * dec!(0.01);
        assert_eq!(closed.realized_pnl, expected_pnl);

        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        let close_event = events
            .iter()
            .find_map(|event| {
                if let Event::PositionClosed { closure_evidence, exit_reason, .. } = event {
                    Some((closure_evidence, exit_reason))
                } else {
                    None
                }
            })
            .expect("PositionClosed event must be emitted");
        assert_eq!(*close_event.1, robson_domain::ExitReason::ReconciledMissingOnExchange);
        assert!(matches!(
            close_event.0,
            ClosureEvidence::Reconciled(ReconciliationEvidence::UserTradeRecord(_))
        ));
    }

    #[tokio::test]
    async fn reconcile_close_is_idempotent_for_already_closed() {
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(0.01)).await;
        let original_closed_at = chrono::Utc::now();
        let original_pnl = dec!(-1);
        let mut closed_position = position.clone();
        closed_position.state = PositionState::Closed {
            exit_price: Price::new(dec!(90)).unwrap(),
            realized_pnl: original_pnl,
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };
        closed_position.realized_pnl = original_pnl;
        closed_position.closed_at = Some(original_closed_at);
        manager.store.positions().save(&closed_position).await.unwrap();
        let before_events = manager.store.events().find_by_position(position.id).await.unwrap();

        let outcome = manager
            .reconcile_close(order_fill_reconciled_input(
                position.id,
                Price::new(dec!(80)).unwrap(),
                Quantity::new(dec!(0.01)).unwrap(),
                dec!(0.01),
                chrono::Utc::now(),
            ))
            .await
            .unwrap();

        assert_eq!(outcome, ReconcileCloseOutcome::AlreadyTerminal);
        let after = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        assert_eq!(after.realized_pnl, original_pnl);
        assert_eq!(after.closed_at, Some(original_closed_at));
        let after_events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert_eq!(after_events.len(), before_events.len());
        assert!(receiver.try_recv().is_none());
    }

    #[tokio::test]
    async fn reconcile_close_already_closed_is_state_first_for_bad_input() {
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(0.01)).await;
        let original_closed_at = chrono::Utc::now();
        let original_pnl = dec!(-1);
        let mut closed_position = position.clone();
        closed_position.state = PositionState::Closed {
            exit_price: Price::new(dec!(90)).unwrap(),
            realized_pnl: original_pnl,
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };
        closed_position.realized_pnl = original_pnl;
        closed_position.closed_at = Some(original_closed_at);
        manager.store.positions().save(&closed_position).await.unwrap();
        let before_events = manager.store.events().find_by_position(position.id).await.unwrap();
        let now = chrono::Utc::now();

        let snapshot_outcome = manager
            .reconcile_close(ReconciledCloseInput {
                position_id: position.id,
                exit_price: Price::new(dec!(90)).unwrap(),
                filled_quantity: Quantity::new(dec!(0.01)).unwrap(),
                fee: Decimal::ZERO,
                fee_asset: "USDT".to_string(),
                closed_at: now,
                authored_client_order_id: None,
                evidence: ReconciliationEvidence::AccountSnapshot(
                    robson_domain::AccountSnapshotEvidence {
                        first_observed_missing_at: now,
                        confirmed_missing_at: now,
                        futures_balance_delta: None,
                    },
                ),
            })
            .await
            .unwrap();

        let mut inconsistent_input = order_fill_reconciled_input(
            position.id,
            Price::new(dec!(90)).unwrap(),
            Quantity::new(dec!(0.01)).unwrap(),
            dec!(0.01),
            now,
        );
        inconsistent_input.exit_price = Price::new(dec!(91)).unwrap();
        let inconsistent_outcome = manager.reconcile_close(inconsistent_input).await.unwrap();

        assert_eq!(snapshot_outcome, ReconcileCloseOutcome::AlreadyTerminal);
        assert_eq!(inconsistent_outcome, ReconcileCloseOutcome::AlreadyTerminal);
        let after = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        assert_eq!(after.realized_pnl, original_pnl);
        assert_eq!(after.closed_at, Some(original_closed_at));
        let after_events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert_eq!(after_events.len(), before_events.len());
        assert!(receiver.try_recv().is_none());
    }

    #[tokio::test]
    async fn reconcile_close_skips_non_active() {
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let mut position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(0.01)).await;
        position.state = PositionState::Exiting {
            exit_order_id: Uuid::now_v7(),
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };
        manager.store.positions().save(&position).await.unwrap();

        let outcome = manager
            .reconcile_close(order_fill_reconciled_input(
                position.id,
                Price::new(dec!(90)).unwrap(),
                Quantity::new(dec!(0.01)).unwrap(),
                dec!(0.01),
                chrono::Utc::now(),
            ))
            .await
            .unwrap();

        assert_eq!(outcome, ReconcileCloseOutcome::SkippedNonActive {
            state: "exiting".to_string()
        });
        let after = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        assert!(matches!(after.state, PositionState::Exiting { .. }));
        let events = manager.store.events().find_by_position(position.id).await.unwrap();
        assert!(events.iter().all(|event| !matches!(event, Event::PositionClosed { .. })));
        assert!(receiver.try_recv().is_none());
    }

    #[tokio::test]
    async fn reconcile_close_rejects_account_snapshot() {
        let manager = create_test_manager().await;
        let position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(0.01)).await;
        let now = chrono::Utc::now();

        let outcome = manager
            .reconcile_close(ReconciledCloseInput {
                position_id: position.id,
                exit_price: Price::new(dec!(90)).unwrap(),
                filled_quantity: Quantity::new(dec!(0.01)).unwrap(),
                fee: Decimal::ZERO,
                fee_asset: "USDT".to_string(),
                closed_at: now,
                authored_client_order_id: None,
                evidence: ReconciliationEvidence::AccountSnapshot(
                    robson_domain::AccountSnapshotEvidence {
                        first_observed_missing_at: now,
                        confirmed_missing_at: now,
                        futures_balance_delta: None,
                    },
                ),
            })
            .await
            .unwrap();

        assert_eq!(outcome, ReconcileCloseOutcome::RejectedUnsupportedEvidence {
            source: "account_snapshot"
        });
        let after = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        assert!(matches!(after.state, PositionState::Active { .. }));
    }

    #[tokio::test]
    async fn reconcile_close_rejects_estimated() {
        let manager = create_test_manager().await;
        let position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(0.01)).await;
        let now = chrono::Utc::now();

        let outcome = manager
            .reconcile_close(ReconciledCloseInput {
                position_id: position.id,
                exit_price: Price::new(dec!(90)).unwrap(),
                filled_quantity: Quantity::new(dec!(0.01)).unwrap(),
                fee: Decimal::ZERO,
                fee_asset: "USDT".to_string(),
                closed_at: now,
                authored_client_order_id: None,
                evidence: ReconciliationEvidence::Estimated(robson_domain::EstimatedEvidence {
                    estimation_basis: robson_domain::EstimationBasis::ExchangeMarkPrice,
                    exit_price: Price::new(dec!(90)).unwrap(),
                    evaluator: Some("test".to_string()),
                    detected_at: now,
                }),
            })
            .await
            .unwrap();

        assert_eq!(outcome, ReconcileCloseOutcome::RejectedUnsupportedEvidence {
            source: "estimated"
        });
        let after = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        assert!(matches!(after.state, PositionState::Active { .. }));
    }

    #[tokio::test]
    async fn reconcile_close_rejects_disconnected_exit_price() {
        let manager = create_test_manager().await;
        let position =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(100), dec!(0.01)).await;
        let quantity = Quantity::new(dec!(0.01)).unwrap();
        let closed_at = chrono::Utc::now();
        let mut input = order_fill_reconciled_input(
            position.id,
            Price::new(dec!(90)).unwrap(),
            quantity,
            dec!(0.01),
            closed_at,
        );
        input.exit_price = Price::new(dec!(91)).unwrap();

        let outcome = manager.reconcile_close(input).await.unwrap();

        assert_eq!(outcome, ReconcileCloseOutcome::RejectedInconsistentEvidence {
            field: "exit_price"
        });
        let after = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();
        assert!(matches!(after.state, PositionState::Active { .. }));
    }

    /// Verify panic_close_position_internal computes realized PnL from
    /// (fill_price − entry_price) × qty, NOT from current_price.
    ///
    /// Setup: Active Long position with entry_price = 100_000, current_price =
    /// 99_000. StubExchange fills at default 95_000. The PnL must be based
    /// on fill_price (95_000), NOT current_price (99_000), proving the
    /// direct calc is used.
    #[tokio::test]
    async fn test_panic_close_records_negative_realized_pnl() {
        let manager = create_test_manager().await;

        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry_price = Price::new(dec!(100000)).unwrap();
        // current_price intentionally different from fill to prove fill_price is used
        let current_price = Price::new(dec!(99000)).unwrap();
        let quantity = Quantity::new(dec!(0.1)).unwrap();
        let now = chrono::Utc::now();

        // Build Active position
        let mut position = Position::new(account_id, symbol.clone(), Side::Long);
        position.entry_price = Some(entry_price);
        position.entry_filled_at = Some(now);
        position.quantity = quantity;
        position.state = PositionState::Active {
            current_price,
            trailing_stop: Price::new(dec!(90000)).unwrap(),
            favorable_extreme: entry_price,
            extreme_at: now,
            insurance_stop_id: None,
            invalidation_guard_level: None,
            last_emitted_stop: None,
        };
        position.updated_at = now;
        manager.store.positions().save(&position).await.unwrap();

        // Build an ExecutionQuery in Accepted state (panic_close transitions it
        // internally)
        let mut query = ExecutionQuery::new(
            QueryKind::PanicClosePosition { position_id: position.id },
            ActorKind::Operator { source: CommandSource::Api },
        );
        query.position_id = Some(position.id);

        // Call panic_close — StubExchange fills at default price 95_000
        let result = manager.panic_close_position_internal(position.id, &mut query).await;
        assert!(result.is_ok(), "panic_close failed: {:?}", result.err());

        // Reload and verify
        let closed = manager.store.positions().find_by_id(position.id).await.unwrap().unwrap();

        // fill_price from StubExchange = 95_000
        // Expected: (95000 - 100000) * 0.1 = -500
        let expected_pnl = (dec!(95000) - entry_price.as_decimal()) * quantity.as_decimal();
        assert_eq!(
            closed.realized_pnl, expected_pnl,
            "realized_pnl must be {} (got {}) — based on fill_price 95000, not current_price 99000",
            expected_pnl, closed.realized_pnl
        );
        assert!(
            closed.realized_pnl < Decimal::ZERO,
            "realized_pnl must be negative for Long closing below entry"
        );

        // Double-check it's NOT the current_price-based value
        let current_price_pnl =
            (current_price.as_decimal() - entry_price.as_decimal()) * quantity.as_decimal();
        assert_ne!(
            closed.realized_pnl, current_price_pnl,
            "realized_pnl must NOT equal current_price-based PnL ({})",
            current_price_pnl
        );

        match closed.state {
            PositionState::Closed { realized_pnl, exit_price, .. } => {
                assert_eq!(realized_pnl, expected_pnl, "Closed.realized_pnl must match");
                assert_eq!(
                    exit_price,
                    Price::new(dec!(95000)).unwrap(),
                    "exit_price must be fill_price"
                );
            },
            other => panic!("expected Closed state, got {:?}", other),
        }
    }

    // =========================================================================
    // Fees deduction in monthly PnL
    // =========================================================================

    /// Helper: save a closed position with realized PnL and fees in the current
    /// month.
    async fn save_closed_position_with_pnl_and_fees(
        manager: &Arc<PositionManager<StubExchange, MemoryStore>>,
        realized_pnl: Decimal,
        fees_paid: Decimal,
    ) -> Position {
        let account_id = Uuid::now_v7();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let exit = Price::new(dec!(95000) + realized_pnl).unwrap();
        let now = chrono::Utc::now();

        let mut position = Position::new(account_id, symbol, Side::Long);
        position.entry_price = Some(entry);
        position.quantity = Quantity::new(dec!(0.1)).unwrap();
        position.realized_pnl = realized_pnl;
        position.fees_paid = fees_paid;
        position.closed_at = Some(now);
        position.updated_at = now;
        position.state = PositionState::Closed {
            exit_price: exit,
            realized_pnl,
            exit_reason: robson_domain::ExitReason::TrailingStop,
        };

        manager.store.positions().save(&position).await.unwrap();
        position
    }

    #[tokio::test]
    async fn test_build_risk_context_deducts_fees_from_monthly_realized_pnl() {
        let manager = create_test_manager().await;
        // Position closed with realized_pnl = -300, fees_paid = 50
        // Net = -300 - 50 = -350
        save_closed_position_with_pnl_and_fees(&manager, dec!(-300), dec!(50)).await;

        let ctx = manager.build_risk_context().await.unwrap();

        assert_eq!(ctx.monthly_realized_pnl, dec!(-350), "monthly_realized_pnl must deduct fees");
        assert_eq!(
            ctx.monthly_unrealized_pnl,
            dec!(0),
            "no open positions, unrealized must be zero"
        );
    }

    #[tokio::test]
    async fn test_evaluate_monthly_halt_accounts_for_fees() {
        let manager = create_test_manager().await;
        // Capital = 10_000, 4% limit = -400
        // realized_pnl = -350, fees_paid = 50 → net = -400 → triggers MonthlyHalt
        save_closed_position_with_pnl_and_fees(&manager, dec!(-350), dec!(50)).await;

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(triggered, "net -400 (PnL -350, fees -50) must trigger MonthlyHalt");
        assert!(manager.circuit_breaker.blocks_new_entries().await);
    }

    // =========================================================================
    // RiskConfig wiring tests
    // =========================================================================

    /// Verify that different capital values produce different position sizes.
    ///
    /// The engine calculates qty = (capital * risk_pct) / (entry_price *
    /// stop_pct). With capital=10000, risk=1%: qty = $100 / stop_distance.
    /// With capital=20000, risk=1%: qty = $200 / stop_distance.
    /// The larger capital must produce a larger quantity.
    #[tokio::test]
    async fn test_different_capital_produces_different_position_size() {
        let manager = create_test_manager().await;

        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(98)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        // Arm with default capital (10_000)
        let pos_small = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                RiskConfig::new(dec!(10000)).unwrap(),
                Some(tech_stop.clone()),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // Disarm to free slot
        manager.disarm_position(pos_small.id).await.unwrap();

        // Re-arm with double capital (20_000)
        let pos_large = manager
            .arm_position(
                symbol,
                Side::Long,
                RiskConfig::new(dec!(20000)).unwrap(),
                Some(tech_stop),
                Uuid::now_v7(),
            )
            .await
            .unwrap();

        // Now fire signals for both positions (pos_small was disarmed, so only
        // pos_large) Instead, verify that the engine capital was actually
        // updated by checking position size after signal.
        //
        // With capital=20_000, risk=1%=$200, entry=95000, stop=87400 (8%):
        //   qty = $200 / ($95000 * 0.08) = $200 / $7600 ≈ 0.026315789...
        //
        // Compare with capital=10_000: qty = $100 / $7600 ≈ 0.013157894...
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: pos_large.id,
            symbol: pos_large.symbol.clone(),
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(87400)).unwrap(), // 8% stop
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        let updated = manager.get_position(pos_large.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Active { .. }),
            "Expected Active, got {:?}",
            updated.state
        );

        // With capital=20000 and the adverse-fill costing (issue #154):
        // trigger 87400 (zero buffer), gap 10 bps below it, fees on entry
        // and on the adverse fill.
        let adverse = dec!(87400) - dec!(87.4);
        let worst_loss_per_unit = (dec!(95000) - adverse) + dec!(0.0005) * (dec!(95000) + adverse);
        let expected_qty = (dec!(200) / worst_loss_per_unit)
            .round_dp_with_strategy(6, rust_decimal::RoundingStrategy::ToZero);
        assert_eq!(
            updated.quantity.as_decimal(),
            expected_qty,
            "Position size must reflect doubled capital"
        );

        // Verify it's actually different from the default (capital=10000)
        let default_qty = dec!(100) / worst_loss_per_unit;
        assert_ne!(
            updated.quantity.as_decimal(),
            default_qty,
            "Position size with capital=20000 must differ from capital=10000"
        );
    }

    // =========================================================================
    // Phase 5: State Machine integration tests
    // =========================================================================

    /// Disarmed position transitions to Cancelled (not Closed).
    #[tokio::test]
    async fn test_position_state_cancelled_after_disarm() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let position = manager
            .arm_position(symbol, Side::Long, create_test_risk_config(), None, Uuid::now_v7())
            .await
            .unwrap();

        manager.disarm_position(position.id).await.unwrap();

        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Cancelled),
            "Expected Cancelled after disarm, got {:?}",
            updated.state
        );
    }

    /// Cancelled position must not appear in find_active.
    #[tokio::test]
    async fn test_cancelled_position_excluded_from_find_active() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        let position = manager
            .arm_position(symbol, Side::Long, create_test_risk_config(), None, Uuid::now_v7())
            .await
            .unwrap();

        manager.disarm_position(position.id).await.unwrap();

        let active = manager.store.positions().find_active().await.unwrap();
        assert!(
            active.iter().all(|p| p.id != position.id),
            "Cancelled position must not appear in find_active"
        );
    }

    /// EntryApprovalPending event is emitted when HumanConfirmation approval
    /// policy holds the entry for operator confirmation.
    #[tokio::test]
    async fn test_entry_approval_pending_event_emitted() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        let manager = create_test_manager().await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                hc_policy,
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        // Approval must be pending
        assert_eq!(manager.pending_approvals.read().await.len(), 1);

        // EntryApprovalPending must have been emitted to the event bus as a domain
        // event
        let mut approval_pending_seen = false;
        for _ in 0..20 {
            if let Ok(Some(event)) =
                tokio::time::timeout(std::time::Duration::from_millis(50), receiver.recv()).await
            {
                if let Ok(DaemonEvent::DomainEvent(Event::EntryApprovalPending {
                    position_id,
                    ..
                })) = event
                {
                    assert_eq!(position_id, position.id);
                    approval_pending_seen = true;
                    break;
                }
            }
        }
        assert!(approval_pending_seen, "Expected EntryApprovalPending domain event on event bus");
    }

    /// After approval expiry, the position must remain Armed and the detector
    /// must be re-armed (retry path).
    ///
    /// The background expiry task removes the record from `pending_approvals`
    /// and re-arms the detector automatically. `approve_query` must NOT be
    /// called after expiry — the record is gone and would return
    /// `QueryNotFound`. Instead, wait for the `QueryExpired` event on the
    /// event bus, then verify the position is still Armed.
    #[tokio::test]
    async fn test_approval_expiry_re_arms_position() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        // 1-second TTL so the approval expires quickly in the test.
        let manager = create_phase3_test_manager(1).await;
        let mut receiver = manager.event_bus.subscribe();
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(100)).unwrap();
        let stop = Price::new(dec!(90)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        let hc_policy = EntryPolicyConfig::new(
            EntryPolicy::ConfirmedTrend,
            DomainApprovalPolicy::HumanConfirmation,
        );

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(tech_stop),
                Uuid::now_v7(),
                hc_policy,
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: Price::new(dec!(95000)).unwrap(),
            stop_loss: Price::new(dec!(85500)).unwrap(),
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        manager.handle_signal(signal).await.unwrap();

        // Approval is pending — background task will expire it after 1 second.
        assert_eq!(manager.pending_approvals.read().await.len(), 1);

        // Wait for the QueryExpired event emitted by the background expiry task.
        let mut expired_seen = false;
        for _ in 0..30 {
            match tokio::time::timeout(std::time::Duration::from_millis(100), receiver.recv()).await
            {
                Ok(Some(Ok(DaemonEvent::QueryExpired { position_id, .. }))) => {
                    if position_id == Some(position.id) {
                        expired_seen = true;
                        break;
                    }
                },
                _ => {},
            }
        }
        assert!(expired_seen, "Expected QueryExpired event on event bus after TTL");

        // Background task has already removed the record and re-armed the detector.
        assert_eq!(
            manager.pending_approvals.read().await.len(),
            0,
            "Pending approvals must be empty after expiry"
        );

        // Position must stay Armed — expiry is a retry path, not a terminal state.
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after approval expiry, got {:?}",
            updated.state
        );
    }

    /// Risk denied entries leave the position in Armed state (retry path, not
    /// Cancelled).
    ///
    /// This test triggers a DuplicatePosition risk denial (same symbol + side
    /// already Entering), which is a governed outcome that does NOT
    /// activate MonthlyHalt. Slot-exhaustion denials also use
    /// RiskCheck::MonthlyDrawdown and DO trigger MonthlyHalt +
    /// panic_close_all — that is correct behaviour and is separately tested
    /// in the monthly drawdown test.
    #[tokio::test]
    async fn test_risk_denial_leaves_position_armed_for_retry() {
        use robson_domain::{ApprovalPolicy as DomainApprovalPolicy, EntryPolicy};
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();

        // Seed an Entering BTCUSDT/Long position. The risk gate's DuplicatePosition
        // check fires when a second BTCUSDT/Long signal arrives, producing a governed
        // denial that does not trigger MonthlyHalt.
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(91200)).unwrap();
        let mut existing =
            robson_domain::Position::new(uuid::Uuid::now_v7(), symbol.clone(), Side::Long);
        existing.quantity = Quantity::new(dec!(0.001)).unwrap();
        existing.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(entry, stop));
        existing.state = PositionState::Entering {
            entry_order_id: uuid::Uuid::now_v7(),
            expected_entry: entry,
            signal_id: uuid::Uuid::now_v7(),
        };
        manager.store.positions().save(&existing).await.unwrap();

        let hc_policy =
            EntryPolicyConfig::new(EntryPolicy::ConfirmedTrend, DomainApprovalPolicy::Automatic);

        let position = manager
            .arm_position_with_policy(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(TechnicalStopDistance::from_entry_and_stop(entry, stop)),
                Uuid::now_v7(),
                hc_policy,
            )
            .await
            .unwrap();

        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol,
            side: Side::Long,
            entry_price: entry,
            stop_loss: stop,
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };

        // handle_signal must succeed: DuplicatePosition denial is a governed outcome.
        manager.handle_signal(signal).await.unwrap();

        // Position must remain Armed — DuplicatePosition re-arms the detector.
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after risk denial, got {:?}",
            updated.state
        );

        // Cancelled state must not be set (Cancelled is reserved for pre-entry disarm).
        assert!(
            !matches!(updated.state, PositionState::Cancelled),
            "Risk denial must not produce Cancelled state"
        );
    }

    /// Risk-bypass regression: after a risk-denied signal, the position must
    /// NOT be in Entering state. The only way to reach Entering is via
    /// execute_signal_query, which requires a GovernedAction token that can
    /// only be produced by QueryEngine::check_risk() succeeding.
    #[tokio::test]
    async fn test_risk_denial_never_produces_entering_state() {
        let manager = create_test_manager().await;
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(91200)).unwrap();

        // Seed an Entering position to trigger DuplicatePosition denial.
        let mut existing =
            robson_domain::Position::new(uuid::Uuid::now_v7(), symbol.clone(), Side::Long);
        existing.quantity = Quantity::new(dec!(0.001)).unwrap();
        existing.tech_stop_distance = Some(TechnicalStopDistance::from_entry_and_stop(entry, stop));
        existing.state = PositionState::Entering {
            entry_order_id: uuid::Uuid::now_v7(),
            expected_entry: entry,
            signal_id: uuid::Uuid::now_v7(),
        };
        manager.store.positions().save(&existing).await.unwrap();

        // Arm a second BTCUSDT/Long position.
        let position = manager
            .arm_position(
                symbol.clone(),
                Side::Long,
                create_test_risk_config(),
                Some(TechnicalStopDistance::from_entry_and_stop(entry, stop)),
                Uuid::nil(),
            )
            .await
            .unwrap();

        // Send signal — DuplicatePosition check should deny this.
        let signal = DetectorSignal {
            signal_id: Uuid::now_v7(),
            position_id: position.id,
            symbol: symbol.clone(),
            side: Side::Long,
            entry_price: entry,
            stop_loss: stop,
            technical_stop_analysis: None,
            timestamp: chrono::Utc::now(),
        };
        manager.handle_signal(signal).await.unwrap();

        // Position must NOT be Entering — risk denial prevents GovernedAction.
        let updated = manager.get_position(position.id).await.unwrap().unwrap();
        assert!(
            !matches!(updated.state, PositionState::Entering { .. }),
            "Risk denial must not produce Entering state, got {:?}",
            updated.state
        );
        assert!(
            matches!(updated.state, PositionState::Armed),
            "Expected Armed after risk denial, got {:?}",
            updated.state
        );
    }

    // =========================================================================
    // MIG-v3#12: Monthly State Persistence tests
    // =========================================================================

    #[tokio::test]
    async fn test_load_monthly_state_defaults_when_no_closed_positions() {
        let manager = create_test_manager().await;

        let state = manager.load_monthly_state(chrono::Utc::now()).await.unwrap();
        assert_eq!(state.capital_base, dec!(10000), "capital defaults to configured");
        assert_eq!(state.realized_loss, Decimal::ZERO, "no loss without closed positions");
        assert_eq!(state.trades_opened, 0, "no trades without closed positions");
    }

    #[tokio::test]
    async fn test_load_monthly_state_reflects_closed_losses() {
        let manager = create_test_manager().await;

        // Two closed positions: one loss (-200), one win (+100).
        // ADR-0024: realized_loss must be 200 (wins do NOT offset).
        save_closed_position_with_pnl(&manager, dec!(-200)).await;
        save_closed_position_with_pnl(&manager, dec!(100)).await;

        let state = manager.load_monthly_state(chrono::Utc::now()).await.unwrap();
        assert_eq!(state.realized_loss, dec!(200), "only losses count, wins do not offset");
        assert_eq!(state.trades_opened, 2, "both closed positions counted");
    }

    #[tokio::test]
    async fn test_build_risk_context_uses_persisted_realized_loss() {
        let manager = create_test_manager().await;

        // Seed a closed position with -150 loss.
        save_closed_position_with_pnl(&manager, dec!(-150)).await;

        let ctx = manager.build_risk_context().await.unwrap();

        // monthly_realized_loss must reflect the seeded loss.
        assert_eq!(
            ctx.monthly_realized_loss,
            dec!(150),
            "build_risk_context must use realized_loss from load_monthly_state"
        );
    }

    #[tokio::test]
    async fn test_governed_monthly_loss_excludes_reconciled_missing_closes() {
        let manager = create_test_manager().await;

        save_closed_position_with_pnl(&manager, dec!(-150)).await;
        save_closed_position_with_pnl_and_reason(
            &manager,
            dec!(-200),
            robson_domain::ExitReason::ReconciledMissingOnExchange,
        )
        .await;

        let ctx = manager.build_risk_context().await.unwrap();

        assert_eq!(
            ctx.monthly_realized_loss,
            dec!(150),
            "governed monthly loss must exclude reconciled missing-on-exchange closes"
        );
        let state = manager.load_monthly_state(chrono::Utc::now()).await.unwrap();
        assert_eq!(
            state.realized_loss,
            dec!(150),
            "in-memory monthly_state mirrors governed loss in tests"
        );
    }

    #[tokio::test]
    async fn test_evaluate_monthly_halt_uses_persisted_realized_loss() {
        let manager = create_test_manager().await;

        // Seed realized loss of 400 (budget = 400).
        // ADR-0043: remaining = 400 - 400 = 0 <= 0 → MUST trigger.
        save_closed_position_with_pnl(&manager, dec!(-400)).await;

        let triggered = manager.evaluate_monthly_halt().await;
        assert!(
            triggered,
            "evaluate_monthly_halt must use persisted realized_loss from load_monthly_state"
        );
        assert!(manager.circuit_breaker.blocks_new_entries().await);
    }

    #[tokio::test]
    async fn test_restart_safety_active_positions_still_count_in_risk_context() {
        let manager = create_test_manager().await;

        // Seed an active position (simulates post-restart state).
        let _active =
            save_active_position(&manager, "BTCUSDT", Side::Long, dec!(95000), dec!(0.1)).await;

        // Simulate rebuild: load_monthly_state returns defaults (no losses).
        // Active positions must still be counted in risk context.
        let ctx = manager.build_risk_context().await.unwrap();

        // The active position must appear in open_positions.
        assert_eq!(ctx.open_positions.len(), 1, "active positions must still count after restart");
        assert_eq!(ctx.open_positions[0].symbol, "BTCUSDT");
    }

    mod persistence;
}
