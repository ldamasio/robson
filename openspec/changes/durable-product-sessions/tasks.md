# Tasks

Status: pending implementation. Every unchecked item requires evidence before
completion. `AUTH-DUR-*` labels identify work packages within this OpenSpec
change, not v3 migration steps or runtime Stages. The authoritative architecture
is ADR-0055; the design and three delta specs supply this change's test contract.

## 1. AUTH-DUR-01: Async authority contract and invariant model

Owner: Robson. Dependencies: none. Start here; no live infrastructure required.

- [ ] 1.1 Define async session/transaction/outbox interfaces and typed outcomes; refactor the in-memory adapter and all awaited server paths without changing accepted policy. Verify existing BFF, frontend and daemon auth regressions.
- [ ] 1.2 Add a deterministic state-machine model with fast-check interleavings for refresh, logout, expiry, activity, callback and step-up; assert terminal invalidation, fixed absolute deadline, activity not breaking a valid refresh fence, bounded waiters and no uncertain credential replay.

## 2. AUTH-DUR-02: Persistence schema and infrastructure contract

Owner: Robson schema; rbx-infra provisions resources. Depends on AUTH-DUR-01.

- [ ] 2.1 Add an additive migration under root migrations for auth sessions, OIDC transactions, attempts, cleanup and recovery evidence; specify constraints/indexes, runtime privileges, epoch and retention. Statically check migration isolation and the grant allowlist; live privilege verification requires a pre-provisioned restricted role in 3.3.
- [ ] 2.2 Prepare the rbx-infra handoff for isolated DATABASE_URL, runtime role, key ring/epoch and sandbox IdP registration; record supplied references without secret values. Keep real-environment acceptance pending until provisioned externally.

## 3. AUTH-DUR-03: PostgreSQL adapter and isolated test harness

Owner: Robson. Depends on AUTH-DUR-02; live tests require the infrastructure handoff.

- [ ] 3.1 Select and pin the Node PostgreSQL driver; implement bounded pool, DB-clock predicates, parameterized queries, atomic versions and typed unavailable outcomes. Verify query counts and dependency/security checks.
- [ ] 3.2 Add ignored sqlx::test harnesses in robsond/tests that launch two Node BFF processes against the harness-owned temporary database; integrate with scripts/test-pg.sh, redact connection data and prove child-process teardown. Never provision a database server or role.
- [ ] 3.3 Prove restart continuity, cross-instance reads, deadline enforcement and datastore failure using the real isolated DB. With infrastructure-provided ROBSON_TEST_RUNTIME_ROLE and schema grant fixture, verify restricted privileges via SET LOCAL ROLE without creating roles; otherwise mark that check pending. Report model/live/privilege results separately.

## 4. AUTH-DUR-04: Encrypted envelopes and recovery epochs

Owner: Robson; key delivery owned by infrastructure. Model tasks depend on
AUTH-DUR-01; live acceptance depends on AUTH-DUR-03 and restore/epoch inputs.

- [ ] 4.1 Implement versioned authenticated envelopes and signed logout/CSRF purposes with external key IDs; use a two-key local fixture to test tamper/cross-session substitution, missing keys, rewrap and retirement preconditions without infrastructure keys.
- [ ] 4.2 Model epoch admission, compulsory restore fencing and quarantine; reject stale BFFs/workers and preserve ordinary restart semantics. Define durable recovery evidence, product-client grant containment and key-compromise response.
- [ ] 4.3 Verify 4.1/4.2 against isolated PostgreSQL and the infrastructure restore contract, including a logout after the last backup/checkpoint and grants absent from that backup. Block restore acceptance if the mandatory external fence cannot be enforced.

## 5. AUTH-DUR-05: Cross-instance refresh and crash recovery

Owner: Robson. Model work depends on AUTH-DUR-01 and 4.1; live acceptance depends
on AUTH-DUR-03 and AUTH-DUR-04's persistence integration.

- [ ] 5.1 Implement/model one committed attempt per credential generation, ten-second leases, bounded two-poll waiters and worker-only recovery without holding DB transactions during HTTP; prove activity/refresh compatibility and stale-owner rejection in the model.
- [ ] 5.2 Model failures before send, after rotation, before commit and after commit; assert no uncertain token replay, distinct old/late-result cleanup jobs and a tokenless remediation job for unknown credentials.
- [ ] 5.3 Repeat 5.1/5.2 with two BFFs and process kills against real isolated PostgreSQL, proving fencing and restart behavior independently of the in-memory model.

## 6. AUTH-DUR-06: Durable login, replacement and step-up

Owner: Robson. Model work depends on 4.1 and 5.1; live acceptance depends on
AUTH-DUR-03 and 5.3.

- [ ] 6.1 Persist and atomically consume state/browser-bound five-minute OIDC transactions; test two callbacks, restart, mix-up, replay, missing binding and uncertain exchange without authorization-code reuse.
- [ ] 6.2 Implement same-subject fresh-auth session rotation and replacement cleanup; test logout/expiry races, preserved deadlines, rotated CSRF and old-auth_time rejection. Treat actual provider grant isolation as an acceptance gate.
- [ ] 6.3 Run 6.1/6.2 across two BFFs with the PostgreSQL adapter, including callback consumption and process death; distinguish fixture grant isolation from real-provider acceptance.

## 7. AUTH-DUR-07: Durable invalidation and cleanup

Owner: Robson. Model work depends on 5.1/5.2 and 6.1/6.2; live acceptance
depends on 5.3 and 6.3.

- [ ] 7.1 Implement/model atomic logout/expiry/supersession plus cleanup intent, terminal tombstones and distinct late-token jobs. Test non-authorizing signed logout intents across store failure/recovery, expiry, forged/cross-session intent, key rotation and lost acknowledgment.
- [ ] 7.2 Implement pre-issuance cleanup reservations, bounded leased cleanup, retry/backoff, dead-letter visibility, upstream-lifetime retention and admission backpressure; verify logout/expiry at full capacity, partial revocation failure, worker death and eventual cleanup without lost jobs. Never revoke an older refresh generation independently while its grant remains active.
- [ ] 7.3 Verify 7.1/7.2 against PostgreSQL; prove weighted queue fairness, conditional 60-second admission recovery, inactive-access evidence versus actual grant containment, tokenless-job remediation and correct recovery of logout intents after restart.

## 8. AUTH-DUR-08: UI and stream consistency

Owner: Robson. Fixture/UI work depends on 7.1/7.2; live acceptance depends on 7.3.

- [ ] 8.1 Add unconfirmed-global-logout feedback, stable offline/reauth distinctions and frozen mutation surfaces; browser-test double submits, expired metadata, wall-clock skew and resumed tabs against dev and built frontend.
- [ ] 8.2 Enforce shared-state pre-dispatch and bounded SSE checks; test cross-instance logout, DB/IdP outage, token expiry, heartbeat/watchdog and snapshot resync. Verify no mutation replay and document the already-dispatched-write boundary.
- [ ] 8.3 Repeat browser/stream scenarios with PostgreSQL; measure the default eight-stream cap, separate request/SSE/worker pools and bounded check scheduling under concurrent logout/refresh/cleanup load.

## 9. AUTH-DUR-09: Real IdP acceptance profile and matrix

Owner: Robson integration; rbx-infra/Identity supply sandbox registration. Depends
on AUTH-DUR-06 through AUTH-DUR-08 and infrastructure inputs.

- [ ] 9.1 Add an explicit constrained TLS sandbox profile in BFF and daemon while keeping production rejected; test issuer/origin/backend allowlists, redirect matching, certificate validation and mutually exclusive verifier modes.
- [ ] 9.2 Run and record the acceptance matrix: canonical identity, audience/scope/type rejection, access/refresh limits, rotation/reuse, fresh auth_time through Google, per-grant isolation and uncertain-family cleanup. Unsupported semantics block activation rather than relax authorization.

## 10. AUTH-DUR-10: Resilience and workload acceptance

Owner: Robson. Depends on AUTH-DUR-08 and AUTH-DUR-09.

- [ ] 10.1 Run state-machine seeds plus real DB multi-process fault injection, restore and key failures; record commands, baseline commits, seeds, fault point, expected invariant and outcome for every case.
- [ ] 10.2 Measure DB/external request counts, latency, pool waits, cleanup age, SSE connection limits and memory/listener growth; soak foreground usage across at least three access-token expirations plus offline/resume cycles. Use virtual time for 7/30-day policy boundaries and do not claim real 30-day endurance.

## 11. AUTH-DUR-11: Security review and acceptance record

Owner: Robson, reviewed by Fable 5.1. Depends on AUTH-DUR-10.

- [ ] 11.1 Run applicable lint, secret/dependency scans and Rust/frontend/BFF/browser regressions; freeze a sanitized implementation evidence pack and request Fable 5.1 review, retaining actual model identity, findings and dispositions.
- [ ] 11.2 Update ADR-0055's current/pending distinction and local runbooks; demonstrate rollback cleanup and retain explicit production availability, native/CLI and deployment follow-ups. Archive only after all scoped work is verified; planning approval alone does not complete these tasks.

## Evidence and execution boundary

No item above is complete merely because the plan has been reviewed. Store
redacted results under docs/reviews and retain raw local test/review artifacts
outside the public repository. External infrastructure dependencies may block a
specific test without blocking the pure model/interface work. Do not schedule
autonomous execution through a planning tool as a side effect of opening this
change; the current request is implementation planning, with no deployment.
