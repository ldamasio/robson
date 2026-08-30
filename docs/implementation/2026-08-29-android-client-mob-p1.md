# MOB-P1 Android Client Implementation

Status: Repository implementation and device launch complete; RBX Identity smoke pending
Date: 2026-08-29
Decision: ADR-0054

## Objective

Produce a repository-verified, internal Android debug client that renders the
Robson dashboard and audit surfaces on the physical POCO device. MOB-P1 is
read-only. It must not arm, approve, disarm, halt, panic, or move funds.

## Current Reality

- `frontend/` is a SvelteKit static application and already talks to robsond
  through typed REST and SSE clients.
- The operator web build retains the ADR-0025 migration fallback. The Android
  build disables manual tokens and implements memory-only RBX Identity OIDC per
  ADR-0055.
- The Mint host has a checksummed, user-local JDK 21 and minimal Android API 36
  toolchain under `~/.local/share/rbx/android`; no IDE or emulator is installed.
- The POCO Termux and SSH worker setup is not yet operationally verified.
- A debug APK has been built, manually installed on the connected POCO, and
  launched. Observer authentication is repository-implemented; operational
  client registration and device login remain pending.

## MOB-P1 Deliverables

1. Capacitor Android project committed under `frontend/android/`.
2. Pinned Capacitor dependencies and Node 22 CI baseline.
3. Android build mode with a fixed read-only client policy.
4. Memory-only bearer token behavior in the Android build.
5. Mobile safe-area and narrow-screen layout baseline.
6. POCO web-worker wrapper limited to sync, install, check, test, and web
   bundle commands.
7. Repository checks passing on Mint.
8. Physical-device APK install and smoke test recorded as operational rollout,
   only after the POCO and Android SDK are connected.

## Verification Gates

| Gate      | Evidence                                             | Status                          |
| --------- | ---------------------------------------------------- | ------------------------------- |
| MOB-P1-G1 | `pnpm install --frozen-lockfile`                     | Pass                            |
| MOB-P1-G2 | `pnpm check`, `pnpm lint`, `pnpm test`, `pnpm build` | Pass                            |
| MOB-P1-G3 | Android read-only web bundle and `cap sync android`  | Pass                            |
| MOB-P1-G4 | OSV scan includes `frontend/pnpm-lock.yaml`          | Pass; no issues found           |
| MOB-P1-G5 | Gradle debug APK build on Mint                       | Pass; 93 Gradle tasks           |
| MOB-P1-G6 | APK installed and MainActivity foreground on POCO    | Pass                            |
| MOB-P1-G7 | Authenticated SSE silence/offline behavior on POCO   | Pending observer authentication |
| MOB-P1-G8 | Non-GET API request rejected in mobile mode          | Pass; three unit cases          |

## Repository Evidence

Collected on 2026-08-29 from the isolated `feat/android-client` worktree:

- Frozen dependency installation completed successfully.
- Type check, security lint, 117 unit tests, normal static build, Android-mode
  build, and Capacitor sync completed successfully.
- ESLint reports no errors. Eight existing object-indexing warnings remain in
  typed lookup tables outside the new mobile policy path.
- `pnpm audit` and OSV reported no known dependency vulnerabilities.
- Gitleaks reported no secrets in the current working tree.
- `bash -n scripts/poco-web-worker.sh` and `git diff --check` passed.
- The Android bundle contains the expected `mobile-readonly` mode and
  `https://api.robson.rbx.ia.br` endpoint.
- The compact Gradle 8.14.3 `bin` distribution has an official pinned SHA-256.
- `assembleDebug` completed 93 tasks in 2 minutes 13 seconds. The resulting
  4.1 MB APK targets API 36, has minimum API 24, and verifies with Android APK
  Signature Scheme v2.
- APK SHA-256:
  `d189860b2fb7fb0bf826057b66cbdf9ede9939a22d89847ed3c0635c7e3fbc89`.
- The minimal JDK/SDK toolchain occupies 980 MB. The reusable first-build
  Gradle cache occupies 689 MB; no Android Studio, emulator, system image,
  NDK, or CMake is installed.
- ADB reports one authorized physical POCO device, Android 16/API 36, ARM64,
  with WebView 151. The device serial is intentionally excluded from repository
  evidence. HyperOS blocked `adb install`, so the verified APK was copied to
  Downloads and installed manually by the operator.
- ADB confirms `br.ia.rbx.robson/.MainActivity` is foreground with no fatal
  application error. Capacitor emits an early, non-fatal safe-area CSS
  injection error that needs follow-up before release packaging.

## Authentication Limitation Found on Device and Resolution

- The original login probe called public `GET /health`, so any placeholder
  passed. IAM-P1 replaces it with protected `GET /auth/session`.
- `robsond` now validates RBX Identity JWTs and enforces observer, operator,
  funding, and emergency roles. Product reads and SSE require at least the
  observer role.
- The Android build no longer renders the legacy-token form. Its OIDC client id
  is intentionally empty until the native client is registered, so it fails
  visibly as not configured rather than accepting a dummy value.
- Physical authenticated validation remains pending because no operational
  ZITADEL client/role grant is repository-verified. Production operator or
  exchange tokens remain prohibited in the debug client.
- The physical-device helper now blocks `install`, `stage-apk`, and `cycle`
  when the effective Android environment lacks structurally complete public
  Identity metadata. Repository and UI-shell builds remain possible without
  claiming an operational login.
- An incomplete Android configuration renders the localized pending-registration
  state instead of exposing an internal English exception to the user.

## Physical Device Smoke Test

1. Enable USB debugging and authorize the Mint host.
2. Verify `adb devices` reports one authorized physical device.
3. Build and sync the Android web bundle.
4. Build the debug APK on Mint.
5. Install with ADB and open Robson.
6. Authenticate through RBX Identity with an invited observer-only test user.
   Never paste a production mutation token into the app, logs, or terminal
   history.
7. Confirm dashboard, operation detail, event history, and stale SSE behavior.
8. Confirm ARM, approval, disarm, halt, panic, and funding mutations are absent
   or rejected.

## Exit Criteria

MOB-P1 repository implementation and physical launch are complete through
MOB-P1-G6 and MOB-P1-G8. Authenticated physical-device rollout remains pending
until observer authentication exists and MOB-P1-G7 is recorded.

## Mobile Continuity Validation

Validated locally on 2026-08-30 without changing phone settings:

- `scripts/android-device-cycle.sh doctor` confirmed the minimal host toolchain
  and exactly one authorized physical device without printing its serial.
- Frontend type-check passed with no diagnostics, ESLint passed with the eight
  pre-existing warnings, and all 122 frontend unit tests passed.
- The Android application unit test, debug APK, and instrumentation APK built
  successfully from the `:app` module.
- ADB replacement installation of the application APK passed and MainActivity
  became the focused activity on the physical device.
- Installation of the `testOnly` instrumentation APK remains locally blocked by
  the device's user-restriction policy. No phone setting was changed to bypass
  it. Device instrumentation is therefore pending, while host-side Android tests
  and the physical application install/launch cycle are validated.

## Offline-State Follow-up

Repository-verified on 2026-08-30:

- The client observes the browser and Android WebView `online` and `offline`
  hints and rechecks the hint when the application returns to the foreground.
- A localized global banner marks displayed data as potentially stale while the
  device reports that it is offline.
- The dashboard reloads its REST snapshot and restarts the bounded-backoff SSE
  connection immediately after an offline-to-online transition.
- Listener setup is idempotent and all network listeners are removed with the
  root application lifecycle. Four focused unit tests cover initialization,
  transitions, foreground refresh, and cleanup.
- `navigator.onLine` is only a device-network hint. Backend errors and SSE
  freshness remain separate signals because an online device can still be
  unable to reach robsond.

Locally validated:

- The online login page rendered without a false offline banner or browser
  console errors.
- The offline banner rendered with the RBX warning color, centered status text,
  and a live-region role in an isolated local browser check.
- Frontend type-check and Android-mode build passed, all 133 unit tests passed,
  and ESLint retained only the eight pre-existing warnings.

Physical loss and restoration of Wi-Fi or mobile data remains pending an
operator-confirmed device test. No phone network setting was changed during
this implementation.

## Native Leave-Application Follow-up

Repository-verified on 2026-08-30:

- The Android app exposes a native-only leave button on every route. The web
  client does not render the action.
- The button uses the Capacitor `minimizeApp()` lifecycle API. It does not call
  `exitApp()`, kill the process, intercept system Back, or disable Android
  Predictive Back behavior.
- Leaving the app and logging out are separate actions. The leave action moves
  Robson to the background and preserves the in-memory session; logout clears
  local authentication and returns to the login route.
- The control has an accessible localized name, a 48 by 48 CSS-pixel touch
  target, keyboard focus behavior from the shared design system, and protection
  against duplicate taps while the native lifecycle call is in flight.
- Two unit tests verify the Android lifecycle call and ensure that the web path
  does not invoke the Android-only API.

Locally validated:

- At a 393 by 852 mobile viewport, the native-only control rendered at 48 by 48
  CSS pixels without horizontal overflow or browser console errors.
- The unmodified web runtime did not render the native action.

Physical-device status:

- The final APK passed the full device cycle, installed successfully, and left
  Robson as the focused activity on the physical POCO.
- The device screen slept before the accessibility-tree and button-tap check.
  HyperOS rejected ADB input injection, so no wake, unlock, or policy bypass was
  attempted. A manual operator tap remains pending.
- No application process, phone permission, or device setting was changed to
  bypass that restriction.
