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
- `robsond/src/query_engine.rs`
- `robson-exec/src/executor.rs`
- `robsond/src/reconciliation_worker.rs`
- `robsond/src/position_manager.rs`

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
  `max_single_position_pct` were eliminated in MIG-v3#11. Entry capacity is
  budget-metered: entries are admitted by actual planned risk against the 4%
  monthly budget, with the 1% per-trade cap and at least 4 full-cap operations
  per month guaranteed. See ADR-0024 and ADR-0043.
- **Estimated evidence is permanently blocked** — `reconcile_close` rejects
  `ReconciliationEvidence::Estimated`. Only `OrderFillRecord` and `UserTradeRecord`
  are valid for automated reconciled closes in v3.
- **Two-layer stop enforcement** — every Active position carries a
  robsond-authored reduce-only conditional stop on the exchange at the
  chart-derived trailing stop; the software monitor stays the primary exit
  path. Daemon availability must never be a precondition for bounded loss.
  See ADR-0039.
- **Exits always take liquidity** — exits and protective stops are market
  orders; maker-first fee optimization is permitted only on entries, where
  non-execution is costless. The 1% per-trade budget is a worst-case cap that
  prices in execution costs. See ADR-0039 and ADR-0040.
- **RBX engineering guardrails are mandatory** — before planning architecture,
  implementing, or reviewing code, apply the checklist in
  `.agents/rbx-engineering-guardrails.md`. CI enforces it via `guardrails.yml`.

## Commit Policy

- Use conventional commits.
- Prefer semantic, isolated commits.
- Leave the worktree clean after push.
