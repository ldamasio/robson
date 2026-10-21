# Spec Delta

## Purpose

Coordinate credential renewal and invalidation across processes so that crashes,
concurrent callbacks and delayed network responses cannot resurrect authority.

## ADDED Requirements

### Requirement: One refresh attempt per credential generation
The system MUST coordinate a single refresh owner for each generation and MUST
reject stale completions. An expired owner lease or ambiguous external outcome
MUST NOT authorize reuse of the old refresh token.

#### Scenario: Concurrent refresh on two instances
- **WHEN** concurrent requests on separate BFFs find a credential due for renewal
- **THEN** one owner attempts renewal and other callers reuse its committed result or receive bounded unavailability.

#### Scenario: Activity during refresh
- **WHEN** legitimate foreground activity updates inactivity while a refresh is in flight
- **THEN** the refresh can commit under its unchanged authority fence, without sliding absolute expiry or ignoring actual logout/rotation.

#### Scenario: Crash after provider consumes refresh
- **WHEN** the IdP consumes the refresh credential but the new result is lost before durable commit
- **THEN** the session requires new login, the old credential is not replayed, and unresolved grant cleanup remains recorded.

#### Scenario: Late owner after lease expiry
- **WHEN** a suspended refresh owner resumes after recovery has changed its authority
- **THEN** its result cannot restore the session and any obtainable returned credentials enter cleanup.

### Requirement: Terminal invalidation wins
Logout, expiry and supersession SHALL commit terminal session state and cleanup
intent atomically. Once invalidated, the same session identity MUST never become
active again, including after failed refresh, process restart or step-up.

#### Scenario: Logout races with refresh and foreground activity
- **WHEN** logout commits while another process renews credentials or reports activity
- **THEN** neither result revives the session or extends its deadlines and the revoked cookie is rejected by all instances.

#### Scenario: Cannot confirm global logout
- **WHEN** the shared store cannot commit invalidation
- **THEN** the response reports unavailable and unconfirmed global logout, replaces the authentication cookie with a bounded signed non-authorizing logout intent after validating the request, and the UI does not claim global revocation succeeded.

#### Scenario: Logout intent survives outage and reload
- **WHEN** the store recovers and a BFF receives a valid logout intent after browser reload
- **THEN** it idempotently commits terminal invalidation before admitting login or authenticated work, and cannot use the intent to read data or revoke another session.

### Requirement: Durable bounded revocation cleanup
The system SHALL retain cleanup intent across restarts, handle partial and
ambiguous revocation outcomes, and expose unresolved work. Retry, retention and
capacity policies MUST NOT silently discard still-live credentials or claim
revocation without evidence.

#### Scenario: Provider revocation outage
- **WHEN** local invalidation commits but upstream revocation fails
- **THEN** local logout stays effective, pending cleanup survives restart, and bounded retries or explicit remediation remain visible.

#### Scenario: Revoke response is insufficient evidence
- **WHEN** revocation returns success but affected access/family status is still active or unknown
- **THEN** the system distinguishes requested revocation from verified containment and retains unresolved cleanup instead of declaring it complete.

#### Scenario: Replacement grant isolation
- **WHEN** a new interactive login or step-up replaces a grant
- **THEN** cleanup must not revoke the newly accepted grant; unsupported provider isolation blocks activation of that replacement behavior.

#### Scenario: Cleanup capacity reached
- **WHEN** pending cleanup reaches its configured bound
- **THEN** the system applies backpressure to new credential issuance while preserving invalidation and its pre-reserved cleanup capacity, including concurrent logout and expiry at the limit.

### Requirement: Revocation reaches open streams and new requests
The system SHALL reject newly authorized requests after terminal session state
and SHALL revalidate open streams within the 60-second target. Authority failure
MUST close streams and show stale/offline state. No mutation may be automatically
replayed after auth, transport or process failure.

#### Scenario: Another instance logs out a streaming session
- **WHEN** logout commits while a stream is open through a different BFF
- **THEN** the stream closes within the revalidation bound and reconnect cannot reuse the revoked session.

#### Scenario: Write was already dispatched
- **WHEN** logout races with a write already dispatched under valid authorization
- **THEN** the UI does not claim cancellation, the response is handled without automatic replay, and daemon action gates remain authoritative.

### Requirement: Protective controls retain independent enforcement
Recent-auth expiry alone SHALL NOT prevent authorized protective close, panic or
halt actions. Unavailable or invalid base authority MUST still deny manual API
writes. Daemon risk controls and exchange stops MUST remain independent of UI
session and identity-provider availability.

#### Scenario: Freshness expired versus provider outage
- **WHEN** the session is valid but recent authentication has expired
- **THEN** sensitive writes require step-up while protective writes remain authorized; if identity authority is unavailable both types fail closed without disabling daemon/exchange protection.
