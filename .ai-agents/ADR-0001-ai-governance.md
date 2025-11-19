# ADR-0001: Mode-First AI Governance Framework

## Status
Accepted - 2024-11-16

## Context

Robson Bot is developed with assistance from AI through tools that support different **modes of work**:

### Available Tools
1. **Cursor Chat** (native in Cursor IDE)
   - Interactive Mode: normal chat
   - Autonomous Mode: Agent mode
   - Models: Claude Sonnet 4.5, GPT-4o, etc.

2. **Codex** (OpenAI via extension in Cursor)
   - Interactive Mode: normal chat
   - Autonomous Mode: Agent mode + Full Access
   - Models: GPT-4, GPT-4o

3. **Claude Code CLI**
   - Autonomous Mode ONLY
   - Model: Claude Sonnet 4.5

### Problem
The traditional decision "which tool to use?" is secondary. The primary decision is: **"do I need interactive or autonomous mode?"**

## Decision

### Mode-First Principle

Governance is based on **work mode**, not specific tool:

```
DECISION 1: Work mode?
├─ INTERACTIVE → Cursor Chat OR Codex (free choice)
└─ AUTONOMOUS → Cursor Chat Agent OR Codex Agent OR Claude Code CLI
```

### Defined Work Modes

#### INTERACTIVE MODE
**Characteristics**:
- You provide continuous feedback
- Guidance and adjustments during implementation
- Real-time decisions
- Fine control of the process

**Tools**:
- Cursor Chat (normal chat, any model)
- Codex (normal chat via extension)

**When to use**:
- Code exploration
- Complex debugging
- Architectural decisions
- Implementation with trade-offs
- Refine requirements into specs
- Code review

#### AUTONOMOUS MODE
**Characteristics**:
- You delegate the task
- Execution without continuous supervision
- Spec/instruction must be clear
- You can do other things while it runs

**Tools**:
- Cursor Chat Agent (agent mode with any model)
- Codex Agent + Full Access (agent mode)
- Claude Code CLI (always autonomous)

**When to use**:
- Complete and clear technical spec
- Mechanical implementation
- Batch generation (tests, migrations)
- Structural refactorings
- When you want to delegate and disconnect

### Commit Tags

Format: `type: subject [mode:tool-model]`

**Interactive Mode**:
```bash
feat: add risk calculator [i:cursor-sonnet]
fix: resolve race condition [i:codex-gpt4]
refactor: optimize queries [i:cursor-gpt4o]
```

**Autonomous Mode**:
```bash
feat: implement multi-timeframe spec [a:claude-cli]
test: generate integration tests [a:cursor-agent-sonnet]
refactor: migrate to Django 5.2 [a:codex-agent-gpt4]
```

**Legend**:
- `i:` = interactive
- `a:` = autonomous
- `cursor` = Cursor Chat
- `codex` = Codex extension
- `claude-cli` = Claude Code CLI
- `sonnet` = Claude Sonnet 4.5
- `gpt4` = GPT-4
- `gpt4o` = GPT-4o

### Main Decision Matrix

```
┌─────────────────────────────────┐
│  I have a task                  │
└───────────┬─────────────────────┘
            │
            ▼
     ┌──────────────┐
     │ Which mode?  │
     └──────┬───────┘
            │
    ┌───────┴────────┐
    │                │
    ▼                ▼
INTERACTIVE      AUTONOMOUS
    │                │
    │                ▼
    │         ┌──────────────┐
    │         │ Spec clear?  │
    │         └──────┬───────┘
    │                │
    │         ┌──────┴──────┐
    │         │             │
    │        YES           NO
    │         │             │
    │         ▼             ▼
    │    Delegate      Refine in
    │   (autonomous)    interactive
    │                      │
    └──────────────────────┘
            │
            ▼
    Choose tool
    (free, based on
     preference/context)
```

### Tool Choice (Within Mode)

#### For Interactive Mode:
**Cursor Chat OR Codex** - choice based on:
- **Cursor Chat**: already in IDE, access to Sonnet 4.5
- **Codex**: preference for GPT-4, familiar with OpenAI

Both are equivalent in interactive mode.

#### For Autonomous Mode:
**Cursor Chat Agent, Codex Agent, or Claude Code CLI** - choice based on:

| Tool | Advantages | When to prefer |
|------|-----------|----------------|
| **Cursor Chat Agent** | IDE open, Sonnet 4.5, project context | Task within IDE, want to see progress |
| **Codex Agent** | GPT-4, Full Access mode | Preference for GPT-4, OpenAI-focused task |
| **Claude Code CLI** | Purely autonomous, can close IDE | Complete spec, delegate 100%, do other things |

### Atomic Session Rule

1. `git pull origin main`
2. Choose **mode** (interactive or autonomous)
3. Choose **tool** within mode
4. Work in small batch
5. Test
6. Commit with tag: `[mode:tool-model]`
7. `git push`
8. Switch mode/tool only after commit

## Consequences

### Positive
- Clear decision: mode first, tool second
- Flexibility of choice within mode
- Traceability of mode AND tool
- Not locked to specific tool
- Sonnet 4.5 accessible via Cursor Chat OR Claude Code CLI

### Negative
- More verbose commit tags
- Learning curve to understand modes
- Increased decision overhead (mode + tool + model)

### Mitigations
- Clear documentation in MODES.md
- Simplified decision matrix
- Practical examples by scenario

## Integration with Existing Governance

This ADR complements existing AI governance:
- **docs/AGENTS.md**: Comprehensive guide (architecture, patterns)
- **.cursorrules**: Hexagonal architecture rules
- **CLAUDE.md**: Quick reference

Mode-first governance adds:
- Explicit work mode framework (interactive vs autonomous)
- Tool selection guidance per mode
- Commit tagging convention
- Atomic session workflow

## Compliance
- All new code follows this ADR
- Commits MUST include complete tag
- Review this ADR every 3 months

## References
- [MODES.md](.ai-agents/MODES.md) - Detailed mode documentation
- [DECISION-MATRIX.md](.ai-agents/DECISION-MATRIX.md) - Decision tree
- [AI-WORKFLOW.md](.ai-agents/AI-WORKFLOW.md) - Workflows by mode
- [docs/AGENTS.md](../docs/AGENTS.md) - Architecture and patterns
