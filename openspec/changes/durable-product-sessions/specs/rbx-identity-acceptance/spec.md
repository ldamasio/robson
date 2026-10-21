# Spec Delta

## Purpose

Require reproducible provider and resilience evidence before activating durable
sessions, keeping sandbox results separate from production authorization.

## ADDED Requirements

### Requirement: Isolated integration boundary
Real identity tests MUST use explicit infrastructure-provided sandbox inputs and
verified TLS. The system MUST reject production mode, unlisted endpoints and
mixed Google/RBX verifier modes during this implementation stage.

#### Scenario: Unsafe configuration
- **WHEN** sandbox configuration names an unlisted issuer/backend or disables TLS verification
- **THEN** startup refuses it before any credentials or requests are sent.

#### Scenario: Missing test infrastructure
- **WHEN** an isolated database or provider registration is unavailable
- **THEN** its acceptance remains pending; tests do not provision infrastructure or substitute production credentials.

### Requirement: Provider capability evidence
Activation SHALL require redacted evidence for canonical identity, audience,
scopes, access/refresh lifetimes, refresh rotation/reuse, fresh authentication,
grant-family containment and revocation isolation. Fixture behavior MUST NOT be
reported as real-provider support or proof of a 30-day login SLA.

#### Scenario: Provider has stricter lifetime
- **WHEN** the actual provider permits a shorter session than the product upper bound
- **THEN** the shorter bound is honored and documented without changing instance-wide IdP settings as a side effect.

#### Scenario: Wrong token or insufficient privilege
- **WHEN** a request uses an ID token as API access, wrong audience/issuer/subject, expired token or missing scope
- **THEN** the API denies it even if the BFF has a locally valid session.

#### Scenario: Required provider semantics are absent
- **WHEN** the provider cannot prove fresh authentication or contain/revoke an uncertain grant
- **THEN** activation is blocked and the architecture decision is revisited without a silent bypass.

### Requirement: Reproducible failure and resource acceptance
Acceptance SHALL include real isolated PostgreSQL concurrency and crash tests,
property-based interleavings, key/restore failure tests and measured bounded
query, network, pool, queue, listener and memory behavior. Failed or skipped
evidence MUST remain distinguishable from successful checks.

#### Scenario: Multi-process crash campaign
- **WHEN** processes are killed at refresh, callback, logout and cleanup boundaries
- **THEN** revoked sessions never reappear, uncertain credentials are not replayed, and durable recovery evidence explains each outcome.

#### Scenario: Long-lived and offline usage
- **WHEN** foreground usage spans multiple token expirations and offline/resume/reconnect cycles
- **THEN** deadlines remain correct, UI freshness is visible, resource counts stay bounded and mutations are never replayed.

### Requirement: Review and operational separation
The change SHALL retain a Fable 5.1 review and disposition of findings before
claiming implementation acceptance. Review approval and completed planning MUST
NOT be represented as deployment authorization or institutional RBX ratification.

#### Scenario: Plan complete but acceptance incomplete
- **WHEN** proposal, design and tasks are complete but live acceptance is pending
- **THEN** the change remains pending implementation/acceptance and is not archived as delivered or scheduled for deployment.
