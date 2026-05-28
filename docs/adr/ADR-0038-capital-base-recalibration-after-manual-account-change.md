# ADR-0038 — Capital Base Recalibration After Manual Account Change

**Date**: 2026-05-28
**Status**: DECIDED — IMPLEMENTATION REQUIRED
**Deciders**: RBX Systems (operator + architecture)

---

## Context

`capital_base` is the anchor for Robson's monthly risk model:

- per-trade risk is `capital_base * 1%`;
- monthly budget is `capital_base * 4%`;
- slot availability is derived from remaining monthly budget.

ADR-0024 made `capital_base` a month-start snapshot. That is correct while the
operated Binance Futures account is exclusively controlled by `robsond`.

Manual account changes break that assumption. If an operator trades, closes,
deposits, withdraws, or otherwise changes the Futures account outside Robson, the
current wallet balance can become materially lower than the stored monthly
`capital_base`. Continuing to size new positions from the stale value would
overstate risk capacity and can cause oversized entries or margin failures.

Manual trading on the Robson-operated account remains prohibited by ADR-0022.
This ADR defines the required recovery behavior when it nevertheless happens.

---

## Decision

When Robson detects a manual or otherwise non-Robson account change that makes
the current Futures wallet/equity materially diverge from Robson's risk ledger,
Robson MUST recalibrate the current month's `capital_base` before allowing any
new entries.

The recalibrated value is:

```text
new_capital_base = max(0, current_futures_wallet_balance - carried_risk)
```

where `carried_risk` is the same pessimistic risk calculation used at a month
boundary: committed risk from `Entering`/`Active` positions plus reserved risk
for `Armed` positions.

The recalibration is an audit event, not a silent projection update.

---

## Required Runtime Behavior

1. **Detect manual account drift.**
   Reconciliation must compare exchange account state with Robson's event-sourced
   ledger. Triggers include UNTRACKED positions, manual closes, balance changes,
   deposits/withdrawals, and wallet/equity deltas not explained by Robson events.

2. **Block new entries while unresolved.**
   Once manual drift is detected, the entry path must fail closed until the
   drift is reconciled and `capital_base` has been recalibrated.

3. **Close/reconcile positions first.**
   Position-level reconciliation still follows ADR-0022:
   UNTRACKED exchange positions are closed; stale Robson `Active` positions are
   reconciled with real evidence where available.

4. **Recalculate current-month `capital_base`.**
   After position reconciliation, query the current Futures wallet balance and
   compute `new_capital_base = max(0, wallet_balance - carried_risk)`.

5. **Emit a dedicated event.**
   Recalibration must be represented by a new domain event, for example:

   ```text
   CapitalBaseRecalibrated {
     previous_capital_base,
     new_capital_base,
     wallet_balance,
     carried_risk,
     reason,
     evidence,
     month,
     year,
     timestamp,
   }
   ```

   The canonical `reason` for this ADR is `manual_account_change`.

6. **Project into `monthly_state`.**
   The projector updates the current `(year, month)` row's `capital_base` to
   `new_capital_base` while preserving already accumulated `realized_loss` and
   `trades_opened`.

7. **Re-evaluate risk gates.**
   MonthlyHalt, slots, and position sizing must be recomputed against the new
   `capital_base` before any new entry can proceed.

---

## Non-Goals

- This does not permit manual trading on the Robson-operated account.
- This does not adopt manual trades into Robson's governance trail.
- This does not replace month-boundary recalculation; it adds an intra-month
  exceptional recalibration path.
- This does not let manual gains silently increase risk. A recalibration may
  reduce or increase `capital_base`, but it must always be auditable and tied to
  detected account drift.

---

## Consequences

### Positive

- Robson no longer sizes positions from a stale monthly base after manual account
  losses or withdrawals.
- Exchange state and risk policy become consistent before new entries resume.
- Manual account interference remains a visible policy violation with an audit
  trail.

### Negative / Trade-offs

- Reconciliation becomes broader: it must reason about account-level balance
  drift, not only position lifecycle drift.
- `monthly_state.capital_base` is no longer strictly immutable for the whole
  calendar month; it is immutable except for explicit recalibration events.
- Additional UI/API observability is needed so the operator can see when and why
  the monthly base changed.

---

## Relationship To ADR-0037

ADR-0037 defines operational states such as `PAUSED`, `DRAINING`, and `STOPPED`.
This ADR is orthogonal. A future runtime state such as `RECONCILING` or
`DEGRADED` may be used to enforce the entry block while capital recalibration is
pending, but the capital accounting rule belongs to ADR-0024/ADR-0022 and this
ADR.

---

## Implementation Notes

Likely code changes:

- `robson-domain`: add `CapitalBaseRecalibrated` event and typed reason/evidence.
- `robson-projector`: update `monthly_state.capital_base` on the event without
  resetting `realized_loss` or `trades_opened`.
- `robsond`: add account-level drift detection to reconciliation and an entry
  gate that blocks while drift is unresolved.
- `robsond`: reuse month-boundary carried-risk calculation for intra-month
  recalibration.
- API/frontend: expose current `capital_base`, recalibration status, last
  recalibration reason, and timestamp.

---

## Related

- ADR-0022 — Robson-Authored Position Invariant
- ADR-0024 — Trading Policy Layer
- ADR-0037 — Runtime State Machine for Operational Control
- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`
