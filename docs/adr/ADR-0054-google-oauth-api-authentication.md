# ADR-0054: Google OAuth (ID Token) API Authentication, replacing Bearer Token

**Date:** 2026-09-10
**Status:** Proposed

## Context

Since ADR-0025, robsond (the Rust/Axum daemon) has authenticated
operator-sensitive and mutating API routes with a single static Bearer
token (`ROBSON_API_TOKEN`), compared via exact string match in
`robsond/src/api.rs`. There is no user concept: the token is a shared
secret, not an identity.

That has been adequate but has real limitations:

- **No identity binding.** The token authenticates "someone who has the
  string," not the operator (`ldamasio@gmail.com`) specifically. Anyone
  who obtains the value — a leaked log line, a shared clipboard, a stale
  CI secret — has full access indefinitely.
- **No expiry.** The token is valid until manually rotated. There is no
  time-bounded blast radius for a leak.
- **Manual rotation across three surfaces.** Rotating the token means
  updating it in robsond's config, the frontend's `sessionStorage` (paste
  again), and `robson-cli`'s `ROBSON_API_TOKEN` env var, by hand, with no
  built-in coordination between them.
- **No SSO.** The operator pastes a token manually on `/login`
  (ADR-0025's accepted trade-off at the time).

ADR-0025 evaluated OAuth for the frontend (GitHub via Auth.js /
SvelteKitAuth) and rejected it specifically because that flow requires
`adapter-node` plus a callback server and session cookies — incompatible
with the frontend's fully static `adapter-static` SvelteKit build and its
"serve from anywhere" goal across two production origins
(`robson.rbx.ia.br`, `robson.rbxsystems.ch`, both calling
`api.robson.rbx.ia.br`). That objection was correct and remains correct
for any OAuth flow that needs a server-side redirect/callback endpoint.

**What changed:** Google Identity Services (GIS) is a different shape of
OAuth flow than the one ADR-0025 evaluated. GIS runs entirely client-side
in the browser and hands the ID token directly to page JavaScript via a
callback — there is no redirect URI, no callback server, and no session
cookie. It fits inside a static bundle exactly as it stands today. This
ADR amends ADR-0025 on that basis; it does not reopen or reverse
ADR-0025's decision that a callback-server-based flow is incompatible
with this frontend.

The system remains single-operator by design (see ADR-0007). This
decision does not add multi-tenancy or a user directory — it replaces
"knows the shared secret" with "is the one Google account," which is a
strictly narrower and more identity-bound admission check for the same
one operator.

## Decision

Replace the static bearer token with **stateless, per-request
verification of Google-issued ID tokens**. robsond issues no session of
its own.

1. **Verification (robsond, every request).** The `Authorization: Bearer
   <token>` header must carry a Google ID token (a JWT). robsond:
   - Fetches and caches Google's public signing keys from
     `https://www.googleapis.com/oauth2/v3/certs` (JWKS), refreshed
     periodically in the background and on-demand for an unrecognized
     `kid`, never per-request (`robsond/src/google_jwks.rs`).
   - Verifies the RS256 signature, `exp`, and that `iss` is one of
     Google's known issuer strings and `aud` is one of robsond's
     configured client ids (`robsond/src/auth.rs`).
   - Additionally requires `email_verified == true` and `email`
     case-insensitively equal to a single allowlisted address
     (`ROBSON_ALLOWED_EMAIL`) — the single-operator admission check.
   - Returns a generic 401 on any failure and logs the real reason
     server-side only (`tracing::warn!`), never in the HTTP response.
2. **Frontend (GIS, client-side only, zero callback server).** `/login`
   loads `https://accounts.google.com/gsi/client`, calls
   `google.accounts.id.initialize({ client_id, callback })` and renders
   the standard Google button. The callback receives `response.credential`
   (the ID token) and stores it exactly where the old bearer token lived
   (`sessionStorage`, renamed key `robson_google_id_token`). No redirect
   URI is registered or needed. A silent-refresh helper
   (`google.accounts.id.prompt()`) runs near expiry (~10 minutes before
   the ID token's ~1h lifetime ends) and on a 401, so a long-lived tab
   keeps working without an interruption most of the time.
3. **CLI (`robson-cli`).** Authenticates via the OAuth 2.0 **Device
   Authorization Grant** (RFC 8628) against Google's device endpoints —
   the standard flow for a CLI with no browser-embeddable redirect.
   `robson auth login` prints a user code and verification URL, polls
   until approved, and persists `{id_token, refresh_token, expiry}` to
   `~/.config/robson/credentials.json` (mode `0600`), refreshed
   transparently thereafter.
4. **Two accepted client ids, one allowlisted human.** robsond accepts
   either `ROBSON_GOOGLE_WEB_CLIENT_ID` (the frontend's GIS client) or
   `ROBSON_GOOGLE_CLI_CLIENT_ID` (the CLI's Device Authorization Grant
   client, a Google "TVs and Limited-Input devices" client type) as a
   valid `aud`. Both still resolve to the same one operator via the
   `ROBSON_ALLOWED_EMAIL` allowlist.

### Rollout bridge (temporary)

Because robsond is a live production system, cutting the old token off
and rolling out Google auth cannot safely be one atomic step. The auth
middleware in `robsond/src/api.rs` dual-accepts during rollout: it
inspects whether the presented bearer credential is JWT-shaped (three
dot-separated base64url segments) and, if so, verifies it as a Google ID
token; otherwise, if a legacy token is still configured via the
separately-named `ROBSON_LEGACY_API_TOKEN` env var
(`ApiConfig::legacy_api_token`), it falls back to the old exact-string
comparison. The old `ROBSON_API_TOKEN` name is rejected outright at
startup (`reject_removed_api_token_env`) so a rollout deploy must
explicitly rename the value, rather than silently keeping the previous
scheme's full blast radius under the same variable name. Every line of
this path is marked `TEMPORARY: dual-accept bridge during ADR-0054
rollout, remove after cutover`. Removing the bridge and revoking the old
token is a follow-up the repo owner performs once Google auth is
confirmed stable in production — not part of the change that introduces
this ADR.

## Consequences

**Positive**

- **Identity-bound access.** Admission is "is Google-authenticated as
  `ldamasio@gmail.com`," not "knows a string." A leaked ID token is
  useless without a live Google session behind it and expires in ~1h
  regardless.
- **Simpler revocation.** Revoking access means removing the allowlisted
  email or the OAuth client in Google Cloud Console — no coordinated
  secret rotation across three surfaces.
- **Shorter blast radius.** A Google ID token's ~1h lifetime bounds how
  long a leaked credential is useful, versus the old token's indefinite
  validity.
- **No new backend statefulness.** Verification is stateless and
  per-request, matching robsond's existing shape — no session store, no
  new database table, no new operational surface to run.
- **ADR-0025's static-bundle constraint is preserved, not overridden.**
  GIS needs no callback server or `adapter-node`; the frontend stays a
  pure static bundle deployable from anywhere, exactly as ADR-0025
  requires.

**Negative / trade-offs**

- **Dependency on Google's identity infrastructure.** If
  `accounts.google.com` or the JWKS endpoint is unreachable, login and
  token verification both degrade. This is accepted as a reasonable
  trade-off for a single-operator system that already depends on Google
  services elsewhere in the org.
- **~1h expiry requires silent-refresh plumbing.** Unlike the old token
  (valid until rotated), the frontend and CLI both need working
  refresh paths (`google.accounts.id.prompt()` client-side; the stored
  `refresh_token` for the CLI) or the operator is logged out roughly
  hourly. This adds real client-side complexity that did not exist
  before.
- **Still single-operator by design.** This is an auth *mechanism*
  change, not a multi-tenancy change. Adding a second operator later
  means extending the allowlist (or replacing it with a real
  authorization model) — out of scope here.
- **Temporary dual-accept bridge is extra surface, on purpose.** The
  rollout bridge described above is deliberately temporary and flagged
  in code; it must be removed once Google auth is confirmed stable (see
  the repo owner's follow-up, not part of this change).

## Alternatives

- **Keep and rotate the static token more often** — rejected. Improves
  nothing about identity binding; only shrinks (without eliminating) the
  blast radius of a leak, at the cost of manual rotation discipline that
  has to be maintained forever.
- **Auth.js / SvelteKitAuth (GitHub or Google OAuth via a standard
  authorization-code flow)** — rejected, same objection as ADR-0025: it
  needs `adapter-node` plus a callback server and session cookies,
  incompatible with the static, dual-origin frontend. GIS's client-side
  flow is the reason this ADR does not need to revisit that objection.
- **robsond-issued session after a one-time Google login** (e.g.
  exchange the Google ID token once for a robsond-minted session
  cookie/token) — rejected as unnecessary complexity. It would reintroduce
  exactly the kind of backend statefulness (a session store, an
  expiry/renewal policy robsond owns) that stateless per-request
  verification avoids, for no benefit given there is one operator and
  Google's ID token already carries a verifiable, time-bounded identity
  claim.
- **mTLS for the CLI** — rejected as overkill. It would require
  provisioning and rotating client certificates for a single operator's
  personal machines, solving a problem (fleet device identity) Robson
  does not have.

## Implementation Notes

**Backend (robsond):**

- `robsond/src/google_jwks.rs` — `GoogleJwksCache`: background-refreshed,
  on-demand-refreshed-once-per-unknown-`kid` (mutex-guarded against a
  thundering herd), read-mostly JWKS cache. Never fetches per-request.
  Test-only constructor `GoogleJwksCache::from_keys_for_test` seeds a
  known key without any network access.
- `robsond/src/auth.rs` — `verify_google_id_token`, `GoogleAuthConfig`,
  `GoogleClaims`, `AuthError` (opaque; real reason logged via
  `tracing::warn!`, never returned to the client), and `looks_like_jwt`
  (the dual-accept bridge's routing check).
- `robsond/src/api.rs` — `ApiState::google_auth: Option<GoogleAuthConfig>`
  (replacing `api_token: Option<String>`; `None` still means auth
  disabled, dev-only, same semantics as before) plus
  `legacy_api_token: Option<String>` for the temporary bridge. The auth
  middleware in `create_router` is otherwise unchanged in structure
  (same `read_only` vs authenticated route split).
- `robsond/src/config.rs` — `ROBSON_GOOGLE_WEB_CLIENT_ID`,
  `ROBSON_GOOGLE_CLI_CLIENT_ID`, `ROBSON_ALLOWED_EMAIL`;
  `reject_removed_api_token_env` (fails startup loudly if the old
  `ROBSON_API_TOKEN` name is set, following the same pattern as
  `reject_removed_stop_policy_env` from ADR-0052); production now
  requires `ROBSON_ALLOWED_EMAIL` plus at least one client id.
- `robsond/src/daemon.rs` — builds `GoogleJwksCache` once at startup and
  starts its refresh loop, constructs `GoogleAuthConfig` from config, and
  wires both into `ApiState`.
- Test fixtures: `robsond/src/testdata/test_rsa_key.pem` /
  `test_rsa_key_pub.pem` — a non-secret RSA keypair generated solely to
  sign fixture ID tokens in `robsond/src/api.rs`'s test module (valid
  token, missing header, wrong `aud`, wrong `iss`, expired `exp`,
  `email_verified: false`, email not allowlisted, auth-disabled-in-dev —
  all covered).

**Frontend (SvelteKit, `adapter-static`, Svelte 5):**

- `frontend/src/routes/login/+page.svelte` — GIS button, loaded
  client-side only (inside `onMount`, guarded so it never touches
  SSR/prerendering).
- `frontend/src/lib/stores/auth.ts` — same `sessionStorage`-based shape;
  storage key renamed to `robson_google_id_token`; `decodeTokenExpiry` /
  `isNearExpiry` (client-side-only `exp` read, no signature check needed
  — robsond re-verifies server-side); `silentRefreshGoogleToken`.
- `frontend/src/lib/api/robson.ts` — `apiFetch` retries once via silent
  refresh on a 401 before giving up; the fetch-based SSE client
  (`FetchEventSource`) re-reads the token from the store at every
  (re)connect instead of closing over the value captured at construction
  time, so a mid-connection refresh is picked up on the next reconnect.
- `frontend/src/routes/(authed)/+layout.svelte` — auth guard is now
  reactive to the auth store (not a one-shot `sessionStorage` read),
  plus a periodic (few-minute) silent-refresh timer tied to
  `onMount`/`onDestroy`.
- New public env var `PUBLIC_GOOGLE_WEB_CLIENT_ID`
  (`frontend/.env.example`).

**CLI (`robson-cli`):**

- `robson-cli/src/auth.rs` — Device Authorization Grant against Google's
  device endpoints via the `oauth2` crate; credential cache at
  `$XDG_CONFIG_HOME/robson/credentials.json` (or `$HOME/.config/...`),
  mode `0600`; `current_id_token()` transparently refreshes within ~5
  minutes of expiry.
- `robson-cli/src/commands/auth.rs` — `robson auth login|status|logout|
  print-token` (the last for scripting/curl use in runbooks).
- `robson-cli/src/commands/reconcile_close.rs`, `income.rs` — dropped
  `--token` / `env = "ROBSON_API_TOKEN"`; call `current_id_token()`,
  erroring with "run `robson auth login` first" when nothing is cached.
- `GOOGLE_CLI_CLIENT_ID` is compiled into the binary as a constant
  (Google's "TVs and Limited-Input devices" client type is not meant to
  be operator-configurable) — placeholder pending manual registration in
  Google Cloud Console, see the constant's doc comment in
  `robson-cli/src/auth.rs`.

**Related**: ADR-0025 (Bearer token auth — amended by this ADR; its
"no callback server, static frontend" constraint is exactly why GIS is
the mechanism chosen here), ADR-0027 (CORS layer; unaffected — this ADR
only changes what the `Authorization` header must contain), ADR-0007
(Robson is single-operator by design; unaffected — this ADR narrows how
that one operator authenticates, not who is allowed to).

**Amends**: ADR-0025 (adds Google OAuth as the frontend/CLI auth
mechanism in place of the pasted static bearer token; does not reopen or
reverse ADR-0025's rejection of a callback-server-based OAuth flow for
this frontend).
