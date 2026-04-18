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

## The Two Invariants

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

---

**Violations?** Report immediately. An UNTRACKED position on the operated account is a
P0 incident.
