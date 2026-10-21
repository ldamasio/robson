# Spec Delta

## Purpose

Preserve valid Robson sessions across service restarts while enforcing trusted
deadlines, encrypted persistence, one-use login and fail-closed recovery.

## ADDED Requirements

### Requirement: Durable shared authority
The system SHALL accept a session only from committed shared authority and SHALL
preserve valid sessions across ordinary process restart without extending their
deadlines. Store unavailability MUST NOT fall back to local memory authorization.

#### Scenario: Restart and second instance
- **WHEN** a remembered session is committed and its BFF restarts or another BFF receives its cookie
- **THEN** the session remains usable with the same absolute and inactivity deadlines, subject to current upstream authorization.

#### Scenario: Datastore unavailable
- **WHEN** authority cannot be read or committed within the bounded timeout
- **THEN** authenticated work fails with a visible unavailable state, no mutation is dispatched, and no invalid-credential conclusion is invented.

### Requirement: Server-owned session policy
The system SHALL enforce 7-day inactivity and 30-day absolute upper bounds for
remembered sessions, and 30-minute inactivity and 12-hour absolute upper bounds
for temporary sessions. Stricter provider deadlines MUST prevail. Only bounded
foreground activity may extend inactivity; absolute expiry MUST never slide.

#### Scenario: Background activity cannot renew user presence
- **WHEN** polling, heartbeats, token renewal and repeated session checks continue without foreground activity
- **THEN** the session expires at its inactivity deadline even if its credential can still be renewed.

#### Scenario: Foreground use and absolute expiry
- **WHEN** authenticated foreground use keeps a remembered session active beyond several access-token expirations
- **THEN** access is renewed as authorized until the earlier upstream or product deadline, and the original 30-day limit is preserved.

#### Scenario: Clock tampering
- **WHEN** a client reports a future time or a BFF has a different local clock
- **THEN** the shared authority's clock controls session expiry and the client cannot extend its session.

### Requirement: Encrypted secrets and controlled key lifecycle
The system MUST persist credentials and login secrets encrypted and bound to
their identity/session context. It MUST keep decryption keys outside the database
and exclude credentials, cookies and callback codes from logs and review packs.

#### Scenario: Ciphertext copied or modified
- **WHEN** an encrypted credential is tampered with or moved to another identity, lineage or credential generation
- **THEN** decryption/authorization fails closed and emits only redacted diagnostic evidence.

#### Scenario: Key rotation and missing key
- **WHEN** a key is rotated or an envelope's key is unavailable
- **THEN** valid old envelopes can be rewrapped with retained authorized keys, and unavailable keys never cause plaintext fallback or silent session acceptance.

### Requirement: Single-use login and recent authentication
The system MUST consume each browser-bound OIDC transaction at most once across
instances. Recent authentication SHALL require verified subject and fresh
`auth_time`; step-up MUST rotate cookie/CSRF state without extending session
deadlines or reviving a logged-out parent.

#### Scenario: Callback race and restart
- **WHEN** two instances receive the same callback or an exchange is interrupted by restart
- **THEN** only one session transition may succeed, and an uncertain consumed code requires a new interactive login.

#### Scenario: Logout races with step-up
- **WHEN** a step-up response arrives after its parent session was revoked or expired
- **THEN** no replacement session is created and returned credentials are handled by the cleanup policy.

#### Scenario: Silent federation is insufficient
- **WHEN** federation returns an old authentication time despite a fresh-login request
- **THEN** sensitive actions remain locked and no client-supplied freshness assertion overrides the provider result.

### Requirement: Safe restore and durable recovery evidence
The system MUST prevent restored backups from reaccepting previously revoked
sessions. Recovery SHALL record its decisions durably and SHALL remain denied
while the restored authority or upstream grants are uncertain.

#### Scenario: Restore predates logout
- **WHEN** a database backup from before logout is restored
- **THEN** old cookies and stale instances remain unusable under the externally advanced session epoch, and grant reconciliation is required before reopening authentication.

#### Scenario: Restart with incomplete credential attempt
- **WHEN** startup finds an attempt that did not reach a known durable outcome
- **THEN** the session requires new authentication and the unresolved attempt remains visible for cleanup.
