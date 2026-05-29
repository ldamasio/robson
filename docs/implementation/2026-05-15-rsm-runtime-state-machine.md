# RSM — Runtime State Machine — Analysis & Execution Plan

**Date**: 2026-05-15
**Author**: ldamasio (with Claude Opus 4.7)
**Status**: Draft
**Related**: [ADR-0037](../adr/ADR-0037-runtime-state-machine.md), [ADR-0023](../adr/ADR-0023-symbol-agnostic-policy-invariant.md), [ADR-0024](../adr/ADR-0024-trading-policy-layer.md)

---

## Executive Summary

**Problem Statement**: Robson's two on/off mechanisms (hard `replicas: 0/1` via
GitOps and the FE-only soft kill-switch) leave a drain gap, lack a single
source of truth, and require git-repo ceremony for routine pause/resume.

**Key Findings**:
- Hard kill round-trip is ~3 min and fights ArgoCD if bypassed via `kubectl scale`.
- Soft kill-switch lives in frontend state only; not consulted by the daemon.
- No orderly drain path between "block new entries" and "scale to zero".
- No audit trail of who paused/resumed and why.

**Recommended Action**: Implement RSM-P1..P5 below to introduce a Postgres-backed
state machine (RUNNING / PAUSED / DRAINING / STOPPED), enforce it in `robsond`,
expose via existing Bearer-token API, and replace the FE kill-switch with a
state panel.

**Estimated Effort**: ~5 working days, sequenced as five PRs (one per phase).

---

## Current State

### System Overview

- `robsond` is a single-replica Recreate deployment (`rbx-infra/apps/prod/robson/robsond-deploy.yml`),
  managed by ArgoCD (`argocd.argoproj.io/instance=robson-prod`).
- Frontend (`robson-frontend-v2`, SvelteKit, 2 replicas) talks to `robsond` via
  Bearer token over an Ingress → Service path.
- Postgres is external (per project invariant), shared across robson services.

### Observed Behavior (today)

- **Pause for manual Binance op**: operator commits `replicas: 1 → 0` in
  `rbx-infra`, waits for ArgoCD reconcile, performs op, commits `0 → 1`,
  waits again. Two commits, ~6 min round-trip.
- **Block new entries (FE)**: type-to-confirm `DESLIGAR` flips a state in the
  frontend; backend honours a 5-minute cooldown but the daemon itself has no
  knowledge of the toggle.
- **Drain**: no first-class path. Operators must close positions manually
  before scaling to zero, or accept that scale-zero leaves positions on the
  exchange unmanaged by Robson.

### Expected Behavior

A single UI surface that lets the operator move Robson through `RUNNING →
PAUSED → DRAINING → STOPPED → RUNNING` with type-to-confirm, audit, and
ms-latency response. Hard `replicas: 0` becomes emergency-only.

### Root Cause Analysis

The two existing mechanisms were built at different times for different
problems (CapEx pause vs. anti-fat-finger). Neither was designed to be the
canonical operational control. Unifying them requires a state machine the
daemon enforces, not a frontend-only flag and not a deploy-time replicas count.

---

## Gaps

### Code Gaps

| Priority | Component | Issue | Blocker For |
|----------|-----------|-------|-------------|
| P0 | `robsond/src/` (new module) | No `RuntimeStateGate` exists | RSM-P2 |
| P0 | `robsond/src/api.rs` | No `GET/PATCH /runtime/state` endpoints | RSM-P3 |
| P0 | Entry call sites | Not gated; bypass possible | RSM-P2 |
| P1 | Frontend | Kill-switch is FE-state only | RSM-P4 |
| P2 | Metrics | No `robson_runtime_state` gauge | RSM-P2 |

### Documentation Gaps

| Priority | File/Location | Issue | Impact |
|----------|---------------|-------|--------|
| P0 | `docs/runbooks/` | No emergency hard-kill runbook | MED |
| P1 | `docs/architecture/v3-runtime-spec.md` | No description of runtime state contract | MED |

### Infrastructure Gaps

| Priority | Resource | Issue | Impact |
|----------|----------|-------|--------|
| P1 | Postgres | New table `runtime_state` + `runtime_state_log` (migration) | Required for RSM-P1 |

---

## Priority Tracks

### RSM-P1: Database schema and migration
**Effort**: ~0.5 day
**Dependencies**: none
**Deliverables**:
- SQLx migration creating `runtime_state` (singleton) and `runtime_state_log` (append-only).
- Initial row inserted with `state='RUNNING'`, `actor='migration'`, `reason='initial'`.

**Tasks**:
1. Add migration under `migrations/` (next sequential number).
2. Define columns: `state` (TEXT with CHECK constraint), `actor`, `reason`,
   `effective_at`, `lock_until`, `updated_at`.
3. Define `runtime_state_log` columns: `id`, `from_state`, `to_state`, `actor`,
   `actor_kind` (`operator|api_token|system`), `reason`, `created_at`.
4. Add unit test exercising the migration on a `sqlx::test` database.

### RSM-P2: Gate, reconciler, metrics
**Effort**: ~1.5 days
**Dependencies**: RSM-P1
**Deliverables**:
- `RuntimeStateGate` trait + Postgres-backed implementation in `robsond/src/runtime_state.rs`.
- Gate consulted by every entry call site.
- `DRAINING` reconciler that closes open positions per existing close path.
- Prometheus metrics: `robson_runtime_state{state}` gauge,
  `robson_runtime_state_transition_total{from,to,actor_kind}` counter.

**Tasks**:
1. Implement `RuntimeStateGate::current() -> RuntimeState` with caching
   (5-second TTL) plus invalidation on transition.
2. Wrap entry call sites; add a single integration test asserting that an
   attempted entry returns `BlockedByRuntimeState` for each non-`RUNNING` state.
3. Implement drain reconciler as a tokio task: every N seconds, if state is
   `DRAINING`, close positions per drain policy and transition to `STOPPED`
   when zero positions remain.
4. Wire metrics in `robsond/src/metrics.rs`.

### RSM-P3: API
**Effort**: ~0.5 day
**Dependencies**: RSM-P2
**Deliverables**:
- `GET /api/runtime/state` → current state + last N audit entries.
- `PATCH /api/runtime/state` → `{target, reason}`, returns new state + audit row.
- 4xx on disallowed transitions; 429 on cooldown violation.

**Tasks**:
1. Define Axum handlers in `robsond/src/api.rs`.
2. Validate transition matrix server-side (do not trust the FE).
3. Enforce 5-minute `lock_until` cooldown on every transition.
4. Write the audit row in the same Postgres transaction as the state update.

### RSM-P4: Frontend state panel
**Effort**: ~1.5 days
**Dependencies**: RSM-P3
**Deliverables**:
- Status badge in the dashboard header (state + time-in-state).
- Admin panel with four actions; each requires reason + type-to-confirm.
- Last 5 transitions visible inline.
- Live updates via WebSocket or 5-second poll.

**Tasks**:
1. Replace `KillSwitch*` components with `RuntimeState*` components.
2. Reuse type-to-confirm UX (preserves current safety property).
3. Add toast notifications for transition success/failure.
4. Disable action buttons when `lock_until` is in the future; show countdown.

### RSM-P5: Runbook + cleanup
**Effort**: ~0.5 day
**Dependencies**: RSM-P4 in production
**Deliverables**:
- `docs/runbooks/EMERGENCY-HARD-KILL.md` with criteria and Ansible/`kubectl` procedure.
- Removal of replicas-flip ops commits as routine practice; document the new flow.
- Update `docs/architecture/v3-runtime-spec.md` to describe the runtime state contract.

**Tasks**:
1. Write runbook with explicit go/no-go criteria.
2. Add an entry to the on-call notes pointing to the new state panel.
3. Add a final ADR amendment marking ADR-0037 status as ACCEPTED once the
   panel is live in production.

---

## Execution Selector

| Objective | Entry Point | Effort |
|-----------|-------------|--------|
| Land DB-backed state contract first (no behavior change yet) | EP-001 | 0.5d |
| Make daemon respect the state (with metrics, no UI) | EP-002 | 1.5d |
| Expose API once daemon enforces it | EP-003 | 0.5d |
| Replace kill-switch with state panel | EP-004 | 1.5d |
| Document emergency path and decommission old flow | EP-005 | 0.5d |

### Default Execution Order

1. EP-001 (lowest blast radius; sets the contract)
2. EP-002 (defaults to `RUNNING`, so behavior identical to today)
3. EP-003 (still no UI; testable via curl)
4. EP-004 (operator-visible)
5. EP-005 (only after the panel is trusted in production)

---

## Entry Points

### EP-001: RSM-P1 — DB migration

**Objective**: Add `runtime_state` and `runtime_state_log` tables.

**Preconditions**:
```bash
test -d /home/psyctl/apps/robson/migrations
psql "$DATABASE_URL" -c "SELECT 1" >/dev/null
```

**Inputs**:
- `DATABASE_URL`: e.g. `postgres://robson@161.97.147.76:5432/robson_dev`

**Steps**:
```bash
cd /home/psyctl/apps/robson
NEXT=$(printf "%04d" $(( $(ls migrations/ | grep -oE '^[0-9]{4}' | sort -n | tail -1 | sed 's/^0*//') + 1 )))
$EDITOR migrations/${NEXT}_runtime_state.sql
sqlx migrate run --database-url "$DATABASE_URL"
cargo test -p robsond -- --test-threads=1 runtime_state
```

**Expected Outcome**:
```bash
psql "$DATABASE_URL" -c "\d runtime_state"   | grep -q "Table"
psql "$DATABASE_URL" -c "\d runtime_state_log" | grep -q "Table"
psql "$DATABASE_URL" -c "SELECT state FROM runtime_state" | grep -q "RUNNING"
cargo test -p robsond -- --test-threads=1 runtime_state 2>&1 | tail -1 | grep -q "test result: ok"
```

**Failure Detection**:
- Migration runs but no row in `runtime_state` → seed step missing.
- CHECK constraint rejects valid state strings → enum mismatch in migration.

**Rollback**:
```bash
sqlx migrate revert --database-url "$DATABASE_URL"
```

---

### EP-002: RSM-P2 — Gate + reconciler + metrics

**Objective**: Daemon respects runtime state; gate is the single chokepoint
for entries; drain reconciler exists.

**Preconditions**:
```bash
psql "$DATABASE_URL" -c "SELECT state FROM runtime_state" | grep -q "RUNNING"
```

**Inputs**: none (defaults to `RUNNING` so behavior is unchanged).

**Steps**:
1. Add `robsond/src/runtime_state.rs` with `RuntimeStateGate` trait and Postgres impl.
2. Identify all entry call sites; route them through the gate. Add an
   integration test that fails if a new entry path bypasses the gate.
3. Add `drain_reconciler` task in the worker tree.
4. Register Prometheus metrics in `robsond/src/metrics.rs`.

**Expected Outcome**:
```bash
cargo build --all 2>&1 | grep -q "Finished"
cargo test -p robsond runtime_state_gate 2>&1 | tail -1 | grep -q "test result: ok"
cargo test -p robsond entry_blocked_by_runtime_state 2>&1 | tail -1 | grep -q "test result: ok"
curl -s localhost:8080/metrics | grep -q '^robson_runtime_state{state="RUNNING"} 1'
```

**Failure Detection**:
- Gate test fails on a new symbol/call path → an entry path bypasses the gate.
- Drain reconciler does not transition to `STOPPED` → close path returned an
  error that did not propagate; check logs.

**Rollback**:
```bash
git restore robsond/src/runtime_state.rs robsond/src/api.rs robsond/src/metrics.rs
git restore robsond/src/$(rg -l "RuntimeStateGate" --type rust | xargs -r dirname | sort -u)
cargo build --all
```

---

### EP-003: RSM-P3 — API

**Objective**: Operators (and the FE) can read and transition state via the
existing Bearer-token API.

**Preconditions**:
```bash
curl -s -H "Authorization: Bearer $ROBSON_API_TOKEN" localhost:8080/api/health | jq -e '.ok'
```

**Steps**:
1. Add handlers in `robsond/src/api.rs`.
2. Add transition matrix as a `const` in `runtime_state.rs`; reuse server-side.
3. Add cooldown enforcement keyed on `lock_until`.
4. Write the state row and audit row in one transaction.

**Expected Outcome**:
```bash
curl -s -H "Authorization: Bearer $ROBSON_API_TOKEN" \
  localhost:8080/api/runtime/state | jq -r '.state' | grep -q "RUNNING"

curl -s -X PATCH -H "Authorization: Bearer $ROBSON_API_TOKEN" \
  -d '{"target":"PAUSED","reason":"smoke test"}' \
  localhost:8080/api/runtime/state | jq -r '.state' | grep -q "PAUSED"

# Disallowed transition rejected
curl -s -o /dev/null -w "%{http_code}" -X PATCH \
  -H "Authorization: Bearer $ROBSON_API_TOKEN" \
  -d '{"target":"STOPPED","reason":"should fail"}' \
  localhost:8080/api/runtime/state | grep -q "409"
```

**Failure Detection**:
- Cooldown not enforced → 200 returned on rapid second PATCH (should be 429).
- Audit row missing after transition → transaction not atomic.

**Rollback**: revert API handlers; data tables harmless if left.

---

### EP-004: RSM-P4 — Frontend state panel

**Objective**: Operator controls Robson lifecycle from the UI.

**Preconditions**: EP-003 deployed; `GET /api/runtime/state` returns 200.

**Steps**:
1. Build `RuntimeStateBadge.svelte`, `RuntimeStatePanel.svelte`,
   `RuntimeStateConfirmDialog.svelte`.
2. Wire poll/WebSocket to backend.
3. Replace existing `KillSwitch*` components with the new panel; preserve
   type-to-confirm UX.
4. Update Playwright/E2E tests to cover each transition path.

**Expected Outcome**:
- Operator can transition through every state from the UI.
- Disallowed transitions show inline error, not 500.
- Cooldown displayed as a countdown disabling the action buttons.

**Failure Detection**:
- Panel shows stale state after transition → poll/WebSocket not invalidating.
- Two operators racing produce inconsistent badge state → server is the
  source of truth; FE must re-fetch on focus.

**Rollback**: revert FE PR; backend remains in place.

---

### EP-005: RSM-P5 — Runbook + decommission

**Objective**: Document emergency hard-kill criteria and remove the
replicas-flip pattern from routine operations.

**Steps**:
1. Write `docs/runbooks/EMERGENCY-HARD-KILL.md` with criteria and procedure.
2. Update `docs/architecture/v3-runtime-spec.md` to describe the runtime state contract.
3. Mark ADR-0037 status as `ACCEPTED` once the panel is live.

**Expected Outcome**:
- Runbook exists and is referenced from on-call notes.
- ADR status updated.

**Rollback**: docs only; no rollback needed.

---

## Verification Commands Reference

```bash
# State table seeded
psql "$DATABASE_URL" -c "SELECT state FROM runtime_state" | grep -q "RUNNING"

# Gate test passing
cargo test -p robsond runtime_state_gate 2>&1 | tail -1 | grep -q "test result: ok"

# Metric exposed
curl -s localhost:8080/metrics | grep -E '^robson_runtime_state\{state="[^"]+"\} 1$'

# API healthy
curl -s -H "Authorization: Bearer $ROBSON_API_TOKEN" localhost:8080/api/runtime/state | jq -e '.state'

# Build green
cargo build --all 2>&1 | grep -q "Finished"
```

---

## Rollback Notes

### Pattern 1: Code changes (RSM-P2..P4)
```bash
git restore <files>
cargo build --all
```

### Pattern 2: DB migration (RSM-P1)
```bash
sqlx migrate revert --database-url "$DATABASE_URL"
```

### Pattern 3: Emergency reset of state to RUNNING
```bash
psql "$DATABASE_URL" <<SQL
UPDATE runtime_state
   SET state='RUNNING', actor='emergency', reason='manual override',
       effective_at=now(), lock_until=now();
INSERT INTO runtime_state_log (from_state, to_state, actor, actor_kind, reason)
VALUES ('UNKNOWN', 'RUNNING', 'emergency', 'system', 'manual override');
SQL
```

### Pattern 4: Hard kill (emergency, see runbook)
```bash
# Edit rbx-infra/apps/prod/robson/robsond-deploy.yml: replicas: 1 → 0
# Commit and push; ArgoCD reconciles in ~3 min.
```

---

## Appendices

### Appendix A: Transition matrix

```
     →     RUNNING  PAUSED  DRAINING  STOPPED
RUNNING       –       ✓        ✓         ✗
PAUSED        ✓       –        ✓         ✗
DRAINING      ✗       ✓ (cancel)  –   auto only
STOPPED       ✓       ✗        ✗         –
```

`DRAINING → STOPPED` is the only transition the system performs autonomously
(when `open_positions = 0`). Operators cannot transition directly to
`STOPPED`; they must drain first.

### Appendix B: Drain policy (open question)

Initial implementation: `DRAINING` closes positions at market in batches of 1
with a 2-second spacing, respecting existing per-symbol close logic. A future
ADR may introduce a configurable drain policy (market vs. trail-to-stop vs.
TWAP). Out of scope for RSM-P1..P5.

### Appendix C: Why not auto-resume after a TTL

Considered: `PAUSED` could auto-promote to `RUNNING` after N hours.
Rejected for v1 because it weakens operator intent ("I paused for a reason").
Can be added later as an opt-in field in `PATCH` payload.

---

## Changelog

| Date       | Change         | Author    |
|------------|----------------|-----------|
| 2026-05-15 | Initial draft  | ldamasio  |
