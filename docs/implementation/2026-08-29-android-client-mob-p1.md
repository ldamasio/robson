# MOB-P1 Android Client Implementation

Status: Repository implementation complete; physical rollout pending
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
- The Mint host has ADB but does not yet have repository evidence of a JDK or
  Android SDK installation.
- The POCO Termux and SSH worker setup is not yet operationally verified.
- No Android APK has been built or installed from this repository yet.

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

| Gate      | Evidence                                             | Status                                 |
| --------- | ---------------------------------------------------- | -------------------------------------- |
| MOB-P1-G1 | `pnpm install --frozen-lockfile`                     | Pass                                   |
| MOB-P1-G2 | `pnpm check`, `pnpm lint`, `pnpm test`, `pnpm build` | Pass                                   |
| MOB-P1-G3 | Android read-only web bundle and `cap sync android`  | Pass                                   |
| MOB-P1-G4 | OSV scan includes `frontend/pnpm-lock.yaml`          | Pass; no issues found                  |
| MOB-P1-G5 | Gradle debug APK build on Mint                       | Blocked: Android SDK/JDK not installed |
| MOB-P1-G6 | APK installed and dashboard rendered on POCO         | Blocked: no ADB device connected       |
| MOB-P1-G7 | SSE silence and offline state visible on POCO        | Blocked: no ADB device connected       |
| MOB-P1-G8 | Non-GET API request rejected in mobile mode          | Pass; three unit cases                 |

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
- Gradle accurately refused to start because no `java` executable or
  `JAVA_HOME` is available; no APK claim is made.
- ADB started successfully but reported no attached devices; no POCO smoke
  test claim is made.

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

MOB-P1 is implemented when MOB-P1-G1 through MOB-P1-G4 and MOB-P1-G8 have
repository evidence. Physical-device rollout remains pending until MOB-P1-G5
through MOB-P1-G7 are recorded from the connected POCO.
