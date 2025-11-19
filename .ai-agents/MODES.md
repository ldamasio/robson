# AI Modes - Work Modes

## Philosophy: Mode-First, Tool-Second

The primary decision is not "which tool?", but:
**"Do I need interactive or autonomous mode?"**

After that, choose the appropriate tool within the mode.

---

## INTERACTIVE MODE

### Definition
You **actively participate** in the development process:
- Give continuous feedback
- Guide decisions
- Adjust during implementation
- Fine control of the outcome

### Available Tools

#### Cursor Chat (Interactive)
**Access**: Native chat in Cursor IDE
**Models**: Claude Sonnet 4.5, GPT-4o, GPT-4, etc.

**Characteristics**:
- Integrated into IDE
- Access to project context
- Fast for code exploration
- Multi-model (switch model per task)

**When to use**:
- Fast code exploration
- Preference for Claude Sonnet 4.5
- Already working in Cursor

**Tag**: `[i:cursor-sonnet]`, `[i:cursor-gpt4o]`, etc.

#### Codex (Interactive)
**Access**: OpenAI extension in Cursor
**Models**: GPT-4, GPT-4o

**Characteristics**:
- Via extension in Cursor
- OpenAI specialization
- Full Access mode available

**When to use**:
- Preference for GPT-4/GPT-4o
- Familiarity with OpenAI
- Tasks where OpenAI excels

**Tag**: `[i:codex-gpt4]`, `[i:codex-gpt4o]`

### Comparison: Cursor Chat vs Codex (Interactive)

| Aspect | Cursor Chat | Codex |
|--------|-------------|-------|
| Models | Sonnet 4.5, GPT-4o, etc. | GPT-4, GPT-4o |
| Access | Native | Extension |
| Switch model | ✅ Easy | ❌ Fixed to GPT |
| IDE context | ✅ Native | ✅ Via extension |

**Free choice**: both work well in interactive mode. Use what you prefer or choose by model.

### Typical Flows (Interactive)

#### Code Exploration
```
[In Cursor Chat or Codex]
"Where is the risk calculation logic implemented?"
"How does multi-tenant isolation work?"
"Show all uses of the Order model"

→ Immediate responses
→ Follow-up questions
→ No commit (exploration only)
```

#### Requirement Refinement
```
[In Cursor Chat or Codex]
1. Open docs/requirements/multi-timeframe.md
2. "Convert this to technical spec"
3. Review generated spec
4. "Add error handling for missing data"
5. Iterate until spec is complete
6. Save to docs/specs/multi-timeframe-spec.md

git commit -m "docs: add multi-timeframe spec [i:cursor-sonnet]"
```

#### Complex Debugging
```
[In Cursor Chat or Codex]
1. Reproduce bug
2. Share stack trace
3. AI analyzes context
4. You validate hypotheses
5. AI suggests fix
6. You test
7. Iterative adjustments until resolved

git commit -m "fix: resolve async race condition [i:codex-gpt4]"
```

#### Implementation with Decisions
```
[In Cursor Chat or Codex]
1. Open spec (with some ambiguities)
2. "Implement this spec"
3. AI asks about trade-offs
4. You decide approach
5. AI implements
6. You review and adjust
7. Iterations until satisfied

git commit -m "feat: implement risk calculator [i:cursor-sonnet]"
```

---

## AUTONOMOUS MODE

### Definition
You **delegate the task** and let AI work:
- Execution without continuous supervision
- Clear spec/instruction required
- You can do other things
- Review result at the end

### Available Tools

#### Cursor Chat Agent
**Access**: "Agent" mode in Cursor Chat
**Models**: Claude Sonnet 4.5, GPT-4o, etc.

**Characteristics**:
- Runs inside IDE (can see progress)
- Full project access
- Can use Sonnet 4.5 (excellent for code)
- You remain in IDE but don't supervise

**When to use**:
- Want to use Sonnet 4.5 in autonomous mode
- Prefer to see progress in IDE
- Task benefits from complete project context
- Don't need to close IDE

**Tag**: `[a:cursor-agent-sonnet]`, `[a:cursor-agent-gpt4o]`

**Invocation**:
```
1. Open Cursor Chat
2. Select model (e.g., Sonnet 4.5)
3. Activate "Agent" mode
4. Give clear instruction
5. Let it run
```

#### Codex Agent / Full Access
**Access**: "Agent" or "Full Access" mode in Codex
**Models**: GPT-4, GPT-4o

**Characteristics**:
- Via extension, autonomous mode
- OpenAI specialization
- Full Access: can modify multiple files
- Runs inside IDE

**When to use**:
- Preference for GPT-4 autonomous
- Task where OpenAI excels
- Stay in IDE

**Tag**: `[a:codex-agent-gpt4]`

**Invocation**:
```
1. Open Codex extension
2. Activate Agent or Full Access mode
3. Give clear instruction
4. Let it run
```

#### Claude Code CLI
**Access**: Command line tool
**Model**: Claude Sonnet 4.5 (fixed)

**Characteristics**:
- **Purely autonomous** (no interactive mode)
- Runs outside IDE
- Can close IDE and do other things
- Best for super clear specs
- Scriptable (can run in CI/CD)

**When to use**:
- Technical spec 100% complete
- Want to fully delegate
- Can close IDE
- Mechanical implementation
- Preference for command line

**Tag**: `[a:claude-cli]`

**Invocation**:
```bash
claude-code --spec docs/specs/feature.md \
            --output src/module/ \
            --test-framework pytest
```

### Comparison: Agents

| Aspect | Cursor Chat Agent | Codex Agent | Claude Code CLI |
|--------|------------------|-------------|-----------------|
| Models | Sonnet 4.5, GPT-4o | GPT-4, GPT-4o | Sonnet 4.5 |
| Environment | Inside IDE | Inside IDE | CLI (outside IDE) |
| See progress | ✅ Yes | ✅ Yes | ❌ No (logs only) |
| Can close IDE | ❌ No | ❌ No | ✅ Yes |
| Scriptable | ❌ No | ❌ No | ✅ Yes (bash/CI) |
| Purely autonomous | ✅ Yes | ✅ Yes | ✅ Yes (only mode) |

### Typical Flows (Autonomous)

#### Implementing Clear Spec (Cursor Chat Agent)
```bash
git pull origin main

# In Cursor Chat
1. Validate that spec is complete
2. Activate Agent mode
3. Select Sonnet 4.5
4. Instruction: "Implement docs/specs/risk-calculator-spec.md in src/risk/"
5. Let it run (can do other things on computer)
6. Review result

pytest tests/risk/
git commit -m "feat: implement risk calculator from spec [a:cursor-agent-sonnet]"
git push
```

#### Batch Test Generation (Codex Agent)
```bash
git pull origin main

# In Codex
1. Activate Agent mode
2. Instruction: "Generate integration tests for all modules in src/signals/"
3. Let it run
4. Review generated tests

pytest tests/signals/
git commit -m "test: generate signals integration tests [a:codex-agent-gpt4]"
git push
```

#### Implementing Spec (Claude Code CLI)
```bash
git pull origin main

# Validate spec
cat docs/specs/portfolio-rebalancer-spec.md

# Execute (can close IDE and do other things)
claude-code --spec docs/specs/portfolio-rebalancer-spec.md \
            --output src/portfolio/

# Review result
pytest tests/portfolio/
git commit -m "feat: implement portfolio rebalancer [a:claude-cli]"
git push
```

#### Structural Migration (Claude Code CLI)
```bash
# Ideal for Claude CLI: can run and go to lunch
git pull origin main

claude-code --spec docs/specs/migration-django-5.2.md \
            --output apps/backend/

# Come back from lunch, review
pytest tests/
git commit -m "refactor: migrate to Django 5.2 [a:claude-cli]"
```

---

## DECISION TREE: Mode → Tool

```
┌─────────────────────────────────┐
│ I have a task                   │
└──────────┬──────────────────────┘
           │
           ▼
    ┌─────────────────┐
    │ Do I need       │
    │ CONTINUOUS      │
    │ FEEDBACK?       │
    └────────┬────────┘
             │
      ┌──────┴──────┐
      │             │
     YES           NO
      │             │
      ▼             ▼
  INTERACTIVE   AUTONOMOUS
      │             │
      ▼             ▼
 ┌─────────┐   ┌─────────┐
 │Prefer   │   │Is spec  │
 │Sonnet?  │   │100% OK? │
 └────┬────┘   └────┬────┘
      │             │
  ┌───┴───┐    ┌────┴─────┐
  │       │    │          │
 YES     NO   YES        NO
  │       │    │          │
  ▼       ▼    │          ▼
Cursor  Codex  │      Refine
 Chat    GPT4  │    (interactive)
Sonnet         │
               ▼
        ┌──────────────┐
        │Want to close │
        │IDE?          │
        └──────┬───────┘
               │
        ┌──────┴──────┐
        │             │
       YES           NO
        │             │
        ▼             ▼
   Claude Code    Cursor/Codex
      CLI            Agent
```

---

## QUICK MATRIX BY TASK TYPE

| Task | Mode | Suggested Tool | Tag | Reason |
|------|------|----------------|-----|--------|
| Explore code | Interactive | Cursor Chat | `[i:cursor-sonnet]` | Fast navigation |
| Understand algorithm | Interactive | Cursor/Codex | `[i:cursor-*]` | Explanation |
| Discuss architecture | Interactive | Cursor/Codex | `[i:*]` | Contextual decisions |
| Refine requirement | Interactive | Cursor/Codex | `[i:*]` | Iteration needed |
| Complex debug | Interactive | Cursor/Codex | `[i:*]` | Iterative feedback |
| Implement clear spec | Autonomous | Claude CLI | `[a:claude-cli]` | Delegate 100% |
| Implement spec (see progress) | Autonomous | Cursor Agent | `[a:cursor-agent-sonnet]` | Sonnet + see progress |
| Generate tests in batch | Autonomous | Any Agent | `[a:*-agent-*]` | Mechanical |
| Structural migration | Autonomous | Claude CLI | `[a:claude-cli]` | Can disconnect |
| Code review | Interactive | Cursor/Codex | `[i:*]` | Critical analysis |
| Exploratory refactoring | Interactive | Cursor/Codex | `[i:*]` | Decisions during |
| Mechanical refactoring | Autonomous | Any Agent | `[a:*]` | Clear spec |

---

## WHEN TO SWITCH MODES

### Interactive → Autonomous
When you realize that:
- Spec became clear enough
- Decisions were made
- Rest is mechanical
- Can delegate

**Action**:
```bash
# Commit interactive work
git commit -m "docs: refine spec with decisions [i:cursor-sonnet]"
git push

# New autonomous session
git pull
# Use agent or Claude CLI
git commit -m "feat: implement refined spec [a:claude-cli]"
```

### Autonomous → Interactive
When you realize that:
- Spec had ambiguities
- Agent stuck on decision
- Result is not expected
- Need to adjust approach

**Action**:
```bash
# Agent generated something, but has issues
# New interactive session to adjust
git commit -m "fix: adjust agent implementation [i:cursor-sonnet]"
```

---

## GOLDEN RULES

### ✅ ALWAYS
1. Decide mode BEFORE tool
2. Atomic session: one mode at a time
3. Complete tag: `[mode:tool-model]`
4. Clear spec for autonomous mode
5. Active feedback for interactive mode

### ❌ NEVER
1. Mix modes in same commit
2. Use autonomous without clear spec
3. Use interactive for mechanical task
4. Omit model in tag
5. Switch tool without commit

---

## COMPLETE TAG EXAMPLES

```bash
# Interactive
[i:cursor-sonnet]   # Cursor Chat, Sonnet 4.5, interactive
[i:cursor-gpt4o]    # Cursor Chat, GPT-4o, interactive
[i:codex-gpt4]      # Codex, GPT-4, interactive

# Autonomous
[a:cursor-agent-sonnet]   # Cursor Agent, Sonnet 4.5
[a:cursor-agent-gpt4o]    # Cursor Agent, GPT-4o
[a:codex-agent-gpt4]      # Codex Agent, GPT-4
[a:claude-cli]            # Claude Code CLI (always Sonnet 4.5)
```

---

## METRICS

### By Mode
```bash
# Interactive vs autonomous commits
git log --grep="\[i:" --oneline | wc -l
git log --grep="\[a:" --oneline | wc -l
```

### By Tool
```bash
# Which tool most used?
git log --grep="\[.*:cursor" --oneline | wc -l
git log --grep="\[.*:codex" --oneline | wc -l
git log --grep="\[.*:claude-cli" --oneline | wc -l
```

### By Model
```bash
# Sonnet vs GPT
git log --grep="sonnet" --oneline | wc -l
git log --grep="gpt4" --oneline | wc -l
```

---

## EXECUTIVE SUMMARY

| Question | Answer |
|----------|--------|
| Need continuous feedback? | **INTERACTIVE** |
| Can delegate? | **AUTONOMOUS** |
| Interactive with Sonnet? | Cursor Chat |
| Interactive with GPT-4? | Codex |
| Autonomous + see progress? | Cursor/Codex Agent |
| Autonomous + close IDE? | Claude Code CLI |
| Incomplete spec? | INTERACTIVE (refine) |
| Perfect spec? | AUTONOMOUS (delegate) |

---

**Remember**: **Mode-first, tool-second**.
