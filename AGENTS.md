# AGENTS.md

Canonical AI-first instructions for this repository.

This file is the project-wide source of truth for agent-facing rules and conventions.
Vendor-specific files such as `CLAUDE.md`, Cursor rules, or other tool adapters must stay thin and must not diverge from this file.

## Project Identity

Robson is an execution and risk management system for leveraged markets.

It is not an autonomous trading bot.
It does not justify signals, generate entries, or bypass operator governance.
Its core concern is what happens after a trading decision exists: execution, risk enforcement, lifecycle control, auditability, and failure handling.

The repository currently contains two major realities:

- Legacy application surfaces in Django/React/CLI at the repository root.
- The Rust runtime migration track under `v2/`, including `robsond`, EventLog, projector, and related specs in `docs/architecture`.

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

## Canonical Planning Vocabulary

Use these identifiers consistently:

- `MIG-v2.5#N`: migration steps from v2 to v2.5
- `MIG-v3#N`: migration steps from v2.5 to v3
- `QE-PN`: QueryEngine implementation phases
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
- `deferred`: intentionally postponed
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
| Schema migrations | Application (`v2/migrations/`) | Applied at deploy time via `sqlx migrate run`; applied automatically by `sqlx::test` during testing |
| Per-test database lifecycle | `sqlx::test` macro | Creates isolated ephemeral databases, runs all migrations, drops after test |

### Rules for agents

1. **Never provision a database** as a side effect of a code or test task. Starting containers, creating users, or running `CREATE DATABASE` is IaC work — delegate it or refuse it.
2. **Never hardcode `DATABASE_URL`** in source files, test helpers, or scripts. The value always flows in from the infrastructure layer via the environment.
3. **When writing a test that requires a live database**, gate it with `#[ignore = "requires DATABASE_URL"]` and `#[sqlx::test(migrations = "../migrations")]`. Do not write setup logic that provisions the database.
4. **`v2/scripts/test-pg.sh`** is the canonical entry point for Postgres-backed tests. It requires `DATABASE_URL` from the environment. It does not resolve or infer it.
5. **`just v2-test-pg`** is the local-dev wrapper. It supplies the known local container URL as an explicit fallback only when `DATABASE_URL` is absent. In CI and staging, `DATABASE_URL` must be injected externally.
6. **Never run Postgres integration tests against a production `DATABASE_URL`**. `sqlx::test` creates and drops databases on the target server. `scripts/test-pg.sh` enforces this with a naming guard; do not bypass it.
7. **Migration ownership**: migrations live in `v2/migrations/`. If you add a migration, verify that existing `sqlx::test`-based tests still pass with the updated schema. Do not apply migrations manually to shared environments.

### Test tiers (v2 Rust)

| Tier | Command | Requires database |
|---|---|---|
| Unit + in-memory | `cargo test --all` | No |
| Feature-gated (compile check) | `cargo test --features postgres` | No (`--ignored` tests skipped) |
| Postgres integration | `just v2-test-pg` or `bash v2/scripts/test-pg.sh` | Yes — `DATABASE_URL` must be set |

CI must pass the first two tiers unconditionally. The third tier requires a provisioned database and should run in environments where `DATABASE_URL` is available.

## Code and Review Expectations

- Follow conventional commits.
- Prefer semantic, isolated commits when making unrelated documentation or code changes.
- Leave the worktree clean after committing and pushing.
- Do not silently preserve stale compatibility files; convert them into thin adapters or remove them.

## High-Value Source Files

Start here for v3/runtime work:

- `docs/architecture/v3-migration-plan.md`
- `docs/architecture/v3-runtime-spec.md`
- `docs/architecture/v3-query-query-engine.md`
- `docs/architecture/v3-control-loop.md`
- `docs/architecture/v3-architectural-decisions.md`

Implementation reality for the current Rust executor/runtime boundary:

- `v2/robsond/src/query_engine.rs`
- `v2/robson-exec/src/executor.rs`

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
