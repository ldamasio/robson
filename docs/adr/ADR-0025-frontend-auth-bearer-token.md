# ADR-0025: Frontend Auth — Bearer Token via Robsond

**Date:** 2026-04-23 (original decision); 2026-04-25 (formalized as ADR)
**Status:** Accepted

## Context

The v3 frontend (SvelteKit + `adapter-static`) is hosted as a static
single-page application. The original FE-P1 plan called for GitHub
OAuth backed by Auth.js (`hooks.server.ts` + `SvelteKitAuth`).

`adapter-static` produces a fully static bundle: no Node runtime, no
edge function, no server-side hooks. Auth.js requires a server
runtime to handle the OAuth callback URL and session cookies. The two
choices are mutually exclusive.

Three paths existed:

1. Switch to `adapter-node` and run a Node process behind a reverse
   proxy. Adds a stateful surface to operate, undermining the
   serve-it-from-anywhere goal.
2. Add an edge function (Cloudflare Workers, Vercel) just for the
   OAuth callback. Adds a third deployment target and a vendor
   dependency.
3. Drop OAuth and authenticate the operator directly against the
   robsond API with a Bearer token.

The primary user is a single operator. The system is governed and
audited end-to-end by robsond regardless of who triggers an action;
identity at the frontend exists only to gate access to the surface.

## Decision

The frontend authenticates with a Bearer token issued by robsond.

- Token entered manually by the operator on `/login`.
- Persisted in `sessionStorage` under key `robson_api_token`.
- Sent on every REST call as `Authorization: Bearer <token>`.
- Sent on the SSE stream as `?token=<token>` query parameter, since
  the browser `EventSource` API does not allow custom headers.
- Validated on `/login` by issuing `GET /health` and only navigating
  to `/dashboard` if the request succeeds.
- Cleared by `clearAuth()` (called on explicit logout).

## Consequences

**Positive**

- Frontend stays a pure static bundle — works on any object-storage
  or container nginx host without runtime constraints.
- One fewer system to operate (no Auth.js, no OAuth app, no callback
  routing).
- Token can be rotated by reissuing on the robsond side without any
  frontend redeploy.
- Backend gates everything anyway, so trust boundary is unchanged.

**Negative / trade-offs**

- No SSO; operators paste a token manually.
- `sessionStorage` is per-tab and lost on close; this is acceptable
  for a single-operator console but does not scale to multi-user.
- SSE token in query string can leak to access logs. Robsond logs
  must filter the `token` parameter before persisting access logs
  (see implementation note below).

## Alternatives

- **Auth.js + adapter-node** — rejected because it adds a stateful
  Node service and a domain-locked callback that conflicts with
  dual-domain hosting (rbx.ia.br + rbxsystems.ch).
- **Auth.js + edge function** — rejected because it pulls in a
  vendor (Cloudflare Workers / Vercel) for a single endpoint, and
  requires synchronizing an OAuth app with two callback URLs.
- **Custom session cookies set by robsond** — rejected because
  cross-origin cookies require backend-controlled `Set-Cookie` plus
  `SameSite=None; Secure` plumbing on every response, and the
  operator-paste-token flow is simpler.

## Implementation Notes

- Auth store: `apps/frontend/src/lib/stores/auth.ts`
  (`authToken`, `setToken`, `clearAuth`, `initAuth`).
- API client: `apps/frontend/src/lib/api/robson.ts`
  attaches `Authorization` header from `authToken`.
- Login flow: `apps/frontend/src/routes/login/+page.svelte`
  validates the token via `robsonApi.health()`.
- Auth guard: `apps/frontend/src/routes/(authed)/+layout.svelte`
  redirects to `/login` when no token is present.
- Root redirect: `apps/frontend/src/routes/+page.svelte` sends `/`
  to `/dashboard` if a token exists, otherwise to `/login`.

**Backend log-hygiene requirement.** Robsond access logs MUST strip
the `token` query parameter before persisting. The token is in the
URL only because `EventSource` cannot send headers; logging it
defeats the security boundary.

**Related**: ADR-0027 (CORS layer for production origins).
**Supersedes**: prior FE-P1 plan to use Auth.js + GitHub OAuth.
