# TD-2026-05-05-001 — Execution Memory

**TD**: Core Position Lifecycle Drift
**Last updated**: 2026-05-11
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
| 5B2B | Startup `auto_reconcile` config + two-phase algorithm | PLANNED | — |
| 5B2C | Runbook finalization + testnet drill | PLANNED | — |

**`main` HEAD** (as of 2026-05-11): `26e82837`

---

## Architecture Decisions (do not reopen without escalation)

### Startup policy
- **`abort` is the default** and the safe baseline. No change planned.
- **`auto_reconcile` is opt-in** via `ROBSON_RECONCILIATION_ON_STARTUP_STALE_ACTIVE=auto_reconcile`.
- `auto_reconcile` is **two-phase / all-or-nothing**:
  1. Detect all stale-Active positions.
  2. Collect real evidence for each (`OrderFillRecord` or `UserTradeRecord`).
  3. If any position lacks real evidence → abort with exit 78. No position closed.
  4. If all have unambiguous real evidence → apply `reconcile_close` for all.

### Evidence restrictions for startup auto-close
- `OrderFillRecord` and `UserTradeRecord`: allowed for auto-close at startup.
- `AccountSnapshot`: **not allowed** for startup auto-close.
- `Estimated`: **never** auto-closes at startup — always downgrades to abort.

### Operator-driven manual path (Slice 5B1)
- Same evidence restriction: `OrderFillRecord` and `UserTradeRecord` only.
- `POST /reconcile-close` + `robson-cli reconcile-close` are live.
- Use this path for all startup-gate scenarios until 5B2B is merged.

---

## Next Implementation Sequence

1. **5B2B** — implement `startup_reverse_reconciliation()` with two-phase logic.
   Accept `auto_reconcile` in `StartupStaleActivePolicy` parser.
   Add integration tests (happy path, Estimated-downgrade, partial-evidence abort).
2. **5B2C** — finalize runbook Path C section. Execute testnet drill.
3. Only after testnet drill passes → consider enabling `auto_reconcile` in production.

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
- [Runbook](../runbooks/td-2026-05-05-001-stale-active-recovery.md) — operator recovery procedure (Paths A/B live, Path C planned)
- [Policy I3](../policies/UNTRACKED-POSITION-RECONCILIATION.md) — I3 invariant text
- [ADR-0022](../adr/ADR-0022-robson-authored-position-invariant.md) — invariant authority

---

## Open Questions / Blockers

None blocking 5B2B. Preconditions before enabling `auto_reconcile` in production:

- [ ] Slice 5B2B merged and CI green.
- [ ] Testnet drill (Slice 5B2C) completed with successful outcome recorded in runbook Run Log.
- [ ] Second authorized operator confirmed for first production `auto_reconcile` run.
