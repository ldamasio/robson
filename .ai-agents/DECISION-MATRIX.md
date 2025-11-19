# Decision Matrix - Mode-First Approach

## Mental Roadmap

```
STEP 1: What type of work?
├─ EXPLORATION/UNDERSTANDING → INTERACTIVE (Cursor Chat)
├─ DECISION/DISCUSSION → INTERACTIVE (Cursor/Codex)
├─ IMPLEMENTATION → STEP 2
└─ DELEGATION → AUTONOMOUS

STEP 2: For implementation, do I have spec?
├─ NO → INTERACTIVE (create spec first)
└─ YES → STEP 3

STEP 3: Is the spec 100% clear?
├─ NO → INTERACTIVE (refine)
└─ YES → STEP 4

STEP 4: Do I need feedback during implementation?
├─ YES → INTERACTIVE
└─ NO → AUTONOMOUS

STEP 5 (if INTERACTIVE): Which model to prefer?
├─ Sonnet 4.5 → Cursor Chat
├─ GPT-4/4o → Codex
└─ Either → Cursor Chat (native, faster)

STEP 6 (if AUTONOMOUS): Can/want to close IDE?
├─ YES → Claude Code CLI
├─ NO → Cursor Agent or Codex Agent
└─ See progress → Cursor/Codex Agent

STEP 7 (if Agent): Which model?
├─ Sonnet 4.5 → Cursor Agent
└─ GPT-4 → Codex Agent
```

---

## Visual Matrix

```
                ┌──────────────┐
                │ TASK         │
                └──────┬───────┘
                       │
          ┌────────────┴────────────┐
          │                         │
          ▼                         ▼
    EXPLORATION              IMPLEMENTATION
          │                         │
          ▼                         ▼
     INTERACTIVE              ┌──────────┐
    (Cursor Chat)             │ Spec?    │
                             └─────┬─────┘
                                   │
                           ┌───────┴────────┐
                           │                │
                          NO               YES
                           │                │
                           ▼                ▼
                      INTERACTIVE      ┌──────────┐
                     (create spec)     │ Clear?   │
                                      └─────┬─────┘
                                            │
                                    ┌───────┴───────┐
                                    │               │
                                   NO              YES
                                    │               │
                                    ▼               ▼
                               INTERACTIVE     ┌──────────┐
                              (refine)         │ Feedback?│
                                              └─────┬─────┘
                                                    │
                                            ┌───────┴───────┐
                                            │               │
                                           YES             NO
                                            │               │
                                            ▼               ▼
                                       INTERACTIVE      AUTONOMOUS
                                            │               │
                                            ▼               ▼
                                      Choose Tool    Choose Tool
                                    (Cursor/Codex)  (Agent/CLI)
```

---

## Decision Table by Scenario

### Scenario 1: "I need to understand existing code"
```
What: Code exploration
↓
Mode: INTERACTIVE (need to ask questions)
↓
Tool: Cursor Chat (faster for navigation)
↓
Tag: [i:cursor-sonnet]
```

### Scenario 2: "I have a requirement to implement"
```
What: Implementation
↓
Do I have spec? NO
↓
Mode: INTERACTIVE (create spec first)
↓
Tool: Cursor Chat or Codex (free choice)
↓
Tag: [i:cursor-sonnet] or [i:codex-gpt4]
↓
After spec is ready: Switch to AUTONOMOUS
```

### Scenario 3: "I have a complete technical spec"
```
What: Implementation
↓
Do I have spec? YES
↓
Is spec clear? YES
↓
Need feedback? NO (spec has all decisions)
↓
Mode: AUTONOMOUS
↓
Can close IDE? YES
↓
Tool: Claude Code CLI
↓
Tag: [a:claude-cli]
```

### Scenario 4: "I found a bug and need to debug"
```
What: Debugging
↓
Mode: INTERACTIVE (always - need investigation)
↓
Tool: Cursor Chat or Codex (preference)
↓
Tag: [i:cursor-sonnet] or [i:codex-gpt4]
```

### Scenario 5: "I need to generate 50 unit tests"
```
What: Batch generation
↓
Is it mechanical? YES
↓
Mode: AUTONOMOUS
↓
Want to see progress? YES (stay in IDE)
↓
Tool: Cursor Agent or Codex Agent
↓
Tag: [a:cursor-agent-sonnet] or [a:codex-agent-gpt4]
```

### Scenario 6: "I need to discuss architecture"
```
What: Architectural decision
↓
Mode: INTERACTIVE (discussion needed)
↓
Tool: Cursor Chat or Codex (free choice)
↓
Tag: [i:cursor-sonnet]
↓
Output: Create ADR documenting decision
```

### Scenario 7: "I need to migrate Django 4 → Django 5.2"
```
What: Structural migration
↓
Do I have spec? YES (migration guide)
↓
Is spec clear? YES
↓
Mode: AUTONOMOUS
↓
Can close IDE? YES (long task)
↓
Tool: Claude Code CLI
↓
Tag: [a:claude-cli]
```

---

## Quick Reference: Task → Mode → Tool

| Task Type | Mode | Primary Tool | Alternative | Tag Example |
|-----------|------|--------------|-------------|-------------|
| **Explore codebase** | Interactive | Cursor Chat | Codex | `[i:cursor-sonnet]` |
| **Understand algorithm** | Interactive | Cursor Chat | Codex | `[i:cursor-sonnet]` |
| **Debug error** | Interactive | Cursor Chat | Codex | `[i:cursor-sonnet]` |
| **Discuss architecture** | Interactive | Cursor Chat | Codex | `[i:cursor-sonnet]` |
| **Create requirement** | Interactive | Cursor Chat | Codex | `[i:cursor-sonnet]` |
| **Refine spec** | Interactive | Cursor Chat | Codex | `[i:cursor-sonnet]` |
| **Code review** | Interactive | Cursor Chat | Codex | `[i:cursor-sonnet]` |
| **Implement clear spec** | Autonomous | Claude CLI | Cursor Agent | `[a:claude-cli]` |
| **Generate tests** | Autonomous | Cursor Agent | Claude CLI | `[a:cursor-agent-sonnet]` |
| **Mechanical refactoring** | Autonomous | Any Agent | Claude CLI | `[a:cursor-agent-sonnet]` |
| **Structural migration** | Autonomous | Claude CLI | - | `[a:claude-cli]` |
| **Add feature (no spec)** | Interactive → Autonomous | Cursor Chat → Agent | - | `[i:*]` then `[a:*]` |

---

## Decision Factors

### Factor 1: Clarity of Requirements
```
┌─────────────────────────────────────────┐
│ Requirement Clarity Spectrum            │
├─────────────────────────────────────────┤
│ Vague idea                              │
│    ↓ INTERACTIVE (explore & refine)     │
│ Clear business requirement              │
│    ↓ INTERACTIVE (create spec)          │
│ Technical spec with gaps                │
│    ↓ INTERACTIVE (refine spec)          │
│ Complete technical spec                 │
│    ↓ AUTONOMOUS (implement)             │
│ Implementation ready spec               │
│    ↓ AUTONOMOUS (Claude CLI ideal)      │
└─────────────────────────────────────────┘
```

### Factor 2: Complexity of Decision-Making
```
┌─────────────────────────────────────────┐
│ Decision Complexity                     │
├─────────────────────────────────────────┤
│ Many trade-offs, unclear path           │
│    → INTERACTIVE                        │
│ Some decisions, mostly clear            │
│    → INTERACTIVE (make decisions first) │
│ All decisions made, execution only      │
│    → AUTONOMOUS                         │
│ Mechanical, no decisions needed         │
│    → AUTONOMOUS (Claude CLI ideal)      │
└─────────────────────────────────────────┘
```

### Factor 3: Need for Supervision
```
┌─────────────────────────────────────────┐
│ Supervision Need                        │
├─────────────────────────────────────────┤
│ Want to guide every step                │
│    → INTERACTIVE                        │
│ Want to review checkpoints              │
│    → AUTONOMOUS (Agent, see progress)   │
│ Trust spec, review at end               │
│    → AUTONOMOUS (Claude CLI)            │
│ Complete delegation                     │
│    → AUTONOMOUS (Claude CLI)            │
└─────────────────────────────────────────┘
```

---

## Anti-Patterns and Solutions

### ❌ Anti-Pattern 1: Using Autonomous without Clear Spec
```
Symptom: Agent produces wrong implementation
↓
Cause: Spec was ambiguous
↓
Solution: Switch to INTERACTIVE, refine spec, then re-run AUTONOMOUS
```

### ❌ Anti-Pattern 2: Using Interactive for Mechanical Task
```
Symptom: Wasting time supervising obvious work
↓
Cause: Task was mechanical, spec was clear
↓
Solution: Switch to AUTONOMOUS, let it run
```

### ❌ Anti-Pattern 3: Wrong Tool for Mode
```
Symptom: Using Claude CLI but need to ask questions
↓
Cause: Claude CLI is autonomous-only, no interactive mode
↓
Solution: Use Cursor Chat or Codex for interactive work
```

### ❌ Anti-Pattern 4: Mixing Modes in Same Session
```
Symptom: Commit tags don't match work done
↓
Cause: Started interactive, switched to autonomous mid-session
↓
Solution: Commit interactive work first, then start new autonomous session
```

---

## Mode Selection Checklist

Before starting work, ask yourself:

### Is this Interactive or Autonomous?
- [ ] Do I know exactly what to build? (If NO → Interactive)
- [ ] Are all architectural decisions made? (If NO → Interactive)
- [ ] Can the task be executed mechanically? (If YES → Autonomous)
- [ ] Do I need to provide guidance during implementation? (If YES → Interactive)

### If Interactive, which tool?
- [ ] Do I prefer Sonnet 4.5? → Cursor Chat
- [ ] Do I prefer GPT-4? → Codex
- [ ] Am I already in Cursor? → Cursor Chat (faster)

### If Autonomous, which tool?
- [ ] Can I close the IDE? → Claude Code CLI
- [ ] Do I want to see progress? → Cursor/Codex Agent
- [ ] Is it a long-running task? → Claude Code CLI
- [ ] Is the spec 100% complete? → Claude Code CLI

---

## Summary: The Four Questions

```
1. Do I need continuous feedback?
   YES → INTERACTIVE
   NO → Continue to Q2

2. Do I have a clear, complete spec?
   NO → INTERACTIVE (create/refine spec first)
   YES → AUTONOMOUS

3. (If Interactive) Which model?
   Sonnet 4.5 → Cursor Chat
   GPT-4 → Codex

4. (If Autonomous) Can I close IDE?
   YES → Claude Code CLI
   NO → Cursor/Codex Agent
```

---

**Remember**: Mode first, tool second. When in doubt, start INTERACTIVE.
