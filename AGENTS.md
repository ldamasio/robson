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

## Canonical Planning Vocabulary

Use these identifiers consistently:

- `MIG-v2.5#N`: migration steps from v2 to v2.5
- `MIG-v3#N`: migration steps from v2.5 to v3
- `QE-PN`: QueryEngine implementation phases
- `Stage N`: runtime or control-loop pipeline stages inside a single execution tick

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
- Cursor project rules
- other editor or agent config files

When updating project-wide AI behavior:

1. Update `AGENTS.md` first.
2. Then adjust adapters so they point back here and do not drift.
