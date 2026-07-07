# ADR-0045 — Income-Ledger Reconciliation; Drift Demoted to Checksum

**Date**: 2026-07-05
**Status**: Decided (fully shipped — hotfix, reconciliation-anchor fix, and the typed income ledger)
**Deciders**: RBX Systems (operator + architecture)

---

## Context

On 2026-07-05, the exchange-side insurance stop closed a short in profit
(~+3.84 USDT net) at 07:50 UTC. The reconciled close stalled
(`no_unambiguous_real_fill_evidence`), leaving the position `Active` in the
book while it was gone on the exchange. At 07:51 the **pure financial drift**
path compared the wallet against `capital_base + robson_month_net`, found an
unexplained +3.84, and recalibrated `capital_base` with reason
`manual_account_change`.

Every step was locally reasonable; the composition was wrong:

1. **Drift became the source of truth for money by accident.** A governed
   trading outcome (an insurance-stop fill) was absorbed through the
   out-of-band path with a wrong label, and the books "balanced" — which
   removed the operational pressure to fix the stuck reconciliation.
2. **The drift guard had a hole.** It skipped recalibration while positions
   were open or armed, but `live_risk_open_positions` deliberately excludes
   Active positions missing on the exchange — precisely the state a position
   is in between an exchange-side close and its reconciled close.
3. **The design does not scale past one symbol.** The wallet balance is one
   scalar; the causes are not. With N positions across N pairs, the residual
   is the sum of every unattributed effect — unreconciled fills on pair A,
   funding on pair B, fees on pair C, an operator transfer — collapsed into a
   single number. "Is this drift correct?" becomes unanswerable by
   construction: it is the wrong aggregation level for accounting.

## Decision

### 1. The typed income ledger is the canonical money decomposition

Ingest the exchange's typed income stream (Binance USD-M:
`GET /fapi/v1/income`) as the canonical record of every balance movement:
`REALIZED_PNL`, `COMMISSION`, `FUNDING_FEE`, `TRANSFER`, and the remaining
types — each carrying symbol, timestamp, and (where applicable) the trade/order
linkage.

Reconciliation matches income items against the governed event log,
item by item:

- `REALIZED_PNL` / `COMMISSION` must map to a fill robsond knows — including
  insurance-stop fills (`ins-` lineage);
- `FUNDING_FEE` maps to the funding tracking;
- `TRANSFER` is, by definition, an operator action — the only category that
  may legitimately recalibrate `capital_base` automatically;
- anything unmatched is a **named, per-item anomaly**, not a scalar mystery.

`expected_wallet_balance` derives from the matched ledger, per symbol. This
answers the multi-pair question directly: attribution happens at the item
level, where the exchange already provides it — never at the wallet-total
level, where it is unrecoverable.

### 2. Drift is a checksum and an alarm — never an accounting writer

`wallet_balance − explained_ledger_sum` should be ~zero at all times. When it
is not:

- raise a loud, persistent alarm listing the unmatched income items (or the
  absence of income records explaining the delta);
- **never** write `capital_base` or any accounting state from an unattributed
  residual;
- automatic recalibration is permitted only when the ledger explains 100% of
  the delta as `TRANSFER`; every other cause requires explicit operator
  confirmation (via the existing recalibration authorization path).

The 2026-07-05 event under this design would have read
`unmatched_income: REALIZED_PNL BTCUSDT +5.31, COMMISSION −1.47 — probable
insurance fill …662172 awaiting reconciliation` instead of
`manual_account_change` — information instead of a guess.

### 3. Interim hotfix (shipped with this ADR)

Until the ledger lands, the pure-financial-drift path is blocked whenever any
book position is in flight — including Active positions missing on the
exchange (`in_flight_count` guard in
`recalibrate_capital_base_after_pure_financial_drift`). A close awaiting
reconciliation is the most likely explanation for a wallet delta; classifying
drift in that window launders governed flow.

Consequence accepted deliberately: while a reconciliation is stuck, the
capital base does not move. That is pressure applied to the right place — the
fix path is resolving the reconciled close, not absorbing its money.

### 4. Rollout

1. **robson**: hotfix now (this PR); income-ledger ingestion + item matching
   as the follow-up implementation (new `IncomePort` on the exchange adapter,
   matching in the reconciliation worker, `/status` exposure of unmatched
   items).
2. **Strategos**: this is the reference pattern for its accounting layer,
   like ADR-0044 for market data. Multi-venue portfolios make the scalar
   version strictly worse there; Strategos adopts item-typed reconciliation
   from day one.
3. **rbx-ledger**: the same principle recorded as a binding rule for the
   internal financial source of truth — reconcile item-by-item against typed
   statements; unexplained residuals alarm and block, they are never
   auto-absorbed as adjustments.

## Failure modes

| Failure | Behavior |
| --- | --- |
| Income endpoint unavailable | Ledger matching pauses; alarm on staleness; no accounting writes; trading unaffected |
| Income item matches nothing governed | Named anomaly, persistent alarm; operator decides (rotation-grade signal if it looks like unauthorized activity) |
| Governed fill has no income item (lag) | Matching retries with backoff; residual alarm carries the pending fill id |
| Residual ≠ 0 with all items matched | Invariant breach — loud alarm, block auto-recalibration, operator review |
| Stuck reconciled close (today's case) | Hotfix guard blocks drift writes; alarm persists until the close resolves |

## Trade-offs — what we are leaving on the table

- **Simplicity**: the scalar drift check was one subtraction; the ledger is
  an ingestion pipeline with matching state. Accepted: the subtraction was
  cheap because it answered the wrong question.
- **Capital-base freshness**: legitimate deposits are recognized slightly
  later (after `TRANSFER` items arrive and match) instead of on the next
  drift scan. Accepted: money appearing without a typed record is exactly
  what must not be trusted quickly.
- **Exchange coupling**: the income types are Binance-specific; the port must
  abstract them for other venues (Strategos). Accepted and contained behind
  `IncomePort`.

## Alternatives considered

### Keep scalar drift, tighten the tolerance (rejected)

Tolerance tuning does not fix aggregation: with multiple pairs the residual
mixes causes at any tolerance, and a labeled-wrong absorption at any size
corrupts the audit trail.

### Reconcile from user-trades only, skip the income ledger (rejected)

Trades explain `REALIZED_PNL` and `COMMISSION` but not `FUNDING_FEE` or
`TRANSFER` — funding alone guarantees a permanent unexplained residual on any
position held across a funding mark, forcing the same bad choice (absorb or
alarm forever) the scalar design has today.

### Block all recalibration, always manual (rejected)

Punishes the one case the exchange types unambiguously (`TRANSFER` deposits/
withdrawals) with operator toil, without adding safety: the typed item IS the
evidence.

## Amendment (2026-07-07) — evidence-gathering anchor bug fixed

A second, distinct bug surfaced on 2026-07-07: `gather_real_evidence` already
had the item-typed fallback this ADR calls for (`gather_order_fill_evidence`
→ `gather_user_trade_evidence`), but it could never *reach* the real closing
trade once too much time had passed.

**Root cause.** An insurance-stop algo order can trigger and be **rejected**
by the exchange (`-2022 ReduceOnly Order is rejected`) when the position it
protects already closed through a different execution. When that happens,
`emit_unresolved` cleared the reconciliation worker's
`missing_observations` entry for the position on every unresolved cycle.
The next cycle then treated the position as *newly* missing and re-anchored
`first_observed_missing_at` to "now" — so the evidence-lookup window
(`observed_at_floor`) passed to `gather_user_trades_since` could never look
back further than one scan interval, no matter how many cycles ran. Once the
real closing trade fell outside that ever-sliding-forward window, it became
permanently unreachable: the position stayed a phantom `Active` ghost,
retried at every market tick (hammering the exchange with rejected
reduce-only exits) and blocking new same-symbol-side entries via the
duplicate-position guard — for 14+ hours until an operator supplied the
`UserTradeRecord` evidence manually via `/reconcile-close`.

**Fix.** `emit_unresolved` no longer clears the observation. The anchor now
persists across unresolved cycles and is cleared only on the two outcomes
that were already correct: the position reappearing on the exchange (false
alarm) or the close actually resolving. Regression test:
`test_unresolved_cycles_preserve_original_first_observed_at`
(`robsond/src/reconciliation_worker.rs`) asserts `first_observed_missing_at`
is byte-identical across repeated unresolved cycles.

This closes the gap between "the fallback evidence path exists" (true since
before this ADR) and "the fallback path can actually find old evidence"
(false until this fix) — the missing half of §1's item-typed reconciliation
promise.

## Amendment (2026-07-07) — §1 typed income ledger implemented

The remaining piece — item-typed reconciliation itself — shipped. New
`IncomePort` trait (`robson-exec/src/ports.rs`), deliberately separate from
`ExchangePort` rather than another flat method on it: `income_ledger.rs`'s
worker is a money-adjacent reconciliation surface that should be
structurally unable to place or cancel orders, and this is the contract
Strategos adopts for its own multi-venue accounting (§4.2). Implemented on
the same concrete exchange types that already implement `ExchangePort`
(`BinanceExchangeAdapter`, `StubExchange`) — no second injected dependency
required anywhere they're already wired.

New `robsond/src/income_ledger.rs` owns poll → ingest → match → alarm,
against a new `income_ledger` table (migration `20240101000020`, idempotent
on the exchange's own `tranId`). `FUNDING_FEE` is always recognized (it
never links to a governed fill by construction — a cost of holding, not a
robsond-authored action); this is the **first-ever attribution of Binance
perpetual funding-rate payments anywhere in robson** — a real, small,
previously-invisible cost (confirmed live 2026-07-07: an open BTCUSDT
position had already paid two funding charges, ~0.056 USDT, entirely
unaccounted for before this change). `TRANSFER` is the only category that
may auto-recalibrate `capital_base`, and only when it explains the wallet
delta 100% with zero other unmatched items in the same window
(`income_ledger::transfer_explains_delta`).
`reconciliation_worker::recalibrate_capital_base_after_pure_financial_drift`
no longer writes `capital_base` for the generic case — it alarms only; the
`in_flight_count` guard from the 2026-07-05 hotfix stays as defense in
depth.

**Known, disclosed matching limitation.** Binance's income items carry a
`tradeId`/`tranId` linkage for `REALIZED_PNL`/`COMMISSION`, but robson does
not yet persist a queryable `exchange_trade_id` on `positions_current` or in
`event_log` payloads for ordinary fills — exact id-level matching isn't
available yet. `REALIZED_PNL`/`COMMISSION` match by (symbol, time proximity
to `entry_filled_at`/`closed_at`) instead: safe (ambiguous candidates stay
unmatched and alarm, mirroring `gather_user_trade_evidence`'s discipline),
but coarser than the exchange's own linkage. The raw `trade_id`/`tran_id` is
preserved on every ledger row regardless, so a future, more precise matcher
can use it without re-ingesting history.

## Related

- [ADR-0024](ADR-0024-trading-policy-layer.md) — capital base semantics
- [ADR-0038](ADR-0038-capital-base-recalibration-after-manual-account-change.md)
  — recalibration path this ADR constrains
- [ADR-0039](ADR-0039-exchange-side-insurance-stop.md) — insurance fills,
  evidence-based reconciled closes
- 2026-07-05 incident: insurance stop gain absorbed as drift (operator
  runbook)
- `rbx-agent-layer/rbx-engineering-guardrails.md` — architecture guardrails
  (failure modes, trade-offs) applied here
