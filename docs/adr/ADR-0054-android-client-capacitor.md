# ADR-0054: Android Client as a Capacitor Delivery Target

Status: Accepted
Date: 2026-08-29

## Context

Robson already has a SvelteKit static operator console, a typed API client,
SSE freshness handling, bilingual copy, and the RBX Voltage design system.
The first Android client needs to produce useful repository evidence quickly,
run on a physical POCO device, and avoid creating a second implementation of
the financial presentation rules.

The Linux Mint development host is x86_64, while the POCO is ARM64. Google
does not support Android Studio on ARM Linux hosts. The phone can still run
the Node-based web checks and bundle, and it is the physical Android test
target, but the official Android SDK and Gradle packaging step remain on the
Mint host.

The current web console exposes governed mutations. The first mobile slice is
an internal, read-only client. Enabling mobile mutations before full-screen
mutation locking, durable secure token storage, and device-level acceptance
tests would expand the risk surface without evidence that the controls work.

## Decision

Add Capacitor 8 to `frontend/` and treat Android as another delivery target of
the existing SvelteKit application.

- The committed Android application ID is `br.ia.rbx.robson`.
- The Android project is source, not a generated CI artifact.
- `pnpm run build:android:web` creates a separate read-only bundle using
  `.env.android`.
- The Android WebView uses the owned hostname `robson.rbx.ia.br`. This matches
  the existing production CORS allow-list while local assets are served by
  Capacitor.
- Mobile read-only mode hides primary mutation affordances and rejects every
  non-GET/HEAD API request in the shared API client. This is defense in depth,
  not an authorization boundary; robsond remains authoritative.
- The bearer token is memory-only in mobile read-only mode and must be entered
  again after the WebView process is destroyed. The existing browser build
  keeps its accepted `sessionStorage` behavior from ADR-0025.
- Android packaging and deployment use the Mint Android SDK. Web bundle,
  type-check, lint, and unit-test work may run on the POCO through the
  allow-listed `scripts/poco-web-worker.sh` wrapper.

```text
                         HTTPS REST + SSE
  +----------------+    ----------------->    +----------------+
  | Robson Android |                            |    robsond     |
  | Capacitor shell|    <-----------------     | policy + audit |
  | SvelteKit UI   |                            +----------------+
  +-------+--------+
          ^ ADB install/test
          |
  +-------+--------+       SSH/rsync       +--------------------+
  | Linux Mint     | <-------------------> | POCO Termux worker |
  | Codex/Claude   |                       | web checks + bundle|
  | Android SDK    |                       +--------------------+
  +----------------+
```

## Decision Matrix

| Option                            | Reuse current UI and contracts | Native capability                   | First useful build | Main trade-off                                             |
| --------------------------------- | ------------------------------ | ----------------------------------- | ------------------ | ---------------------------------------------------------- |
| Capacitor over current SvelteKit  | High                           | Available through plugins or Kotlin | Fast               | WebView runtime and some platform-specific seams           |
| Kotlin and Jetpack Compose        | Low                            | Highest                             | Slow               | Duplicates presentation, auth, SSE, and accessibility work |
| Installable PWA only              | Highest                        | Limited                             | Fastest            | Does not exercise Android packaging or native integration  |
| Separate React Native application | Medium                         | High                                | Medium             | Adds a second JavaScript UI stack without a current need   |

Capacitor is selected for MOB-P1. Native Compose remains an option for a
future feature that cannot be expressed safely through the WebView or a small
Capacitor plugin.

## Performance and Resource Controls

- Dashboard startup budget: at most three REST requests plus one SSE
  connection. More than 15 requests for one screen operation is a review
  failure under the RBX guardrails.
- SSE retains the existing read-idle watchdog, reconnect backoff, visible
  stale state, and bounded client event buffer.
- Every subscription, timer, fetch, and stream must be released on route or
  process teardown. Android validation must include a LeakCanary pass before
  mutations are enabled.
- The POCO worker never runs Gradle or the Android SDK. It handles only the
  architecture-neutral web workload.

## Security Controls

- No API token, signing key, or endpoint credential is embedded in the APK.
- MOB-P1 tokens are memory-only and read-only mode is enforced at the UI and
  API-client layers.
- Release signing material stays outside the repository. MOB-P1 produces
  debug APKs only.
- Dependencies are version-pinned, the pnpm lockfile is committed, and the
  JavaScript lockfile is included in OSV scanning.
- Enabling mutations requires a follow-up ADR covering Android Keystore
  storage, optional biometric re-authentication, full-screen mutation locks,
  idempotency, and device-loss revocation.

## Failure Modes

| Dependency or failure                                         | Client behavior                                                              | Visibility and recovery                                                   |
| ------------------------------------------------------------- | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| robsond unavailable                                           | No cached state is presented as live; mutations remain blocked               | Full connection error with explicit retry                                 |
| SSE connects but becomes silent                               | Existing idle watchdog aborts and reconnects                                 | `STALE, reconnecting` remains visible                                     |
| Database, exchange, or market feed unavailable behind robsond | Client never bypasses robsond or talks to those systems directly             | Render backend status/blockers; fail closed when state cannot be verified |
| Android app or WebView process dies                           | No trade-management responsibility moves to the client; memory token is lost | Restart and authenticate again; robsond continues managing positions      |
| Device loses network                                          | Current view becomes stale, not authoritative                                | Visible offline/stale state and bounded reconnect backoff                 |
| POCO Termux worker is unavailable                             | Mobile development continues on Mint                                         | Run the same pnpm commands locally                                        |
| Mint Android SDK is unavailable                               | Web work can continue, but no APK is claimed as built                        | Setup remains pending until SDK/JDK verification passes                   |
| Android WebView is obsolete                                   | Capacitor refuses unsupported WebView versions                               | Update system WebView before retrying                                     |

## Consequences

### Positive

- One UI, API contract, event semantics, and design system across web and
  Android.
- A working mobile slice can be tested early on the real target device.
- The POCO contributes compute where its ARM environment is reliable without
  pretending it can host the official Android toolchain.
- Read-only MOB-P1 creates value without increasing the trading mutation
  surface.

### Negative and trade-offs

- The first client is not fully native and cannot execute trades.
- The local WebView hostname is coupled to an existing production frontend
  origin so that the current CORS policy remains narrow.
- Mobile-specific UX will add conditional seams to the shared Svelte code.
- Android packaging remains on the older Mint CPU.

### What this leaves on the table

- Native Compose rendering and navigation.
- Background push notifications and offline snapshots.
- Biometric unlock and Keystore-backed durable sessions.
- Mobile mutations, Play Store release signing, and iOS packaging.

## Implementation Notes

- Milestone: `MOB-P1`, internal Android read-only client.
- Web application and Capacitor config: `frontend/`.
- Native project: `frontend/android/`.
- POCO worker wrapper: `scripts/poco-web-worker.sh`.
- Verification guide:
  `docs/implementation/2026-08-29-android-client-mob-p1.md`.
- Related decisions: ADR-0025, ADR-0027, ADR-0030, ADR-0032, ADR-0047.
