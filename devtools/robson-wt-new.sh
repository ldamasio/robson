#!/usr/bin/env bash
# robson-wt-new.sh
# Create a git worktree for Robson and open a dedicated tmux session (Claude/Codex/Shell).
#
# Usage examples:
#   ./robson-wt-new.sh claude strategy "feat/strategy-refactor"
#   ./robson-wt-new.sh codex backend  "feat/backend-adjust"
#   ./robson-wt-new.sh shell infra   "infra/k8s-tasks"
#
# Optional env vars:
#   ROBSON_TRUNK=~/apps/robson
#   ROBSON_WT_PARENT=~/apps
#   ROBSON_AGENT_CMD_CLAUDE="claude ."
#   ROBSON_AGENT_CMD_CODEX="codex"   # adjust if your command differs

set -euo pipefail

AGENT="${1:-}"
NAME="${2:-}"
BRANCH="${3:-}"

if [[ -z "${AGENT}" || -z "${NAME}" || -z "${BRANCH}" ]]; then
  echo "Usage: $0 <claude|codex|shell> <name> <branch>"
  exit 1
fi

case "${AGENT}" in
  claude|codex|shell) ;;
  *)
    echo "Invalid agent: ${AGENT} (use: claude | codex | shell)"
    exit 1
    ;;
esac

TRUNK="${ROBSON_TRUNK:-$HOME/apps/robson}"
WT_PARENT="${ROBSON_WT_PARENT:-$HOME/apps}"

if [[ ! -d "${TRUNK}/.git" ]]; then
  echo "TRUNK does not look like a git repo: ${TRUNK}"
  exit 1
fi

# Ensure trunk is on disk and usable
cd "${TRUNK}"

# If you want to enforce clean trunk, uncomment:
# if [[ -n "$(git status --porcelain)" ]]; then
#   echo "Trunk has uncommitted changes. Commit/stash first: ${TRUNK}"
#   exit 1
# fi

# Normalize names
SAFE_NAME="$(echo "${NAME}" | tr '[:upper:] ' '[:lower:]-' | tr -cd 'a-z0-9._-')"
SESSION="${AGENT}-${SAFE_NAME}"
WT_DIR="${WT_PARENT}/robson-wt-${SAFE_NAME}"

# Pick agent command
CMD_CLAUDE="${ROBSON_AGENT_CMD_CLAUDE:-claude .}"
CMD_CODEX="${ROBSON_AGENT_CMD_CODEX:-codex}"

AGENT_CMD=""
case "${AGENT}" in
  claude) AGENT_CMD="${CMD_CLAUDE}" ;;
  codex)  AGENT_CMD="${CMD_CODEX}"  ;;
  shell)  AGENT_CMD=""              ;;
esac

# If tmux session exists, just attach
if tmux has-session -t "${SESSION}" 2>/dev/null; then
  echo "tmux session already exists: ${SESSION}"
  exec tmux attach -t "${SESSION}"
fi

# Create worktree (or reuse if exists)
if [[ -d "${WT_DIR}" ]]; then
  if [[ ! -d "${WT_DIR}/.git" && ! -f "${WT_DIR}/.git" ]]; then
    echo "Worktree dir exists but is not a git worktree: ${WT_DIR}"
    exit 1
  fi
  echo "Reusing existing worktree: ${WT_DIR}"
else
  echo "Creating worktree: ${WT_DIR} (branch: ${BRANCH})"
  # If branch already exists locally/remotely, create worktree without -b
  if git show-ref --verify --quiet "refs/heads/${BRANCH}"; then
    git worktree add "${WT_DIR}" "${BRANCH}"
  else
    git worktree add "${WT_DIR}" -b "${BRANCH}"
  fi
fi

# Start tmux session in the worktree
echo "Starting tmux session: ${SESSION} (cwd: ${WT_DIR})"
tmux new-session -d -s "${SESSION}" -c "${WT_DIR}"

# Split into two panes: left agent, right shell
tmux split-window -h -t "${SESSION}" -c "${WT_DIR}"

# Left pane = agent (if any)
if [[ -n "${AGENT_CMD}" ]]; then
  tmux select-pane -t "${SESSION}:.0"
  tmux send-keys -t "${SESSION}:.0" "${AGENT_CMD}" C-m
else
  tmux select-pane -t "${SESSION}:.0"
  tmux send-keys -t "${SESSION}:.0" "cd \"${WT_DIR}\"" C-m
fi

# Right pane = shell in repo (useful for git/tests/rg)
tmux select-pane -t "${SESSION}:.1"
tmux send-keys -t "${SESSION}:.1" "cd \"${WT_DIR}\"" C-m

# Nice window name
tmux rename-window -t "${SESSION}:0" "${SAFE_NAME}"

# Attach
exec tmux attach -t "${SESSION}"
