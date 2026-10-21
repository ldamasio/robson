# OpenSpec GitHub tracking acceptance and Fable review

Date: 2026-10-20 (America/Sao_Paulo). Scope: publish the approved session plan,
delivery issues and a bounded progress projection for Robson. No authentication
runtime, infrastructure, production, trading or Studio Specflow changes.

## Current result

- [Parent issue #244](https://github.com/ldamasio/robson/issues/244), with eleven
  native sub-issues #245–#255. The relationship list was read back and verified.
- [Progress comment](https://github.com/ldamasio/robson/issues/244#issuecomment-6306049749)
  records 0/28 implementation tasks at an immutable source commit. Its initial
  creation returned exactly the submitted body.
- The plan was published on a separate branch from main. The pre-existing local
  session prototype is not part of the tracking PR; ADR-0055 labels that boundary.
- The integration has no dependency on a Studio fork. The Studio proposal links
  to the GitHub tracker; repository files remain authoritative.
- Automatic refresh is implemented and tested, but activation requires merging
  the workflow into main. The ordinary PR validation job has no issue-write
  permission. No schedule or background agent was configured.
- GitHub Projects remained pending at this acceptance point: the CLI token
  lacks `read:project`, and the browser session requires operator sign-in. No
  token scopes, account credentials or visibility settings were changed.

## Fable 5.1 review

The user-requested reviewer completed using canonical model
`claude-fable-5-1`, medium effort, with no tools/MCP, hooks, browser integration
or session persistence. The frozen pack contained only the publisher, tests,
workflow, public mapping and runbook. The verdict was **approve-with-changes**.
The following dispositions were implemented and verified afterwards; they are
not a second clean model approval.

| Finding | Disposition |
| --- | --- |
| An unrelated newer main commit can strand a path-filtered stale run | Removed the push path filter: every main push queues the serialized job. Stale runs still cannot overwrite newer-source reports. Documented source-SHA updates even with unchanged counts. |
| Body newline normalization might invalidate the checksum | Verified an exact live API round trip; canonicalized CRLF and trailing newline/whitespace on sealing, verification, no-op and echo checks. Added normalization regression coverage. |
| Malformed task bullets could be silently omitted | Reject malformed checkbox or numbered-task bullets; added missing-space, missing-checkbox and alternate-bullet cases. |
| Read timeout surfaced as a raw socket error | Translate TimeoutError to the same bounded failure as URLError; verify no retry. |
| Fixed 28-task regression check obstructs legitimate plan changes | Compare the real parsed package IDs with the mapping instead; counts derive from the current source. |
| Verify checkout pin label | GitHub's actions/checkout v4 tag resolved to `11d5960a326750d5838078e36cf38b85af677262`, matching the workflow pin. |
| Document concurrent editor and bootstrap limitations | Runbook explicitly requires exclusive publisher ownership of the report, acknowledges the GET/PATCH race, and requires API bootstrap/reconciliation instead of duplicate creation. |

## Verification and limits

- Fourteen Python tests passed, including 200 deterministic randomized checklist
  cases, mapping/parser failures, newline normalization, stale/current-main
  succession, changed-comment protection, wrong-issue refusal, lost-ack/no-op
  behavior, API failures, request/response limits, redirects and event gating.
- Bandit 1.8.6 passed for the publisher. Its seven tooling dependencies are
  version/hash pinned; OSV Scanner 2.4.0 reported no affected packages in that
  requirements file. These are tooling dependencies, not Robson runtime packages.
- Gitleaks 8.30.1 found no secrets in the new planning/review files, scripts and
  workflows. The downloaded executable's upstream asset SHA-256 was verified.
- actionlint 1.7.12 passed on both changed workflows; its release digest was
  verified. OpenSpec CLI 1.14.3 strict validation and `git diff --check` passed.
- Bootstrap readbacks verified all eleven sub-issues and the dedicated report.
  The recurring publisher's live main-branch execution remains pending merge;
  its HTTP and recovery contracts were exercised with controlled test doubles.
- No Rust, frontend or authentication source was changed by the tracking PR;
  their full runtime suites were not repeated for this tooling/documentation
  change. Existing repository-wide dependency alerts are not resolved or claimed
  clean by the isolated tooling scan; the normal CI guardrails still apply.

Raw review input/output, hashes, bootstrap mapping, dependency scan results and
preview are retained outside the public repository at the local artifact
directory `robson-openspec-github-20261020`. See the
[runbook](../runbooks/openspec-github-tracking.md) for failure handling, request
budgets, activation and rollback. Deployment acceptance remains outside scope.
