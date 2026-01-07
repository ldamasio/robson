#!/usr/bin/env bash
# devtools/robson-wt.sh
#
# Robson worktree + tmux session bootstrapper.
# Opinionated layout:
#   - 1 tmux session per worktree
#   - window "agents" with 4 panes:
#       [0] Claude (Anthropic)
#       [1] Claude (GLM)
#       [2] Codex
#       [3] Shell
#
# Usage:
#   ./devtools/robson-wt.sh <name> <branch>
#
# Examples:
#   ./devtools/robson-wt.sh fix-operations "fix/fix-operations"
#   ./devtools/robson-wt.sh fix-stop-loss  "fix/fix-stop-loss"
#
# Optional env vars (Robson-focused now, but generalizable later):
#   ROBSON_TRUNK=~/apps/robson
#   ROBSON_WT_PARENT=~/apps
#   ROBSON_SESSION_PREFIX="robson"
#   ROBSON_WT_PREFIX="robson-wt"
#
# Agent commands (customize as needed):
#   ROBSON_CLAUDE_CMD_ANTHROPIC='claude .'
#   ROBSON_CLAUDE_CMD_GLM='claude .'
#   ROBSON_CODEX_CMD='codex'
#
# Notes:
# - This script creates/reuses a worktree at:  $ROBSON_WT_PARENT/$ROBSON_WT_PREFIX-<name>
# - It creates a tmux session named:          $ROBSON_SESSION_PREFIX-<name>
# - If the session already exists, it attaches and exits.

set -euo pipefail

NAME="${1:-}"
BRANCH="${2:-}"

if [[ -z "${NAME}" || -z "${BRANCH}" ]]; then
  echo "Usage: $0 <name> <branch>"
  exit 1
fi

# Normalize name -> safe slug
SAFE_NAME="$(echo "${NAME}" | tr '[:upper:] ' '[:lower:]-' | tr -cd 'a-z0-9._-')"
if [[ -z "${SAFE_NAME}" ]]; then
  echo "Invalid name after normalization: '${NAME}'"
  exit 1
fi

TRUNK="${ROBSON_TRUNK:-$HOME/apps/robson}"
WT_PARENT="${ROBSON_WT_PARENT:-$HOME/apps}"
SESSION_PREFIX="${ROBSON_SESSION_PREFIX:-robson}"
WT_PREFIX="${ROBSON_WT_PREFIX:-robson-wt}"

SESSION="${SESSION_PREFIX}-${SAFE_NAME}"
WT_DIR="${WT_PARENT}/${WT_PREFIX}-${SAFE_NAME}"

CLAUDE_ANTHROPIC_CMD="${ROBSON_CLAUDE_CMD_ANTHROPIC:-claude .}"
CLAUDE_GLM_CMD="${ROBSON_CLAUDE_CMD_GLM:-claude .}"
CODEX_CMD="${ROBSON_CODEX_CMD:-codex}"

if [[ ! -d "${TRUNK}/.git" ]]; then
  echo "TRUNK does not look like a git repo: ${TRUNK}"
  exit 1
fi

# If tmux session exists, just attach
if tmux has-session -t "${SESSION}" 2>/dev/null; then
  echo "tmux session already exists: ${SESSION}"
  exec tmux attach -t "${SESSION}"
fi

# Ensure trunk is on disk and usable
cd "${TRUNK}"

# Create or reuse worktree
if [[ -d "${WT_DIR}" ]]; then
  if [[ ! -d "${WT_DIR}/.git" && ! -f "${WT_DIR}/.git" ]]; then
    echo "Worktree dir exists but is not a git worktree: ${WT_DIR}"
    exit 1
  fi
  echo "Reusing existing worktree: ${WT_DIR}"
else
  echo "Creating worktree: ${WT_DIR} (branch: ${BRANCH})"
  # If branch exists locally, create worktree without -b; else create new branch.
  if git show-ref --verify --quiet "refs/heads/${BRANCH}"; then
    git worktree add "${WT_DIR}" "${BRANCH}"
  else
    git worktree add "${WT_DIR}" -b "${BRANCH}"
  fi
fi

echo "Starting tmux session: ${SESSION} (cwd: ${WT_DIR})"

# Create session detached in WT_DIR
tmux new-session -d -s "${SESSION}" -c "${WT_DIR}" -n "agents"

# Pane 0 (default) -> Claude Anthropic
tmux send-keys -t "${SESSION}:agents.0" "cd \"${WT_DIR}\"" C-m
tmux send-keys -t "${SESSION}:agents.0" "${CLAUDE_ANTHROPIC_CMD}" C-m

# Split right -> Pane 1 (Claude GLM)
tmux split-window -h -t "${SESSION}:agents" -c "${WT_DIR}"
tmux send-keys -t "${SESSION}:agents.1" "cd \"${WT_DIR}\"" C-m
tmux send-keys -t "${SESSION}:agents.1" "${CLAUDE_GLM_CMD}" C-m

# Split bottom-left -> Pane 2 (Codex)
tmux split-window -v -t "${SESSION}:agents.0" -c "${WT_DIR}"
tmux send-keys -t "${SESSION}:agents.2" "cd \"${WT_DIR}\"" C-m
tmux send-keys -t "${SESSION}:agents.2" "${CODEX_CMD}" C-m

# Split bottom-right -> Pane 3 (Shell)
tmux split-window -v -t "${SESSION}:agents.1" -c "${WT_DIR}"
tmux send-keys -t "${SESSION}:agents.3" "cd \"${WT_DIR}\"" C-m

# Make it readable
tmux select-layout -t "${SESSION}:agents" tiled

# Optional: add a second window for running tests/logs (comment out if you don't want it)
tmux new-window -t "${SESSION}" -n "run" -c "${WT_DIR}"
tmux send-keys -t "${SESSION}:run.0" "cd \"${WT_DIR}\"" C-m

# Focus agents window
tmux select-window -t "${SESSION}:agents"
tmux select-pane -t "${SESSION}:agents.3"

if [[ -n "${TMUX:-}" ]]; then
  echo "ℹ️  Already inside tmux → switching client to session: ${SESSION}"
  exec tmux switch-client -t "${SESSION}"
else
  exec tmux attach -t "${SESSION}"
fi

