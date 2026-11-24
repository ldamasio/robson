# ADR-0058: Tauri and RBX Identity Client Standard

**Date:** 2026-11-18

**Status:** Accepted for implementation; operational rollout pending

**Depends on:** ADR-0057 (RBX Identity and tenant boundary)

## Context

ADR-0054 made the browser send a Google ID token directly to `robsond` and
persisted it in `sessionStorage`. That no longer matches the RBX identity
boundary: Google is an external identity provider behind RBX Identity
(ZITADEL), while `robsond` accepts only RBX Identity access tokens with an
organization-scoped role claim and a tenant-bound subject.

Robson also needs the same mobile packaging standard as Strategos. The
existing Android experiment used Capacitor and is not part of the current
`main` branch. The current web client is a static SvelteKit application.

## Decision

Robson clients use **Tauri 2 + SvelteKit + RBX Identity**. Google remains the
human-facing sign-in choice, but the application never accepts or forwards a
Google token directly.

```text
Google account
      |
      v
RBX Identity / ZITADEL -- organization policy: Google only
      |
      | Authorization Code + PKCE
      v
+----------------------+       Bearer access token       +---------+
| Tauri Rust host      | ------------------------------> | robsond |
| token: process memory|                                 +---------+
| REST + SSE owner     |
+----------------------+

Browser -- preferred --> BFF HttpOnly session --> robsond
        -- fallback  --> SPA PKCE memory token --> robsond
```

### ZITADEL client contract

The native application is a public client. It has no client secret and must
be registered with:

- exact redirect URI `br.ia.rbx.robson://oauth/callback`;
- Authorization Code grant with PKCE S256;
- `accessTokenRoleAssertion=true`, so project roles are present in the access
  token that `robsond` verifies;
- the real Robson client ID and project ID; placeholder IDs are forbidden.

Every authorization request contains:

- the configured base scopes, including `openid`;
- `urn:zitadel:iam:org:id:<orgId>`;
- `urn:zitadel:iam:org:project:id:<projectId>:aud`;
- `urn:zitadel:iam:org:roles:id:<orgId>` to filter the role assertion to the
  RBX organization;
- optionally, `urn:zitadel:iam:org:idp:id:<googleIdpId>` only when the real
  Google IdP ID is configured.

The RBX organization login policy must disable local username/password login,
enable external IdP login, and expose only the approved Google IdP. The
optional IdP scope is a direct routing optimization, not the security policy.
The organization policy remains authoritative when that ID is absent.

### Credential boundary

- Tauri exchanges the code in Rust and keeps the access token only in
  zeroizing process memory. It is never returned to the WebView and never
  written to `localStorage`, `sessionStorage`, a file, logs, or Android backup.
- Native REST and SSE connections originate in Rust, which adds the bearer
  header. Only response bodies and application event payloads reach Svelte.
- Restarting the app or reaching token expiry requires authorization again.
  The existing ZITADEL/Google SSO session normally makes that inexpensive.
- Browser BFF mode uses a Secure, HttpOnly, SameSite session cookie. Direct
  browser PKCE remains an explicit compatibility mode and keeps its access
  token only in JavaScript memory. Only the one-time PKCE verifier and state
  survive the browser redirect in `sessionStorage`.
- No API key, static bearer token, Google ID token, or legacy fallback is
  accepted by the client.

### Backend contract

The client depends on ADR-0057 and uses `GET /auth/session` as the authenticated
probe. The response identifies the RBX issuer, subject, client, organization
roles, permissions, and projection tenant. `/health` is not an authentication
probe.

## Options considered

| Option                          | Token boundary                                                                                           | Mobile fit                                  | Decision |
| ------------------------------- | -------------------------------------------------------------------------------------------------------- | ------------------------------------------- | -------- |
| Tauri Rust-owned OAuth and HTTP | Token remains outside WebView and web storage                                                            | Native Android and desktop from one shell   | Selected |
| Capacitor with JavaScript OAuth | Token lives in the WebView runtime                                                                       | Android-capable but diverges from Strategos | Rejected |
| Remote web launcher only        | BFF cookie can be HttpOnly, but external-browser callback cookies do not reliably cross into the WebView | Thin shell, weak offline/native control     | Rejected |
| Direct Google GIS               | Google ID token reaches application JavaScript and bypasses RBX Identity roles/tenant policy             | Browser only                                | Rejected |

## Reliability and failure modes

| Dependency or failure                                            | Client behavior                                                      | Visibility and recovery                                                      |
| ---------------------------------------------------------------- | -------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| ZITADEL discovery or token endpoint unavailable                  | Login fails closed; no prior credential fallback                     | Login displays a generic identity error; retry after service recovery        |
| Google IdP unavailable or user denied                            | ZITADEL returns an OAuth error                                       | Login displays the sanitized provider error; no session is created           |
| Wrong issuer, organization, project audience, callback, or state | Callback/token is rejected before API use                            | Visible login failure; correct the client registration/configuration         |
| `robsond` unavailable                                            | Native REST fails and SSE reconnects with capped exponential backoff | Existing offline/stale dashboard indicators remain visible                   |
| SSE opens but becomes silent                                     | Rust read-idle watchdog trips after 45 seconds and reconnects        | Staleness callback marks the UI stale; activity/reconnect events are emitted |
| Access token expires or API returns 401                          | Native session is zeroized and all requests fail closed              | UI returns to login; user authorizes again through SSO                       |
| Tauri process dies                                               | In-memory token disappears                                           | Restart starts unauthenticated; no credential recovery file exists           |
| BFF dies                                                         | Browser cookie cannot be exchanged for an authenticated response     | Browser auth probe/API fails; no automatic fallback to a JavaScript token    |

There is one discovery request and one token exchange per login. API and SSE
operations do not perform per-item identity calls, so the design introduces no
N+1 identity request path. Identity metadata responses, API responses, SSE
buffers, concurrent streams, timeouts, and reconnect backoff are bounded.

## Trade-offs

Tauri adds a Rust/mobile toolchain and generated Android project to the
repository. Keeping the token outside the WebView also requires native REST
and SSE adapters instead of reusing browser `fetch` everywhere. The benefit is
a substantially narrower credential boundary and one RBX mobile standard.

The selected design leaves refresh-token persistence, offline session
restoration, iOS packaging, a verified HTTPS Android App Link, and app-store
signing outside this decision. The custom scheme is adequate for the first
public-client build because state and PKCE are mandatory; a verified App Link
remains a hardening follow-up.

## Operational prerequisites

Repository implementation does not prove operational rollout. Before a build
can log in, operators must provision and inject the real:

1. ZITADEL public client ID;
2. ZITADEL project ID/audience;
3. RBX organization ID (the repository example uses the already-established
   RBX organization ID);
4. optional Google IdP ID, only if direct IdP routing is desired;
5. matching backend allowlisted client IDs, audience, organization, subject,
   roles, and tenant binding from ADR-0057.

No deployment, ZITADEL mutation, signing-key creation, or production rollout
is performed by this ADR.
