# ADR-0055: RBX Identity OIDC and Role Authorization

Status: Accepted; repository implementation complete; operational activation pending
Date: 2026-08-30

## Context

Robson's original frontend authentication used one manually entered static
bearer token. The Android MOB-P1 smoke test proved that the login page accepted
any non-empty value because it probed public `GET /health`. The same production
token also authorized observer reads, command mutations, funding, and emergency
actions. That model cannot provide RBX-wide SSO, named-user audit identity, or
least-privilege mobile access.

RBX already owns a shared identity boundary built around standards-based OIDC
and ZITADEL. Google must be an upstream identity provider brokered by that
boundary; Robson must not trust Google tokens or embed a Google client secret.

## Decision

Robson uses RBX Identity as its only human identity issuer.

- Android is a native public OIDC client using Authorization Code with PKCE in
  the system browser/Chrome Custom Tab. It has no client secret.
- The Android callback is `br.ia.rbx.robson://oauth/callback`. PKCE and exact
  callback/state/issuer validation protect the code exchange. A verified HTTPS
  Android App Link remains the preferred release-hardening follow-up.
- The OIDC access token is kept in memory and is sent to `robsond` as a bearer
  token. No refresh token or `offline_access` scope is requested in this slice.
- Google is configured in ZITADEL, preferably at the RBX organization boundary
  for internal Robson access. Google authentication never grants a Robson role.
- Automatic account creation is disabled for Robson operators. Linking a Google
  identity to a privileged existing RBX identity requires an authenticated
  linking flow or administrator-controlled onboarding; email equality alone is
  not authorization.
- Production web authentication will use the RBX session BFF and an
  `HttpOnly`, `Secure`, `SameSite` cookie. Direct browser tokens remain outside
  the production target. The current static web token login remains a migration
  fallback until that BFF integration is deployed.
- `robsond` validates JWT signature, exact issuer, audience, expiry,
  not-before, signing algorithm, authorized client id, and organization-scoped
  ZITADEL project roles. Provider domains and email addresses are not
  authorization inputs.
- `GET /auth/session` is the authenticated probe. Public `GET /health` is never
  evidence of authentication.

```text
 Google / RBX passkey / MFA
             |
             v
   +----------------------+
   | RBX Identity/ZITADEL |
   | canonical subject    |
   | project role grants  |
   +----------+-----------+
              | Authorization Code + PKCE
              v
   +----------------------+       Bearer JWT       +------------------+
   | Robson Android       | ---------------------> | robsond          |
   | system browser       |                        | issuer/aud/JWKS   |
   | memory-only token    | <--------------------- | role enforcement |
   +----------------------+   protected data/SSE   +------------------+

   Robson Web -> RBX session BFF -> robsond       (target rollout)
```

## Authorization Matrix

| ZITADEL project role | Backend permission | Initial surface |
| --- | --- | --- |
| `rbx:robson:observer` | `observer` | Status, positions, safety, halt state, event history/SSE, funding history |
| `rbx:robson:operator` | `operator` plus observer | Arm, signal, approve, close, reconcile, income acknowledgement |
| `rbx:robson:funding` | `funding` plus observer | Funding quote/execute/recovery and capital refresh |
| `rbx:robson:emergency` | `emergency` plus observer | Panic and MonthlyHalt trigger |

Roles are independent. `operator` does not imply `funding` or `emergency`.
The temporary legacy token grants all four permissions only during migration.

## Decision Matrix

| Option | SSO | Secret isolation | Product role control | Main trade-off |
| --- | --- | --- | --- | --- |
| RBX Identity brokers Google; native PKCE plus web BFF | Yes | Strong | Backend-enforced | Requires IdP/client/BFF operations |
| Direct Google integration in every product | Partial | Duplicated per app | Fragmented | Google identity becomes coupled to every product |
| Direct SPA/native OIDC tokens everywhere | Yes | No client secret | Backend-enforced | Browser token exposure and weaker revocation model |
| Shared static API token | No | Weak | None | No named identity or least privilege |
| Custom RBX password/social-login service | Possible | High implementation risk | Custom | Reimplements an identity provider |

The first option is selected. Native PKCE is appropriate for Android; the BFF
is appropriate for production browsers.

## Performance and Resource Controls

- Authentication adds no database query and no per-item request loop.
- JWKS is cached for ten minutes, capped at 32 keys and 256 KiB per response.
- An unknown `kid` may trigger one refresh, rate-limited to once per 30 seconds,
  preventing attacker-controlled JWKS request storms.
- Failed/stale-cache refreshes are also retried at most once per 30 seconds.
- JWKS fetches have a five-second timeout. Authentication never waits
  indefinitely on the identity provider.
- Native listeners are removed when the login route is destroyed. Access
  tokens and OIDC transactions have bounded lifetimes.

## Failure Modes

| Dependency or failure | Behavior | Visibility and recovery |
| --- | --- | --- |
| Google unavailable | Existing RBX password/passkey methods may still authenticate | ZITADEL login reports provider failure; retry with RBX method |
| ZITADEL login/token endpoint unavailable | New login fails; no credentials are accepted locally | Login error; retry after identity recovery |
| JWKS endpoint unavailable with stale/missing cache | `robsond` rejects OIDC authentication with `503 identity_unavailable` | Backend warning without token contents; restore IdP/JWKS |
| ZITADEL rotates signing key | Unknown `kid` causes one bounded refresh | New key becomes valid after refresh; repeated misses are rate-limited |
| Role is missing or granted by another organization | Request fails with `403 insufficient_robson_role` | Correct the RBX organization grant; email/domain matching cannot bypass it |
| Role/session is revoked while a JWT is live | Local JWT validation may accept it until short expiry | Use short access-token TTL; urgent rollout also revokes product sessions and removes the grant |
| `robsond` dies | Client cannot read or mutate; trading safety remains server/exchange-owned | Health/readiness and client connection errors; restart `robsond` |
| Database, exchange, or market feed dies | Authentication may succeed, but business endpoints fail closed or expose stale/blocker state | Existing readiness, blocker, and stale-stream controls remain authoritative |
| Android/WebView process dies | Memory-only OIDC token is lost | Restart and authenticate again; no trading responsibility lives on device |
| Device is lost | No durable OIDC or refresh token exists in this slice | Revoke the ZITADEL session/grant and uninstall remotely if managed |
| Both OIDC and legacy auth are absent in production | `robsond` refuses startup | Configuration error names the missing authentication contract |

## Trade-offs and What This Leaves on the Table

- Memory-only Android tokens favor device-loss safety over session continuity.
- Local JWT validation avoids one introspection call per API request but cannot
  observe immediate revocation; short token lifetime is mandatory.
- A custom-scheme callback ships before verified Android App Links/passkeys.
- Google federation, client registration, issuer selection, role grants, and
  production secrets are operational changes outside this repository and are
  not performed by merging this code.
- Global single logout, refresh-token rotation, Android Keystore persistence,
  biometrics, and production web BFF wiring remain follow-ups.

## Migration and Rollback

1. Confirm the canonical production issuer and register the Robson API, native,
   and web/BFF clients.
2. Activate OIDC beside `ROBSON_API_TOKEN` and validate observer access on the
   physical Android device.
3. Activate the web BFF and validate operator, funding, and emergency roles.
4. Rotate and remove `ROBSON_API_TOKEN` after all supported clients migrate.

Rollback disables the Robson OIDC client/configuration while retaining the
rotated legacy token. It does not disable RBX Identity or Google for unrelated
products.

## Related Decisions

- ADR-0025: legacy frontend bearer-token authentication
- ADR-0027: CORS at the `robsond` boundary
- ADR-0054: Android client through Capacitor
