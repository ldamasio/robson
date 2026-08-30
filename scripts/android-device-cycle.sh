#!/usr/bin/env bash

set -euo pipefail

ACTION="${1:-}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="${REPO_ROOT}/frontend"
ANDROID_DIR="${FRONTEND_DIR}/android"
APK_PATH="${ANDROID_DIR}/app/build/outputs/apk/debug/app-debug.apk"
TEST_APK_PATH="${ANDROID_DIR}/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
APP_ID="br.ia.rbx.robson"
MAIN_ACTIVITY="br.ia.rbx.robson.MainActivity"

usage() {
  cat <<'EOF'
Usage: scripts/android-device-cycle.sh <action>

Actions:
  doctor             Check the minimal host toolchain and one physical device.
  verify             Run frontend checks, lint, and unit tests.
  build              Sync Capacitor, run Android unit tests, and build the APK.
  install            Install or replace the debug APK with ADB.
  stage-apk          Copy the APK to Downloads for operator-confirmed install.
  instrumented-test  Run Android tests on the connected physical device.
  launch             Open the installed debug application.
  cycle              Run doctor, verify, build, install, and launch.

The script never prints device serials. It does not install an emulator, change
phone settings, inspect application data, or read device logs.
EOF
}

fail() {
  echo "android-device-cycle: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "$1 is unavailable"
}

resolve_java_home() {
  local candidate="${RBX_ANDROID_JAVA_HOME:-${JAVA_HOME:-}}"
  local portable_default="${XDG_DATA_HOME:-${HOME}/.local/share}/rbx/android/jdk-21"

  if [[ -z "${candidate}" && -x "${portable_default}/bin/java" ]]; then
    candidate="${portable_default}"
  fi
  [[ -n "${candidate}" && -x "${candidate}/bin/java" && -x "${candidate}/bin/javac" ]] ||
    fail "JDK 21 is unavailable; set RBX_ANDROID_JAVA_HOME or JAVA_HOME"
  printf '%s' "${candidate}"
}

resolve_android_sdk() {
  local candidate="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
  local properties_file="${ANDROID_DIR}/local.properties"

  if [[ -z "${candidate}" && -f "${properties_file}" ]]; then
    candidate="$(sed -n 's/^sdk\.dir=//p' "${properties_file}" | tail -n 1)"
    candidate="${candidate//\\:/:}"
    candidate="${candidate//\\\\/\\}"
  fi
  [[ -d "${candidate}" && -x "${candidate}/platform-tools/adb" ]] ||
    fail "Android SDK with platform-tools is unavailable; set ANDROID_HOME"
  printf '%s' "${candidate}"
}

prepare_host() {
  local java_home android_sdk node_home node_major

  java_home="$(resolve_java_home)"
  android_sdk="$(resolve_android_sdk)"
  node_home="${RBX_NODE_HOME:-}"
  if [[ -n "${node_home}" ]]; then
    [[ -x "${node_home}/bin/node" ]] || fail "RBX_NODE_HOME does not contain bin/node"
    PATH="${node_home}/bin:${PATH}"
  fi
  require_command node
  require_command pnpm
  node_major="$(node --version | sed -E 's/^v([0-9]+).*/\1/')"
  [[ "${node_major}" =~ ^[0-9]+$ && "${node_major}" -ge 22 ]] ||
    fail "Node.js 22 or later is required"
  [[ -d "${android_sdk}/platforms/android-36" ]] ||
    fail "Android platform 36 is unavailable"
  [[ -d "${android_sdk}/build-tools/36.0.0" ]] ||
    fail "Android build-tools 36.0.0 are unavailable"
  export JAVA_HOME="${java_home}"
  export ANDROID_HOME="${android_sdk}"
  export PATH="${ANDROID_HOME}/platform-tools:${PATH}"
}

require_one_physical_device() {
  local device_count authorized_count emulator_flag

  adb start-server >/dev/null 2>&1
  device_count="$(adb devices | awk 'NR > 1 && NF {count++} END {print count + 0}')"
  authorized_count="$(adb devices | awk 'NR > 1 && $2 == "device" {count++} END {print count + 0}')"
  [[ "${device_count}" == "1" && "${authorized_count}" == "1" ]] ||
    fail "expected exactly one authorized ADB device"

  emulator_flag="$(adb shell getprop ro.kernel.qemu 2>/dev/null | tr -d '\r')"
  [[ "${emulator_flag}" != "1" ]] || fail "the connected ADB target is an emulator"
}

doctor() {
  local java_version node_version gradle_version api_level abi app_state

  prepare_host
  require_one_physical_device
  java_version="$("${JAVA_HOME}/bin/java" -version 2>&1 | awk -F'"' '/version/ {print $2; exit}')"
  node_version="$(node --version)"
  gradle_version="$("${ANDROID_DIR}/gradlew" --version | awk '/^Gradle / {print $2; exit}')"
  api_level="$(adb shell getprop ro.build.version.sdk 2>/dev/null | tr -d '\r')"
  abi="$(adb shell getprop ro.product.cpu.abi 2>/dev/null | tr -d '\r')"
  if adb shell pm path "${APP_ID}" >/dev/null 2>&1; then
    app_state="installed"
  else
    app_state="not-installed"
  fi

  printf 'JDK=%s\nNode=%s\nGradle=%s\nAndroidAPI=%s\nABI=%s\nApp=%s\n' \
    "${java_version}" "${node_version}" "${gradle_version}" \
    "${api_level}" "${abi}" "${app_state}"
}

verify_frontend() {
  prepare_host
  (
    cd "${FRONTEND_DIR}"
    pnpm run check
    pnpm run lint
    pnpm run test
  )
}

build_apk() {
  prepare_host
  (
    cd "${FRONTEND_DIR}"
    pnpm run android:sync
  )
  (
    cd "${ANDROID_DIR}"
    ./gradlew \
      :app:testDebugUnitTest \
      :app:assembleDebug \
      :app:assembleDebugAndroidTest
  )
  [[ -f "${APK_PATH}" ]] || fail "debug APK was not produced"
  [[ -f "${TEST_APK_PATH}" ]] || fail "instrumented-test APK was not produced"
  echo "Debug APK built successfully."
}

install_apk() {
  prepare_host
  require_one_physical_device
  [[ -f "${APK_PATH}" ]] || fail "debug APK is missing; run build first"
  if ! adb install -r "${APK_PATH}"; then
    fail "ADB install was rejected; use stage-apk for operator-confirmed installation"
  fi
}

stage_apk() {
  prepare_host
  require_one_physical_device
  [[ -f "${APK_PATH}" ]] || fail "debug APK is missing; run build first"
  adb push "${APK_PATH}" /sdcard/Download/Robson-debug.apk >/dev/null
  echo "APK staged in device Downloads. Confirm installation on the device."
}

run_instrumented_tests() {
  prepare_host
  require_one_physical_device
  [[ -f "${APK_PATH}" && -f "${TEST_APK_PATH}" ]] ||
    fail "Android test artifacts are missing; run build first"
  if ! adb shell pm path "${APP_ID}" >/dev/null 2>&1; then
    adb install -r "${APK_PATH}" ||
      fail "the application APK could not be installed for device tests"
  fi
  adb install -r -t "${TEST_APK_PATH}" ||
    fail "the test APK was rejected; do not change phone settings silently"
  adb shell am instrument -w \
    "${APP_ID}.test/androidx.test.runner.AndroidJUnitRunner"
}

launch_app() {
  prepare_host
  require_one_physical_device
  adb shell am start -W -n "${APP_ID}/${MAIN_ACTIVITY}" >/dev/null
  if adb shell dumpsys activity 2>/dev/null |
    grep -q "mFocusedApp.*${APP_ID}"; then
    echo "Robson is foreground on the physical device."
  else
    fail "Robson did not become the foreground activity"
  fi
}

case "${ACTION}" in
  doctor) doctor ;;
  verify) verify_frontend ;;
  build) build_apk ;;
  install) install_apk ;;
  stage-apk) stage_apk ;;
  instrumented-test) run_instrumented_tests ;;
  launch) launch_app ;;
  cycle)
    doctor
    verify_frontend
    build_apk
    install_apk
    launch_app
    ;;
  -h|--help|help) usage ;;
  *)
    usage >&2
    exit 2
    ;;
esac
