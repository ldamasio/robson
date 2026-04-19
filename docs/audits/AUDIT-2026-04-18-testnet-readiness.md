# AUDIT: Robson v3 Testnet Readiness — 2026-04-18

**Auditor**: GLM (executor)
**Reviewer**: Codex (reviewed — corrections applied 2026-04-18)
**Scope**: Full position lifecycle from arm to reconciliation
**Repository state**: robson HEAD `44a28737`, rbx-infra HEAD `c4dbf3d`
**Deployed testnet SHA**: `5db3daad` (11 commits behind HEAD — docs-only)
**Deployed prod SHA**: `5db3daad` (same image)

---

## 2026-04-19 Addendum — MIG-v3#11 Repository Unblock

The static exposure-limit blocker documented in this audit has been superseded by
ADR-0024 and MIG-v3#11:

- `robson` commit `2db23ad2` added `TradingPolicy`, `TechStopConfig`, and dynamic
  slot calculation.
- `robson` commit `0b3653a7` corrected realized-loss accounting so wins do not
  offset losses in the ADR-0024 slot budget.
- `robson` commit `19130cf3` updated the architecture verification SHA references.
- `rbx-infra` commit `c3b1bc3` added the testnet `ROBSON_MIN_TECH_STOP_PCT`
  configuration.

VAL-001 Phase 2 is no longer blocked in repository state by the legacy
`max_single_position_pct=15%` and `max_total_exposure_pct=30%` gates. The remaining
Phase 2 work is operational: deploy the latest image, sync the testnet manifest, and
execute the runbook to collect exchange order, fill, trailing-stop, exit, and PnL
evidence.

The other production-readiness findings remain open: `exchange_order_id` in domain
events, entry event ordering after exchange acknowledgement, startup reconciliation,
and monthly state persistence.

---

## 1. Documentation Alignment Findings

### 1.1 Consistent Documents

| Document | Alignment | Notes |
|----------|-----------|-------|
| AGENTS.md | Self-consistent | Canonical rules, invariants clearly stated |
| v3-migration-plan.md | Internally consistent | Status table accurate as of 2026-04-18 |
| v3-runtime-spec.md | Consistent with migration plan | QE-P1–P4 status matches |
| v3-query-query-engine.md | Consistent with implementation | QE-P4 implementation notes match code |
| v3-control-loop.md | Consistent with runtime spec | Stage mapping aligned |
| v3-risk-engine-spec.md | Internally consistent | PnL model section correctly reflects code |
| VAL-001 runbook | Consistent with current blockers | Run log accurate |
| VAL-002 runbook | Consistent with VAL-001 dependency | Blocking precondition correct |
| UNTRACKED-POSITION-RECONCILIATION.md | Internally consistent | Clearly marks target architecture items |
| SYMBOL-AGNOSTIC-POLICIES.md | Internally consistent | Migration guidance clear |
| technical-stop-requirements.md | Self-consistent | Correctly describes chart-derived approach |
| POSITION-SIZING-GOLDEN-RULE.md | Self-consistent | Formula correct |
| ROBSON-TESTNET-ENVIRONMENT.md | Consistent with manifests | ConfigMap values match rbx-infra |
| rbx-infra ARCHITECTURE.md | Consistent with testnet doc | Environment tiers aligned |

### 1.2 Conflicts Between Documents

> **Note**: All conflicts below were pre-fix findings discovered during the audit. Those marked "Resolved" have been corrected by the documentation patch (D1–D7) applied in this working tree. No unresolved doc-vs-doc conflicts remain.

| Conflict | Documents | Detail | Status |
|----------|-----------|--------|--------|
| Circuit breaker model | v3-migration-plan.md vs v3-risk-engine-spec.md | Pre-fix: migration plan §2, §3, §7, §11, §13, §14 described L1–L4 escalation ladder; risk engine spec replaces with binary MonthlyHalt. Post-fix (D1+D7): migration plan and risk-engine-spec status line now consistently describe binary MonthlyHalt. Remaining L1/L4 references are historical/rejection context only. | **Resolved** — D1, D7 |
| Circuit breaker in runtime spec | v3-runtime-spec.md vs v3-migration-plan.md | Pre-fix: runtime spec was already correct (binary MonthlyHalt). Stale L1–L4 text was in migration plan only. Post-fix (D1): migration plan aligned. | **Resolved** — D1 |
| MIG-v2.5#5 status | v3-migration-plan.md | Pre-fix: table said "Circuit breaker escalation ladder — Done". Post-fix (D1): description updated to "Binary MonthlyHalt circuit breaker — replaces L1-L4 escalation". | **Resolved** — D1 |
| POSITION-SIZING-GOLDEN-RULE.md references | Golden Rule doc | Pre-fix: referenced Django/v1 paths. Post-fix (D3): references updated to Rust implementation paths. | **Resolved** — D3 |
| technical-stop-requirements.md references | Tech stop reqs | Pre-fix: referenced pandas/numpy and Python API contracts (v1 era). Post-fix (D4): implementation note added identifying v2 Rust implementation as current. | **Resolved** — D4 |

### 1.3 Target Architecture Presented as Current Implementation

| Document | Section | What is presented as current | Actual status |
|----------|---------|----------------------------|---------------|
| v3-runtime-spec.md | Recovery §Scenario 5 | UNTRACKED position detection + close | Not implemented — reconciliation worker is MIG-v3#9 pending |
| v3-runtime-spec.md | "Robson-Authored Position Invariant" | StartupReconciling state, mandatory close | Target only — no StartupReconciling state in daemon |
| v3-control-loop.md | Crash Recovery | Full reconciliation with exchange | Partial — startup restores from projection, but no UNTRACKED scan |
| v3-migration-plan.md | §0 Regulatory Posture | "Written legal opinion from Zug lawyer" | Deferred to backlog — doc correctly labels this but it reads as more mature than reality |
| UNTRACKED-POSITION-RECONCILIATION.md | Enforcement §2 | Exchange order-id ↔ event-log link | `EntryOrderPlaced`/`ExitOrderPlaced` domain events do NOT carry `exchange_order_id`. There is no dedicated `event_log` exchange_order_id index. Existing DB support is `orders_current.exchange_order_id` (populated by `OrderAcked` connector-level events via projector), a GIN index on event_log payload, and order-id indexes on `orders`/`orders_current`. The domain event gap means reconciliation cannot trace from exchange positions to `robsond`-authored entries via domain events. |
| UNTRACKED-POSITION-RECONCILIATION.md | Operator Workflow | `/reconciliation/suspend` endpoint | Doc explicitly labels as "v3 target, not current feature" — correct |
| v3-runtime-spec.md | Permission System | Configurable permission matrix with scopes | Doc explicitly labels as "QE-P3 minimum" and notes "broader matrix remains v3 target" — correct |
| v3-runtime-spec.md | Context Management | LLM integration, ReasoningPort | Correctly deferred to v3+ — no issue |
| v3-risk-engine-spec.md | Follow-up Required | Multiple items | All correctly labeled as not implemented — good |

### 1.4 Missing "Not Ready and Why" Status

| Document | Gap |
|----------|-----|
| v3-migration-plan.md | MIG-v3#9 (Reconciliation Worker) says "Pending — follow-up from ADR-0022" but does not explicitly say what subcomponents are missing (StartupReconciling state, UNTRACKED event types, exchange_order_id in order events) |
| v3-migration-plan.md | MIG-v3#10 (Symbol-Agnostic Sweep) says "Pending" but does not itemize which files still have BTC-coupled assumptions |
| v3-runtime-spec.md | Runtime Recovery section does not label itself as partially implemented vs fully implemented — a reader would assume crash recovery includes UNTRACKED scanning |

---

## 2. Lifecycle Readiness Matrix

### Row Format
Component Owner | Status | Evidence | Testnet Ready | Blocker | Next Action

### 2.1 Infrastructure & Environment

| Step | Owner | Status | Evidence | Testnet | Blocker | Next Action |
|------|-------|--------|----------|---------|---------|-------------|
| testnet environment isolation | rbx-infra | **implemented** | namespace `robson-testnet`, separate DB `robson_testnet`, isolated ConfigMap, ArgoCD app | Yes | None | Deploy latest docs-only commits |
| API token and mutating route auth | robsond/api.rs | **implemented** | Bearer token middleware on POST/DELETE routes | Yes | None | — |

### 2.2 Position Lifecycle — Entry Side

| Step | Owner | Status | Evidence | Testnet | Blocker | Next Action |
|------|-------|--------|----------|---------|---------|-------------|
| arm position | PositionManager | **implemented** | `arm_position()` in position_manager.rs, `/positions` POST in api.rs | Yes | None | — |
| detector signal generation | detector.rs | **implemented** | MA crossover detection, `TechnicalStopAnalyzer` for chart stops | Yes | Stop distance too tight for testnet capital | Detector must emit wider stops, or testnet capital/policy adjusted |
| chart-derived TechnicalStopAnalyzer | robson-engine | **implemented** | `technical_stop_analyzer.rs` — swing point detection, support/resistance, ATR fallback | Yes | Chart conditions produce stops ~2-3% from entry; need ≥6.67% for single-position limit | This is a market-conditions blocker, not a code blocker |
| Golden Rule sizing | robson-domain | **implemented** | `calculate_position_size()` in entities.rs, `RISK_PER_TRADE_PCT = 1` | Yes | None | — |
| RiskGate approval/denial | robson-engine | **implemented** | `RiskGate::evaluate()` in risk.rs, wired in `QueryEngine::check_risk()` | Yes | 2026-04-18 static exposure denials superseded by ADR-0024 dynamic slots | Deploy MIG-v3#11 and rerun VAL-001 Phase 2 |
| high-notional approval gate | QueryEngine (QE-P3) | **implemented** | `AwaitingApproval` state, `/queries/{id}/approve`, 300s TTL, `/status` pending | Yes | Not validated after MIG-v3#11 policy rollout | Rerun VAL-001 Phase 2 |
| entry order placement | Executor | **implemented** | `executor.rs`, `ExchangePort::place_order()`, Binance REST adapter | Yes | None | — |

### 2.3 Position Lifecycle — Fill & Monitoring

| Step | Owner | Status | Evidence | Testnet | Blocker | Next Action |
|------|-------|--------|----------|---------|---------|-------------|
| entry fill handling | PositionManager | **implemented** | `handle_entry_fill()`, `EntryFilled` event, `PositionActive` transition | Yes | Not validated after MIG-v3#11 policy rollout | Pass VAL-001 Phase 2 |
| EventLog append | robson-eventlog | **implemented** | `append_event()` with idempotency_key, `execute_and_persist()` fail-fast path | Yes | None | — |
| projection update | robson-projector | **implemented** | `apply_event_to_projections()`, handlers for all lifecycle events including `entry_signal_received` | Yes | None | — |
| active position monitoring | position_monitor.rs | **implemented** | `PositionMonitor` spawned in daemon.rs, tick-based monitoring | Yes | None | — |
| position_monitor_tick evidence | robson-engine | **implemented** | `create_position_monitor_tick_event()` in lib.rs:726, `PositionMonitorTick` event in events.rs, projector handler in apply.rs:62 | Yes | None | — |
| trailing_stop_updated evidence | robson-engine + projector | **implemented** | `TrailingStopUpdated` event in events.rs:296, discrete step algorithm in `trailing_stop.rs`, handler in positions.rs:360 | Yes | None | — |

### 2.4 Position Lifecycle — Exit Side

| Step | Owner | Status | Evidence | Testnet | Blocker | Next Action |
|------|-------|--------|----------|---------|---------|-------------|
| manual exit | api.rs + PositionManager | **implemented** | `DELETE /positions/{id}`, calls exit flow | Yes | Not validated after MIG-v3#11 policy rollout | Pass VAL-001 Phase 2 → Phase 5A |
| stop-triggered exit | robson-engine + PositionManager | **implemented** | `TriggerExit` in EngineAction, `update_trailing_stop_discrete()` in trailing_stop.rs | Yes | Not validated after MIG-v3#11 policy rollout | Pass VAL-001 Phase 2 → Phase 5B |
| exit fill handling | PositionManager | **implemented** | `handle_exit_fill()`, `ExitFilled` event, `PositionClosed` transition | Yes | Not validated after MIG-v3#11 policy rollout | Pass VAL-001 Phase 2 → Phase 5 |
| position_closed + PnL | robson-domain | **implemented** | `PositionClosed` event with `realized_pnl`, `fees_paid` | Yes | Not validated after MIG-v3#11 policy rollout | Pass VAL-001 Phase 2 → Phase 5 |

### 2.5 Cross-Cutting Concerns

| Step | Owner | Status | Evidence | Testnet | Blocker | Next Action |
|------|-------|--------|----------|---------|---------|-------------|
| untracked position reconciliation | MIG-v3#9 | **pending** | ADR-0022 + policy doc exist; NO reconciliation worker, NO `StartupReconciling` state, NO `position_untracked_detected` event type. Safety Net infrastructure exists but does not implement ADR-0022 flow. | No | Missing implementation | Implement reconciliation worker (MIG-v3#9) |
| exchange_order_id in order events | robson-domain | **pending** | `EntryOrderPlaced` and `ExitOrderPlaced` carry `cycle_id` (governance proof) but do NOT carry `exchange_order_id`. The engine emits `EntryOrderPlaced` via `EmitEvent` BEFORE the `PlaceEntryOrder` action reaches the executor (lib.rs:418–434), so `exchange_order_id` is not yet available at emission time. `ExitOrderPlaced` can likely be amended directly because the executor creates it after the exchange response. Existing DB support: `orders_current.exchange_order_id` is populated by `OrderAcked` connector-level events (projector handlers/orders.rs), not by domain events. There is no dedicated `event_log` exchange_order_id column index. | No | Critical for reconciliation | Design note required — see §7 |
| symbol-agnostic behavior | MIG-v3#10 | **partial** | Core risk/trailing-stop/sizing algorithms are symbol-agnostic. Risk tests use ETHUSDT in some cases. But `sample_proposed()` in risk.rs defaults to BTCUSDT. Most tests single-symbol. | Partial | No cross-symbol VAL-001 validation | Parameterize risk tests across ≥2 symbols |
| VAL-001 pass/fail evidence | runbook | **Phase 1 PASS / Phase 2 pending rerun** | Run log entries for 2026-04-16 and 2026-04-18 confirm the original blocker; 2026-04-19 addendum records repository unblock via MIG-v3#11 | — | Latest image and testnet config not yet operationally validated | Deploy and run VAL-001 Phase 2 |
| VAL-002 block condition | runbook | **blocked** | VAL-002 requires VAL-001 PASS; VAL-001 Phase 2 not passed | — | VAL-001 Phase 2 | — |
| MonthlyHalt circuit breaker | circuit_breaker.rs | **implemented** | Binary `Active | MonthlyHalt`, `trigger_halt()`, blocks entries and signals, 4% drawdown trigger | Yes | None | — |
| Monthly drawdown PnL model | risk.rs + position_manager | **implemented** | `build_risk_context()` sums realized PnL - fees from `find_closed_in_month()`, adds unrealized PnL from Active positions | Yes | Unrealized PnL uses last tick, not exchange mark price (known approximation) | — |

---

## 3. Proposed Patch Plan

### 3.1 Documentation-Only Fixes

All fixes marked ✅ Applied below are present in the current working tree — they are not proposed changes awaiting implementation.

| ID | File | Change | Risk |
|----|------|--------|------|
| D1 | v3-migration-plan.md §2 Decision #5, §3 Circuit Breaker, §12 MIG-v2.5#5, §7 UI degradation, §11 failure recovery, §13 critical risks, §14 final recommendations | Update description: "Circuit breaker escalation ladder" → "Binary MonthlyHalt (4% drawdown → halt all)". Mark that L1–L4 was decided against per v3-risk-engine-spec.md. Replace L1→L4 auto-escalation text with MonthlyHalt + K8s restart + manual runbooks. Remove completed MIG-v2.5 items from final recommendations, update to current priorities (VAL-001, MIG-v3#9, exchange-order identity, MIG-v3#10). | ✅ Applied 2026-04-18 |
| D2 | v3-runtime-spec.md §5 Permission System | Runtime spec `[circuit_breaker]` config already correctly states binary MonthlyHalt. Minor: §5 "Escalation path" reference in permission section uses generic "escalation" terminology, not L1–L4 — acceptable, no change needed. | No change needed |
| D3 | POSITION-SIZING-GOLDEN-RULE.md | Update "Related Documents" to reference Rust implementation paths, not Django/Python paths | ✅ Applied 2026-04-18 |
| D4 | technical-stop-requirements.md | Add note: "Requirements document from v1 era. Current implementation is in v2/robson-engine/src/technical_stop_analyzer.rs (Rust). Python/pandas references are historical." | ✅ Applied 2026-04-18 |
| D5 | v3-runtime-spec.md Recovery §Scenario 5, "Robson-Authored Position Invariant" section | Add explicit "TARGET ARCHITECTURE — FOLLOW-UP REQUIRED" label to UNTRACKED detection flow. The runtime spec config and state model already correctly describe binary MonthlyHalt; the gap is only in the recovery/reconciliation sections. | ✅ Applied 2026-04-18 |
| D6 | v3-control-loop.md Crash Recovery | Add label: "UNTRACKED position scanning on startup is target architecture (MIG-v3#9). Current recovery restores from EventLog replay + projection." | ✅ Applied 2026-04-18 |
| D7 | v3-risk-engine-spec.md status line | Update status to "APPROVED (revised — replaces the previous L1–L4 escalation design with binary MonthlyHalt)" | ✅ Applied 2026-04-18 |

### 3.2 Implementation Fixes

| ID | Component | Change | Scope | Risk |
|----|-----------|--------|-------|------|
| I1 | `robson-domain/src/events.rs` + executor wiring | Exit-side: add `exchange_order_id: Option<String>` to `ExitOrderPlaced`, populate from `ActionResult`. Entry-side: requires design decision (see §7) — `EntryOrderPlaced` is emitted before exchange placement. Options include new `EntryOrderAcked` event or action-sequence restructuring. | Critical for reconciliation (MIG-v3#9 dependency) | Exit-side: low (additive). Entry-side: medium (requires architecture choice). |
| I2 | `robson-engine/src/risk.rs` | Replace `sample_proposed()` BTCUSDT default with parameterized symbol. Add at least one risk test using a non-BTC symbol with different notional characteristics. | ADR-0023 compliance | Low |
| I3 | MIG-v3#9: Reconciliation worker | Implement in `robsond/src/reconciliation_worker.rs`: periodic scan of exchange positions, match against `entry_order_placed` events by `exchange_order_id`, classify UNTRACKED, close via Safety Net. Add `StartupReconciling` daemon state. Add `position_untracked_detected` and `untracked_position_closed` event types. | Critical for production safety | Medium — new subsystem, needs careful testing |
| I4 | VAL-001 Phase 2 unblock | **Resolved in repository by ADR-0024 / MIG-v3#11.** Static exposure caps are no longer enforced; the correct unblock path is deploy latest `robsond`, sync testnet config, and rerun Phase 2. | Operational rollout | Needs live testnet validation; no testnet-only exposure exception required. |

### 3.3 Test Additions

| ID | Test | Scope |
|----|------|-------|
| T1 | Risk gate test with non-BTC symbol (different tick/lot) | ADR-0023 compliance |
| T2 | Exit-side `exchange_order_id` propagation test: trigger exit → verify `ExitOrderPlaced` event payload includes exchange_order_id | I1 exit-side validation |
| T3 | Reconciliation worker unit test: given exchange position with no matching event → UNTRACKED classification | I3 validation |
| T4 | Reconciliation worker integration test: UNTRACKED position close → verify events emitted | I3 validation |

### 3.4 Operational/Testnet Runbook Changes

| ID | Change | Detail |
|----|--------|--------|
| R1 | VAL-001 testnet policy exception procedure | **Superseded by ADR-0024.** Do not add a 100% exposure-limit exception; deploy MIG-v3#11 and use dynamic slots. |
| R2 | VAL-001 prerequisite P7 enhancement | Add automated check script for UNTRACKED positions (currently manual "query Binance for ALL open positions") |
| R3 | Deploy latest HEAD to testnet | 11 docs-only commits (ADR integration) not yet deployed. No code changes, but docs alignment improvements. New image build + tag update needed. |

### 3.5 Items That Must NOT Be Changed Without Explicit Operator Approval

| Item | Why |
|------|-----|
| Risk per trade percentage (1%) | Core invariant — ADR/AGENTS.md non-negotiable |
| Monthly drawdown threshold (4%) | Core invariant — risk engine spec |
| Primary immutable policy values (`risk_per_trade_pct=1`, `max_monthly_drawdown_pct=4`) | Core ADR-0024 invariants |
| Production ConfigMap — any change to `apps/prod/robson/` | Production isolation |
| `ROBSON_POSITION_MONITOR_ENABLED` in production | VAL-002 gating |
| Testnet → production endpoint routing | Could cause real trades on testnet credentials or vice versa |
| Reconciliation worker auto-close behavior for production | Must be validated on testnet first |
| Any bypass of RiskGate for VAL-001 Phase 2 | Hard rule in mission statement |

---

## 4. Risk Callouts

### 4.1 Real Capital Risk (Post-VAL-002)

| Risk | Severity | Detail |
|------|----------|--------|
| **Missing `exchange_order_id` in domain order events** | HIGH | `EntryOrderPlaced` and `ExitOrderPlaced` do not carry `exchange_order_id`. The engine emits `EntryOrderPlaced` before exchange placement (action order: `EmitEvent(EntryOrderPlaced)` then `PlaceEntryOrder` — lib.rs:418–434), so the field is not available at emission time. Without this field in domain events, the reconciliation worker cannot match exchange positions to Robson-authored entries via domain events. Existing DB support: `orders_current.exchange_order_id` is populated by `OrderAcked` connector-level events, but this is a separate event model from domain lifecycle events. Design note required — see §7. |
| **No reconciliation worker** | HIGH | A position opened manually on the Robson-operated Binance account (operator experiment, leaked API key, shared session) would silently consume margin and exposure budget. The Safety Net monitors for "rogue" positions but does not implement the ADR-0022 UNTRACKED classification flow. Fix: I3 above. |
| **No StartupReconciling gate** | MEDIUM | On daemon restart, positions could exist on the exchange from a prior crash or external source. Current startup restores from projection (correct for tracked positions) but does not scan for UNTRACKED. Until MIG-v3#9 is implemented, the operator should manually verify the Binance account is clean before and after daemon restarts. |
| **Unrealized PnL uses last tick, not exchange mark price** | LOW | In high-volatility scenarios, the local PnL approximation may diverge from exchange-reported PnL. Monthly drawdown trigger could fire late or early. Documented as known approximation. |

### 4.2 Testnet Integrity Risk

| Risk | Severity | Detail |
|------|----------|--------|
| **Testnet production confusion** | LOW | rbx-infra audit confirmed: testnet ConfigMap has `ROBSON_BINANCE_USE_TESTNET: "true"`, production ConfigMap does NOT have this key. Proper isolation. |
| **Stale image on testnet** | LOW | Testnet at `sha-5db3daad`, HEAD at `44a28737`. The 11 commits are docs-only (no code changes), so deployed behavior is identical. But ADR references in runbooks won't match until redeployed. |

### 4.3 Governance Integrity Risk

| Risk | Severity | Detail |
|------|----------|--------|
| **cycle_id is Optional on order events** | LOW | `EntryOrderPlaced.cycle_id` and `ExitOrderPlaced.cycle_id` are `Option<Uuid>`. If a code path places an order without going through QueryEngine, the field would be `None`. The `GovernedAction` token makes this unlikely from within robsond, but the Option leaves the door open. The VAL-001 Phase 2 acceptance criteria correctly checks for cycle_id presence. |
| **Executor accepts raw `Vec<EngineAction>`** | LOW | The executor boundary does not enforce GovernedAction at the type level. Governance is enforced inside robsond before dispatch. A future refactor could move enforcement to the executor signature. This is a known follow-up, not an active vulnerability. |

### 4.4 Action-Order Risk (New Finding)

| Risk | Severity | Detail |
|------|----------|--------|
| **`EntryOrderPlaced` emitted before `PlaceEntryOrder`** | MEDIUM | `decide_entry()` (lib.rs:418–434) returns actions in order `[EmitEvent(EntryOrderPlaced), PlaceEntryOrder]`. The executor processes these sequentially (executor.rs:82–91): first `EmitEvent` persists the event and applies to Store (transitioning position to `Entering`), then `PlaceEntryOrder` executes on the exchange. If the exchange placement fails after the event was already persisted, the Store/EventLog will show a position in `Entering` state with an `EntryOrderPlaced` event, but no exchange order exists. This creates a divergence: EventLog says "order placed" but the exchange has no corresponding order. The intent journal provides some protection (idempotency on retry), but a retry after a real exchange failure would re-emit `EntryOrderPlaced` (which is already persisted). **Investigation required**: what happens to the position state if `PlaceEntryOrder` fails after `EntryOrderPlaced` was already applied? Does the daemon recover correctly, or does the position remain stuck in `Entering`? |

### 4.5 Documentation Integrity Risk

| Risk | Severity | Detail | Status |
|------|----------|--------|--------|
| **Migration plan describes L1–L4 but implementation is binary** | ~~MEDIUM~~ | Pre-fix: a reader of v3-migration-plan.md would believe the system has L1–L4 escalation. Post-fix (D1): migration plan now consistently describes binary MonthlyHalt. Remaining L1/L4 text is explicitly labeled as historical/rejection context. | **Resolved** — D1 |
| **Runtime spec Recovery §Scenario 5 implies UNTRACKED scanning exists** | ~~MEDIUM~~ | Pre-fix: a reader would assume startup reconciliation includes UNTRACKED position detection. Post-fix (D5, D6): both v3-runtime-spec.md and v3-control-loop.md now carry explicit "TARGET ARCHITECTURE — FOLLOW-UP REQUIRED (MIG-v3#9)" labels. A reader will no longer confuse target with current implementation. | **Resolved** — D5, D6 |

---

## 5. Answer: What Is Still Not Ready, and Why?

### VAL-001 Phase 2 post-audit status:

The 2026-04-18 finding that RiskGate blocked BTCUSDT entries due to the legacy
15%/30% exposure caps is resolved in repository state by ADR-0024 / MIG-v3#11.
RiskGate now evaluates monthly-budget slots:

- 1% risk per trade
- 4% monthly budget
- realized-loss-only budget consumption
- latent risk from open positions

Phase 2 is still not passed because the latest code and testnet ConfigMap have not
yet been deployed and exercised against Binance testnet. The valid next action is
operational rollout plus the VAL-001 runbook. A testnet-only exposure exception is
no longer valid or necessary.

### Not ready for VAL-002:

VAL-002 is correctly blocked on VAL-001 PASS. Even if VAL-001 passes, the following must be addressed before real capital activation:

1. **exchange_order_id in order events** (I1) — Without this, reconciliation is blind.
2. **Reconciliation worker** (I3/MIG-v3#9) — Without this, UNTRACKED positions go undetected.
3. **At least one full VAL-001 lifecycle** including fill → trailing stop → exit → PnL → clean state.

### What IS repository-verified (but not operationally validated):

Repository implementation is present for the full lifecycle, but exchange placement, fill, trailing stop, and exit have NOT been validated in VAL-001 after the MIG-v3#11 policy change:

- Arm → detector signal → RiskGate evaluation ✅ (validated in VAL-001 Phase 1)
- EventLog persistence and projection recovery ✅ (repository-verified)
- MonthlyHalt circuit breaker ✅ (repository-verified)
- Trailing stop discrete span algorithm ✅ (repository-verified)
- position_monitor_tick audit events ✅ (repository-verified)
- QueryEngine governance pipeline (QE-P1–P4) ✅ (repository-verified)
- Testnet environment isolation ✅ (validated in VAL-001 Phase 1)
- Bearer token auth on mutating routes ✅ (validated in VAL-001 Phase 1)
- Entry order placement → fill → active monitoring → exit → PnL ⏳ (pending Phase 2 redeploy-and-run)

---

## 6. Proposed Action Sequence (For Codex Review)

1. **Deploy MIG-v3#11 to testnet**: build latest image and sync `rbx-infra` commit `c3b1bc3`.
2. **VAL-001 Phase 2 execution**: Run full lifecycle and record exchange, fill, trailing-stop, exit, and PnL evidence.
3. **Design note for exchange identity** (§7): Resolve open questions before reconciliation implementation.
4. **Exit-side `exchange_order_id` minimal safe patch**: Lower-risk than entry-side. Add field to `ExitOrderPlaced` (emitted after exchange response via `execute_and_persist()`).
5. **Entry-side event ordering fix**: Emit request before exchange call, then placed/failed after exchange acknowledgement.
6. **StartupReconciling gate**: Prevent new decisions until startup exchange state has been checked.
7. **I2: symbol-agnostic test parameterization**: Can parallel with VAL-001.
8. **I3: reconciliation worker**: Largest item. Can start design while VAL-001 runs.
9. **MIG-v3#12 monthly state persistence**: Required before VAL-002.
10. **VAL-001 full PASS**: Including fill → exit → clean state.
11. **I3 completion + testnet validation**: Reconciliation worker tested on testnet.
12. **VAL-002**: Real capital activation.

---

## 7. Design Note: Exchange Order Identity in Domain Events

**Status**: Open questions — requires Codex/operator decision before implementation.

### 7.1 Problem

The reconciliation worker (MIG-v3#9) needs to match exchange positions to `robsond`-authored entries. The match key is `exchange_order_id` — the identifier Binance assigns when it accepts an order. Today, domain events `EntryOrderPlaced` and `ExitOrderPlaced` do NOT carry `exchange_order_id`.

The engine emits `EntryOrderPlaced` via `EmitEvent` BEFORE `PlaceEntryOrder` reaches the executor (lib.rs:418–434). At emission time, the exchange has not yet been called, so `exchange_order_id` is not available.

### 7.2 Open Questions

**Q1: Where should entry exchange identity live?**

Options:
- **A. On `EntryOrderPlaced`**: Requires restructuring the action sequence — emit the event AFTER exchange placement. This means the position stays in `Armed` until the exchange acknowledges, which is more conservative but changes the state machine semantics.
- **B. On `EntryFilled`**: `EntryFilled` is emitted after the executor reports fill, which includes `fill_price` but currently not `exchange_order_id`. This is the earliest point where we have the exchange ID with certainty. Risk: if the order is placed but never fills, we have no domain event linking the position to the exchange order.
- **C. New `OrderAcked` domain event**: Emit a separate `EntryOrderAcked` event after `PlaceEntryOrder` succeeds. This preserves the current `EntryOrderPlaced` semantics ("engine decided to place order") while adding a second event ("exchange acknowledged the order"). More events, but each event has a clear semantic meaning.
- **D. Split `EntryOrderPlaced` into `EntryOrderRequested` + `EntryOrderPlaced`**: Rename current `EntryOrderPlaced` to `EntryOrderRequested` (emitted before exchange). Add new `EntryOrderPlaced` emitted after exchange acknowledgment. This is the cleanest semantic model but requires the most refactoring.

**Q2: How do we avoid partial store mutation if exchange placement fails?**

Current behavior: `EntryOrderPlaced` is persisted and position transitions to `Entering` BEFORE `PlaceEntryOrder` executes. If exchange placement fails:
- Position is stuck in `Entering` with an `EntryOrderPlaced` event.
- No exchange order exists.
- On restart, projection recovery would restore the position as `Entering`.
- The intent journal provides idempotency on retry, but does not provide rollback.

Options:
- **Emit after placement** (options A/C/D): Position stays in `Armed` until exchange confirms. Safer state machine, but requires restructuring.
- **Compensating event**: On exchange failure, emit `EntryOrderFailed` that transitions `Entering → Error`. Current code has `OrderFailed` in `ActionResult` but does not emit a compensating domain event.
- **Accept the risk**: Current behavior. Position stuck in `Entering` until operator intervenes or daemon restart reconciles.

**Q3: What is the minimal safe patch for exit-side `exchange_order_id`?**

`ExitOrderPlaced` is emitted by the executor AFTER the exchange response (via `execute_and_persist()`), so `exchange_order_id` IS available at emission time. The minimal patch:
1. Add `exchange_order_id: Option<String>` to `ExitOrderPlaced`.
2. Populate it from `ActionResult::OrderPlaced.exchange_order_id` in `execute_and_persist()`.
3. Update projector handler for `exit_order_placed` to persist the field.

This is safe because it does not change the action sequence and is purely additive.

**Q4: Migration/backward-compatibility impact?**

- Existing `EntryOrderPlaced` and `ExitOrderPlaced` events in EventLog do NOT have `exchange_order_id`.
- Adding `Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` is backward-compatible: old payloads deserialize without the field.
- The projector handler for these events must handle both old (missing field) and new (present field) payloads.
- The `orders_current.exchange_order_id` column is already populated by connector-level `OrderAcked` events, so the projection table is not affected by this domain-event change.

### 7.3 Superseded Recommendation

> **Superseded by §7.4 (Codex Design Decision, 2026-04-18).** The recommendation below was the pre-decision tentative proposal. The adopted direction is option D from Q1, not option C.

Original tentative proposal:
1. **Short-term**: Apply Q3 minimal safe patch for exit-side only.
2. **Medium-term**: Adopt option C (new `EntryOrderAcked` domain event) for entry-side. This preserves current `EntryOrderPlaced` semantics, avoids restructuring the action sequence, and adds exchange identity at the point where it becomes available.
3. **Action-order fix**: Add compensating `EntryOrderFailed` event for exchange placement failure.

### 7.4 Codex Design Decision (2026-04-18)

**Direction**: Split the entry event model to separate intent from acknowledgment:

1. **Rename `EntryOrderPlaced` → `EntryOrderRequested`**: Emitted BEFORE exchange placement (current `EmitEvent` action). Semantics: "engine decided to place this order — governed intent, not exchange acknowledgment." Position transitions from `Armed` to a pre-`Entering` state (or stays `Armed` with a pending-request flag).

2. **New `EntryOrderPlaced` event**: Emitted AFTER exchange acknowledgment. Carries `exchange_order_id`. Position transitions to `Entering`. This is the event reconciliation will match on.

3. **New `EntryOrderAccepted` event** (optional, for filled-on-place): If Binance fills immediately on placement, emit this with fill details + `exchange_order_id`.

4. **New `EntryOrderFailed` / existing `PositionError` event**: Compensating event if `PlaceEntryOrder` fails after `EntryOrderRequested` was emitted. Transitions position to `Error` state. The position must NOT be left silently stuck in `Entering` after a failed exchange placement.

**Why**: This resolves both the action-order risk (§4.4) and the exchange identity gap (Q1/Q2) in a single design.

**Backward compatibility**: Existing `entry_order_placed` events in EventLog use legacy pre-exchange semantics (emitted before exchange placement, no `exchange_order_id`). A split event model requires explicit backward-compatibility handling:

- Legacy `entry_order_placed` payloads in EventLog represent governed intent, not exchange acknowledgment. The new `EntryOrderPlaced` event carries different semantics (post-exchange, with `exchange_order_id`). Code that replays or projects these events must not silently reinterpret old payloads as exchange-acknowledged orders.
- Implementation must include one of: legacy event-type mapping at the projector/replay boundary, versioned event handling, or an explicit migration plan for existing EventLog entries.
- The `Option<String>` serde default for `exchange_order_id` handles payload-level compatibility but does not address the semantic change. A projector encountering a legacy `entry_order_placed` without `exchange_order_id` must treat it as `EntryOrderRequested`-equivalent, not as a confirmed exchange placement.

**Not yet implemented**: This is a design direction, not a code change. Implementation blocked on operator approval and should follow after VAL-001 Phase 2 unblocks. Exit-only `exchange_order_id` (Q3) remains the safe short-term patch.
