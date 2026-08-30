# MOB-P1 Android Client Implementation

Status: Repository implementation and device launch complete; authenticated smoke pending
Date: 2026-08-29
Decision: ADR-0054

## Objective

Produce a repository-verified, internal Android debug client that renders the
Robson dashboard and audit surfaces on the physical POCO device. MOB-P1 is
read-only. It must not arm, approve, disarm, halt, panic, or move funds.

## Current Reality

- `frontend/` is a SvelteKit static application and already talks to robsond
  through typed REST and SSE clients.
- The web application stores the operator token in `sessionStorage` under the
  accepted ADR-0025 policy.
- The Mint host has a checksummed, user-local JDK 21 and minimal Android API 36
  toolchain under `~/.local/share/rbx/android`; no IDE or emulator is installed.
- The POCO Termux and SSH worker setup is not yet operationally verified.
- A debug APK has been built, manually installed on the connected POCO, and
  launched. Observer-scoped authentication is not implemented yet.

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
- ADB reports the authorized physical device `2602BPC18G`, Android 16/API 36,
  ARM64, with WebView 151. HyperOS blocked `adb install`, so the verified APK
  was copied to Downloads and installed manually by the operator.
- ADB confirms `br.ia.rbx.robson/.MainActivity` is foreground with no fatal
  application error. Capacitor emits an early, non-fatal safe-area CSS
  injection error that needs follow-up before release packaging.

## Authentication Limitation Found on Device

- The login probe calls public `GET /health`, so any non-empty placeholder
  currently passes the login screen. This does not prove authentication.
- robsond currently uses one `ROBSON_API_TOKEN` for authenticated SSE/history
  and every mutation route; it does not expose an observer-only scope.
- The mobile API client still rejects every non-GET/HEAD request and keeps the
  token in memory, but that client-side control cannot reduce the authority of
  a production token.
- MOB-P1 device testing therefore uses a non-secret dummy value only. Exchange
  API keys and the production Robson operator token are prohibited in this
  debug client.
- The next security slice must add backend-enforced observer credentials and a
  real authentication probe before MOB-P1-G7 can pass.

## Physical Device Smoke Test

1. Enable USB debugging and authorize the Mint host.
2. Verify `adb devices` reports one authorized physical device.
3. Build and sync the Android web bundle.
4. Build the debug APK on Mint.
5. Install with ADB and open Robson.
6. Enter a test or scoped read-only token. Never paste a production mutation
   token into logs or terminal history.
7. Confirm dashboard, operation detail, event history, and stale SSE behavior.
8. Confirm ARM, approval, disarm, halt, panic, and funding mutations are absent
   or rejected.

## Exit Criteria

MOB-P1 repository implementation and physical launch are complete through
MOB-P1-G6 and MOB-P1-G8. Authenticated physical-device rollout remains pending
until observer authentication exists and MOB-P1-G7 is recorded.
