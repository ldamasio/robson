# Codex Briefing — VAL-001 Pre-flight Audit + Parallel Support

**Your role**: Auditor / Reviewer / Analyst
**Parallel track**: GLM is executing VAL-001 (testnet E2E). You run in parallel on non-conflicting work.

---

## Context

Robson is a Rust execution and risk management daemon for leveraged crypto trading (RBX Systems).
Architecture: hexagonal, event-sourced, single control loop. The Rust daemon (`robsond`) is the sole
execution authority — every order passes through a blocking Risk Engine via a `GovernedAction` token
before reaching the exchange.

**Repository**: the current Robson workspace
**Runtime crate**: `robsond/src/`
**Key source files**:
- `robsond/src/position_manager.rs` — state machine, signal processing, fill handling
- `robsond/src/api.rs` — HTTP routes (arm, signal, disarm, panic)
- `robsond/src/market_data.rs` — WebSocket tick handling
- `robson-engine/src/lib.rs` — chart-derived trailing-stop decisions and
  `position_monitor_tick` audit events
- `robsond/src/query_engine.rs` — GovernedAction + Risk Engine gate
- `robsond/src/detector.rs` — signal detection
- `robsond/src/reconciliation_worker.rs` — unconditional UNTRACKED-position enforcement

**Canonical rules**: read `AGENTS.md` and `docs/architecture/v3-migration-plan.md` first.
**English only** in all output — code, comments, reports.

---

## Critical Constraint

Before Phase 1, record the testnet Deployment image, pod UID, and restart count.
Do not deploy, restart, or trigger an image rollout during GLM's execution. A
runtime change mid-run invalidates the validation. All code analysis in this
session is read-only unless explicitly instructed otherwise.

---

## Your Three Tasks (in order)

---

### Task B3 — Pre-flight Codebase Audit (deliver BEFORE GLM starts Phase 1)

**Objective**: identify any known risks in the `arm → signal → fill → trailing stop → exit` path
before GLM executes it. Give GLM actionable warnings.

**Read these files** (in order):
1. `robsond/src/api.rs` — arm handler, signal handler, disarm handler
2. `robsond/src/position_manager.rs` — `arm_position()`, `execute_signal_query()`, `process_market_data()`
3. `robson-engine/src/lib.rs` — trailing-stop tick processing and
   `position_monitor_tick` emission
4. `robsond/src/query_engine.rs` — `GovernedAction` creation, approval gate, `cycle_id` injection

**For each phase of the E2E cycle, answer**:
- Is there any known error path that would silently fail without returning an HTTP error?
- Is `cycle_id` guaranteed to be set on the current `entry_order_accepted` and
  `exit_order_placed` events?
- Is there any condition where the trailing engine would not emit its
  `position_monitor_tick` audit event or advance the stop on eligible ticks?
- Are there any timeout or retry limits GLM should know about?
- Is there any known issue with signal injection when `capital = 100` USDT?

**Deliver**: a concise risk report (under 30 lines) formatted as:

```
PRE-FLIGHT RISK REPORT — VAL-001
Generated: <timestamp>

PHASE 1 (ARM): <CLEAR | RISK: description>
PHASE 2 (SIGNAL): <CLEAR | RISK: description>
PHASE 3 (FILL): <CLEAR | RISK: description>
PHASE 4 (TRAILING STOP): <CLEAR | RISK: description>
PHASE 5 (EXIT): <CLEAR | RISK: description>

KNOWN ISSUES: <list or NONE>
RECOMMENDED ACTIONS FOR GLM: <list or NONE>
```

---

### Task B1 — Audit VAL-002 Runbook (run in parallel while GLM executes Phases 1–5)

**Objective**: review the existing
`docs/runbooks/val-002-real-capital-activation.md` against current code and
infrastructure. Do not create or rotate credentials during this audit.

**This runbook covers the 4-step blocking sequence after VAL-001 PASS**:

1. Verify the canonical `rbx/robson/binance-api-key` and
   `rbx/robson/binance-api-secret` entries exist without printing their values.
2. Verify current `rbx-infra` Ansible variables and paths directly; never infer
   them from old `robson-v2` names.
3. Verify the production daemon selects `Exchange: Binance (production)` and
   that `ROBSON_BINANCE_USE_TESTNET` is absent or not true. The USD-M connector
   uses `fapi.binance.com`, but the URL is not emitted in startup logs.
4. Keep the infrastructure rollback guard
   `ROBSON_POSITION_MONITOR_ENABLED: "false"`, roll out the reviewed immutable
   image through GitOps, verify both retired `/safety/*` routes return `404`, and
   verify reconciliation-worker liveness. The current application does not parse
   the legacy flag; never enable it.

Confirm the runbook keeps repository-verified state separate from operational
rollout evidence, requires a flat account before any change, and validates the
exact immutable image plus reconciliation liveness after rollout. Report any
stale command or unsafe secret-handling instruction; do not execute VAL-002 as
part of this audit.

---

### Task B2 — EventLog Phase Audit (triggered by GLM at each phase boundary)

**Objective**: after each phase, GLM will output a signal like `PHASE <N> COMPLETE`. Run the
corresponding SQL audit and confirm or flag the result.

**Database access**: the EventLog is in the `robson-testnet` namespace PostgreSQL.
Connection: `kubectl exec -n robson-testnet <paradedb-pod> -- psql -U robson -d robson`

**At each GLM signal, run**:

**After Phase 1 (ARM)**:
```sql
SELECT event_type, timestamp FROM event_log
WHERE stream_key = 'position:<POSITION_ID>' ORDER BY sequence;
-- Verify: position_armed present
```

**After Phase 2 (SIGNAL)**:
```sql
SELECT event_type,
       payload->>'cycle_id' AS cycle_id,
       payload->>'exchange_order_id' AS exchange_order_id,
       timestamp
FROM event_log
WHERE stream_key = 'position:<POSITION_ID>' ORDER BY sequence;
-- Verify: technical_stop_analyzed, entry_signal_received,
-- entry_order_requested, and entry_order_accepted are present.
-- Require a non-null cycle_id and exchange_order_id on entry_order_accepted.
```

**After Phase 3 (FILL)**:
```sql
SELECT event_type, payload->>'fill_price', payload->>'entry_price', timestamp
FROM event_log
WHERE stream_key = 'position:<POSITION_ID>'
  AND event_type IN ('entry_signal_received', 'entry_filled')
ORDER BY sequence;
-- Verify: entry_filled present; compare its fill_price with the
-- entry_signal_received entry_price and confirm GET /positions/:id is Active.
```

**After Phase 4 (TRAILING STOP)**:
```sql
SELECT event_type, payload, timestamp
FROM event_log
WHERE stream_key = 'position:<POSITION_ID>'
  AND event_type IN ('position_monitor_tick', 'trailing_stop_updated')
ORDER BY sequence;
-- position_monitor_tick is a current robson-engine event despite its historical name;
-- it is unrelated to the removed fixed-percentage PositionMonitor module.
-- Require at least 3 position_monitor_tick rows and verify high_watermark is
-- non-decreasing for a long. trailing_stop_updated remains optional on a short run
-- because it emits only after a full favorable span.
```

**After Phase 5 (EXIT)**:
```sql
SELECT event_type, payload->>'cycle_id', payload->>'realized_pnl', timestamp
FROM event_log
WHERE stream_key = 'position:<POSITION_ID>'
ORDER BY sequence;
-- Verify full sequence present (see runbook)
-- Verify: exit_order_placed has cycle_id
-- Verify: position_closed has a numeric realized_pnl field
```

**At each phase, output**:
```
AUDIT PHASE <N>: PASS | FAIL
  Events found: <list>
  Missing: <list or NONE>
  cycle_id present on orders: YES | NO
  Anomalies: <description or NONE>
```

---

## Final Deliverable

After Phase 5 audit, write the PASS/FAIL verdict in the VAL-001 Run Log:

```
File: docs/runbooks/val-001-testnet-e2e-validation.md

Run Log entry:
| <date> | GLM + Codex | ✅ PASS / ❌ FAIL | <one-line summary including POSITION_ID> |
```

Then report to the PO (Claude):
1. Verdict: PASS or FAIL
2. Full event sequence found (list all event_types in order)
3. Any governance gap detected (missing cycle_id, bypassed Risk Engine)
4. VAL-002 runbook audit status (clear / findings / blocked)
5. Any code issues found in B3 that should become follow-up tasks
