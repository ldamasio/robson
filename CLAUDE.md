# CLAUDE.md

Claude Code compatibility adapter.

Canonical project-wide AI instructions live in [AGENTS.md](AGENTS.md).
If this file and `AGENTS.md` ever diverge, `AGENTS.md` is the source of truth.

## Claude-Specific Working Notes

1. Read `AGENTS.md` first for project-wide rules.
2. Keep repository content in English.
3. Use canonical planning identifiers:
   - `MIG-v2.5#N` (all complete)
   - `MIG-v3#N` (in progress)
   - `MIG-v4#N` (backlog — see `docs/architecture/v4-backlog.md`)
   - `QE-PN`
   - `Stage N`
4. Use repository-verified status semantics for migration and architecture docs.
5. Keep current implementation clearly separated from target architecture.
6. `abandoned` = explicitly dropped from v3 scope (documented in v3-migration-plan.md).
7. `deferred` = moved to v4 backlog (documented in v4-backlog.md).

## High-Value Files

- `AGENTS.md`
- `docs/architecture/v3-migration-plan.md`
- `docs/architecture/v4-backlog.md`
- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`
- `docs/policies/SYMBOL-AGNOSTIC-POLICIES.md`
- `docs/adr/ADR-0022-robson-authored-position-invariant.md`
- `docs/adr/ADR-0023-symbol-agnostic-policy-invariant.md`
- `docs/adr/ADR-0024-trading-policy-layer.md`
- `docs/architecture/v3-runtime-spec.md`
- `docs/architecture/v3-query-query-engine.md`
- `docs/architecture/v3-control-loop.md`
- `docs/architecture/v3-architectural-decisions.md`
- `v3/robsond/src/query_engine.rs`
- `v3/robson-exec/src/executor.rs`
- `v3/robsond/src/reconciliation_worker.rs`
- `v3/robsond/src/position_manager.rs`

## Core Invariants (must never be violated)

- **Robson-authored position invariant** — every open position on the operated
  Binance account must trace to a `robsond`-authored entry. UNTRACKED positions
  MUST be closed. Applies to all account types and all symbols. See ADR-0022.
- **Symbol-agnostic policy invariant** — rules apply to every trading pair.
  Symbols appear only as labeled examples or operator-configured values, never as
  hard-coded assumptions in policy text. See ADR-0023.
- **Technical Stop from chart analysis** — never `entry × (1 − pct)`. See ADR-0021.
- **Opportunity detection vs Technical Stop Analysis** are separate responsibilities.
  See ADR-0021.
- **No static hard limits** — `max_open_positions`, `max_total_exposure_pct`, and
  `max_single_position_pct` were eliminated in MIG-v3#11. Only slot logic and the
  4% monthly drawdown policy govern entry capacity. See ADR-0024.
- **Estimated evidence is permanently blocked** — `reconcile_close` rejects
  `ReconciliationEvidence::Estimated`. Only `OrderFillRecord` and `UserTradeRecord`
  are valid for automated reconciled closes in v3.

## Commit Policy

- Use conventional commits.
- Prefer semantic, isolated commits.
- Leave the worktree clean after push.
