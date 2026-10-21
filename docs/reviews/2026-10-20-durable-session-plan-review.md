# Durable session implementation plan: Fable 5.1 review

Date: 2026-10-20. Scope: planning and local artifacts; no runtime implementation,
database provisioning, real IdP request, trading operation or deployment.

Related [OpenSpec proposal](../../openspec/changes/durable-product-sessions/proposal.md),
[design](../../openspec/changes/durable-product-sessions/design.md),
[tasks](../../openspec/changes/durable-product-sessions/tasks.md) and
[ADR-0055](../adr/ADR-0055-product-sessions-web-native-cli.md).

## Review evidence

The first CLI invocation timed out after 300 seconds without a verdict; it is
not counted as approval. The second invocation completed with canonical model
`claude-fable-5-1`, provider `firstParty`, and verdict **approve-with-changes**.
Both used the already-authorized Claude CLI in safe mode, no tools, no MCP,
disabled hooks/browser integration and no session persistence. The completed
review used medium effort and a narrower plan-only pack, without runtime source.
Its source-baseline table was checked locally against `d9ec2b08`.

| Evidence | SHA-256 |
|---|---|
| Completed review input | `1aaee28b4a46ab924f85e8f3fea265dfaf5575054253cacd1e09a9a2cb224f2d` |
| Completed review output Markdown | `0b4ce6a5f17914baf36f17d62ce9fb8c74bbf7396d4d1232c7d55a64af5933f4` |

Raw input/output, model metadata and timeout/result manifests are retained as
local task artifacts outside this public repository. Pattern scans of the
transmitted packs found no private-key block, provider-key pattern or JWT. This
is not a comprehensive secret audit. No credentials or private Identity source
were included.

## Findings and planning dispositions

These are amendments to the plan, not completed implementation fixes or a second
clean approval from Fable. All implementation checkboxes remain unchecked.

| ID | Severity | Finding | Disposition and verification task |
|---|---|---|---|
| D1 | High | Activity could invalidate the refresh row-version fence | Separate authority version from activity updates; recheck live deadlines without rejecting harmless activity. Model interleaving in 1.2, cross-process acceptance in 5.3. |
| D2 | High | Clearing the cookie on unconfirmed logout removes the retry handle | Add signed, purpose-bound, non-authorizing logout intent, ten-minute expiry and cross-instance replay before admission. Independently verifiable signed CSRF proof binds the request; invalid proof cannot mint an intent. Test failed commit/reload/recovery, key rotation, tampering and expiry in 4.1, 7.1/7.3 and 8.1. |
| D3 | High | Restore protection cannot rely on a human remembering to advance an epoch | Require infrastructure-enforced fencing before restore, external epoch transition, rejection of stale processes, and containment of all affected Robson-client grants including those missing from the backup. Periodic high-water checkpoints were **not adopted as the safety guarantee**: a logout between checkpoints can still be lost. Acceptance of the mandatory restore path remains an infrastructure gate in 4.3; arbitrary privileged out-of-band tampering is not claimed detectable. |
| D4 | High | Generation-only outbox uniqueness drops late returned credentials | Key by lineage/generation/attempt/token role/fingerprint, permit separate old/late token pairs and a tokenless unknown-grant job. Verify both retained outcomes and remediation in 5.2/5.3 and 7.3. |
| D5 | High | A per-test DB cannot provision a cluster-wide runtime role | Static migration/grant-contract checks run locally; live behavior uses infrastructure-provisioned ROBSON_TEST_RUNTIME_ROLE and schema grant fixture under the isolated harness. No role creation; absent role means pending privilege acceptance in 3.3. |
| D6 | Medium | Cleanup backlog can prevent admission after IdP recovery | Reserve capacity before issuance, preserve logout at full capacity, deduplicate expiry triggers without losing distinct credentials, use fair urgent/expiry scheduling and a conditional 60-second recovery test. Jobs requiring manual containment remain visible and block unsafe admission. Tasks 7.2/7.3 and 10.2. |
| D7 | Medium | Cleanup/SSE load could exhaust the request pool | Separate request, stream and worker pools; default eight-stream cap per BFF and bounded two-check scheduling, explicit total connection budget. Measure in 8.3 and 10.2. |
| D8 | Medium | Waiter polling and recovery writes lack budgets | Two bounded waiter polls, ten-second refresh lease, bounded transaction duration, worker-only expired-attempt recovery and explicit query budgets. Test in 1.2, 5.1/5.3 and 10.2. |
| D9 | Medium | Revoke success is not sufficient containment evidence | Separate requested revocation, inactive access and contained grant; budget post-revoke access introspection. **Inactive access alone also does not prove refresh-family revocation**: require verified provider grant semantics/status or bounded lifetime, tested on disposable grants. Never refresh-probe live cleanup credentials. Tasks 7.3 and 9.2. |
| D10 | Low | Infrastructure dependency serializes model work unnecessarily | Split model/adapter work from explicit live acceptance in 4.3, 5.3, 6.3, 7.3 and 8.3; these remain pending until infrastructure is supplied. AUTH-DUR-01 and subsequent pure model work can start independently. |
| D11 | Low | No local key-ring fixture specified | Add ephemeral two-key fixtures with seeded inputs for model tests, explicit test key IDs refused in real-provider mode. Task 4.1. |

Additional implementer checks made cleanup reservation mandatory before external
issuance and prevented independent revocation of old refresh generations while
their grant remains active. Those details complement D4/D6 and require tests.

## Validation and handoff

OpenSpec CLI 1.14.3 strict validation passes. The change contains three capability
delta specs, 34 scenarios and 28 pending implementation tasks. Local links and
whitespace were checked. Runtime tests were not rerun for this documentation-only
change; earlier implementation evidence remains in the previous review record.

OpenSpec Studio source at `d564f10f6e21cc0a35d64e9eb33ca98f17744dda` was inspected
for folder/task parsing. The expected Kanban state is `todo`; the application
was not launched or registered and no Specflow/provider was configured. See the
[Studio handoff](../../openspec/changes/durable-product-sessions/studio.md).

The next executable slice is AUTH-DUR-01: asynchronous authority interfaces and
the deterministic concurrent-state model. Infrastructure requirements are an
explicit handoff, not authorization to provision or deploy. A later review of
the implemented code and real acceptance results is still required by task 11.1.
