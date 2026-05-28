# Untracked Position Reconciliation Policy

**Status**: Active
**Effective Date**: 2026-04-18
**Owner**: Risk Engineering / robsond Runtime
**Version**: 1.0
**Companion ADR**: [ADR-0022 — Robson-Authored Position Invariant](../adr/ADR-0022-robson-authored-position-invariant.md)

---

## Golden Rule

**Every open position on the operated Binance account MUST be the direct result of an
entry authored by `robsond`. Any open position that is not traceable to a `robsond`-authored
entry is unauthorized and MUST be closed.**

This rule applies to spot, margin (isolated and cross), and futures — to every symbol,
not just `BTC/USDT` or `BTC/USDC`. See also the companion
[Symbol-Agnostic Policies](SYMBOL-AGNOSTIC-POLICIES.md).

No exceptions. No workarounds. No shortcuts.

---

## The Four Invariants

The policy is symmetric: the relation between Robson's local lifecycle state
and the exchange's view of the account must hold in both directions. I1/I2
protect the exchange-to-Robson direction (foreign positions get closed). I3
protects the opposite direction (Robson-Active positions whose exchange
counterpart has disappeared get reconciled). I4 protects the account-level
risk base after manual or otherwise non-Robson balance changes.

### I1 — Authorship Invariant

For the Binance account operated by Robson (both testnet and production), the set of
open positions reported by the exchange MUST be a subset of the set of positions whose
entry order was placed by `robsond` under a `GovernedAction` token.

- "Open position" = any position, order, or balance delta whose lifecycle is still
  active on the exchange (`NEW`, `PARTIALLY_FILLED`, `FILLED` with open inventory,
  `PLACED` futures position with non-zero size).
- "Robson-authored" = there exists, in `event_log`, an `entry_order_placed` event
  (or equivalent v3 event) with a `cycle_id` pointing to a `GovernedAction` cleared by
  the Risk Engine, whose exchange order id matches the position's originating order.

### I2 — Reconciliation Invariant

When the reconciliation worker observes an open position that fails I1, the runtime
MUST:

1. Classify the position as **UNTRACKED** (not tracked as `robsond`'s).
2. Record the classification as a `position_untracked_detected` event in `event_log`,
   with evidence: exchange order id, symbol, side, quantity, open timestamp.
3. Alert the operator at severity `CRITICAL`.
4. **Close the UNTRACKED position at market** via the Safety Net close path, tagged
   with exit reason `UNTRACKED_ON_EXCHANGE`.
5. Record `untracked_position_closed` in `event_log` with the resulting fill.

The close is mandatory and non-overridable by configuration flags. An operator may
abort the close only through an explicit, audited panic override (see Rollback below).

### I3 — Reverse Reconciliation Invariant (TD-2026-05-05-001)

> **Robson-Active ⊆ Exchange-Open.** Every position the local store holds in
> `Active` MUST have a corresponding open position on the exchange, matched by
> `(symbol, side)` and within the configured `quantity` tolerance. If the
> position is missing on the exchange after a grace period and a second
> consecutive observation, Robson MUST gather evidence from the exchange and
> transition the local position to `Closed` via reverse reconciliation.

I3 is the symmetric counterpart of I1/I2:

| Direction | Invariant | Symptom | Action |
|---|---|---|---|
| Exchange has, Robson does not | I1 / I2 (UNTRACKED) | Foreign position on operated account | Close at market, tag `UNTRACKED_ON_EXCHANGE` |
| Robson has `Active`, Exchange does not | I3 (stale-Active) | Local lifecycle drift after liquidation, manual close, externally-resident insurance stop fill, etc. | Gather evidence, close locally with `ReconciledMissingOnExchange` |

**Companion documents**:

- [ADR-0022 — Robson-Authored Position Invariant](../adr/ADR-0022-robson-authored-position-invariant.md) — origin of I1/I2; updated 2026-05-08 to reference I3.
- [Implementation guide TD-2026-05-05-001](../implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md) — slice-by-slice plan.
- [Runbook td-2026-05-05-001-stale-active-recovery](../runbooks/td-2026-05-05-001-stale-active-recovery.md) — operator recovery when the startup gate aborts.

#### I3 §A — Scope: `Active` only

For this technical debt, **only `Active` positions are eligible for automatic
reconciliation-close.**

| Local state | Behavior under I3 |
|---|---|
| `Active` | Detected, evidence gathered, closed via `ReconciledMissingOnExchange` |
| `Entering` | Detected, **logged + skipped** (`DaemonEvent::ReconciliationStaleNonActiveDetected`). May be a placement/fill latency race; auto-close risks double-close. Operator-driven path only. |
| `Exiting` | Detected, **logged + skipped**. Already mid-flight to terminal via the legitimate path; auto-close risks double-close. |
| `Armed` / `Cancelled` / `Closed` / `Error` | Out of scope — I3 only matters when the local state asserts an open exchange position. |

A separate technical debt entry will design a safe auto-close for stale
`Entering`/`Exiting` once the `Active` path is proven in production.

#### I3 §B — Detection: grace period and second observation

A single observation that the exchange does not return the expected
`(symbol, side)` is insufficient to declare drift. Causes of false-positive
absence include placement-to-reflection latency, websocket vs REST snapshot
inconsistencies, and exchange maintenance windows.

The reconciliation worker MUST therefore:

1. **First observation** — exchange snapshot does not contain the expected
   `(symbol, side)` while the local state is `Active`. Worker records
   `first_observed_missing_at` for the position id and emits a debug-level
   structured log entry. **No close.**
2. **Grace period** — at least one full reconciliation cycle (default 60 s,
   configurable via `reconciliation.missing_grace_secs`) MUST elapse before
   the second observation is consulted.
3. **Second consecutive observation** — the next eligible cycle observes
   the same absence. Only then is `confirmed_missing_at` recorded and the
   close path invoked. If the position reappears in any cycle between the
   two, the grace state for that position is cleared.
4. **Close** — emit `Event::PositionClosed { exit_reason:
   ReconciledMissingOnExchange, closure_evidence: Reconciled(...), ... }`
   via the same eventlog → projector path used by normal exits.

Idempotency: invoking `reconcile_close` for a position that is already in a
terminal state is a no-op.

#### I3 §C — Evidence ordering, no silent fallback

Every reconciliation-close event MUST carry a `closure_evidence:
ClosureEvidence::Reconciled(ReconciliationEvidence::*)` payload (introduced
in Slice 1 of TD-2026-05-05-001). Evidence sources are tried in **strict
priority order**:

1. **`OrderFillRecord`** — A specific exchange order (typically the resident
   `insurance_stop_id`, but also any candidate exit order whose
   `exchange_order_id` is known to the daemon) was confirmed `FILLED` via
   `GET /fapi/v1/order`. Captures fill price, filled quantity, fee, fee
   asset, filled timestamp, exchange order id. Highest fidelity — used
   whenever obtainable.
2. **`UserTradeRecord`** — Per-symbol user trade history from
   `GET /fapi/v1/userTrades` covering the gap between the last known live
   tick and the missing observation. Used when no specific candidate
   `exchange_order_id` is known (e.g. operator closed the position
   manually via the Binance UI, producing a market order Robson never
   knew about). Captures the same fields plus the originating order id.
3. **`AccountSnapshot`** — Two consecutive `get_all_open_positions()`
   snapshots prove the position is zeroed; no fill data is available.
   Captures `first_observed_missing_at`, `confirmed_missing_at`, optional
   `futures_balance_delta` derived from `get_futures_balance()` between
   the two snapshots. Used when (1) and (2) failed (rate limit, history
   outside API window, network error after retries).
4. **`Estimated`** — Last resort. Captures `estimation_basis`
   (`TrailingStopAtDetection`, `ExchangeMarkPrice`, or `LastObservedPrice`),
   the chosen `exit_price`, optional `evaluator` identity, and
   `detected_at`.

#### I3 §E — Startup policy and operational status

**Current operational state (as of 2026-05-11):**

- I3 runtime steady-state reconciliation (worker loop) is **live** (Slice 4B).
- Startup default is **fail-closed abort** — exit code 78 (Slice 5A).
- Operator-driven manual recovery is **live** via `robson-cli reconcile-close`
  and `POST /reconcile-close` (Slice 5B1). Accepts `OrderFillRecord` and
  `UserTradeRecord` only.
- Startup `auto_reconcile` is **planned** (Slice 5B2B) and is **not yet live**.

**Rules for startup `auto_reconcile` (when it ships):**

- Opt-in only: default remains `abort`.
- Two-phase / all-or-nothing: collect evidence for all stale-Active positions
  first; apply closes only if every position has real evidence.
- Only `OrderFillRecord` or `UserTradeRecord` may auto-close positions at
  startup. `AccountSnapshot` and `Estimated` do not qualify for startup
  auto-close.
- If any position lacks real evidence, abort with exit 78 (same as `abort`
  policy). No partial close.

Until Slice 5B2B is merged and validated via testnet drill (Slice 5B2C), use
the operator manual path (Slice 5B1) for all startup-gate scenarios.

#### I3 §D — Estimated evidence: never silent

A reconciliation-close that reaches the `Estimated` branch MUST produce a
visible audit trail and an operator alert:

- The realized PnL derived from `Estimated` evidence is flagged as
  estimated, not real. Downstream consumers (monthly accounting,
  dashboards) MUST surface this provenance distinctly. Estimated PnL
  feeds into `monthly_state.realized_loss` because conservatism is the
  correct default — but every estimated close is auditable.
- A `CRITICAL` operator alert is emitted on every estimated close
  (`position_reconciled_with_estimated_evidence`). The Prometheus counter
  `robson_reconciliation_estimated_closes_total` increments on each
  occurrence.
- At startup, an `Estimated`-only close path **never runs automatically**
  even when `reconciliation.on_startup_stale_active = "auto_reconcile"`.
  The startup phase aborts with the same exit code as the default
  `abort` policy and defers the close to the operator runbook.
- The runtime steady-state path may produce `Estimated` closes
  automatically (capital safety > delayed detection), but the alert and
  metric are mandatory and non-overridable.

`Estimated` is never silently substituted for a real fill in any field.
The on-wire `Event::PositionClosed.closure_evidence.kind` is `reconciled`
and the inner `source` is `estimated`; nothing about the JSON shape lets a
downstream consumer mistake estimated PnL for a real exchange fill.

### I4 — Capital Base Recalibration Invariant (ADR-0038)

> **Manual account drift invalidates the current risk base.** If Robson detects
> a manual or otherwise non-Robson change to the operated Futures account that
> makes the exchange wallet/equity diverge materially from Robson's monthly
> risk ledger, Robson MUST block new entries and recalculate the current
> month's `capital_base` from the current Futures wallet balance before
> trading resumes.

I4 is account-level reconciliation. I1/I2/I3 reconcile positions; I4 reconciles
the monthly risk base used for position sizing and MonthlyHalt.

Required behavior:

1. Detect account-level drift during reconciliation.
2. Finish position-level reconciliation first: close UNTRACKED positions and
   reconcile stale Robson `Active` positions according to I2/I3.
3. Compute `new_capital_base = max(0, current_futures_wallet_balance - carried_risk)`.
4. Emit a dedicated `CapitalBaseRecalibrated` event with previous base, new
   base, wallet balance, carried risk, reason, evidence, month, year, and
   timestamp.
5. Project the event into `monthly_state.capital_base` without resetting
   `realized_loss` or `trades_opened`.
6. Recompute slots, position sizing, and MonthlyHalt using the recalibrated
   value before any new entry proceeds.

The canonical reason for this path is `manual_account_change`. Recalibration
does not legitimize manual trading on the operated account and must not create
synthetic Robson-authored entry history for manual trades.

---

## Why This Matters

1. **Governance is meaningless if bypassable.** Robson's central guarantee is that every
   trade passes the Risk Engine. A position opened outside `robsond` has not been sized
   against the 1% per-trade rule, has no technical stop, has no span, and is not counted
   in the monthly drawdown calculation. Leaving it open corrupts every subsequent risk
   decision.
2. **The EventLog is the source of audit truth.** An open position without a matching
   `entry_order_placed` event breaks replay, reconciliation, and post-mortem analysis.
3. **Operator safety.** A human-opened position on the same account silently consumes
   margin and exposure budget that `robsond` assumes is free. Under leverage this
   cascades into unintended liquidations.
4. **No side accounts.** This policy eliminates the pattern of "I'll just place one
   trade manually" — the account is owned by the daemon. Manual trading requires a
   separate account not connected to `robsond` credentials.

---

## Scope

The policy applies to **every Binance account whose credentials are configured for
`robsond`** — this explicitly includes:

| Environment | Credential source | In scope? |
|---|---|---|
| `robson-testnet` | `robsond-testnet-secret` | Yes |
| `robson` (production) | `robsond-secret` from `pass rbx/robson/*` | Yes |
| Per-tenant client credentials (future) | `Client.set_credentials()` | Yes, per-client |

**Out of scope**: accounts Robson does not operate. If the operator wishes to trade
manually, they must use an account whose API keys are **never** loaded by `robsond`.

The policy applies uniformly to **any symbol** the exchange supports (see
[Symbol-Agnostic Policies](SYMBOL-AGNOSTIC-POLICIES.md)). The reconciliation worker
must not special-case `BTCUSDT`, `BTCUSDC`, or any other pair.

---

## Detection

### Reconciliation Worker

The runtime runs a periodic **Position Reconciliation Worker** (recommended interval:
60s, max 300s; runs unconditionally, not gated by `ROBSON_POSITION_MONITOR_ENABLED`).

Each cycle:

1. Query Binance for all open positions / non-zero balances across all account types
   (spot, margin isolated, margin cross, futures) — for every symbol, not just tracked
   symbols.
2. For each open position, look up the matching `entry_order_placed` event in
   `event_log` by exchange order id.
3. If no matching event exists → the position is **UNTRACKED**.
4. If a matching event exists but it references a different symbol, side, or size
   outside exchange tolerance → the position is **DIVERGENT** and is handled by the
   standard `ReconciliationEvent` flow (not by this policy).

### Startup Reconciliation

On daemon startup (after `RuntimeState::replay_from_log()`), a reconciliation pass
MUST run **before** the Control Loop begins accepting observations. If any UNTRACKED
position is detected at startup, the daemon enters `StartupReconciling` state and
closes the UNTRACKED positions first; new entries are blocked until the set is empty.

---

## Enforcement

The runtime MUST enforce I1/I2 with at least the following mechanisms:

1. **`GovernedAction` boundary** (already in place): no `EngineAction::PlaceEntryOrder`
   may reach the Executor without Risk Engine clearance (see
   `docs/architecture/v3-runtime-spec.md` — Zero-Bypass Guarantee).
2. **Exchange order-id ↔ event-log link** (follow-up required): every order placed via
   the Executor MUST be recorded in `event_log` with the exchange-assigned order id
   indexed for O(1) lookup by the reconciliation worker.
3. **Reconciliation worker close path** (follow-up required): a dedicated Safety Net
   path for closing UNTRACKED positions. This path does not consult the entry-side
   risk gate (closing is always permitted) but does emit full audit events.
4. **Alerting** (follow-up required): `position_untracked_detected` triggers a
   `CRITICAL` alert to the operator channel with the close outcome.

---

## Operator Workflow

### Normal Operation

The operator does not manually open positions on the Robson-operated account. All
entries go through:

- `arm` a position (REST API or CLI)
- Detector signal (MA crossover or other configured strategy) produces a
  `DetectorSignal`
- `QueryEngine` clears the entry through the Risk Engine
- Executor places the entry order
- `event_log` records `entry_order_placed` with the exchange order id

### If The Operator Needs To Experiment

Use an account whose credentials are not loaded by `robsond`. Credentials that enter
`robsond-secret` or `robsond-testnet-secret` are considered under daemon authority.

### If An UNTRACKED Position Is Detected

- The reconciliation worker closes it automatically. Do not attempt to "adopt" an
  UNTRACKED position into the event log. Back-dating an `entry_order_placed` event for
  a position the operator opened by hand is an audit-integrity violation.
- Post-incident: investigate how the UNTRACKED position was opened (leaked API key?
  shared account? forgotten script?) and remediate at the source.

---

## Prohibited Practices

The following are **STRICTLY FORBIDDEN**:

### ❌ Manual Orders Via Binance UI/CLI On The Operated Account

The account key loaded into `robsond` is for `robsond` only. Placing manual orders
via Binance website, mobile app, or a side script is a policy violation even if the
order "looks correct".

### ❌ Post-Hoc Event Back-Filling

```python
# NEVER DO THIS
event_log.append({
    "event_type": "entry_order_placed",
    "exchange_order_id": "123456",  # order placed manually earlier
    ...
})
```

Inventing a governance trail for a trade that never passed the Risk Engine breaks
every downstream guarantee.

### ❌ Configuring A Symbol Whitelist To Hide An UNTRACKED Position

The reconciliation worker does not use the `allowed_symbols` whitelist when scanning
for UNTRACKED positions. Attempting to "scope out" a symbol to make the
reconciliation worker ignore an UNTRACKED position is a policy violation.

### ❌ Disabling The Reconciliation Worker

`ROBSON_POSITION_MONITOR_ENABLED` gates the **trailing-stop / active-position monitor**.
It does not gate the reconciliation worker. The reconciliation worker is always on.

---

## Approved Workflow For "I Want To Hold A Different Asset"

If the operator wants to hold a long-term position in a symbol Robson does not trade:

1. Transfer the asset out of the Robson-operated account to a separate wallet or
   account.
2. Robson does not see it and does not touch it.
3. Never keep long-term holdings on the account Robson operates — the reconciliation
   worker will close them.

---

## Testing Requirements

VAL-001 (testnet E2E) and VAL-002 (real capital activation) MUST include a dedicated
check for this policy:

- VAL-001 Phase 0 (pre-flight): assert `GET /status` reports zero open positions on
  testnet AND reconciliation worker scan returns zero UNTRACKED positions.
- VAL-002 safety checks before flip: identical assertion on production.

A new `UNTRACKED_DETECTION` scenario should be added to VAL-001 (follow-up required):
manually open a position via Binance testnet UI, then confirm the reconciliation
worker classifies it as UNTRACKED, emits the expected events, and closes it within
one reconciliation interval.

---

## Abort / Panic Override

The close path is mandatory. The only permitted override is an operator-issued
panic suspension:

```bash
curl -s -X POST http://localhost:8080/reconciliation/suspend \
  -H "Authorization: Bearer $ROBSON_TOKEN" \
  -d '{"reason": "<auditable reason>", "ttl_seconds": 300}'
```

- Max TTL: 300 seconds. Auto-resumes after TTL expires.
- Emits `reconciliation_suspended` event with reason and operator identity.
- While suspended: no automatic close, but reconciliation scans continue and
  `position_untracked_detected` events are still emitted.

**The suspension endpoint is a v3 target, not a current feature.** Until implemented,
the reconciliation close is non-overridable.

---

## Related Documentation

- **[ADR-0022 — Robson-Authored Position Invariant](../adr/ADR-0022-robson-authored-position-invariant.md)**
- **[ADR-0038 — Capital Base Recalibration After Manual Account Change](../adr/ADR-0038-capital-base-recalibration-after-manual-account-change.md)**
- **[SYMBOL-AGNOSTIC-POLICIES.md](SYMBOL-AGNOSTIC-POLICIES.md)**
- [v3-runtime-spec.md](../architecture/v3-runtime-spec.md) — Recovery Procedures §Reconciliation
- [v3-control-loop.md](../architecture/v3-control-loop.md) — Crash Recovery §Reconciliation
- [v3-risk-engine-spec.md](../architecture/v3-risk-engine-spec.md)
- [VAL-001 — Testnet E2E Validation](../runbooks/val-001-testnet-e2e-validation.md)
- [VAL-002 — Real Capital Activation](../runbooks/val-002-real-capital-activation.md)

---

## Changelog

| Version | Date | Changes | Author |
|---------|------|---------|--------|
| 1.0 | 2026-04-18 | Initial policy creation | Engineering Team |
| 1.1 | 2026-05-08 | Added §I3 — Reverse Reconciliation Invariant (TD-2026-05-05-001). | Claude Opus 4.7 |
| 1.2 | 2026-05-11 | Added §I3 §E — startup policy and operational status. Reflects Slices 5A/5B1 live, 5B2A merged (refactor), 5B2B planned. | Claude Sonnet 4.6 |

---

**Violations?** Report immediately. An UNTRACKED position on the operated account is a
P0 incident.
