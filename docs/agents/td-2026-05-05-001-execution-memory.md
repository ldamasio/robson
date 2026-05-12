# TD-2026-05-05-001 — Execution Memory

**TD**: Core Position Lifecycle Drift
**Last updated**: 2026-05-12
**Scope**: `robsond` reconciliation — stale-Active positions not present on the exchange

---

## Current State

| Slice | Description | Status | PR / Commit |
|-------|-------------|--------|-------------|
| 0 | Baseline reproduction test | DONE | commit `28b7a58e` |
| 1 | Domain: `ClosureEvidence` + `ExitReason::ReconciledMissingOnExchange` | DONE | commit `fbdc8f0e` |
| 2 | Policy I3 + ADR-0022 amendment + runbook skeleton | DONE | commit `6d999a09` |
| 3 | `ExchangePort` evidence retrieval methods | DONE | commit `0835150b` |
| 4A | `PositionManager::reconcile_close` with real evidence | DONE | commit `93db6bb9` |
| 4B | Symmetric stale-Active reconciliation worker loop | DONE | commit `2a87fb8e` |
| 5A | Startup abort gate, config, exit code 78 | DONE | commit `33877728` |
| 5B1 | `robson-cli reconcile-close` + `POST /reconcile-close` | DONE | PR #60 |
| Hotfix | Dockerfile: include `robson-cli` workspace member | DONE | PR #61, commit `1b275638` |
| 5B2A | Evidence helper refactor in `reconciliation_worker.rs` | DONE | commits `20283d9e` + `26e82837` |
| 5B2B | Startup `auto_reconcile` config + two-phase algorithm | DONE (v4 opt-in) | commit `5458c36e` |
| 5B2C | Runbook finalization + testnet drill for `auto_reconcile` | 🔄 Deferred to MIG-v4#3 | — |

**`main` HEAD** (as of 2026-05-11): `26e82837`
**Branch `feat/…-5b2b-auto-reconcile` HEAD** (as of 2026-05-12): `5458c36e` — pending merge to main.

**TD status**: Open. Remaining v3 work is zero (all Slices done or deferred). TD will be
closed once Slices 0–5B2B are merged and the `docs/technical-debt.md` entry is updated.

---

## Architecture Decisions (final for v3 — do not reopen without escalation)

### Startup policy
- **`abort` is the default** and the production baseline for v3.
- **`auto_reconcile` is implemented** (Slice 5B2B) but **not production-enabled in v3**.
  Enable only in v4 after testnet drill (MIG-v4#3).
- `auto_reconcile` is two-phase / all-or-nothing:
  1. Detect all stale-Active positions.
  2. Collect real evidence for each (`OrderFillRecord` or `UserTradeRecord`).
  3. If any position lacks real evidence → abort with exit 78. No position closed.
  4. If all have unambiguous real evidence → apply `reconcile_close` for all.

### Evidence restrictions (v3 final)
- `OrderFillRecord` and `UserTradeRecord`: accepted.
- `AccountSnapshot`: rejected.
- `Estimated`: **permanently hard-blocked** — `reconcile_close` returns
  `RejectedUnsupportedEvidence { source: "estimated" }` and logs a warning. No operator
  confirmation path exists in v3. Estimated auto-close is deferred to MIG-v4#7.

### Operator-driven manual path (v3 production path)
- `POST /reconcile-close` + `robson-cli reconcile-close` (Slice 5B1) are the only
  production reconciliation paths in v3.
- No "second operator" requirement. Single operator is the v3 model.

---

## Next Implementation Sequence (v3 remaining)

1. Merge current branch (`feat/…-5b2b-auto-reconcile`) to main.
2. Close TD in `docs/technical-debt.md`.

v4 follow-up: MIG-v4#3 (testnet drill + production enable of `auto_reconcile`).

---

## Agent Roles

| Agent | Role |
|-------|------|
| GLM | Implementation for small scoped slices (Rust code) |
| Codex | Diff review before push/PR |
| Sonnet | Orchestration, docs, planning |
| Opus | Optional — high-risk architecture decisions only |

---

## Key Documents

- [Implementation Guide](../implementation/TD-2026-05-05-001-CORE-LIFECYCLE-DRIFT.md) — slice plan, amendments, algorithm decisions
- [Runbook](../runbooks/td-2026-05-05-001-stale-active-recovery.md) — operator recovery (Paths A/B live)
- [Policy I3](../policies/UNTRACKED-POSITION-RECONCILIATION.md) — I3 invariant text
- [ADR-0022](../adr/ADR-0022-robson-authored-position-invariant.md) — invariant authority
- [v4 backlog](../architecture/v4-backlog.md) — MIG-v4#3, MIG-v4#7
