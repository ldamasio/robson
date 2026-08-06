# Stale-Active Recovery (TD-2026-05-05-001)

**Severity**: Critical
**Time to Execute**: 10–30 min per affected position (steady state); up to 60 min on first incident
**Required Access**: `kubectl` for `robson` and `robson-testnet` namespaces, Binance Futures account access (web UI or `binance-cli`), `robsond` API token, and a reviewed `robson-cli` binary compatible with the deployed daemon
**Status**: Manual `reconcile-close` and startup `auto_reconcile` are implemented. Production configuration was read-only verified as `auto_reconcile` on 2026-08-06; verify live state again before an incident action. The CLI is not packaged in the runtime image.

---

## Run Log

| Date | Executor | Result | Notes |
|------|----------|--------|-------|
| _(no execution is recorded in this file; production policy notes prior manual recoveries whose incident artifacts are external)_ | | | |

---

## Startup Stale-Active Behavior

After restoring positions, the daemon compares each local `Active` position
with exchange open positions by `(symbol, side)`. `Entering` and `Exiting`
positions are excluded.

Two startup policies exist:

- `abort` is the code default. A stale-Active position produces exit code 78
  without changing the store.
- `auto_reconcile` is opt-in. It gathers real exchange evidence before any
  reconciliation close. Production configuration was read-only verified as
  `auto_reconcile` on 2026-08-06, but live configuration must be checked again.

**Known policy drifts**:

- When `auto_reconcile` finds a stale-Active position but no unambiguous real
  evidence, current source logs a warning, returns success, and continues
  startup for periodic reconciliation instead of exiting 78.
- Phase 2 persists each reconciliation close sequentially. A later apply failure
  stops the batch but does not roll back an earlier persisted close, so the
  policy's no-partial-close guarantee is not mechanically enforced.

Treat either condition as an unresolved critical incident and escalate. Verify
each affected position from persisted events; do not infer resolution from the
process exit alone.

**Immediate action**:

1. Inspect recent logs for `stale-active` and `Startup auto-reconcile`.
2. Identify every affected `position_id` and whether a `PositionClosed` event
   was actually persisted.
3. Do not issue a manual close until the evidence and deployed revision receive
   an independent review.

---

## Symptoms

This runbook fires when ONE of the following is true:

1. **Startup reported stale-Active state**: exit 78 under `abort`, an evidence-backed close under `auto_reconcile`, or the known no-evidence warning where current source continues startup.
2. **Steady-state unresolved event**: logs contain `Reverse reconciliation stale Active unresolved`, or `/status.reconciliation_blockers[]` reports the affected position because the worker could not gather unambiguous real fill evidence.
3. **Operator-initiated** verification: `/status` reports `stale_active_count > 0` or a non-empty `reconciliation_blockers[]` list. The affected position may be omitted from `positions[]` so operators do not count it as live capacity, but it remains a blocker until reconciled.

If the trigger is anything else (UNTRACKED on the exchange, DivergentQuantity, MonthlyHalt, panic close failed, etc.), this is the **wrong runbook**. See [val-002-real-capital-activation.md](val-002-real-capital-activation.md) §Safety Checks for the index of safety paths.

### Projection-only orphan path

If the affected `position_id` exists only in `positions_current` and has no row
in canonical `positions`, no related `orders`, and no lifecycle `events`, do not
run `reconcile-close`: there is no honest exchange-grade close evidence to
attach. Treat it as a projection repair. The permitted write is deleting that
single `positions_current` row inside a transaction after rechecking the three
negative predicates in the same statement. Record the operator, timestamp, and
SQL output in the incident log.


---

## Safety Principle

> **Every manual or startup reconciliation close MUST carry exchange-grade evidence. No evidence means no close.**

Concretely:

- Manual `reconcile-close` accepts `OrderFillRecord` or `UserTradeRecord` only.
- The code default is `abort`; production currently opts into `auto_reconcile`.
- Startup may close only when real evidence is unambiguous for the required batch.
- The current no-evidence continue behavior is documented drift, not proof of resolution.
- If evidence is doubtful or ambiguous, stop and escalate. Do not estimate a fill for this command.

---

## Preconditions

- [ ] You can read the cluster: `kubectl config current-context | grep -E '(robson|robson-testnet)'`.
- [ ] You can read `robsond` logs: `kubectl logs -n <ns> deploy/robsond --tail=500`.
- [ ] You have Binance Futures account access for the operated account (NOT a side account). Required for evidence collection (steps below).
- [ ] You have at least one of: a logged `insurance_stop_id` for the affected position (look in `robsond` logs near the last successful reconciliation), or access to `GET /fapi/v1/userTrades` for the affected symbol.
- [ ] You are authorized to issue terminal events for the position. Use an independent evidence review when available and follow the active incident authority.
- [ ] You have the current [reconciliation policy](../policies/UNTRACKED-POSITION-RECONCILIATION.md) and [operator CLI reference](../CLI.md) open. The implementation guide is historical context, not current operational authority.
- [ ] The `robson-cli` binary was built from a reviewed revision compatible with the deployed daemon. It is not supplied by the runtime image.
- [ ] `robson-cli reconcile-close --help` returns a usage message. If it errors with `unknown command`, STOP and resolve the version mismatch.

---

## Evidence Collection Order

Walk the two supported real-evidence sources in order. **Stop at the first source that yields a confirmed, unambiguous answer.** Record that answer for `robson-cli reconcile-close`. If neither source yields real fill evidence, do not invoke the close command; leave the blocker visible and escalate.

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

### Unsupported fallback evidence

`AccountSnapshot` and `Estimated` exist in the domain model as target architecture, but the current `robson-cli`, authenticated API, and automatic worker do not accept them for a reconciliation close. Do not prepare or submit either evidence type.

If `OrderFillRecord` and `UserTradeRecord` are unavailable or ambiguous, keep the position unresolved, preserve the stale-Active blocker, and escalate under [policy I3 section C](../policies/UNTRACKED-POSITION-RECONCILIATION.md).

---

## Manual Verification Checklist

Before issuing the close, every operator MUST tick all of:

- [ ] The exchange position for `(symbol, side)` is **definitively gone**. Check `binance-cli futures positions` AND the web UI under the same account credentials. Two-source confirmation only.
- [ ] No outstanding orders for `(symbol, side)` are still open on the exchange (`binance-cli futures open-orders`). If yes, cancel them first or escalate.
- [ ] The `position_id` you are about to close matches the position you observed missing — copy from `/positions/:id`, do not retype from memory.
- [ ] The `quantity` recorded locally matches the quantity that was on the exchange when last seen (or that the evidence proves was filled).
- [ ] The evidence is an unambiguous `OrderFillRecord` or `UserTradeRecord`; no snapshot or estimated fallback is being submitted.
- [ ] You have read the [policy I3 §C and §D](../policies/UNTRACKED-POSITION-RECONCILIATION.md) within the last 24 hours OR have memorized the evidence ordering rule.

---

## Recovery Command (operator-driven manual path)

> **Only `order_fill_record` and `user_trade_record` evidence are accepted.**
> `account_snapshot` and `estimated` are not accepted by the manual command.

### Command

Ensure `ROBSON_API_TOKEN` is already present in the process environment. Do not
place the token on the command line.

```bash
robson-cli reconcile-close \
  --position-id <UUID> \
  --evidence-file evidence.json \
  --robsond-url http://localhost:8080
```

Flags:

| Flag | Required | Default | Description |
|---|---|---|---|
|------|----------|---------|-------------|
| `--position-id` | Yes | - | UUID of the position to close |
| `--evidence-file` | Yes | - | Path to JSON file with evidence |
| `--robsond-url` | No | `http://localhost:8080` | Base URL of robsond API |
| `--token` | No | `$ROBSON_API_TOKEN` env | Supported by the binary, but avoid it because process arguments and shell history may expose the token |

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

---

## Recovery Paths Summary

| Path | Label | Status | How |
|------|-------|--------|-----|
| A | Startup `abort` policy | **IMPLEMENTED** | Code default exits 78 on stale-Active state without changing the store. |
| B | Operator-driven manual close | **IMPLEMENTED** | `robson-cli reconcile-close` and authenticated `POST /reconcile-close`. |
| C | Startup `auto_reconcile` | **IMPLEMENTED; PRODUCTION-CONFIGURED ON 2026-08-06** | Opt-in configuration gathers real evidence before startup reconciliation. Re-verify live configuration. |

### Path C: Startup `auto_reconcile`

- Enabled through `ROBSON_RECONCILIATION_ON_STARTUP_STALE_ACTIVE=auto_reconcile`.
- Phase 1 gathers real evidence for stale-Active positions before any close.
- Only `OrderFillRecord` or `UserTradeRecord` may be applied automatically.
- `AccountSnapshot` and `Estimated` are not accepted by this startup close path.
- Phase 2 applies the evidence-backed reconciliation closes sequentially and
  stops on the first rejection or persistence error. It is not a batch
  transaction; an earlier persisted close is not rolled back if a later apply
  fails.
- Current source does not exit 78 when evidence gathering returns no match. It
  logs a warning and continues startup for periodic reconciliation.
- Both deviations from the fail-closed, no-partial-close policy require a
  separate money-path review.

---

## Post-Recovery Validation

After the close, whether manual or from startup `auto_reconcile`, verify:

- [ ] `kubectl logs -n <ns> deploy/robsond --tail=200 | grep <position_id>` shows `PositionClosed` emitted with the chosen `closure_evidence`.
- [ ] `curl -s -H "Authorization: Bearer $ROBSON_API_TOKEN" http://localhost:8080/positions/<id>` returns `state: "closed"` and a non-null `exit_price`.
- [ ] `/status.occupied_slots` decremented by 1 (or matches the new ground truth).
- [ ] `monthly_state.realized_loss` updated (if the close was a loss).
- [ ] `/status.stale_active_count` and `/status.reconciliation_blockers[]` no longer include the resolved position.
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
- Neither supported source yields unambiguous real fill evidence. Leave the position unresolved; snapshots and estimates are not accepted fallbacks.
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
| 2026-08-06 | Updated repository and read-only production status, documented CLI distribution limits, removed command-line token guidance, and recorded the startup no-evidence and batch-atomicity policy drifts. | Codex |
