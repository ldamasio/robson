# Proposal: Durable Robson product sessions

## Why

ADR-0055's local session prototype fixes stale authentication state and defines
remembered sessions, but its sessions, pending logins and revocation cleanup
disappear on restart. Refresh coordination is confined to one process. Those
properties cannot support reliable remembered login or multiple BFF instances.

The operator accepted the architectural direction and authorized implementation
planning on 2026-10-20, retaining the local-only, no-deployment scope. This change
is a pending implementation plan, not evidence of production readiness. The
starting code is commit `d9ec2b08` (implementation in `a0e50c91`).

## What Changes

- Introduce an asynchronous storage contract and a PostgreSQL-backed session
  store with encrypted credentials, single-use authorization transactions,
  terminal revocation records and a durable cleanup queue.
- Coordinate refresh across processes using persisted generations and fenced
  attempts. An ambiguous external exchange requires authentication again; a
  crashed worker's refresh token is never automatically replayed.
- Define restart, datastore loss, key rotation, backup restore and logout
  failure behavior, including visible evidence and bounded resource use.
- Add isolated integration and fault-injection acceptance for the actual RBX
  identity provider. Local fixture results cannot establish provider guarantees.
- Preserve ADR-0055's session deadlines, recent-auth rules, CSRF protections,
  canonical operator identity and independent daemon/exchange risk enforcement.

## Capabilities

### New Capabilities

- `durable-product-sessions`: session continuity, trusted deadlines, encrypted
  persistence, single-use login and safe recovery across restarts.
- `distributed-credential-lifecycle`: concurrent refresh, logout, replacement,
  revocation cleanup and stream invalidation across BFF processes.
- `rbx-identity-acceptance`: evidence required for real provider integration,
  workload bounds and failure visibility before any operational rollout.

### Modified Capabilities

None in the OpenSpec baseline, which is being introduced with this change.
Existing behavior remains documented in ADR-0055 and repository tests; adding
delta specs does not imply that the current product has no authentication.

## Impact

Expected code boundaries: `robson-session-bff/src/{sessions,server,oidc,main}.mjs`,
new storage/cleanup modules and tests, root `migrations/`, PostgreSQL test harness
in `robsond/tests/`, `scripts/test-pg.sh`, the frontend session store, and the
daemon's explicit sandbox configuration. Exact files may be split during coding.

Infrastructure owns database provisioning, roles, secret delivery, test-provider
registration and TLS. This plan creates no infrastructure and sends no requests
to a real identity or trading environment. A provisioned test environment is a
dependency for acceptance, not a reason to bypass repository ownership rules.

CLI/Tauri adapters, production activation and an organization-wide RBX standard
remain follow-ups. This iteration establishes their session contract, not their
implementation. Planning and implementation progress live in [tasks.md](tasks.md);
the OpenSpec Studio Kanban is a projection of the change and its task checklist,
using the same repository files. See [studio.md](studio.md) for the checked
integration boundary; no agent flow is scheduled by preparing this plan.

Reference decision: [ADR-0055](../../../docs/adr/ADR-0055-product-sessions-web-native-cli.md).
Planning review and dispositions:
[Fable 5.1](../../../docs/reviews/2026-10-20-durable-session-plan-review.md).
