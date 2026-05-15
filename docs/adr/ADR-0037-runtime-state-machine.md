# ADR-0037: Runtime State Machine for Operational Control

**Date**: 2026-05-15
**Status**: PROPOSED
**Deciders**: RBX Systems (operator + architecture)

---

## Context

Robson currently has two operational on/off mechanisms, used for distinct
purposes but with overlapping intent:

1. **Hard kill** — `replicas: 0 ↔ 1` committed to `rbx-infra`
   (`apps/prod/robson/robsond-deploy.yml`). Used for "manual Binance operation"
   pauses. Round-trip is git → push → ArgoCD reconcile (~3 min). Requires
   `rbx-infra` write access. Bypassing GitOps with `kubectl scale` is reverted
   by ArgoCD self-heal.
2. **Soft kill-switch** — type-to-confirm `DESLIGAR/DISABLE` in the frontend
   with a 5-minute backend cooldown. Documented to "prevent new entries only";
   the daemon stays up and continues managing existing positions.

Both mechanisms work, but together they leave operational gaps:

- No orderly **drain** path: today an operator must either keep positions open
  (soft kill) or scale to zero with positions still on the exchange (hard kill).
- No **single source of truth**. The kill-switch lives in frontend state; the
  scale lives in `rbx-infra`. There is no canonical record of "is Robson
  operating right now, and why not".
- No **audit trail** for who paused/resumed and when. Git history captures
  scale events, but not soft pauses, and conflates ops events with image bumps.
- The hard kill **fights ArgoCD** unless executed through the GitOps repo,
  which makes it slow and ceremony-heavy for what should be a single click.
- The soft kill is **frontend-resident**, so any non-FE entry signal path (a
  future scheduled job, an adapter, an operator endpoint) must reimplement the
  gate or risks bypassing it.

The operator has asked for a single, elegant UI control covering pause /
drain / stop / resume, with safety properties at least as strong as the current
type-to-confirm + cooldown.

---

## Decision

Robson adopts a **canonical runtime state** persisted in Postgres, enforced by
`robsond`, exposed through the existing Bearer-token API, and surfaced in the
frontend as the primary operational control. The replicas `0 ↔ 1` flip is
demoted to an **emergency-only** mechanism with a runbook, no longer exposed in
the UI and no longer the routine on/off path.

### State machine

| State      | Daemon behavior                                                      | Allowed transitions                |
|------------|----------------------------------------------------------------------|------------------------------------|
| `RUNNING`  | Normal operation. Entries permitted. Existing positions managed.     | → `PAUSED`, → `DRAINING`           |
| `PAUSED`   | No new entries. Existing positions continue to be managed (stops/targets active). | → `RUNNING`, → `DRAINING` |
| `DRAINING` | No new entries. Reconciler closes open positions per drain policy. Auto-transitions to `STOPPED` when no positions remain. | → `PAUSED` (cancel drain) |
| `STOPPED`  | No entries. No position management beyond observability. Pod alive (healthz/readyz green). | → `RUNNING` |

Direct `RUNNING → STOPPED` and `STOPPED → DRAINING` are **disallowed**:
operators must pass through `PAUSED` or `RUNNING` to make state changes
explicit and avoid one-click drains.

### Source of truth

A singleton `runtime_state` row in Postgres holds the current state plus
metadata (actor, reason, `effective_at`, `lock_until` for anti-flap). Every
transition writes a row to `runtime_state_log` (append-only).

### Enforcement

`robsond` reads the state at startup and reconciles continuously. A new
`RuntimeStateGate` is consulted in the entry path; any code path that opens a
position must pass through the gate, with a single test ensuring no entry
bypass exists. `DRAINING` is driven by an existing close path under a new
reconciler.

### UI

The frontend replaces the current kill-switch with a **state panel**:

- A status badge in the header (state + time-in-state).
- An admin panel with four actions (Pausar / Drenar / Parar / Religar). Each
  action requires a textarea reason and a type-to-confirm matching the target
  state name (preserves the existing UX safety property).
- Live updates (WebSocket or short-poll) so `DRAINING` progress is observable.
- Last-N transitions visible inline (actor + reason + timestamp).

### Hard kill (out of scope for UI)

`replicas: 0` remains valid as an **emergency** action, executed via Ansible
or `kubectl` against the GitOps repo, with a runbook listing the criteria
(daemon unresponsive to `STOPPED`, suspected exploit, state corruption). It
is no longer the routine on/off mechanism.

---

## Consequences

### Positive

- **Millisecond-latency control** with no rollout, no ArgoCD lag.
- **Single source of truth** for "is Robson operating", queryable from any
  surface (FE, ops scripts, Prometheus).
- **Audit trail** of every transition (actor, reason, timestamp).
- **Orderly drain** becomes a first-class operation, removing the gap between
  "block new entries" and "scale to zero with positions still on exchange".
- **No K8s API surface** added to the backend → no new ServiceAccount/RBAC,
  no `ignoreDifferences` on `spec.replicas`, no fight with ArgoCD self-heal.
- **Symbol-agnostic** by construction — the gate operates on entries, not on
  any specific instrument (consistent with [ADR-0023](ADR-0023-symbol-agnostic-policy-invariant.md)).

### Negative / Trade-offs

- **Pod stays alive** when `STOPPED` — still consumes 250m CPU / 256Mi memory
  and an exchange API connection. Acceptable; emergency hard kill remains
  available.
- **Gate is a load-bearing invariant**. Any future entry path that bypasses
  `RuntimeStateGate` is a correctness bug. Mitigation: a single test asserts
  that all `enter_position`-like callers route through the gate.
- **Soft state, hard consequences**. A bug that flips state to `RUNNING`
  unintentionally would re-enable trading. Mitigation: anti-flap lock,
  type-to-confirm in UI, audit log makes regressions detectable.
- **Adds a Postgres dependency** to the entry path. Already present
  (DB is in the position lifecycle), so no new external dependency.

---

## Alternatives Considered

### UI calls Kubernetes API to scale the deployment

Rejected. Requires a ServiceAccount with `apps/v1.deployments` patch
permissions in `robson` namespace, ArgoCD `ignoreDifferences` on
`spec.replicas`, and exposes a high-blast-radius capability in the backend. A
state-machine read/write is dramatically smaller surface area for the same
operator outcome.

### UI triggers a CI pipeline that commits to `rbx-infra`

Rejected. Adds CI latency (~30s build + push + ArgoCD reconcile) to every
pause, requires a bot identity with write access to `rbx-infra`, and couples
operational control to CI availability. The current pattern (manual commit)
is preserved as the emergency path.

### Keep current dual mechanism (hard scale + FE kill-switch)

Rejected. Two mechanisms with overlapping intent and different sources of
truth invite operational mistakes (e.g., hard kill while soft kill is active,
or vice versa). The drain gap remains unaddressed.

### Move state into a ConfigMap watched by the daemon

Rejected. ConfigMap is harder to make transactional with the audit log,
loses the natural append-only history a Postgres table provides, and requires
the daemon to learn a Kubernetes watch path it does not need today.

---

## Implementation Notes

- Implementation guide: [`docs/implementation/2026-05-15-rsm-runtime-state-machine.md`](../implementation/2026-05-15-rsm-runtime-state-machine.md) (RSM-P1..P5).
- Likely code paths:
  - `robsond/src/runtime_state.rs` (new) — `RuntimeStateGate` trait + Postgres-backed impl.
  - `robsond/src/api.rs` — `GET/PATCH /runtime/state` handlers.
  - `robsond/src/reconciliation_worker.rs` or sibling — `DRAINING` reconciler.
  - Entry path call sites — wrap with gate consultation.
  - `apps/frontend/src/lib/components/RuntimeState*.svelte` (new).
- Tests:
  - Gate blocks entries in every non-`RUNNING` state.
  - `DRAINING` transitions to `STOPPED` only when `open_positions = 0`.
  - Disallowed transitions return 4xx and write nothing to the audit log.
  - Cooldown rejects rapid flips.
- Metrics:
  - `robson_runtime_state{state}` (gauge, one labelled series active).
  - `robson_runtime_state_transition_total{from,to,actor_kind}` (counter).
- Operational documentation:
  - Runbook for emergency hard-kill criteria and procedure.
  - Update `feedback_agent_git_remote_write_policy` references if any tooling
    exposes a state-change endpoint to agents.

---

## References

- [ADR-0023: Symbol-Agnostic Policy Invariant](ADR-0023-symbol-agnostic-policy-invariant.md)
- [ADR-0024: Trading Policy Layer](ADR-0024-trading-policy-layer.md)
- Frontend kill-switch (current behavior, to be replaced): see `apps/frontend/src/lib/components/KillSwitch*.svelte`
- GitOps deployment manifest: `rbx-infra/apps/prod/robson/robsond-deploy.yml`
