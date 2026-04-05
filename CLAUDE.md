# CLAUDE.md

Claude Code compatibility adapter.

Canonical project-wide AI instructions live in [AGENTS.md](AGENTS.md).
If this file and `AGENTS.md` ever diverge, `AGENTS.md` is the source of truth.

## Claude-Specific Working Notes

1. Read `AGENTS.md` first for project-wide rules.
2. Keep repository content in English.
3. Use canonical planning identifiers:
   - `MIG-v2.5#N`
   - `MIG-v3#N`
   - `QE-PN`
   - `Stage N`
4. Use repository-verified status semantics for migration and architecture docs.
5. Keep current implementation clearly separated from target architecture.

## High-Value Files

- `AGENTS.md`
- `docs/architecture/v3-migration-plan.md`
- `docs/architecture/v3-runtime-spec.md`
- `docs/architecture/v3-query-query-engine.md`
- `docs/architecture/v3-control-loop.md`
- `docs/architecture/v3-architectural-decisions.md`
- `v2/robsond/src/query_engine.rs`
- `v2/robson-exec/src/executor.rs`

## Commit Policy

- Use conventional commits.
- Prefer semantic, isolated commits.
- Leave the worktree clean after push.
