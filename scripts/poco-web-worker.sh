#!/usr/bin/env bash

set -euo pipefail

ACTION="${1:-}"
WORKER_HOST="${ROBSON_POCO_HOST:-poco-worker}"
REMOTE_DIR="work/robson-frontend"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="${REPO_ROOT}/frontend"

case "${WORKER_HOST}" in
  *[!a-zA-Z0-9._-]*|'')
    echo "ROBSON_POCO_HOST must be an SSH config alias" >&2
    exit 2
    ;;
esac

case "${ACTION}" in
  sync|install|check|test|build|android-web|verify) ;;
  *)
    echo "Usage: $0 {sync|install|check|test|build|android-web|verify}" >&2
    exit 2
    ;;
esac

sync_source() {
  ssh "${WORKER_HOST}" "mkdir -p '${REMOTE_DIR}'"
  rsync -az --delete-delay \
    --exclude node_modules \
    --exclude .svelte-kit \
    --exclude build \
    --exclude android \
    "${FRONTEND_DIR}/" "${WORKER_HOST}:${REMOTE_DIR}/"
}

run_remote() {
  ssh "${WORKER_HOST}" "cd '${REMOTE_DIR}' && $1"
}

sync_source

case "${ACTION}" in
  sync) ;;
  install) run_remote "pnpm install --frozen-lockfile" ;;
  check) run_remote "pnpm run check" ;;
  test) run_remote "pnpm run test" ;;
  build) run_remote "pnpm run build" ;;
  android-web)
    run_remote "pnpm run build:android:web"
    rsync -az "${WORKER_HOST}:${REMOTE_DIR}/build/" "${FRONTEND_DIR}/build/"
    ;;
  verify)
    run_remote "pnpm install --frozen-lockfile"
    run_remote "pnpm run check"
    run_remote "pnpm run test"
    run_remote "pnpm run build:android:web"
    ;;
esac
