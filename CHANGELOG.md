# Changelog

All notable changes to Robson Bot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added - Startup repair of suspended position streams (ADR-0056 step 7, PR P, part 3)

- At start, before the store is rebuilt, each suspended position stream is repaired: its suffix past the leg's
  `last_seq` is validated first, 500 events a page, and any event type without a guarded position handler means no
  repair; then it is applied in seq order, 500 a page, with `repaired_through_seq` written after each page, and the
  suspension is cleared. A failure leaves it suspended, with `repair_error`, and the leg's snapshot invalid; a repair
  interrupted by the process's death resumes past its checkpoint. System streams are never repaired automatically.

### Changed - Venue results kept, streams suspended, admissions closed on the latch (ADR-0056 step 7, PR P, part 2)

- A venue result is no longer dropped when its event fails to persist: an entry that filled still gets its exchange stop,
  and an exit still closes its leg in memory. The handlers that record a close after a venue fill (exit fill, panic close,
  reconciled close) keep it too, and MonthlyHalt is still evaluated; `/panic` reports such a leg as closed. The entry's
  pre-send writes stay barriers. On the tick path, and in startup recovery, one leg's event-only batch that did not persist
  no longer stops the other legs.
- Event persistence is serialized per stream. A failed append or projection apply suspends its stream: it takes no more
  appends in this process, and the suspension is recorded in `stream_suspensions` (migration 33) through the journal's
  retry set and loaded at the next start. `robsond_streams_suspended{kind}` exports both kinds at 0 from startup.
- The journal latch now closes admissions until a restart, whatever set it, including a journal write that did not land on
  a bounded send: arm and approval answer 503, a signal re-arms under the governed backoff, and an entry already past those
  checks is refused at its send. Protection, exits, sweeps and the protective API continue.
- A suspended system stream of the current month keeps admissions closed after a restart. There is no release before the
  fence PR, and a failed month reset no longer heals itself on the next 60 s poll: it stays as it was, fail-closed.
- Once a leg's venue work is done, a failure writing its query's audit is reported (ERROR and
  `robsond_query_audit_failures_after_venue_work_total`) and stops nothing: the tick's and `/panic`'s other legs still run, and
  a close or an entry keeps its outcome. A stream suspension whose write keeps failing no longer keeps the journal's other
  pending rows from being retried.

### Fixed - Four domain audit events had no projection route (ADR-0056 step 7, PR P, part 1)

- `entry_attempt_exhausted`, `signal_strategy_evaluated`, `entry_approval_pending` and `position_error`
  landed in the event log and then failed their apply as unhandled, so every exhaustion, every confirmed
  strategy signal and every wait for an approval logged an event-log error. They now route as audit-only:
  no projection table changes. The projector's freeze test now covers every domain event type, read from
  the enum, so a new event without a route fails CI. An older binary returns the same error on them as
  before, so this adds no rollback condition.
- The daemon refuses to start with a `PROJECTION_STREAM_KEY` in the `position:` or `system:` namespace: the
  projection worker would apply those streams beside the daemon's own writer. Production uses
  `robson:daemon`, the default.

### Added - Two-phase stop on SIGTERM and SIGINT (ADR-0056 step 7, PR S, part 3)

- SIGTERM, as well as SIGINT, now stops the daemon: as PID 1 it used to ignore SIGTERM until Kubernetes
  SIGKILLed it at the grace period, managing the account until then; management now stops as the stop
  closes. The stop drains for at most 12 s, refusing the ordinary API class with 503 `stopping` while
  protection, exits and the protective class continue, then closes: everything new is refused and the
  operations in flight finish.
- 25 s after the signal is the stop's own, cooperative deadline: nothing still running is waited for after
  it, and the runtime's teardown is bounded by it. A stop that is itself blocked, as by a log pipe no one
  reads, has no result then. 27 s after the signal is the independent bound: a thread of its own exits the
  process with `_exit(70)` whatever is stuck, writing nothing; exit code 70 is its only record. Both fit the
  pod's 30 s grace. Signals are handled from the start of `main`: one during the start's preparation
  (configuration, the database connection and its retries, the schema check, the journal's readiness, the
  epoch's insert) ends the start with nothing sent, even if the database comes back. Once the startup phase
  runs, a signal drains it like any operation, and venue requests may already have been made.
- A clean stop marks the process's epoch (`clean_stop_at`, `marker_version = 1`); a stop that fails a check
  (deadline, aborted operation, latch, anything unlanded) writes no marker. A marker write that times out or
  fails is unconfirmed: its update may have committed, and the log says so. A stop that gets past its waits
  only after the deadline starts no marker. A version-1 marker records only those checks: it stands if
  teardown work that holds no permit later keeps the process to the hard bound.
  An entry reached while draining is refused at its send. A cancel-first exit cut short by the stop can
  leave its leg without an exchange stop until the next process heals it.

### Added - Process epochs (ADR-0056 step 7, PR S, part 2)

- Migration 32 adds `process_epochs`. Each start records its epoch before any venue request and refuses to start
  when it cannot. A start after an unmarked epoch, or one marked below version 2, is logged and exported as
  unclean (`robsond_unclean_start`); nothing acts on it before the fence ships, and until then every start is.
- Without `DATABASE_URL`, or in a build without PostgreSQL, only the stub venue starts: a venue that can trade,
  `binance_usdm` named or selected from credentials, is refused before any request to it.

### Added - Operation permits and slot classes (ADR-0056 step 7, PR S, part 1)

- Every operation that can change the venue runs under a permit taken before the manager lock: 16 ordinary API
  slots, 8 protective ones (close, `/panic`, MonthlyHalt, `/reconcile-close`) and one reserved per internal loop.
  A full class answers 503 `busy` at once with nothing done.
- A venue mutation without a permit is counted and sets the journal latch; so does an operation dropped
  before it completed, such as one that panicked, which answers 500.
- An API operation runs in its own task, which owns its permit: a client that disconnects mid-operation no
  longer cancels it.
- Each REST request has one 10 s bound for the send and the body together, instead of 10 s each.

### Changed - Typed classification of stop cancels (ADR-0056 step 7, PR C)

- Every production stop cancel is classified the same way, with its class on its own journal intent. Only a
  typed `Cancelled` or an "already gone" with a final state is definite; no error type or message can be.
- A software-exit cancel answered by a broad order error now emits `InsuranceStopFailed` and keeps the stop
  id, instead of being read as cancelled. Uncertainty is journaled and does not close admissions until the
  fence ships.
- That cancel is not rate-limited: while the id stays on the leg it repeats once per exit attempt, with its
  own journal intent and failure event, because a retried cancel can be what lets the next exit through.
- An `InsuranceStopFailed` no longer changes a stop heal's deadline on record, overdue included; only a leg
  with none gets a new window. A heal left outstanding by the last process is rebuilt from the event log at
  startup.

### Added - Every order written to the journal (ADR-0056 step 7, PR J, part 3)

- An entry does not go out without its row. Reductions, protective stops and cancels try for 500 ms and go
  out anyway, setting `robsond_journal_latch` if the row did not land.
- Outcomes are recorded only: uncertainty is journaled and does not close admissions until the fence ships.
- The governed re-arm backoff now resets only after an entry reached the venue's send, on success and failure
  alike. A proven no-send on an `ExecutableSpan` leg ends the attempt (`entry_not_sent`); a legacy leg re-arms.

### Added - Disposition endpoint and journal metrics (ADR-0056 step 7, PR J, part 2)

- `POST /recovery/disposition` records an operator's terminal evidence or accepted risk for an outstanding
  intent, with the verified identity. It never sets a venue outcome.

### Added - Order journal storage and rules (ADR-0056 step 7, PR J, part 1)

- Migration 31 adds the order journal's tables and the rollback view
  `order_intents_blocking_rollback`. Nothing calls the journal yet.

### Fixed - A month row no longer gets a zero capital base without account evidence

- The entry and close counters inserted `capital_base = 0` when the month's row
  was missing, so a fill or a losing close before the first boundary poll of a
  month left a zero base visible until the reset overwrote it. A close in that
  window evaluated MonthlyHalt against it. They now only update an existing row:
  a missing row keeps falling back to configured capital, and the reset already
  zeroes both counters.
- `GET /positions?month=` now charges closes from `positions_current`, the source
  `/status` and the budget already use, instead of the `realized_loss` counter,
  which misses closes from before the month's row and reset. A past month's
  `slot_cells_total` can differ where the two sources disagreed, for example on
  a losing close that is not governed. A close stamped exactly at a month's
  start now counts in that month, for the budget as well.
- On a fresh database, migrations 11 and 21 left a current-month row with a zero
  base and `boundary_reset_at` set, so the month's reset was skipped and arm
  stayed blocked. Migration 30 removes such a row when it records nothing and
  has no event on the month's boundary stream. Production's current row has a
  real base, so it does not match.

### Added - Model-aware monthly budget gauge and ADR-0051 activation runbook

- On 2026-10-15 a net +83.50 USDT October with an empty book showed 2 of 4
  slots and "1.0% of a 4.0% limit": production still runs the superseded
  `hwm_v1` model (ADR-0046), which charges the give-back from an intra-trade
  equity peak, while the operator decision ADR-0051 (net-from-start) has sat
  dormant since August with both models computed in the shadow.
- `/status` now carries `monthly_budget_utilization_pct`, the share of the
  4% budget consumed under the ACTIVE model, and serves `month_peak_net` /
  `monthly_giveback_pct` only while `hwm_v1` is persisted (`null` under
  net-from-start, ADR-0051 §4). Slots, remaining budget and halt were
  already model-aware.
- Dashboard: `deriveBudgetView` prefers the utilization field and exposes
  the model; the MONTHLY LIMIT card explains the limit the daemon actually
  enforces ("Net governed loss since month start · Profits do not raise the
  limit", with `net / reserved / left`) instead of `peak / now`. Older
  backends keep the previous rendering.
- ADR-0051 §1 enforced: under net-from-start a confirmed deposit or manual
  account drift can no longer raise `capital_base` intra-month
  (`MonthlyBudgetModel::allows_capital_base_change`, applied in the income
  ledger and reconciliation recalibration paths); decreases still apply.
  Known consequence: after an intra-month deposit the idle drift scan warns
  every cycle until the month boundary (fail-loud; persisted attribution is
  #242).
- New runbook `docs/runbooks/activate-net-from-start-budget.md`: a policy
  gate (the ADR-0051 "Activation exceptions" amendment, approved by the
  operator on 2026-10-21 for the 2026-11-01 boundary; October stays
  `hwm_v1`), a scheduled-activation checklist, §5.4 preconditions (flat account, halt not latched, ledger
  reconciled with evidence rather than acknowledged, governed-net
  cross-check, shadow sanity), the guarded `UPDATE monthly_state` for the
  current month, validation and rollback. Later months inherit the
  persisted model on every insert path.
### Added - Entry preview before arming

- Three of October's stops were ATR fallbacks or single-level stops at 0.15% to
  0.26% of price, stopped by noise within minutes; the operator only learned
  the method after the fill. `POST /entry/preview` now dry-runs an immediate
  arm through the production pieces (same capital base, transient Armed
  position, the detector's immediate-signal analysis, the engine's entry
  decision on a cloned engine) and returns reference price, technical stop
  with distance, method and confidence, size, notional, planned risk, the
  executable trigger and span, the domain rejection the arm would hit, and
  advisory warnings. Nothing is persisted and no order is placed.
- The arm modal gains a STOP PREVIEW panel. Warnings turn the button into
  ARM ANYWAY; a rejection disables it with the reason shown.
### Changed - Live position card without polling the exchange

- `/status` and `/positions/:id` priced open positions through an exchange
  REST round trip on every call. They now use the last market price the tick
  path already holds per symbol; the round trip remains only as a fallback
  before the first tick after startup.
- New daemon event `PositionMarkAdvanced`, emitted after a persisted
  `trailing_stop_updated` or `favorable_extreme_advanced`, is published on the
  SSE stream as `position.changed` with a `reason`, so the dashboard refreshes
  the card when the stop or the watermark moves instead of waiting for a
  lifecycle transition. Bounded by the engine's rung and watermark-step rules.
- Dashboard polls `/status` every 20 s only while a position is open and the
  tab is visible, and once more when the tab becomes visible again.
### Fixed - Feed hygiene: watermark steps and deploy-time STRATEGY_CREATED

- `GET /events/history` now also excludes `favorable_extreme_advanced`; the
  card already shows the extreme, the daily feed is for decisions.
- `robson-db` init appended a strategy row and a `STRATEGY_CREATED` event on
  every ArgoCD sync (#231). Root cause: the migrate hook runs
  `robsond db init --tenant-id ...` without an account id, so each run
  minted a fresh account and a fresh strategy, and the per-(tenant, account)
  existence check never matched. The bootstrap identity is now the tenant:
  when the tenant already owns a strategy, its oldest account is returned
  and nothing is written. The event-log insert is additionally gated on the
  idempotency claim. Regression tests run against Postgres
  (`cargo test -p robson-db -- --include-ignored` with `DATABASE_URL`).

### Fixed - Favorable watermark persisted as it advances (#192)

- `favorable_extreme` / `extreme_at` were written only on discrete trailing
  advances (`TrailingStopUpdated`), so between rungs the real extreme lived in
  memory only, the EXTREME line and `/status` lagged, and a restart replayed
  candles from a stale anchor. The ladder itself was not affected: rung
  completion is detected on the crossing tick and the restart replay covers
  gaps.
- New bounded domain event `favorable_extreme_advanced`, emitted by the engine
  when the watermark beats the persisted extreme by at least a tenth of the
  executable span (10 bps of price without a span), applied to the in-memory
  state and to `positions_current`. `/status` now carries `favorable_extreme`
  on the flat summary and the dashboard RUNG line uses it.

### Added - Technical stop method and confidence on the position card

- Two consecutive arms on 2026-10-11 got ATR-fallback stops (no valid
  support/resistance level in bounds, ADR-0050 §7) at 0.15% and 0.26% of
  price; the first was stopped by noise 25 s after the fill. Nothing on the
  dashboard said the stop was a fallback.
- `/status` and `/positions/:id` now carry `tech_stop_method`
  (`swing_point` | `atr_fallback`), `tech_stop_level_n` and
  `tech_stop_confidence`, served from memory when the analysis ran in this
  process and re-read once from the event log (`technical_stop_analyzed`)
  after a restart. The card shows a `METHOD` line, e.g.
  `ATR fallback, no chart level in bounds · LOW confidence`.

### Fixed - Failed insurance-stop re-placement is now retried (ADR-0039)

- After a trailing advance the executor places the new exchange stop and then
  cancels the old one; if the placement failed it only emitted an audit
  `InsuranceStopFailed` and left the previous stop live. Because
  `TrailingStopUpdated` still recorded the new level as emitted, the engine
  never re-issued the move, so the exchange stop could sit one full rung behind
  the software stop until the next rung or a restart (daemon-down exposure of
  one span).
- `PositionManager` now remembers such positions, re-places the stop at the
  current software trigger on a later tick with a 30 s backoff, and clears the
  entry when the exchange accepts the order. Failed initial placements after an
  entry fill are healed the same way. New gauge
  `robsond_insurance_stop_stale_positions`.

### Fixed - Month equity marked at live price (ADR-0046)

- `month_equity_net` valued open positions at the stored `current_price`, which
  only moves on fill and on trailing-stop advances and resets to the entry price
  after a restart. The month peak was therefore set at ratchet extremes while
  intra-trade give-back never showed, and a restart with an open winner showed a
  phantom drawdown (2026-10-07: 1.7% of the 4% budget and one slot lost while
  the position was at its best).
- `PositionManager` now keeps the last market price per symbol and marks Active
  positions with it in the canonical budget snapshot, the monthly halt
  evaluation and the equity peak refresh. The stored price remains the fallback
  until the first tick after startup.

### Fixed - Position monitor tick storm (ADR-0049)

- `position_monitor_tick` was emitted once per Binance `aggTrade` message per
  Active position since the market-data WebSocket became live (#145), with no
  throttle: tens of rows per second into `event_log` plus one clone per tick
  pushed into the in-memory `MemoryStore` event vector, which is never read at
  runtime. That unbounded vector is the most likely cause of the 2026-08-18
  OOMKill recorded in rbx-infra (growth only while a position was Active).
- `MemoryStore::append` now sequences but does not retain `PositionMonitorTick`.
- `PositionManager` throttles audit-only ticks per position with
  `ROBSON_POSITION_MONITOR_TICK_INTERVAL_SECS` (default 20; 0 restores one event
  per trade). The engine still evaluates every trade; exits and trailing advances
  are never throttled and keep their accompanying tick.
- `GET /events/history` excludes `position_monitor_tick` so the dashboard's
  daily feed shows decision events instead of the last three seconds of ticks.
  `QueryOptions::exclude_event_type` is the new eventlog filter.
- Operational rollout pending: existing tick rows in `event_log_2026_08/09/10`
  remain until pruned under the bronze-v1 retention interlock.
### Added - Unmatched exchange income is visible on the dashboard (ADR-0045)

- `/status` already reported `unmatched_income_count`, but the frontend neither
  parsed nor displayed it; the only trace was an hourly `warn!` digest in the
  daemon log. Production carried 4 unmatched items on 2026-10-07, silently
  blocking the automatic `capital_base` recalibration from confirmed transfers.
- The risk dashboard now shows a banner with the count and the acknowledgement
  path while the count is above zero. Listing and acknowledging the items from
  the UI remains backlog.
### Fixed - Dashboard showed "STALE, reconnecting" on every mount

- The SSE client only reported a connection to the page when it was a
  recovery (`retries > 0`), and the freshness badge requires a connect or an
  event to turn green. Since the server's first heartbeat arrives after 15 s of
  silence and live events are rare, every dashboard load sat on "STALE,
  reconnecting" while REST had already filled the page.
- `FetchEventSource` now exposes `onOpen`, fired on every successful stream
  start including the first; the dashboard marks the stream connected from it.
  `onReconnect` keeps its recovery-only meaning (refetch missed state).

### Removed - Legacy fixed-percentage PositionMonitor artifacts (ADR-0014 / ADR-0022)

- Physically removed the superseded `PositionMonitor` implementation, its
  `DetectedPosition` domain and store types, repository adapters, monitor-only
  configuration, daemon/API wiring, frontend client surface, and unused
  `SafetyPanic` event. ADR-0022 `ReconciliationWorker` remains the unconditional
  owner of UNTRACKED-position enforcement; tracked positions remain protected
  by the chart-derived trailing policy and exchange-side insurance stop.
- Removed the corresponding public Rust exports:
  `robson_domain::{DetectedPosition, CalculatedStop, StopMethod}`, the
  detected-position repository/DTO exports from `robson-store`, and
  `robsond::{PositionMonitor, MonitorError, MonitorConfig,
  PositionMonitorConfig}` plus `DaemonError::Monitor`.
- Removed `/safety/status` and `/safety/test`. API contract coverage now verifies
  that both retired routes return `404`; no replacement safety-net API exists.
- Moved the existing Binance credential environment inputs into the dedicated
  `BinanceConfig`. The application no longer parses
  `ROBSON_POSITION_MONITOR_ENABLED`, `ROBSON_POSITION_MONITOR_POLL_INTERVAL`, or
  `ROBSON_POSITION_MONITOR_SYMBOLS`.
- Preserved `position_monitor_tick`: despite its historical name, it is a current
  `robson-engine` audit event for chart-derived trailing-stop processing and is
  unrelated to the deleted fixed-percentage monitor.
- Preserved the already-applied safety-net migration files unchanged. This
  change removes application storage access but does not drop the legacy
  `detected_positions` or `safety_net_executions` tables.
- Production's legacy enable flag is already false through `rbx-infra` PR #325.
  It remains an infrastructure rollback guard for images that still contain the
  old code path; the new application does not read it. Operational rollout of
  this application removal remains pending until the reviewed image is deployed
  and reconciliation liveness is revalidated.

### Added - Reconciliation worker liveness metrics (ADR-0022)

- Instrumented both startup and periodic exchange-reconciliation scans with
  bounded-cardinality Prometheus signals for completed/error outcomes, an
  in-progress gauge, and last-attempt/last-completed timestamps. This makes a
  stopped, hung, or repeatedly failing ADR-0022 worker externally observable
  without inferring its health from unrelated legacy PositionMonitor activity.
- A `completed` scan means the worker returned successfully at its top-level
  boundary; it deliberately does not claim that every best-effort maintenance
  substep succeeded.

## [2.5.0] - 2026-08-23

### Changed - v2.5 release baseline

- Aligned every Rust workspace crate and the SvelteKit frontend package on
  version `2.5.0`.
- Restricted the v2.5 dashboard ARM flow to the operationally accepted
  `immediate` entry mode. The request now always sends
  `entry_policy.mode = "immediate"` explicitly, so the backend's legacy
  `confirmed_trend` default cannot be selected accidentally.
- Kept the three strategy-backed entry modes in the backend for event/API
  compatibility, but removed them from the operator surface until each mode
  has passed its own operational acceptance gate.

### Added - Typed income-ledger reconciliation (ADR-0045 §1, 2026-07-07)

- New `IncomePort` trait ingests Binance's typed income stream
  (`GET /fapi/v1/income`) into a new `income_ledger` table, matching each
  item against governed state instead of diffing the wallet total.
  `REALIZED_PNL`/`COMMISSION` match by (symbol, time proximity) against
  `positions_current` — a disclosed, coarser substitute for exact `tradeId`
  linkage robson doesn't persist yet (ambiguous candidates stay unmatched
  and alarm, never guessed). `FUNDING_FEE` is always recognized — this is
  the first-ever attribution of Binance perpetual funding-rate payments
  anywhere in robson (previously a real, invisible cost). `TRANSFER` is the
  only category that may auto-recalibrate `capital_base`, and only when it
  explains the wallet delta 100% with zero other unmatched items in the same
  window.
- `reconciliation_worker::recalibrate_capital_base_after_pure_financial_drift`
  no longer writes `capital_base` for the generic case — an unexplained
  residual is now an alarm only, per ADR-0045 §2. The `in_flight_count`
  guard from the 2026-07-05 hotfix stays as defense in depth.
- `/status` exposes `unmatched_income_count` additively.

### Fixed - Reconciliation evidence-gathering anchor reset (ADR-0045 amendment, 2026-07-07)

- `emit_unresolved` cleared the reconciliation worker's missing-position
  observation on every unresolved cycle, re-anchoring
  `first_observed_missing_at` to "now" on the next cycle. When an
  insurance-stop algo order triggered but was rejected (`-2022 ReduceOnly
  Order is rejected`, meaning the position had already closed through a
  different execution), the evidence-lookup window could never look back
  far enough to find the real closing trade — the position stayed a phantom
  `Active` ghost for 14+ hours, hammering the exchange with rejected
  reduce-only exit retries every ~5s and blocking new same-symbol-side
  entries via the duplicate-position guard, until an operator supplied
  `UserTradeRecord` evidence manually via `/reconcile-close`. The observation
  now persists across unresolved cycles; it is cleared only when the
  position reappears on the exchange or the close actually resolves.

### Changed - Budget-metered entry admission (ADR-0043, 2026-07-05)

- The risk gate now admits an entry when its **actual planned worst-case
  loss** (cost-priced per ADR-0039: stop distance + executable-stop buffer +
  gap allowance + round-trip fees, × final quantity) fits the remaining
  monthly budget, instead of reserving the full 1% per-trade cap for every
  prospective trade. The 1% cap and the 4% monthly budget are unchanged.
  Product framing: **at least 4 chances per month — saved risk becomes an
  extra operation**.
- `EngineDecision` carries `planned_entry_risk`; `ProposedTrade` carries
  `planned_risk` (zero = unpriced → the full cap is reserved, the previous
  behavior).
- New governed denial `risk_budget_insufficient`: the trade does not fit the
  remaining budget, but budget remains. It re-arms with standard backoff and
  does not trigger MonthlyHalt.
- MonthlyHalt (periodic evaluation and gate denial `monthly_drawdown`) now
  fires only when `remaining_budget ≤ 0`, not when the tail is smaller than
  one full risk unit.
- `slots_available` / `new_slots_available` now mean the guaranteed minimum
  of remaining full-cap entries (a floor, not a ceiling).

### Fixed - Phantom daily loss limit removed (2026-07-04)

- `RiskGate` enforced a `daily_loss_limit_pct = 1%` check that no policy
  document ever adopted — AGENTS.md §10 states "There is no daily loss
  limit" (the policy is 1% worst-case loss per operation plus the 4%
  monthly drawdown, ADR-0024). The check had been structurally inactive
  (zeroed inputs, see ADR-v3-024) until daily PnL was wired into
  `RiskContext`, which silently re-activated it and blocked every new
  entry for the rest of the UTC day after a single budget-sized stop-out.
  The check, `RiskLimits::daily_loss_limit_pct`, and the daily PnL
  plumbing were removed (PR #110). `RiskCheck::DailyLossLimit` remains as
  a legacy variant for historical event compatibility.

### Fixed - Governed re-arm backoff (2026-07-04)

- A governed entry denial (risk gate, expired approval, entry-quantity
  rejection) re-armed the detector, and in Immediate mode the fresh
  detector re-fired instantly — a ~1/s hot loop against the Binance
  OHLCV API and the event store (84 denials in 92 s observed in prod).
  Governed re-arms now delay the detector's first evaluation with a
  per-position exponential backoff (5 s doubling to a 15-minute cap),
  reset when a signal clears the risk gate or the position is cancelled.
  The wait is cancellable, so shutdown and cancel-position interrupt it
  immediately (PR #110).

### Fixed - Position card shows the executable stop (2026-07-04)

- The frontend Active card rendered the raw technical trailing stop,
  which with an active invalidation guard (ADR-0042) read as if the stop
  sat below the recent adverse extreme. The card now shows the executable
  stop (`effective_stop`, ADR-0041 buffer + guard clamp) with the guard
  level as context while it binds, e.g.
  `stop 63,132.67 (guard 63,069.60)` (PR #109).

### Changed - Dashboard live-list semantics

- The live dashboard operations panel now shows only positions that still occupy a slot.
- Terminal operations such as `Closed`, `Error`, and `Canceled` are omitted from the live list and remain in historical views.

### Changed - Repository Cleanup and Layout Normalization (2026-05-13)

- Removed dead legacy roots from the repository: `apps/`, `data/`,
  `k8s/`, `pyproject.toml`, and `sonar-project.properties`.
- Moved the worktree helper from `devtools/` to `scripts/` and updated
  the live developer and deployment docs to match the current layout.
- Updated the pull request template and frontend deployment runbook to
  reference the current Rust + SvelteKit + `rbx-infra` structure.

### Fixed - v2 CI/CD Pipeline (2026-04-14)

- **rustfmt.toml restored**: Rich nightly-only configuration (`imports_granularity`,
  `group_imports`, `wrap_comments`, `brace_style`, `where_single_line`,
  `normalize_comments`, `format_code_in_doc_comments`) reinstated after being
  stripped down to stable-only options. Obsolete options `version` and
  `edition_idioms` removed (renamed/removed in current nightly).
- **CI uses nightly rustfmt**: Added `rustup toolchain install nightly --component rustfmt`
  step; formatting check now runs `cargo +nightly fmt --all --check` instead of stable.
- **GitOps paths corrected**: `Build & Push Image` job was targeting non-existent paths
  `apps/prod/robson/robsond-deploy.yml` in rbx-infra. Fixed to correct paths:
  `apps/prod/robson/robsond-deploy.yml` and `apps/prod/robson/robsond-db-migrate-job.yml`.
- Codebase reformatted with nightly rustfmt (62 files, net −534 lines).

### Added - Pattern Detection Engine (2025-12-28)

#### Core Engine (CORE 1.0)

- **Pattern Detection System**: Deterministic, idempotent technical pattern recognition
  - 7 patterns implemented (5 candlestick + 2 chart):
    - Candlestick: Hammer, Inverted Hammer, Bullish Engulfing, Bearish Engulfing, Morning Star
    - Chart: Head & Shoulders, Inverted Head & Shoulders
  - Pure domain layer (NO Django dependencies in core)
  - Hexagonal architecture (ports & adapters)
  - All timestamps from exchange data (timezone-independent)
  - Idempotent persistence (stable uniqueness keys)
  - Module: `apps/backend/monolith/api/application/pattern_engine/` (11 files, 3,176 lines)

- **Pattern Detectors**:
  - `HammerDetector` - Bullish reversal (1 candle, confidence: 0.75)
  - `InvertedHammerDetector` - Bullish reversal (1 candle, confidence: 0.70)
  - `EngulfingDetector` - Bullish/Bearish reversal (2 candles, confidence: 0.80)
  - `MorningStarDetector` - Bullish reversal (3 candles, confidence: 0.85)
  - `HeadAndShouldersDetector` - Bearish reversal (multi-bar, confidence: 0.80)
  - `InvertedHeadAndShouldersDetector` - Bullish reversal (multi-bar, confidence: 0.80)

- **Pattern Lifecycle Management**:
  - States: FORMING → CONFIRMED → INVALIDATED
  - Automatic confirmation checks (e.g., Hammer confirmed on close above high)
  - Automatic invalidation checks (e.g., Hammer invalidated on close below low)
  - PatternAlert emission (idempotent, uniqueness key: instance_id + alert_type + alert_ts)

- **Management Command**:
  - `python manage.py detect_patterns SYMBOL TIMEFRAME [--all|--candlestick|--chart]`
  - Flexible detector selection (individual pattern flags: --hammer, --hns, etc.)
  - Rich terminal output with idempotency tracking
  - File: `apps/backend/monolith/api/management/commands/detect_patterns.py` (371 lines)

- **Tests**:
  - 22+ test cases (869 lines)
  - Pure unit tests (helpers, golden OHLC sequences)
  - Idempotency integration tests (CRITICAL: 2nd run creates 0 instances/alerts)
  - Property tests (optional, requires Hypothesis)
  - File: `apps/backend/monolith/api/tests/test_pattern_engine.py`

- **Documentation**:
  - ADR-0018: Architecture decision record
  - PATTERN_ENGINE_V1.md: Technical specification with detection rules
  - PATTERN_ENGINE_IMPLEMENTATION_PLAN.md: Milestones M1-M8
  - PATTERN_ENGINE_IMPLEMENTATION_SUMMARY.md: Usage guide with examples
  - PATTERN_ENGINE_SESSION_HANDOFF.md: Deployment guide

#### Key Features

- ✅ **Deterministic**: Same OHLC → Same patterns (no randomness)
- ✅ **Idempotent**: Re-scans create 0 duplicates (verified: alerts_created=0 on 2nd run)
- ✅ **Timezone-Independent**: All timestamps from exchange data (no `datetime.now()`)
- ✅ **Non-Executing**: Emits alerts only (NO order placement)
- ✅ **Zero Schema Changes**: Uses existing production models (PatternInstance, PatternAlert)

#### Integration Points

- Uses existing `MarketDataService` for candle fetching (with caching)
- Uses existing Django pattern models (no migrations required)
- Output: `PatternAlert` table ready for EntryGate (CORE 1.2) consumption
- Future: Hand-Span Trailing Stop (CORE 1.1) triggered on confirmed patterns

### Added - BTC Portfolio Tracking (2025-12-26)

#### Backend

- **BTC-Portfolio Tracking**: Complete portfolio valuation and profit tracking in BTC terms
  - New transaction types: `DEPOSIT` and `WITHDRAWAL` for external flows
  - New movement category: `EXTERNAL` for deposits/withdrawals
  - BTC fields in `BalanceSnapshot`: `total_equity_btc`, `spot_btc_value`, `margin_btc_value`

- **BTCConversionService**: Price discovery and conversion service
  - Multi-route price discovery (direct pair, USDT, BUSD)
  - 60-second price caching to avoid rate limits
  - Converts any asset balance to BTC
  - File: `apps/backend/monolith/api/services/btc_conversion_service.py` (250 lines)

- **PortfolioBTCService**: Portfolio calculation and profit tracking
  - Total portfolio value denominated in BTC
  - Profit formula: `Current Balance (BTC) + Withdrawals (BTC) - Deposits (BTC)`
  - Historical BTC value tracking
  - File: `apps/backend/monolith/api/services/portfolio_btc_service.py` (358 lines)

- **Binance Sync**: Automatic deposit/withdrawal synchronization
  - Syncs from Binance API using `get_deposit_history()` and `get_withdraw_history()`
  - Deduplicates by `binance_order_id`
  - Only syncs successful transactions (status=6)
  - Extended `AuditService` with `sync_deposits_and_withdrawals()` method

- **REST API Endpoints**:
  - `GET /api/portfolio/btc/total/` - Current portfolio value in BTC
  - `GET /api/portfolio/btc/profit/` - Profit in BTC with breakdown
  - `GET /api/portfolio/btc/history/` - Historical BTC value over time
  - `GET /api/portfolio/deposits-withdrawals/` - List of deposits/withdrawals

- **CLI Commands**:
  - `python manage.py portfolio_btc` - Show portfolio in BTC
  - `python manage.py portfolio_btc --profit` - Show profit since inception
  - `python manage.py sync_deposits --days-back 30` - Sync deposits/withdrawals

#### Frontend

- **BTCPortfolioDashboard Component**: Complete portfolio dashboard with tabbed navigation
  - **Overview Tab**: Total value, profit metrics, account breakdown
  - **History Tab**: Interactive chart using Recharts with timeline filtering
  - **Transactions Tab**: Filterable table of deposits/withdrawals
  - Auto-refresh every 60 seconds
  - File: `apps/frontend/src/components/logged/BTCPortfolioDashboard.jsx`

- **Patrimony Component Updated**: Now displays BTC-denominated portfolio
  - Shows total portfolio value in BTC
  - Displays profit/loss in BTC with color coding
  - Account and transaction summaries
  - File: `apps/frontend/src/components/logged/Patrimony.jsx`

- **Footer Updated**:
  - Changed "Designed by Deepmind Team" → "Designed by RBX Robótica"
  - Removed debug text about application mode and BACKEND_URL

#### Testing

- **Unit Tests**: `test_btc_conversion_service.py`
  - Price discovery edge cases
  - Zero division handling
  - Invalid asset handling
  - Cache behavior verification

- **Integration Tests**: `test_btc_portfolio_endpoints.py`
  - Profit calculation correctness
  - API error handling
  - Transaction filtering
  - Empty data handling

- **Frontend Tests**: `BTCPortfolioDashboard.test.js`
  - Loading and error states
  - Tab navigation
  - Data rendering
  - Auto-refresh functionality

#### Database

- **Migration 0019**: Added BTC portfolio tracking fields
  - New transaction types: `DEPOSIT`, `WITHDRAWAL`
  - New category: `EXTERNAL`
  - New BalanceSnapshot fields: `total_equity_btc`, `spot_btc_value`, `margin_btc_value`

### Changed

- Updated `AuditService` to support external flow synchronization
- Extended `main_urls.py` with BTC portfolio routes
- Updated `LoggedHomeScreen.jsx` to include BTCPortfolioDashboard

### Technical Details

- **Architecture**: Hexagonal (Ports & Adapters)
- **Language**: 100% English (code, comments, docs)
- **Testing**: pytest (backend), vitest (frontend)
- **Frontend Library**: Recharts 2.12.7 (already installed)
- **Backend**: Django 5.2, Python 3.12

### Migration Guide

```bash
# Apply database migration
python manage.py migrate api

# Sync historical deposits (last 90 days)
python manage.py sync_deposits --days-back 90

# Verify portfolio in BTC
python manage.py portfolio_btc --profit
```

---

## Previous Versions

For versions prior to 2025-12-26, please refer to git history.
