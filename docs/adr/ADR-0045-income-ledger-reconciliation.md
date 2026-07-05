# ADR-0045 — Income-Ledger Reconciliation; Drift Demoted to Checksum

**Date**: 2026-07-05
**Status**: Decided (hotfix shipped; ledger implementation pending)
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
