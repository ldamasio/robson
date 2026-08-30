# IAM-P1 RBX Identity and Google-Federated Login

Status: Repository-verified; operational activation pending
Date: 2026-08-30
Decision: ADR-0055

## Objective

Replace the fake health-probe login with a real RBX Identity contract, make the
Android client an observer-scoped OIDC public client, and prepare a safe
dual-auth migration from `ROBSON_API_TOKEN`.

## Current Repository Reality

- The Android client implements Authorization Code with S256 PKCE through the
  Capacitor system-browser and app URL APIs.
- The native callback is registered in `AndroidManifest.xml` as
  `br.ia.rbx.robson://oauth/callback`.
- The client validates its stored transaction, expiry, exact callback, state,
  optional response issuer, discovery issuer, and token response before storing
  the access token in memory.
- The client never accepts or stores a client secret and does not request a
  refresh token.
- `robsond` validates RS256 JWTs through a bounded JWKS cache and enforces exact
  issuer, audience, allowed clients, allowed RBX organization ids, and project
  roles.
- Read-only product data and SSE now require observer authentication. Health
  probes and Prometheus metrics remain public for platform integration.
- `GET /auth/session` replaces public `/health` as the login proof.
- The legacy token remains available to the operator web build and can coexist
  with OIDC. It is disabled in `.env.android`.
- Google is not yet configured and no ZITADEL/Google secret or production
  client identifier exists in this repository.

## Client Session Lifecycle

- An OIDC token response must include a positive integer `expires_in`. The
  client rejects the response instead of creating a session with an unknown
  lifetime.
- The access token and its expiry deadline remain in memory. A bounded timer
  clears the session at expiry, while `visibilitychange` and `pageshow` checks
  cover application suspension and resume.
- Starting an OIDC session removes any migration-only legacy token from browser
  session storage. The Android build still has no normal API-token login path.
- Logout and expiry currently clear only client-local state. Provider session
  termination and token revocation remain pending an approved RBX Identity
  contract for the relevant endpoints, token types, and client requirements.
- Refresh-token issuance, secure persistence through Android Keystore, and
  background renewal are not implemented.

## Operational Values Required

The identity and infrastructure owners must resolve and inject these values.
No value below is a secret except the Google provider secret, which never
enters Robson.

### ZITADEL registrations

1. Confirm the canonical issuer. RBX source currently contains both
   `auth.rbx.ia.br` and `auth.merovelis.com`; do not activate Robson against an
   inferred issuer.
2. Register a native public client for package `br.ia.rbx.robson` with exact
   callback `br.ia.rbx.robson://oauth/callback` and Authorization Code + PKCE.
3. Register a dedicated Robson API audience and configure JWT access tokens.
4. Create the four project roles from ADR-0055.
5. Assign `rbx:robson:observer` only to invited test users in the RBX
   organization.
6. Record the real project/audience scope and role-request scope emitted by
   ZITADEL. Do not use the logical placeholder `robson-api` unless it is the
   actual audience claim.

### Google federation in ZITADEL

1. Create the Google OAuth web application for ZITADEL itself.
2. Register the callback URL displayed by the ZITADEL Google provider template,
   not the Robson Android callback.
3. Store the Google client secret through the approved Identity/Infrastructure
   secret mechanism; never in Git, an APK, a prompt, or a Robson environment.
4. Limit scopes to `openid profile email`.
5. Enable external login at the RBX organization level for the internal Robson
   rollout.
6. Disable automatic privileged-account creation. Use invitation and explicit
   authenticated linking for existing identities.

### `robsond` environment

```text
ROBSON_OIDC_ISSUER=https://REPLACE_WITH_CONFIRMED_ISSUER
ROBSON_OIDC_AUDIENCE=REPLACE_WITH_ACTUAL_AUDIENCE
ROBSON_OIDC_JWKS_URI=https://REPLACE_WITH_CONFIRMED_ISSUER/oauth/v2/keys
ROBSON_OIDC_ALLOWED_CLIENT_IDS=REPLACE_WITH_NATIVE_CLIENT_ID,REPLACE_WITH_WEB_CLIENT_ID
ROBSON_OIDC_ALLOWED_ORGANIZATION_IDS=REPLACE_WITH_RBX_ORG_ID
```

Keep `ROBSON_API_TOKEN` during the dual-auth validation window. Rotate it before
rollout and remove it after all supported clients migrate.

### Android public build values

```text
PUBLIC_RBX_OIDC_ISSUER=https://REPLACE_WITH_CONFIRMED_ISSUER
PUBLIC_RBX_OIDC_CLIENT_ID=REPLACE_WITH_NATIVE_CLIENT_ID
PUBLIC_RBX_OIDC_REDIRECT_URI=br.ia.rbx.robson://oauth/callback
PUBLIC_RBX_OIDC_SCOPES=openid profile email REPLACE_WITH_AUDIENCE_SCOPE REPLACE_WITH_OBSERVER_ROLE_SCOPE
```

These values are public client metadata. A `PUBLIC_*_SECRET` variable must
never exist.

## Verification Gates

| Gate | Evidence | Status |
| --- | --- | --- |
| IAM-P1-G1 | OIDC config rejects partial/insecure contracts | Pass in Rust and TypeScript unit tests |
| IAM-P1-G2 | RS256 signature, issuer, audience, client, organization, and role validation | Pass with signed JWT fixture |
| IAM-P1-G3 | Observer cannot acquire operator/funding/emergency permission | Pass in backend unit tests |
| IAM-P1-G4 | `/auth/session` rejects missing bearer and accepts migration token | Pass in router test |
| IAM-P1-G5 | PKCE authorization request contains S256/state and no client secret | Pass in frontend unit test |
| IAM-P1-G6 | Callback rejects wrong state or invalid token lifetime | Pass in frontend unit tests |
| IAM-P1-G7 | Capacitor sync, Android debug build, install, and launch | Pass on an authorized physical POCO |
| IAM-P1-G8 | Physical Google/RBX login and authenticated SSE on POCO | Pending operational registration |
| IAM-P1-G9 | Google account cannot self-create privileged Robson access | Pending ZITADEL policy verification |

## Rollout Sequence

1. Confirm issuer and create registrations/roles in a non-production or
   explicitly controlled RBX Identity scope.
2. Build Android with the real public client metadata.
3. Grant one invited test identity only the observer role.
4. Verify dashboard, event history, SSE reconnection, logout, app restart, wrong
   audience, removed role, and provider outage behavior on the POCO.
5. Enable OIDC beside the rotated legacy token in the deployed `robsond`.
6. Implement and deploy the web session BFF before removing the legacy web
   login.
7. Remove `ROBSON_API_TOKEN` and its frontend form in a separately verified
   cleanup.

## Operational Non-Claims

- Google federation is decided but not configured by this repository change.
- No ZITADEL client, project role, user grant, secret, DNS record, or production
  deployment has been created.
- The issuer conflict is not resolved by choosing the value in `.env.android`;
  that file remains non-operational until a real client id and approved scopes
  are injected.

## Build Evidence

- Frontend type-check completed with zero errors and zero warnings.
- All 127 frontend unit tests passed; ESLint reported zero errors and the same
  eight pre-existing object-indexing warnings.
- Normal and Android-mode static builds, Capacitor sync, and Gradle
  `assembleDebug` passed.
- The updated debug APK installed successfully and Robson reached the foreground
  on one authorized physical POCO. No device setting was changed.
- Debug APK: `frontend/android/app/build/outputs/apk/debug/app-debug.apk`
  (4,360,586 bytes).
- SHA-256:
  `390af73ad36004c4aac1ca06c69be1d45ad6a26e201e85fcb620ac9daa533c67`.
