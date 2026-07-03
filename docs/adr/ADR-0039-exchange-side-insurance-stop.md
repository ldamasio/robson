# ADR-0039: Exchange-Side Insurance Stop

Status: Accepted (operator-initiated 2026-07-02; implemented and merged 2026-07-03 via PR #102)
Date: 2026-07-02

## Context

Robson v3 enforces exits purely in software ("Robson manages exits"): the engine
monitors market ticks and, when price crosses the trailing stop
(`should_exit`, `robson-engine/src/lib.rs`), it places a market exit order.
No protective order exists on the exchange while a position is open —
`PositionState::Active.insurance_stop_id` is always `None`.

This design makes stop enforcement conditional on daemon availability.
Production incident (repository-verified via pod logs and `monthly_state`):

- Position `019f10a3-22b7-7093-bb2f-c66a51882bf5` (BTCUSDT Long) entered
  2026-06-28 23:49 UTC at 59696.90, quantity 0.00435116, technical stop
  58888.00 (distance 808.90), budgeted max loss 3.52 USDT (1% of
  capital_base 351.71 — Policy 10 cap).
- `robsond` crashed (`fatal runtime error: stack overflow`) and the position
  went unmonitored for 2726 minutes (~45.4 h) with no order on the exchange.
- On restart (2026-06-30 21:15 UTC), startup recovery replayed 182 candles.
  During the gap the price had completed one full span of profit, so the
  replay legitimately advanced the trailing stop to breakeven (59696.90);
  the price had then fallen through it. Recovery exited at market: fill
  58614.50 — 1082.40 below the breakeven stop the daemon would have
  enforced live. The outage turned a ~breakeven exit into the month's
  full loss.
- Realized June loss recorded in `monthly_state`: 5.2155 USDT = **1.48% of
  capital_base**, violating the 1% per-trade maximum-loss cap (ADR-0024,
  Policy Invariant 10). The entire excess is attributable to the outage gap;
  the exit market order itself filled near the trigger.

The same crash recurred on 2026-07-02 08:28 UTC. Any future crash, node
failure, network partition, or deploy window reproduces this exposure for
whatever position is open at that moment.

## Decision

Place a protective stop order on the exchange for every active position, as
a fail-safe that does not depend on the daemon being alive:

1. **On entry fill**, robsond places a reduce-only `STOP_MARKET` order at the
   technical stop price (the chart-derived stop — unchanged semantics) and
   records its ID in `PositionState::Active.insurance_stop_id`.
2. **On discrete trailing-stop advance** (span/palmo logic), robsond
   cancel-replaces the insurance order at the new stop price, idempotently
   (keyed off `last_emitted_stop`).
3. **The soft monitor remains the primary exit path.** If the software exit
   fills first, robsond cancels the insurance order. If the insurance order
   fills first (daemon down or race), the reconciliation worker closes the
   position from `OrderFillRecord`/`UserTradeRecord` evidence only
   (Policy Invariant 11 unchanged).
4. **Authorship**: the insurance order is placed through a `GovernedAction`
   authored by robsond, so exchange state remains traceable to robsond
   entries (ADR-0022 invariant preserved; the reconciliation worker must
   recognize the insurance order as robson-authored, not UNTRACKED).

Policy compliance:

- **Rule 6 / ADR-0021 untouched**: the stop price is still computed
  exclusively from chart analysis (second support/resistance on the
  15-minute chart). This ADR changes *where the stop is enforced*, never
  *how it is computed*. `stop_loss = entry × (1 − pct)` remains forbidden.
- **Policy 10**: `risk_per_trade_pct = 1%` is a maximum-loss cap. A stop
  market order can still slip in a gap, so position sizing must reserve an
  execution-cost buffer (taker fees entry+exit plus expected gap slippage):
  size such that worst-expected realized loss ≤ 1%. Realized risk lower than
  1% is explicitly acceptable.
- **ADR-0023**: tick size, price filters, and fee rates come from exchange
  metadata at runtime; nothing symbol-specific is hard-coded.

## Consequences

Positive
- Stop enforcement survives daemon crashes, node failures, deploys, and
  network partitions — the incident scenario is structurally closed.
- Gap-below-stop exposure bounded by exchange-side trigger latency instead
  of daemon downtime (hours → market-native).
- Audit story improves: exchange order ID for the protective stop exists
  from entry fill onward.

Negative / Trade-offs
- Order lifecycle complexity: cancel-replace races on trailing updates,
  partial fills, duplicate-exit races between soft exit and insurance stop.
  Mitigation: idempotent client order IDs derived from position ID + stop
  generation; reconcile duplicates via fill evidence.
- Exchange rate limits on frequent stop amendments. Mitigation: discrete
  span logic already quantizes updates.
- A reduce-only STOP_MARKET left behind after a manual/external close would
  linger. Mitigation: reconciliation worker cancels orphaned insurance
  orders (extends ADR-0022 sweep).

## Alternatives

- **A. Keep soft-only stops, harden the daemon (HA, supervision).**
  Rejected as sole measure: single-instance k3s deployment; the crash class
  (stack overflow) recurred within 48 h. Availability work is necessary but
  cannot guarantee coverage.
- **B. STOP_LIMIT instead of STOP_MARKET.** Rejected: in a gap the limit may
  never fill, converting bounded slippage into unbounded loss — worse for a
  maximum-loss-cap policy.
- **C. Exchange-native trailing stop orders.** Rejected: trailing semantics
  (discrete span anchored to entry, chart-derived distance) are Robson
  policy logic and must not be delegated to exchange-side approximations
  (percent-based callbacks would violate Rule 6).

## Implementation Notes

- `robson-domain`: `PositionState::Active.insurance_stop_id` already exists
  (currently always `None`); add stop-order generation counter if needed for
  idempotent cancel-replace.
- `robson-engine`: emit new actions `PlaceInsuranceStop`,
  `ReplaceInsuranceStop`, `CancelInsuranceStop` from `process_entry_fill`,
  trailing-update, and exit paths.
- `robson-exec` / `ExchangePort`: add `place_stop_market_order`
  (reduce-only) and `cancel_order`; wire through executor with intent
  journaling like existing entry/exit orders.
- `robsond` reconciliation worker: recognize insurance orders as
  robson-authored; close positions from insurance-stop fills using
  `OrderFillRecord` evidence; cancel orphaned insurance orders.
- Sizing buffer (Policy 10): adjust `calculate_position_size` to
  `qty = (max_risk − est_fees) / (stop_distance + slippage_allowance)` with
  parameters from exchange metadata. Wide stops keep qty margin-capped as
  today.
- Tests: engine action emission on each lifecycle transition; executor
  cancel-replace idempotency; reconciliation of insurance fill vs soft exit
  race; sizing property test asserting worst-expected loss ≤ 1% of capital.
- Docs to align when implemented: `docs/architecture/v3-control-loop.md`,
  `docs/architecture/v3-runtime-spec.md` (both currently state Robson
  manages exits without exchange-side stops).

## Related operational findings (2026-07-02, tracked separately)

Recorded here for traceability; each needs its own fix, none blocks this ADR:

1. Recurring `stack overflow` crash in robsond (two occurrences ≤ 48 h) —
   root cause not yet identified; the direct trigger of the incident above.
2. `event_log`/`snapshots` partitions ran out at 2026-06; July–October
   partitions created manually on 2026-07-02 via
   `create_event_log_partitions(3)` / `create_snapshot_partitions(3)`.
   The maintenance functions are session-timezone-sensitive (DATE bounds on
   a `timestamptz` partition key); server default TZ (+02) produces
   overlapping bounds — must run with UTC session TZ. Needs automation and a
   timezone-explicit rewrite.
3. `Daemon::initial_month_check()` seeds `last_month_check` with the current
   month, so a restart after the month boundary permanently skips the
   `MonthBoundaryReset` for that month. July 2026 has no `monthly_state`
   row as a result; needs operator remediation and a code fix (seed from
   persisted state, not wall clock).
4. RETRACTED (2026-07-02, same day): `ExitTriggered` logging `stop=59696.90`
   initially looked like a parameter mix-up (value equals the entry price).
   Code review of `startup_recovery::replay_candles` showed the replay had
   legitimately advanced the trailing stop by one full span to breakeven
   (58888.00 + 808.90 = 59696.90) before the exit — the audit fields were
   correct. No fix needed; kept here so the wrong lead is not re-chased.
