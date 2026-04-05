# AGENTS.md (docs subtree)

Canonical project-wide AI instructions live in the repository root at [`/AGENTS.md`](../AGENTS.md).

This file exists only as a subtree adapter so that agents working inside `docs/` receive the same guidance without a second policy store.

## Docs-Specific Reminders

1. Follow the canonical rules in [`/AGENTS.md`](../AGENTS.md).
2. Keep all documentation in English.
3. Use canonical identifiers in planning docs:
   - `MIG-v2.5#N`
   - `MIG-v3#N`
   - `QE-PN`
   - `Stage N`
4. Use `repository-verified` semantics when marking status from repository evidence.
5. Keep `current implementation` distinct from `target architecture`.
6. When changing one v3 architecture document, verify adjacent docs that describe the same concept:
   - `docs/architecture/v3-migration-plan.md`
   - `docs/architecture/v3-runtime-spec.md`
   - `docs/architecture/v3-query-query-engine.md`
   - `docs/architecture/v3-control-loop.md`
   - `docs/architecture/v3-architectural-decisions.md`
