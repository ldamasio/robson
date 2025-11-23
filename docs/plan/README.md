# Planning Documentation

## Purpose

This directory contains planning documents for Robson Bot development, organized by time period (weekly, sprint, etc.).

## Planning Approach

**Mode**: INTERACTIVE (planning requires discussion)
**Tool**: Cursor Chat or Codex
**When**: Start of week/sprint

See `.ai-agents/AI-WORKFLOW.md` for detailed planning workflows.

---

## Document Types

### Weekly Plans
- **Format**: `week-YYYY-MM-DD.md`
- **Created**: Every Monday
- **Purpose**: Organize week's tasks with mode annotations

### Sprint Plans
- **Format**: `sprint-XX-YYYY-MM-DD.md`
- **Created**: Sprint start
- **Purpose**: 2-week iteration planning

### Execution Plans
- **Location**: `docs/execution-plans/`
- **Purpose**: Detailed multi-day feature implementation plans

### Infrastructure Plans
- **Location**: `infra/`
- **Purpose**: Infrastructure deployment and configuration plans
- **Format**: Detailed execution plans with prerequisites, steps, validations
- **Examples**:
  - `infra/INFRASTRUCTURE_DEPLOYMENT_PLAN.md` - Full cluster deployment (F1-F6)
  - `infra/ANSIBLE_BOOTSTRAP_PLAN.md` - k3s bootstrap
  - `infra/TLS_CERT_MANAGER_HTTP01.md` - TLS configuration
  - `infra/dns/` - DNS setup guides

---

## Weekly Plan Template

```markdown
# Week Plan: YYYY-MM-DD to YYYY-MM-DD

## Goals
- [Goal 1]
- [Goal 2]
- [Goal 3]

## Tasks

### Monday: Planning & Refinement
- [ ] **Task**: Review backlog and prioritize
  - **Mode**: Interactive
  - **Tool**: Cursor Chat
  - **Output**: This plan document
  - **Estimated**: 1h

### Tuesday: Spec Creation
- [ ] **Task**: Create technical spec for Feature X
  - **Mode**: Interactive
  - **Tool**: Cursor Chat (Sonnet 4.5)
  - **Input**: docs/requirements/feature-x.md
  - **Output**: docs/specs/feature-x-spec.md
  - **Estimated**: 2-3h

### Wednesday: Implementation
- [ ] **Task**: Implement Feature X
  - **Mode**: Autonomous
  - **Tool**: Claude Code CLI (can delegate completely)
  - **Input**: docs/specs/feature-x-spec.md
  - **Output**: apps/backend/core/domain/feature_x.py + tests
  - **Estimated**: 4-5h

### Thursday: Review & Debug
- [ ] **Task**: Code review and fix issues
  - **Mode**: Interactive
  - **Tool**: Cursor Chat
  - **Focus**: Performance, security, test coverage
  - **Estimated**: 2-3h

### Friday: Testing & Documentation
- [ ] **Task**: Integration tests and docs
  - **Mode**: Hybrid (autonomous for tests, interactive for docs)
  - **Tool**: Cursor Agent (tests) + Cursor Chat (docs)
  - **Estimated**: 3-4h

## Blockers
- [List any blockers]

## Notes
- [Additional context]
```

---

## How to Create Weekly Plan

### Step 1: Create Plan (INTERACTIVE MODE)

```bash
# Monday morning
cd docs/plan

# Open Cursor Chat (Interactive Mode)
# Prompt:
"""
Help me create this week's plan (week-2024-11-18.md).

Context:
- Backlog items: [paste from issues/project board]
- Current priorities: [list priorities]
- Available time: [estimate]

For each task, suggest:
1. Which mode to use (interactive vs autonomous)
2. Which tool is best
3. Time estimate
"""

# Iterate with AI to refine plan
# Save to week-2024-11-18.md

git add docs/plan/week-2024-11-18.md
git commit -m "docs: add weekly plan for 2024-11-18 [i:cursor-sonnet]"
git push
```

### Step 2: Follow Plan Throughout Week

- **Monday PM**: Complete planning, start spec refinement
- **Tuesday**: Spec creation (interactive)
- **Wednesday**: Implementation (autonomous if spec clear)
- **Thursday**: Review (interactive)
- **Friday**: Finalize and document

### Step 3: Review at Week End

```bash
# Friday afternoon
# Update plan with actuals:
# - What was completed
# - What blocked
# - What moved to next week

git add docs/plan/week-2024-11-18.md
git commit -m "docs: update weekly plan with actuals [i:cursor-sonnet]"
```

---

## Mode Annotations in Plans

When planning tasks, annotate with mode and tool:

```markdown
### Task: Implement Risk Calculator
- **Mode**: Autonomous (spec is complete)
- **Tool**: Claude Code CLI
- **Why**: Spec at docs/specs/risk-calculator-spec.md is 100% clear,
  can delegate completely and do other work
- **Tag**: `feat: implement risk calculator [a:claude-cli]`
```

```markdown
### Task: Debug Race Condition
- **Mode**: Interactive (investigation needed)
- **Tool**: Cursor Chat
- **Why**: Need iterative debugging, hypotheses testing
- **Tag**: `fix: resolve order race condition [i:cursor-sonnet]`
```

---

## Integration with Other Docs

### Links to Requirements
```markdown
### Task: Refine Multi-Timeframe Requirement
- **Input**: docs/requirements/multi-timeframe.md
- **Output**: docs/specs/multi-timeframe-spec.md
```

### Links to Specs
```markdown
### Task: Implement Order Tracking
- **Input**: docs/specs/order-tracking-spec.md
- **Output**: apps/backend/core/domain/order_tracking.py
```

### Links to ADRs
```markdown
### Task: Implement Event Bus
- **Reference**: docs/adr/ADR-XXXX-event-bus-pattern.md
```

---

## Best Practices

### ✅ Do
- Create plan Monday morning (interactive mode)
- Annotate every task with mode + tool
- Estimate time realistically
- Update plan with actuals Friday
- Review and adjust as week progresses

### ❌ Don't
- Skip mode annotations (defeats purpose of mode-first)
- Over-commit (better to under-promise and over-deliver)
- Ignore blockers (document them immediately)
- Create plan in autonomous mode (planning needs discussion)

---

## Example Plans

See:
- `week-2024-11-18.md` - Current week example
- `sprint-01-2024-11-04.md` - Sprint planning example

---

## Metrics

```bash
# Tasks completed by mode
grep "\[i:" docs/plan/week-*.md | wc -l  # Interactive
grep "\[a:" docs/plan/week-*.md | wc -l  # Autonomous

# Compare planned vs actual
# (Manual review of week plans)
```

---

## References

- **.ai-agents/AI-WORKFLOW.md** - Detailed workflows by mode
- **.ai-agents/MODES.md** - Mode selection guide
- **docs/execution-plans/** - Multi-day detailed plans

---

**Maintained by**: Robson Bot Core Team
**Last Updated**: 2025-11-23
**Version**: 1.1
