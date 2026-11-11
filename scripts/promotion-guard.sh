#!/usr/bin/env bash
# scripts/promotion-guard.sh
#
# Decides whether a Robsond CI/CD run on main may move production's image pin
# (rbx-infra apps/prod/robson/kustomization.yml) to its own commit. Called by
# the "Update image tags in rbx-infra" step of .github/workflows/robsond.yml.
#
# Usage, from a checkout with main's full history:
#   scripts/promotion-guard.sh <candidate-sha> <pinned-tag> <main-tip-sha>
#
# Prints one line, "promote: <why>" or "skip: <why>", and exits 0 either way.
# Any other exit is an error, which fails the step.
#
# The pin only moves forward along main:
#   - a candidate no longer in main's history is skipped;
#   - the pinned commit is the candidate or one of its ancestors: promote;
#   - the candidate is an ancestor of the pinned commit: skip, because the pin
#     would move backwards. That is a run that started late (on 2026-11-11 one
#     sat queued for over an hour while a newer merge deployed) or a re-run of
#     an old one;
#   - otherwise the tag names no commit here, or it is off main: promote only
#     main's current tip.

set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 <candidate-sha> <pinned-tag> <main-tip-sha>" >&2
  exit 2
fi

# A shallow history would make every ancestry check below answer "no".
if [ "$(git rev-parse --is-shallow-repository)" != false ]; then
  echo "promotion-guard: needs main's full history (fetch-depth: 0)" >&2
  exit 2
fi

candidate=$(git rev-parse --verify --quiet "$1^{commit}") || {
  echo "promotion-guard: no commit $1" >&2
  exit 2
}
tip=$(git rev-parse --verify --quiet "$3^{commit}") || {
  echo "promotion-guard: no commit $3" >&2
  exit 2
}

# Succeeds when $1 is $2 or one of its ancestors. Exits on a git error
# instead of reading it as "no".
is_ancestor() {
  local rc=0
  git merge-base --is-ancestor "$1" "$2" || rc=$?
  if [ "$rc" -gt 1 ]; then
    echo "promotion-guard: git merge-base failed ($rc)" >&2
    exit "$rc"
  fi
  return "$rc"
}

# Only a well-formed tag is echoed back; the step prints this line as a
# workflow command.
pinned=
label="the current pin"
if [[ $2 =~ ^sha-([0-9a-f]{8})$ ]]; then
  label=$2
  # Unknown or ambiguous prefixes leave it empty.
  pinned=$(git rev-parse --verify --quiet "${BASH_REMATCH[1]}^{commit}" 2> /dev/null) || pinned=
fi

c=${candidate:0:8}
if ! is_ancestor "$candidate" "$tip"; then
  echo "skip: $c is not in the history of main's tip ${tip:0:8}"
elif [ -n "$pinned" ] && is_ancestor "$pinned" "$candidate"; then
  echo "promote: $c is $label or descends from it"
elif [ -n "$pinned" ] && is_ancestor "$candidate" "$pinned"; then
  echo "skip: $c is older than $label, so moving the pin would roll production back"
elif [ "$candidate" = "$tip" ]; then
  echo "promote: $c is main's tip, and $label is not in its history"
else
  echo "skip: $label is not in $c's history, and $c is not main's tip (${tip:0:8})"
fi
