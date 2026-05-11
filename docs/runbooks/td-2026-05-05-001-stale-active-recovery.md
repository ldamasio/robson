# Stale-Active Recovery (TD-2026-05-05-001)

**Severity**: Critical
**Time to Execute**: 10–30 min per affected position (steady state); up to 60 min on first incident
**Required Access**: `kubectl` for `robson` and `robson-testnet` namespaces, Binance Futures account access (web UI or `binance-cli`), `robsond` API token, `robson-cli` binary at the version that ships Slice 5B1
**Status**: Slice 5B1 LIVE — operator-driven manual recovery available via `robson-cli reconcile-close`. Slice 5B2A merged (evidence helper refactor, no operational change). Startup `auto_reconcile` is Slice 5B2B (planned, not yet live).

---

## Run Log

| Date | Executor | Result | Notes |
|------|----------|--------|-------|
| _(no executions yet — runbook is a Slice 2 skeleton)_ | | | |

---

## Startup Abort — What Happened (Slice 5A)

When the daemon refuses to start with exit code 78, the log contains:

```
CRITICAL: Startup gate: Robson-Active position absent from exchange
  position_id=<UUID> symbol=<PAIR> side=<Long|Short> quantity=<DECIMAL>
...
Startup aborted: N stale-active position(s) detected (exit 78 — see runbook ...)
```

**What this means**: At startup, after restoring the in-memory position store,
the daemon compared every local `Active` position against the exchange's open
positions. At least one `Active` position was NOT found on the exchange by
`(symbol, side)`. The daemon refused to enter the control loop (fail-closed).

**What this does NOT mean**:
- It does NOT close the position. The store is unchanged.
- It does NOT invent PnL or evidence.
- `Entering` and `Exiting` positions are excluded from this check (they do not
  trigger abort).

**Immediate action** (before any evidence collection):
1. `kubectl logs -n <ns> deploy/robsond --tail=200 | grep "CRITICAL\|stale-active"`
   — identify the affected `position_id`(s).
2. Do NOT restart the daemon until the affected position is resolved (see below).

---

## Symptoms

This runbook fires when ONE of the following is true:

1. **Daemon refused to start** with the message `Startup aborted: N stale-active position(s) detected`. Exit code 78 (`EX_CONFIG`). Logged at `CRITICAL`. *(Live since Slice 5A.)*
2. **Steady-state alert** `position_reconciliation_estimated_evidence_required` fired (Slice 4+ alerting layer): the runtime reconciliation worker confirmed drift but only `Estimated` evidence is available, so the close was deferred for operator confirmation.
3. **Operator-initiated** verification: `/status` shows an `Active` position whose `(symbol, side)` is not present in `binance-cli futures positions` or the Binance Futures web UI for the operated account.

If the trigger is anything else (UNTRACKED on the exchange, DivergentQuantity, MonthlyHalt, panic close failed, etc.), this is the **wrong runbook**. See [val-002-real-capital-activation.md](val-002-real-capital-activation.md) §Safety Checks for the index of safety paths.

---

## Safety Principle

> **Every reconciled close MUST carry exchange-grade evidence. Estimated PnL is the floor, never the default. Capital safety > delayed detection: if you must choose, abort startup and stay paused.**

Concretely:

- A reconciled close that uses `Estimated` evidence pollutes monthly accounting. The pollution is bounded (single position, conservative price) and is preferable to leaving an `Active` ghost forever, but it is **operator-confirmed**, never silent.
- The startup gate is `abort` by default for exactly this reason. `auto_reconcile` is opt-in after this runbook has been exercised at least once on testnet.
- If the operator has any doubt about which evidence applies, prefer the more conservative path (lower exit price for Long, higher for Short). The audit trail records every choice.

---

## Preconditions

- [ ] You can read the cluster: `kubectl config current-context | grep -E '(robson|robson-testnet)'`.
- [ ] You can read `robsond` logs: `kubectl logs -n <ns> deploy/robsond --tail=500`.
- [ ] You have Binance Futures account access for the operated account (NOT a side account). Required for evidence collection (steps below).
- [ ] You have at least one of: a logged `insurance_stop_id` for the affected position (look in `robsond` logs near the last successful reconciliation), or access to `GET /fapi/v1/userTrades` for the affected symbol.
- [ ] You are authorized to issue terminal events for the position. In production, that means: operator on call AND a second pair of eyes (paired execution) for any close that requires `Estimated` evidence.
- [ ] You have the implementation guide open in a separate window: [`TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md`](../implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md). It is the source of truth on evidence shapes and event payloads.
- [ ] (Slice 5+) `robson-cli reconcile-close --help` returns a usage message. If it errors with `unknown command`, the runtime is older than Slice 5; STOP and escalate.

---

## Evidence Collection Order

Walk the four sources in order. **Stop at the first source that yields a confirmed answer.** Record the answer; you will hand it to `robson-cli reconcile-close` (Slice 5+).

### 1. `OrderFillRecord` — preferred

**When to use**: the daemon logged an `insurance_stop_id` for the affected position before drift, or you have any candidate exchange `orderId` that could have closed the position.

**How to gather** (manual, until Slice 3 wraps `get_order_by_exchange_id`):

```bash
# Find the candidate insurance_stop_id in robsond logs
kubectl logs -n <ns> deploy/robsond --since=24h \
  | grep -E "(insurance_stop|<position_id>)" \
  | tail -50

# Query the exchange directly (requires Binance API access)
# GET /fapi/v1/order?symbol=<SYM>&orderId=<ID>
# (Use binance-cli or the operator's REST helper.)
```

**What to capture**:

- `exchange_order_id` (string)
- `fill_price` (decimal, exchange-reported)
- `filled_quantity` (decimal)
- `fee` (decimal)
- `fee_asset` (string, e.g. `USDT`)
- `filled_at` (ISO-8601, UTC)
- Order status MUST be `FILLED`. If `CANCELED`, `EXPIRED`, or `NEW`, this is NOT valid `OrderFillRecord` evidence — fall through to step 2.

### 2. `UserTradeRecord` — when no candidate order id is known

**When to use**: the operator closed the position manually on the Binance UI (which produces a market order Robson never knew about), or any other case where there is no candidate `orderId` but a per-symbol trade history covers the drift window.

**How to gather**:

```bash
# Determine the search window: from the last live tick robsond saw
# (look in logs for the latest position_monitor_tick or trailing_stop_updated)
# to "now" — typically 1–6 hours.

# GET /fapi/v1/userTrades?symbol=<SYM>&startTime=<MS>&endTime=<MS>
# Filter trades whose `side` matches the close side for the position
# (Long position closes via SELL, Short closes via BUY).
```

**What to capture**: same fields as `OrderFillRecord` plus `exchange_trade_id`. If there are multiple candidate trades, pick the one whose timestamp is closest to the last live tick AND whose `qty` matches the position quantity (within tolerance).

### 3. `AccountSnapshot` — fill data unavailable

**When to use**: rate-limited from `userTrades`, history outside the API window, or the exchange does not surface the fill (rare, but happens for some liquidation paths). The position is provably zero on the exchange but no fill price is available.

**How to gather**:

```bash
# Two consecutive snapshots, separated by at least the grace period
# (default 60s, see config).
# GET /fapi/v2/positionRisk?symbol=<SYM>
# Confirm `positionAmt` is "0" in both snapshots.

# Optional: capture wallet balance delta.
# GET /fapi/v2/account
# Save `availableBalance` from each snapshot.
```

**What to capture**:

- `first_observed_missing_at` (ISO-8601)
- `confirmed_missing_at` (ISO-8601)
- `futures_balance_delta` (decimal, `confirmed.availableBalance - first.availableBalance`) — optional, use when reasonable.

### 4. `Estimated` — last resort, alarmed

**When to use**: ALL of (1), (2), and (3) are unobtainable. Network outage, prolonged exchange downtime, or the drift was discovered too late.

**MANDATORY SIGN-OFF**: paired execution. A second authorized operator must confirm in writing (PR comment, Slack `#ops`, or audit channel) the choice of `estimation_basis` and the resulting `exit_price` BEFORE the close is issued.

**What to capture**:

- `estimation_basis` — one of:
  - `TrailingStopAtDetection` (preferred — use the trailing stop price from the last `Active` projection)
  - `ExchangeMarkPrice` (use Binance mark price at detection time)
  - `LastObservedPrice` (use the last `position_monitor_tick.price`)
- `exit_price` (decimal, computed from `estimation_basis`)
- `evaluator` (string, e.g. `"operator:ldamasio+kbenitez"` for paired sign-off)
- `detected_at` (ISO-8601)

`Estimated` closes will increment `robson_reconciliation_estimated_closes_total` and emit `CRITICAL` to the alerting channel. Both are intentional.

---

## Manual Verification Checklist

Before issuing the close, every operator MUST tick all of:

- [ ] The exchange position for `(symbol, side)` is **definitively gone**. Check `binance-cli futures positions` AND the web UI under the same account credentials. Two-source confirmation only.
- [ ] No outstanding orders for `(symbol, side)` are still open on the exchange (`binance-cli futures open-orders`). If yes, cancel them first or escalate.
- [ ] The `position_id` you are about to close matches the position you observed missing — copy from `/positions/:id`, do not retype from memory.
- [ ] The `quantity` recorded locally matches the quantity that was on the exchange when last seen (or that the evidence proves was filled).
- [ ] The `evaluator` string in `Estimated` evidence (if used) names every operator who reviewed the call.
- [ ] You have read the [policy I3 §C and §D](../policies/UNTRACKED-POSITION-RECONCILIATION.md) within the last 24 hours OR have memorized the evidence ordering rule.

---

## Recovery Command (Slice 5B1 — operator-driven manual path)

> **Only `order_fill_record` and `user_trade_record` evidence are accepted.**
> `account_snapshot` and `estimated` are not supported in Slice 5B1. They will
> land in a future slice.

### Command

```bash
robson-cli reconcile-close \
  --position-id <UUID> \
  --evidence-file evidence.json \
  --robsond-url http://localhost:8080 \
  --token "$ROBSON_API_TOKEN"
```

Flags:

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `--position-id` | Yes | — | UUID of the position to close |
| `--evidence-file` | Yes | — | Path to JSON file with evidence |
| `--robsond-url` | No | `http://localhost:8080` | Base URL of robsond API |
| `--token` | No | `$ROBSON_API_TOKEN` env | Bearer token for auth |

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success, position closed |
| 1 | Generic error (network, parse) |
| 2 | Usage error or evidence locally rejected |
| 3 | Position not found (404) |
| 4 | Position not Active (409) |
| 5 | Evidence inconsistent (400) |
| 6 | Unauthorized (401) |

### Sample evidence.json — OrderFillRecord

```json
{
  "source": "order_fill_record",
  "data": {
    "exchange_order_id": "12345678",
    "fill_price": "95000.50",
    "filled_quantity": "0.010",
    "fee": "0.95",
    "fee_asset": "USDT",
    "filled_at": "2026-05-09T14:30:00Z"
  }
}
```

### Sample evidence.json — UserTradeRecord

```json
{
  "source": "user_trade_record",
  "data": {
    "exchange_order_id": "12345678",
    "exchange_trade_id": "87654321",
    "fill_price": "95000.50",
    "filled_quantity": "0.010",
    "fee": "0.95",
    "fee_asset": "USDT",
    "filled_at": "2026-05-09T14:30:00Z"
  }
}
```

### What happens

1. CLI validates evidence shape locally (rejects `account_snapshot` and `estimated`).
2. CLI sends `POST /reconcile-close` to `robsond` with the evidence.
3. API validates Bearer token, deserializes evidence, calls `PositionManager::reconcile_close()`.
4. `reconcile_close()` checks position is `Active`, validates evidence consistency, emits `PositionClosed { exit_reason: ReconciledMissingOnExchange }`.
5. Eventlog → projector → position becomes `Closed`.
6. CLI prints `realized_pnl` and `exit_price`.

### Not yet supported

- `account_snapshot` evidence — rejected at CLI and API level.
- `estimated` evidence — rejected at CLI and API level.
- Startup `auto_reconcile` — Slice 5B2, future work.

---

## Recovery Paths Summary

| Path | Label | Status | How |
|------|-------|--------|-----|
| A | Startup abort (fail-closed) | **LIVE** (Slice 5A) | Daemon exits 78; use Path B to resolve manually, then restart. |
| B | Operator-driven manual close | **LIVE** (Slice 5B1) | `robson-cli reconcile-close` + `POST /reconcile-close`. |
| C | Startup `auto_reconcile` | **PLANNED** (Slice 5B2B) | Opt-in config; not live yet. See below. |

### Path C — Startup `auto_reconcile` (planned, Slice 5B2B)

> **DO NOT enable in production.** No functional `auto_reconcile` config exists
> until Slice 5B2B is merged. Setting `on_startup_stale_active = "auto_reconcile"`
> today produces a config error.

When Slice 5B2B ships, Path C will work as follows:

- Enabled via `ROBSON_RECONCILIATION_ON_STARTUP_STALE_ACTIVE=auto_reconcile`.
- **Two-phase / all-or-nothing**: the daemon first collects evidence for ALL
  stale-Active positions, then applies `reconcile_close` to all only if every
  position has real evidence (`OrderFillRecord` or `UserTradeRecord`).
- If any position lacks real evidence → abort with exit 78. No position is closed.
- `Estimated` evidence at startup always downgrades to abort — never auto-closes.
- **Current fallback**: use Path B (manual) until 5B2B is merged and validated
  on testnet (Slice 5B2C drill).

---

## Post-Recovery Validation

After the close (whether via the CLI or via a Slice 5 `auto_reconcile` startup phase), verify:

- [ ] `kubectl logs -n <ns> deploy/robsond --tail=200 | grep <position_id>` shows `PositionClosed` emitted with the chosen `closure_evidence`.
- [ ] `curl -s -H "Authorization: Bearer $ROBSON_TOKEN" http://localhost:8080/positions/<id>` returns `state: "closed"` and a non-null `exit_price`.
- [ ] `/status.occupied_slots` decremented by 1 (or matches the new ground truth).
- [ ] `monthly_state.realized_loss` updated (if the close was a loss).
- [ ] `robson_reconciliation_stale_active_total` Prometheus counter incremented; `robson_reconciliation_estimated_closes_total` incremented IFF `Estimated` evidence was used.
- [ ] Daemon restarts (if startup gate fired) come up clean: `kubectl rollout status deploy/robsond -n <ns>`.
- [ ] The policy artifact: an audit entry in this runbook's Run Log table above, with date, executor, evidence source, and outcome.

---

## Rollback / When to Stop

There is **no rollback** for a reconciled close. Once `Event::PositionClosed` is appended to the eventlog and applied to the projection, the position is terminal. Replay is deterministic.

**If, after issuing the close, you discover the close was wrong** (e.g. evidence was misinterpreted, the position was actually still open under a different account), the response is:

1. STOP — do not attempt to "re-open" the closed position. Back-dating an `entry_order_placed` is a policy violation per ADR-0022 and the UNTRACKED policy.
2. ESCALATE — page engineering on the on-call channel.
3. INVESTIGATE — collect the eventlog payload, the evidence used, and the actual exchange state. Treat as a P0 incident.
4. REMEDIATE — at the SOURCE, never via post-hoc event manipulation. If a new entry is genuinely required, arm a fresh position through the normal governance gate.

**When to stop the runbook and escalate without acting**:

- Two-source exchange confirmation disagrees (web UI says one thing, REST says another).
- The `position_id` shown in logs differs from the `position_id` shown in `/positions/:id`.
- You cannot find ANY evidence in sources (1)–(3) AND there is no second authorized operator available to sign off on `Estimated`.
- The Binance API is returning 5xx errors persistently — wait for the exchange to recover before closing on possibly-stale data.

---

## Related Documentation

- [Policy: UNTRACKED-POSITION-RECONCILIATION.md §I3](../policies/UNTRACKED-POSITION-RECONCILIATION.md) — full I3 text
- [ADR-0022 — Robson-Authored Position Invariant](../adr/ADR-0022-robson-authored-position-invariant.md) — symmetric invariants
- [Implementation Guide: TD-2026-05-05-001](../implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md) — slice plan
- [Analysis: 2026-05-08 lifecycle drift repro](../analysis/2026-05-08-lifecycle-drift-repro.md)
- [VAL-002 — Real Capital Activation](val-002-real-capital-activation.md) — production gate (lists this runbook under §Safety Checks once Slice 5 lands)

---

## Changelog

| Date | Change | Author |
|---|---|---|
| 2026-05-08 | Initial skeleton (Slice 2 of TD-2026-05-05-001). Operational structure, evidence ordering, decision flow. CLI command deferred to Slice 5B. | Claude Opus 4.7 |
| 2026-05-09 | Slice 5A: startup abort is live (exit 78). Added §Startup Abort section, updated status and recovery command note. | Claude Sonnet 4.6 |
| 2026-05-09 | Slice 5B1: operator-driven manual recovery via `robson-cli reconcile-close` + `POST /reconcile-close`. OrderFillRecord and UserTradeRecord evidence accepted. AccountSnapshot/Estimated rejected. Exit codes 0-6 documented. | Claude Opus 4.7 |
| 2026-05-11 | Slice 5B2A merged (evidence helper refactor, no operational change). Added Recovery Paths Summary table and Path C (planned) section. Status updated. | Claude Sonnet 4.6 |
