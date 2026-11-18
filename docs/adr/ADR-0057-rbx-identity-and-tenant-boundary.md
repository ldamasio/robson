# ADR-0057: RBX Identity and tenant-bound Robson access

**Date:** 2026-11-18
**Status:** Accepted for implementation; production activation remains gated

## Context

ADR-0054 replaced a shared API token with Google ID tokens and an email
allowlist. That made credentials shorter-lived, but it bypassed the RBX
identity plane, coupled authorization to an email address, and did not bind an
authenticated identity to Robson's projection tenant.

Robson clients now include the web application, a Tauri mobile application,
and the CLI. RBX products need one identity contract across those clients:
Google is a federated login provider, while ZITADEL/RBX Identity remains the
issuer and authorization authority.

ADR-0055 already decided the product-session split between a web BFF, native
PKCE clients, and CLI. This ADR implements the resource-server, tenant, role,
and CLI seams needed for that direction. It does not claim that the durable web
session, early-revocation, real-IdP, or native acceptance gates in ADR-0055 are
complete.

## Decision

`robsond` accepts only RBX Identity/ZITADEL access tokens for human access.
Static API tokens and Google-issued tokens are rejected.

The verifier requires all of the following:

- an exact HTTPS issuer and API audience;
- an RS256 signature from the bounded, cached issuer JWKS;
- valid `exp` and `nbf` claims;
- an authorized public client ID;
- the canonical ZITADEL `sub` of an allowed owner;
- a Robson project role granted by an allowed ZITADEL organization.

Authorization is split into exact roles:

- `rbx:robson:observer` for trading state, event history, SSE, and funding
  reads;
- `rbx:robson:operator` for ordinary position and reconciliation mutations;
- `rbx:robson:funding` for funding and capital mutations;
- `rbx:robson:emergency` for panic, monthly-halt, recovery disposition, and
  recovery release actions.

Any authorized role implies observer access, but mutation roles do not imply
one another. Only health, readiness, and Prometheus metrics remain public.

The configured `ROBSON_OIDC_TENANT_BINDING` maps exactly one ZITADEL
organization to exactly one local tenant UUID. Production startup requires the
bound UUID to equal `PROJECTION_TENANT_ID`; the verified organization-scoped
role therefore selects the tenant returned by `/auth/session`. The initial
isolation model is one Robson runtime, database projection scope, exchange
credential set, and event stream per tenant. A future shared runtime must not
be introduced without row-level tenant enforcement and denial tests.

Client authentication follows the RBX application profile:

- web deployments may use the product BFF and an HttpOnly session cookie;
- Tauri uses Authorization Code with PKCE as a public client and never embeds
  a client secret;
- the CLI uses an RBX Identity public-client flow supported by its registered
  client;
- authorization requests include the tenant organization's ZITADEL scope so
  its Google-only login policy and role context apply;
- Tauri refresh credentials are stored by the operating-system secure store,
  never browser storage. The CLI keeps its credential cache in a mode-`0600`
  file under the user's configuration directory.

Client registrations, audiences, organization grants, and the owner's
canonical subject are deployment configuration. Source code and manifests
must not invent placeholder identities or silently fall back to another auth
scheme when they are absent. Every public-client registration used by Robson
asserts its granted project roles in the access token; clients request the
Robson API project audience explicitly.

## Consequences

RBX Identity becomes the single place for login federation, revocation,
organization membership, and Robson roles. A Google account alone no longer
grants access. The owner's existing trading state remains associated with the
tenant-scoped runtime rather than with a reusable API key.

Production startup now fails closed when identity or tenant configuration is
incomplete, and stale direct-Google or API-token variables are rejected. An
identity outage prevents new authenticated requests; existing trading safety
logic continues inside the daemon.

JWT verification does not provide instantaneous revocation. Before production
activation, the actual access-token lifetime and revocation bound must satisfy
ADR-0055's acceptance gates; the web BFF retains its own session and token
lifecycle. No opaque-token introspection fallback is implied by this change.

Rollout requires coordinated client registrations and role grants before the
backend cutover. Rollback uses the prior application image and configuration;
there is no live dual-authentication bridge.

For the Google-only human login requirement, the tenant organization's
ZITADEL login policy disables username/password login, enables external IdP
login, and links only the approved Google provider. The product clients select
that policy with the organization scope; the API continues to trust only the
resulting ZITADEL access token.

## Alternatives

- Direct Google token verification was rejected because it bypasses RBX
  organizations, roles, revocation, and the common product identity contract.
- A static token compatibility window was rejected because possession of the
  old secret is not proof of the owner's identity.
- Email allowlisting was rejected as an authorization key because email is a
  mutable presentation attribute; the ZITADEL subject is canonical.
- A shared multi-tenant Robson runtime was deferred until storage and exchange
  adapters can enforce tenant scope structurally.

## Supersedes

ADR-0054. ADR-0025 remains historical context for the original static-token
frontend.
