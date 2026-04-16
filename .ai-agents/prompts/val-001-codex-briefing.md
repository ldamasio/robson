# Codex Briefing — VAL-001 Pre-flight Audit + Parallel Support

**Your role**: Auditor / Reviewer / Analyst
**Parallel track**: GLM is executing VAL-001 (testnet E2E). You run in parallel on non-conflicting work.

---

## Context

Robson is a Rust execution and risk management daemon for leveraged crypto trading (RBX Systems).
Architecture: hexagonal, event-sourced, single control loop. The Rust daemon (`robsond`) is the sole
execution authority — every order passes through a blocking Risk Engine via a `GovernedAction` token
before reaching the exchange.

**Repository**: `/home/psyctl/apps/robson`
**Runtime crate**: `v2/robsond/src/`
**Key source files**:
- `v2/robsond/src/position_manager.rs` — state machine, signal processing, fill handling
- `v2/robsond/src/api.rs` — HTTP routes (arm, signal, disarm, panic)
- `v2/robsond/src/market_data.rs` — WebSocket tick handling
- `v2/robsond/src/position_monitor.rs` — trailing stop tracking
- `v2/robsond/src/query_engine.rs` — GovernedAction + Risk Engine gate
- `v2/robsond/src/detector.rs` — signal detection

**Canonical rules**: read `AGENTS.md` and `docs/architecture/v3-migration-plan.md` first.
**English only** in all output — code, comments, reports.

---

## Critical Constraint

**Do NOT push to `main` or any branch that triggers a CI build for `robsond` during GLM's execution.**
The deployed testnet image is `sha-88242685`. A rebuild mid-run invalidates the validation.
All code analysis in this session is read-only unless explicitly instructed otherwise.

---

## Your Three Tasks (in order)

---

### Task B3 — Pre-flight Codebase Audit (deliver BEFORE GLM starts Phase 1)

**Objective**: identify any known risks in the `arm → signal → fill → trailing stop → exit` path
before GLM executes it. Give GLM actionable warnings.

**Read these files** (in order):
1. `v2/robsond/src/api.rs` — arm handler, signal handler, disarm handler
2. `v2/robsond/src/position_manager.rs` — `arm_position()`, `execute_signal_query()`, `process_market_data()`
3. `v2/robsond/src/position_monitor.rs` — trailing stop tick processing
4. `v2/robsond/src/query_engine.rs` — `GovernedAction` creation, approval gate, `cycle_id` injection

**For each phase of the E2E cycle, answer**:
- Is there any known error path that would silently fail without returning an HTTP error?
- Is `cycle_id` guaranteed to be set on `entry_order_placed` and `exit_order_placed` events?
- Is there any condition where the position monitor would NOT emit trailing stop updates on ticks?
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

### Task B1 — Write VAL-002 Runbook (run in parallel while GLM executes Phases 1–5)

**Objective**: create `docs/runbooks/val-002-real-capital-activation.md`.

**This runbook covers the 4-step blocking sequence after VAL-001 PASS**:

1. Create Binance real API keys in `pass`: `rbx/robson-v2/binance-api-key` and `rbx/robson-v2/binance-api-secret`
2. Update Ansible defaults: change `pass_robson_v2_testnet_binance_api_key` from `rbx/robson-v2-testnet/` to `rbx/robson-v2/` and re-run Ansible
3. Verify prod daemon connects to `api.binance.com` (not testnet) with the real keys
4. Enable `ROBSON_POSITION_MONITOR_ENABLED: "true"` in `apps/prod/robson/robsond-config.yml` → push → ArgoCD auto-sync → verify

**Format**: follow the exact runbook template from `docs/runbooks/README.md`.

**Include**:
- Run Log header (same anti-abandonment pattern as VAL-001)
- Prerequisites: VAL-001 PASS required, real Binance keys available, Ansible access
- A "Safety checks before flip" section: verify prod namespace has NO active positions before enabling monitor
- Abort criteria: if prod connects to testnet endpoint after Ansible run, rollback immediately
- Related docs: link VAL-001, `docs/architecture/v3-migration-plan.md`, Ansible role path in `rbx-infra`

**Do not** speculate about the Ansible role internals — reference `rbx-infra/bootstrap/ansible/` as the path and note that the exact variable names must be verified against the current role.

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
SELECT event_type, payload->>'cycle_id' AS cycle_id, timestamp
FROM event_log
WHERE stream_key = 'position:<POSITION_ID>' ORDER BY sequence;
-- Verify: entry_signal_received AND entry_order_placed both present
-- Note: cycle_id may be null in payload (known gap — GovernedAction token is not serialized
--   to EventLog in current implementation). Absence is NOT a FAIL for VAL-001; record as
--   follow-up item "cycle_id serialization to EventLog" in your final report.
```

**After Phase 3 (FILL)**:
```sql
SELECT event_type, payload->>'fill_price', payload->>'entry_price', timestamp
FROM event_log
WHERE stream_key = 'position:<POSITION_ID>'
  AND event_type IN ('entry_filled', 'position_active')
ORDER BY sequence;
-- Verify: entry_filled present, fill_price within 1% of entry_price
```

**After Phase 4 (TRAILING STOP)**:
```sql
SELECT event_type, payload, timestamp
FROM event_log
WHERE stream_key = 'position:<POSITION_ID>'
  AND event_type ILIKE '%stop%'
ORDER BY sequence;
-- Note: trailing_stop_updated events only emit after a full favorable price span.
-- On a short testnet run, zero such events is acceptable.
-- Primary evidence for Phase 4 is GLM's log output showing ticks processed.
-- If events ARE present: verify stop value is increasing (long position). Record count.
```

**After Phase 5 (EXIT)**:
```sql
SELECT event_type, payload->>'cycle_id', payload->>'pnl', timestamp
FROM event_log
WHERE stream_key = 'position:<POSITION_ID>'
ORDER BY sequence;
-- Verify full sequence present (see runbook)
-- Verify: exit_order_placed has cycle_id
-- Verify: position_closed has pnl field with numeric value
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
File: /home/psyctl/apps/robson/docs/runbooks/val-001-testnet-e2e-validation.md

Run Log entry:
| <date> | GLM + Codex | ✅ PASS / ❌ FAIL | <one-line summary including POSITION_ID> |
```

Then report to the PO (Claude):
1. Verdict: PASS or FAIL
2. Full event sequence found (list all event_types in order)
3. Any governance gap detected (missing cycle_id, bypassed Risk Engine)
4. VAL-002 runbook status (created / blocked)
5. Any code issues found in B3 that should become follow-up tasks
