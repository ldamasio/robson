# AGENTS.md

Canonical AI-first instructions for this repository.

This file is the project-wide source of truth for agent-facing rules and conventions.
Vendor-specific files such as `CLAUDE.md`, Cursor rules, or other tool adapters must stay thin and must not diverge from this file.

## Project Identity

Robson is an execution and risk management system for leveraged markets.

It is not an autonomous trading bot.
It does not justify signals, generate entries, or bypass operator governance.
Its core concern is what happens after a trading decision exists: execution, risk enforcement, lifecycle control, auditability, and failure handling.

The repository contains the Rust runtime (canonical) at the repository root, the SvelteKit frontend under `frontend/`, and related specs in `docs/architecture`.

## Non-Negotiable Rules

1. All repository content must be in English.
2. Prefer minimal, high-signal diffs over broad rewrites.
3. Do not present target architecture as if it were current implementation.
4. Do not mark operational work as done without explicit repository evidence.
5. When code and docs disagree, either:
   - update docs to match current code, or
   - label the doc text clearly as target architecture / follow-up.
6. **`TechnicalStopDistance` MUST always be computed from chart analysis — never from
   a fixed percentage of entry price.** `stop_loss = entry × (1 − pct)` is a policy
   violation regardless of context. The stop is the second support/resistance level on
   the 15-minute chart (see `docs/requirements/technical-stop-requirements.md` and
   ADR-0021). This invariant applies to all code, tests, runbooks, prompts, and agent
   briefings without exception.
7. **Opportunity detection (WHEN to enter) and Technical Stop Analysis (WHERE the stop
   is) are architecturally separate responsibilities** and must never be conflated in a
   single component. See ADR-0021.
8. **Robson-authored position invariant.** Every open position on the operated Binance
   account MUST be the direct result of an entry authored by `robsond` through a
   `GovernedAction`. Any open position that is not traceable to a `robsond`-authored
   entry is UNTRACKED and MUST be closed by the reconciliation worker. Applies to every
   account type (spot, margin, futures) and every symbol. See ADR-0022 and
   `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`.
9. **Symbol-agnostic policy invariant.** Every Robson policy applies to every trading
   pair. A rule that hard-codes a symbol (e.g., "Robson trades BTC/USDT") is
   non-compliant; symbols appear only as labeled examples or as operator-configured
   values. Symbol-specific constants (tick size, lot step, min notional, max leverage,
   fee rate) come from exchange metadata at runtime, never from policy text. See
   ADR-0023 and `docs/policies/SYMBOL-AGNOSTIC-POLICIES.md`.

## Canonical Planning Vocabulary

Use these identifiers consistently:

- `MIG-v2.5#N`: migration steps from v2 to v2.5 (all complete)
- `MIG-v3#N`: migration steps from v2.5 to v3 (in progress — see `docs/architecture/v3-migration-plan.md`)
- `MIG-v4#N`: items deferred to v4 (see `docs/architecture/v4-backlog.md`)
- `QE-PN`: QueryEngine implementation phases (QE-P1–P4 complete; QE-P5 deferred to v4)
- `Stage N`: runtime or control-loop pipeline stages inside a single execution tick
- `VAL-N`: operational validation gates — runbook-format procedures required before go-live events (see `docs/runbooks/val-*.md`)

Rules:

- Never use bare references like "phase 5" or "step 3" when the intended axis could be ambiguous.
- Use `Stage`, not `Phase`, for runtime/control-loop pipeline sequencing.
- If a document mixes migration planning and subsystem evolution, always prefix both.

## Canonical Status Vocabulary

Prefer these status terms:

- `repository-verified`: status supported by repository evidence
- `implemented`: code and tests/docs in repo support the feature
- `pending`: not yet implemented or not yet accepted
- `operational rollout`: code may exist, but real deployment/activation is not yet repository-verified
- `deferred`: intentionally postponed to a later version (v4+)
- `abandoned`: explicitly dropped from scope — will not be implemented (reason must be documented)
- `follow-up required`: architectural direction is decided, but code alignment is still pending

Repository status rule:

- Code-backed items may be marked done from repository evidence.
- Operational rollout items remain pending unless the repository contains explicit rollout confirmation.

## Documentation Rules

When editing architecture, migration, or runtime docs:

1. Keep `current reality` separate from `target architecture`.
2. Keep these files aligned when the same concept appears in more than one place:
   - `docs/architecture/v3-migration-plan.md`
   - `docs/architecture/v3-runtime-spec.md`
   - `docs/architecture/v3-query-query-engine.md`
   - `docs/architecture/v3-control-loop.md`
   - `docs/architecture/v3-architectural-decisions.md`
3. If an architectural decision is made but not fully implemented, record it as `DECIDED` plus `FOLLOW-UP REQUIRED`, not as `IMPLEMENTED`.
4. Use ADRs for stable architectural decisions that should outlive a single migration task.

## Database and Infrastructure Layer Rules

Database provisioning is the responsibility of `rbx-infra` (Ansible bootstrap).
Application and agent code must never provision databases directly.

### Layer ownership

| Layer | Owner | Responsibility |
|---|---|---|
| Server, user, base database | `rbx-infra` Ansible | Provisions the Postgres server and emits `DATABASE_URL` to vault |
| Schema migrations | Application (`migrations/`) | Applied at deploy time via `sqlx migrate run`; applied automatically by `sqlx::test` during testing |
| Per-test database lifecycle | `sqlx::test` macro | Creates isolated ephemeral databases, runs all migrations, drops after test |

### Rules for agents

1. **Never provision a database** as a side effect of a code or test task. Starting containers, creating users, or running `CREATE DATABASE` is IaC work — delegate it or refuse it.
2. **Never hardcode `DATABASE_URL`** in source files, test helpers, or scripts. The value always flows in from the infrastructure layer via the environment.
3. **When writing a test that requires a live database**, gate it with `#[ignore = "requires DATABASE_URL"]` and `#[sqlx::test(migrations = "../migrations")]`. Do not write setup logic that provisions the database.
4. **`scripts/test-pg.sh`** is the canonical entry point for Postgres-backed tests. It requires `DATABASE_URL` from the environment. It does not resolve or infer it.
5. **`just test-pg`** is the local-dev wrapper. It supplies the known local container URL as an explicit fallback only when `DATABASE_URL` is absent. In CI and staging, `DATABASE_URL` must be injected externally.
6. **Never run Postgres integration tests against a production `DATABASE_URL`**. `sqlx::test` creates and drops databases on the target server. `scripts/test-pg.sh` enforces this with a naming guard; do not bypass it.
7. **Migration ownership**: migrations live in `migrations/`. If you add a migration, verify that existing `sqlx::test`-based tests still pass with the updated schema. Do not apply migrations manually to shared environments.

### Test tiers (v3 Rust)

| Tier | Command | Requires database |
|---|---|---|
| Unit + in-memory | `cargo test --all` | No |
| Feature-gated (compile check) | `cargo test --features postgres` | No (`--ignored` tests skipped) |
| Postgres integration | `just test-pg` or `bash scripts/test-pg.sh` | Yes — `DATABASE_URL` must be set |

CI must pass the first two tiers unconditionally. The third tier requires a provisioned database and should run in environments where `DATABASE_URL` is available.

## Code and Review Expectations

- Follow conventional commits.
- Prefer semantic, isolated commits when making unrelated documentation or code changes.
- Leave the worktree clean after committing and pushing.
- Do not silently preserve stale compatibility files; convert them into thin adapters or remove them.

## Policy Invariants

These are architectural constraints that MUST NOT be violated by any agent or human:

10. **No static exposure hard limits.** `max_open_positions`, `max_total_exposure_pct`, and `max_single_position_pct` were eliminated in MIG-v3#11 (ADR-0024). The only static constraint is `risk_per_trade_pct = 1%` as a maximum-loss cap and `max_monthly_drawdown_pct = 4%` of `capital_base`. The 1% is a **worst-case cap, not a target**: position sizing prices execution costs — a gap allowance past the stop plus round-trip taker fees — into the denominator, so a stop that fills does not breach the cap (ADR-0039; motivated by the 2026-06 incident where distance-only sizing guaranteed a breach on every stop hit). Entry capacity is budget-metered (ADR-0043): each entry is admitted against its actual planned worst-case loss (cost-priced per ADR-0039), not the full 1% cap, so a month holds **at least** 4 full-cap operations and more when trades risk less — "saved risk becomes an extra operation". `slots_available` is the guaranteed-minimum floor of remaining full-cap entries, not a ceiling. MonthlyHalt fires only when `remaining_budget ≤ 0`; a trade that merely does not fit the remaining budget is a governed `risk_budget_insufficient` denial (re-arm with backoff), never a halt. There is no daily loss limit and no entry-count limit per day or month — a phantom 1%/day check that had silently re-activated in `RiskGate` was removed on 2026-07-04 (PR #110; it blocked all entries for the rest of the UTC day after one budget-sized stop-out). Governed entry denials re-arm detectors with exponential backoff (5 s → 15 min cap), never in a hot loop. Funding rate does not determine exposure limits in long positions. Realized risk may be lower when available margin caps the position size.
11. **`Estimated` evidence is permanently blocked in `reconcile_close`.** Only `OrderFillRecord` and `UserTradeRecord` are accepted for automated reconciled closes. If evidence is unavailable, the daemon aborts startup and requires manual operator intervention via `robson-cli reconcile-close`. Insurance-stop fills satisfy this by chaining the exchange's conditional-order record to the real triggered order's fill (ADR-0039).
12. **No LLM, no autonomous trading.** v3 has zero LLM coupling. The operator decides WHEN to trade. Robson decides HOW MUCH and enforces THAT risk rules are never violated. QE-P5 (Context Governance) is deferred to v4.
13. **Two-layer stop enforcement; exits always take liquidity.** Every Active position carries a robsond-authored, reduce-only conditional stop order on the exchange at the chart-derived trailing stop (the "insurance stop", `ins-` client-id prefix), placed on entry fill and cancel-replaced as the trailing stop advances. The software monitor remains the primary exit path; the exchange-side order exists so stop enforcement survives daemon crashes, deploys, and partitions — availability of `robsond` must never be a precondition for bounded loss (ADR-0039; the 2026-06 incident turned a breakeven exit into the month's full loss during a 45 h outage). Exits and protective stops always execute as market orders: on the loss leg, non-execution is unbounded, so fee optimization (maker-first, ADR-0040) is only ever permitted on legs where non-execution is costless — entries. Execution may trigger a small operator-configured buffer BEYOND the technical stop (`stop_buffer_bps`, default 0; below it for longs, above for shorts) so fills avoid the obvious chart level; the buffer is an execution offset priced into sizing, never a change to the chart-derived stop itself (ADR-0041). An optional invalidation guard may clamp the effective stop beyond a recent adverse extreme before the buffer is applied; the guard is entry-time only, released on the first trailing-stop advance, and rejects the entry when it would exceed the maximum validated stop distance (ADR-0042).

## High-Value Source Files

Start here for v3 runtime work:

- `docs/architecture/v3-migration-plan.md` — authoritative roadmap and status
- `docs/architecture/v4-backlog.md` — items explicitly deferred to v4
- `docs/architecture/v3-runtime-spec.md`
- `docs/architecture/v3-query-query-engine.md`
- `docs/architecture/v3-control-loop.md`
- `docs/architecture/v3-architectural-decisions.md`
- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`
- `docs/policies/SYMBOL-AGNOSTIC-POLICIES.md`
- `docs/adr/ADR-0022-robson-authored-position-invariant.md`
- `docs/adr/ADR-0023-symbol-agnostic-policy-invariant.md`
- `docs/adr/ADR-0024-trading-policy-layer.md`
- `docs/adr/ADR-0039-exchange-side-insurance-stop.md`
- `docs/adr/ADR-0040-maker-first-entry-execution.md`
- `docs/adr/ADR-0041-executable-stop-buffer.md`
- `docs/adr/ADR-0042-invalidation-guard.md`

Implementation reality for the current Rust executor/runtime boundary:

- `robsond/src/query_engine.rs`
- `robson-exec/src/executor.rs`
- `robsond/src/reconciliation_worker.rs`
- `robsond/src/position_manager.rs`

## Adapter Policy

Tool-specific files may exist for compatibility, but they are adapters, not primary policy stores.

Examples:

- `CLAUDE.md`
- `.cursor/rules/*`
- other editor or agent config files

Legacy compatibility note:

- `.cursorrules` is deprecated and should not be used as a primary rule store in this repository.

When updating project-wide AI behavior:

1. Update `AGENTS.md` first.
2. Then adjust adapters so they point back here and do not drift.
