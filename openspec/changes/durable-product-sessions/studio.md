# OpenSpec Studio handoff

## GitHub delivery visibility

The plan is published under [Robson issue #244](https://github.com/ldamasio/robson/issues/244),
with eleven delivery sub-issues (#245–#255) and a
[generated progress report](https://github.com/ldamasio/robson/issues/244#issuecomment-6306049749).
OpenSpec remains the technical source; GitHub owns discussion, assignment and
delivery links. The repository publisher refreshes one report after the workflow
is merged into main. It does not write back into OpenSpec or execute Specflow.
See the [tracking runbook](../../../docs/runbooks/openspec-github-tracking.md).

The Mint installation was completed after the initial planning inspection:
the local web interface opens from the system menu and the separate local
session-implementation worktree is registered. The earlier no-install statements below describe only
the planning inspection. No Studio fork is needed for this first integration.

## Initial planning inspection

The operator identified [davi-fati/openspec-studio](https://github.com/davi-fati/openspec-studio)
as the suggested Kanban tool. Reference source checked on 2026-10-20 at commit
`d564f10f6e21cc0a35d64e9eb33ca98f17744dda`; its backend module uses the historical
`github.com/davidlima/openspec-studio/backend` name.

OpenSpec Studio uses the repository's OpenSpec files directly. Open this Robson
worktree using its **Open** action, not **New**, then select **Kanban** and the
`durable-product-sessions` change. One change becomes one card; its internal
implementation checklist remains in `tasks.md`. The project has `openspec/specs`
and `openspec/changes`; the baseline specs directory is intentionally empty until
the pending change is implemented and archived.

The expected Studio column is `todo` (To do), with all planning artifacts present
and zero implementation tasks completed.
That label does not mean provider acceptance, deployment approval or execution
has occurred. Do not duplicate specifications into a separate board database.
Any Studio edit must be reviewed through the same Git diff and OpenSpec checks.

Repository compatibility was inspected in `backend/internal/service/project.go`
and `change.go`. OpenSpec CLI validation is separate from a rendered Studio UI
test. The Studio was not installed/launched and the project was not registered
in its local database during this planning step. No provider or Specflow was
configured. The documented desktop packaging targets macOS; this workspace is
Linux, where a future local launch can use the documented web frontend.

The planning files are sufficient for opening the board. Installation/building
the Studio, if desired later, is separate tooling work; no Studio source is
copied into the public Robson repository.

OpenSpec CLI 1.14.3 was installed in an isolated temporary tooling directory,
with installation scripts and telemetry disabled. It is not a runtime dependency
of Robson. Recheck the change with that version:

```sh
OPENSPEC_TELEMETRY=0 DO_NOT_TRACK=1 openspec validate durable-product-sessions --strict --json --no-interactive
OPENSPEC_TELEMETRY=0 DO_NOT_TRACK=1 openspec status --change durable-product-sessions --json
```

The CLI's planning-complete fields describe artifact completeness; inspect
`tasks.md` for implementation completion. Studio file compatibility was checked
against its folder/checkbox parser without registering a project or launching
its backend. Neither check is a substitute for authentication acceptance tests.
