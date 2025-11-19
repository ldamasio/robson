# AI Workflow - Mode-First Approach

## Philosophy

Workflows are organized by **work mode** (interactive vs autonomous), not by specific tool.

**Principle**: Choose mode based on task nature, then choose tool within mode.

---

## Weekly Flow Example

### MONDAY: Planning
**Mode**: INTERACTIVE (needs discussion)
**Tool**: Cursor Chat or Codex (free choice)
**Input**: Backlog, issues, ideas
**Output**: `docs/plan/week-YYYY-MM-DD.md`

```bash
# In Cursor Chat or Codex (interactive mode)
1. Open project
2. Create docs/plan/week-2024-11-18.md
3. Chat: "Help me organize this week's tasks based on:
   - [list requirements]
   - [list priorities]"
4. Review and adjust plan
5. Annotate which mode to use for each task

git commit -m "docs: add weekly plan [i:cursor-sonnet]"
git push
```

**Example plan with modes annotated**:
```markdown
# Week Plan: 2024-11-18

## Tasks

### T1: Refine multi-tenant isolation requirement
- **Mode**: Interactive (needs discussion)
- **Tool suggested**: Cursor Chat (Sonnet good for architecture)
- **Input**: docs/requirements/multi-tenant.md
- **Output**: docs/specs/multi-tenant-isolation-spec.md

### T2: Implement risk calculator spec
- **Mode**: Autonomous (spec already clear)
- **Tool suggested**: Claude Code CLI (can delegate)
- **Input**: docs/specs/risk-calculator-spec.md
- **Output**: apps/backend/core/domain/risk.py + tests
```

---

### TUESDAY: Refinement
**Mode**: INTERACTIVE (refinement always requires iteration)
**Tool**: Cursor Chat or Codex
**Input**: `docs/requirements/*.md`
**Output**: `docs/specs/*.md`

#### Atomic Refinement Session

```bash
# Pull latest
git pull origin main

# ============================================
# INTERACTIVE MODE - Cursor Chat (Sonnet 4.5)
# ============================================

# 1. Open requirement in Cursor
code docs/requirements/order-tracking.md

# 2. Select requirement text

# 3. Cursor Chat (choose Sonnet 4.5):
"""
Convert this business requirement to technical spec with:

1. Module structure (hexagonal architecture)
2. Domain entities with type hints
3. Use case definitions
4. Port interfaces (Repository, ExchangeClient, etc.)
5. Adapter implementations
6. Test scenarios (unit + integration)
7. Django model mapping

Format as executable spec ready for autonomous implementation.
"""

# 4. ITERATION with feedback:
You: "Add multi-tenant isolation validation"
Cursor: [updates spec]

You: "How should we handle WebSocket notifications?"
Cursor: [adds WebSocket strategy]

You: "Include integration with existing Portfolio model"
Cursor: [adds integration section]

# 5. When spec is complete and clear
# Save to docs/specs/order-tracking-spec.md

# 6. Validate spec (mental checklist):
# - [ ] All interfaces defined?
# - [ ] Type hints specified?
# - [ ] Step-by-step logic clear?
# - [ ] Test cases defined?
# - [ ] Error handling described?
# - [ ] No ambiguities?

# 7. Commit
git add docs/specs/order-tracking-spec.md
git commit -m "docs: add order tracking technical spec [i:cursor-sonnet]"
git push
```

**Why interactive?**
- Requirements have natural ambiguities
- Architectural decisions emerge during refinement
- Feedback on technical viability
- Iteration until spec is executable

---

### WEDNESDAY: Implementation (AUTONOMOUS MODE)
**Mode**: AUTONOMOUS (spec already clear from Tuesday)
**Tool**: Choice based on context
**Input**: `docs/specs/*.md`
**Output**: `apps/backend/**/*.py` + tests

#### Scenario A: Claude Code CLI (Delegate 100%)

```bash
# ===================================================
# AUTONOMOUS MODE - Claude Code CLI
# ===================================================

# New atomic session
git pull origin main

# 1. Validate that spec is complete
cat docs/specs/order-tracking-spec.md
# Mental checklist:
# ✓ Interfaces clear?
# ✓ Logic detailed?
# ✓ Test cases?
# ✓ Error handling?
# ✓ No "TODO" or "TBD"?

# 2. Execute Claude Code
claude-code --spec docs/specs/order-tracking-spec.md \
            --output apps/backend/core/ \
            --test-framework pytest

# 3. CAN CLOSE IDE AND DO OTHER THINGS
# Claude Code runs autonomously

# 4. When finished, review generated code
cat apps/backend/core/domain/order_tracking.py
ls -la apps/backend/monolith/api/tests/

# 5. Test
pytest apps/backend/monolith/api/tests/ -v

# 6. If all OK, commit
git add apps/backend/
git commit -m "feat: implement order tracking from spec [a:claude-cli]"
git push

# 7. If found issues, next session in interactive mode
```

**Why Claude CLI?**
- Spec is 100% clear
- Implementation is mechanical (follow spec)
- Can disconnect and do other things
- Scriptable (can run in CI later)

#### Scenario B: Cursor Agent (See Progress)

```bash
# ===================================================
# AUTONOMOUS MODE - Cursor Agent (Sonnet 4.5)
# ===================================================

# New atomic session
git pull origin main

# In Cursor:
# 1. Open Cursor Chat
# 2. Select model: Claude Sonnet 4.5
# 3. Activate "Agent" mode
# 4. Give clear instruction:

"""
Implement the spec at docs/specs/risk-calculator-spec.md

Output:
- apps/backend/core/domain/risk.py (domain entities)
- apps/backend/core/application/calculate_risk.py (use case)
- apps/backend/core/application/ports.py (add RiskRepository port)
- apps/backend/core/adapters/driven/persistence/django_risk_repo.py
- apps/backend/monolith/api/tests/test_risk_calculator.py

Follow hexagonal architecture. Include all test cases specified.
"""

# 5. Let Agent run
# YOU CAN SEE PROGRESS IN IDE
# But don't need to supervise

# 6. When finished, review
pytest apps/backend/monolith/api/tests/test_risk_calculator.py -v

# 7. Commit
git add apps/backend/
git commit -m "feat: implement risk calculator from spec [a:cursor-agent-sonnet]"
git push
```

**Why Cursor Agent (not Claude CLI)?**
- Want to see progress
- Prefer having IDE open
- Sonnet 4.5 available in agent mode
- More comfortable with graphical interface

---

### THURSDAY: Code Review (INTERACTIVE MODE)

**Mode**: INTERACTIVE (critical analysis requires discussion)
**Input**: Code generated (autonomous mode from Wednesday)
**Output**: Improvements, optimizations

```bash
# ===================================================
# INTERACTIVE MODE - Cursor Chat or Codex
# ===================================================

git pull origin main

# Review code that was generated autonomously yesterday
# 1. Open generated code
apps/backend/core/domain/order_tracking.py

# 2. Request critical review
Cursor Chat: "Review this code for:
- Hexagonal architecture compliance
- Domain model purity (no framework dependencies)
- Missing error handling
- Edge cases not covered
- Django integration issues
- Multi-tenant isolation
- Missing tests
- Performance bottlenecks"

# 3. Iterative analysis
AI: "Found 3 issues:
     1. Domain entity imports Django models (violates hexagonal arch)
     2. Missing validation for negative quantities
     3. Port interface missing tenant_id parameter"

You: "Fix issue 1 and 2. For issue 3, show me affected code"

AI: [shows problematic code]

You: "Add tenant_id to all port methods"

AI: [implements fix]

# 4. Test improvements
pytest apps/backend/monolith/api/tests/ -v

# 5. Commit improvements
git add apps/backend/
git commit -m "refactor: fix hexagonal violations and add tenant validation [i:cursor-sonnet]"
git push
```

---

### FRIDAY: Debugging (INTERACTIVE MODE)

**Mode**: ALWAYS INTERACTIVE (debugging requires iteration)
**Tool**: Cursor Chat or Codex

```bash
# ===================================================
# INTERACTIVE MODE - Debugging
# ===================================================

git pull origin main

# 1. Reproduce bug
pytest apps/backend/monolith/api/tests/test_order_service.py::test_concurrent_updates
# FAILED - race condition detected

# 2. Copy stack trace

# 3. Cursor Chat or Codex:
"""
I'm getting this race condition in order processing:

[stack trace here]

The test fails intermittently when multiple requests
update the same order concurrently. Here's the relevant code:

[paste code]

Help me debug and fix this.
"""

# 4. Iterative analysis
AI: "The issue is in line 45 - you're reading and writing
     Order.quantity without transaction isolation. Use select_for_update()."

You: "Show me the fix with Django's select_for_update"

AI: [shows code with proper locking]

You: "Will this impact performance significantly?"

AI: "Minimal impact since lock is only held during update.
     You're using PostgreSQL which handles this well."

You: "Implement the fix"

AI: [implements]

# 5. Test fix
pytest apps/backend/monolith/api/tests/ -v --count=100
# All pass!

# 6. Add regression test
You: "Add a test that specifically checks for this race condition"

AI: [adds test_concurrent_order_updates_no_race]

# 7. Commit
git add apps/backend/
git commit -m "fix: resolve race condition in order updates [i:cursor-sonnet]"
git push
```

**Why always interactive for debugging?**
- Bugs rarely have clear spec
- Investigation requires hypotheses and tests
- Decisions about fix during process
- Learning about the system

---

## Specialized Workflows by Mode

### EXPLORATION (Always Interactive)

```bash
# ===================================================
# EXPLORATION - INTERACTIVE MODE
# ===================================================

# Cursor Chat (faster for exploration)

# Navigation
"Where is the multi-tenant isolation implemented?"
"Show me all Django models in trading domain"
"How does the signal distribution work?"

# Understanding
"Explain this algorithm in PlaceOrderUseCase"
"What's the purpose of the BinanceClient adapter?"

# Pattern search
"Find all classes that implement RepositoryPort"
"Show me everywhere we query database without tenant filtering"

# NO COMMIT - exploration and learning only
```

### ARCHITECTURAL DISCUSSION (Always Interactive)

```bash
# ===================================================
# DISCUSSION - INTERACTIVE MODE
# ===================================================

# Cursor Chat or Codex (free choice)

You: "Should I use observer pattern or Django signals for order notifications?
      Current system has tight coupling between order creation and notifications."

AI: "Let's analyze both:

    Observer Pattern:
    Pros: ...
    Cons: ...

    Django Signals:
    Pros: ...
    Cons: ...

    For your hexagonal architecture, Observer Pattern is better because..."

You: "How would observer pattern work with Django?"

AI: [explains implementation]

You: "Show me a prototype"

AI: [creates prototype in /tmp/]

You: "I'll go with Observer Pattern. Help me create ADR"

AI: [creates docs/adr/ADR-XXXX-observer-pattern.md]

git commit -m "docs: add ADR for observer pattern in notifications [i:cursor-sonnet]"
```

### BATCH MIGRATION (Autonomous Mode Ideal)

```bash
# ===================================================
# MIGRATION - AUTONOMOUS MODE
# ===================================================

# Ideal: Claude Code CLI (can take time, disconnect)

# 1. Create migration spec
docs/specs/migration-django-5.2.md

# 2. Execute
claude-code --spec docs/specs/migration-django-5.2.md \
            --output apps/backend/

# 3. GO HAVE COFFEE, LUNCH, ETC
# Claude Code runs autonomously

# 4. Come back, review
pytest apps/backend/monolith/api/tests/ -v

git commit -m "refactor: migrate to Django 5.2 [a:claude-cli]"
```

---

## Decision by Task Type

| Task Type | Mode | Reason | Suggested Tool |
|-----------|------|--------|----------------|
| **Exploration** | INTERACTIVE | Q&A needed | Cursor Chat |
| **Discussion** | INTERACTIVE | Contextual decisions | Cursor/Codex |
| **Refinement** | INTERACTIVE | Iteration needed | Cursor/Codex |
| **Debugging** | INTERACTIVE | Iterative investigation | Cursor/Codex |
| **Code Review** | INTERACTIVE | Critical analysis | Cursor/Codex |
| **Implementation (clear spec)** | AUTONOMOUS | Mechanical | Claude CLI or Agent |
| **Implementation (ambiguous spec)** | INTERACTIVE | Decisions during | Cursor/Codex |
| **Test generation** | AUTONOMOUS | Mechanical | Any Agent |
| **Migration** | AUTONOMOUS | Repetitive | Claude CLI |
| **Exploratory refactoring** | INTERACTIVE | Design decisions | Cursor/Codex |
| **Mechanical refactoring** | AUTONOMOUS | Clear pattern | Agent or CLI |

---

## Mode Transitions

### From Interactive to Autonomous

**Trigger**: Spec became clear enough to delegate

```bash
# SESSION 1: INTERACTIVE
# Refine spec until executable
git commit -m "docs: refine feature spec [i:cursor-sonnet]"
git push

# SESSION 2: AUTONOMOUS
# Implement clear spec
git pull
claude-code --spec docs/specs/feature.md
git commit -m "feat: implement feature from spec [a:claude-cli]"
```

### From Autonomous to Interactive

**Trigger**: Agent found problem, unexpected result

```bash
# SESSION 1: AUTONOMOUS
# Agent implemented, but has bug
git commit -m "feat: implement feature [a:cursor-agent-sonnet]"

# SESSION 2: INTERACTIVE
# Debug and adjust
git commit -m "fix: handle edge case in feature [i:cursor-sonnet]"
```

---

## Atomic Session Checklist

### Before Each Session
- [ ] `git pull origin main`
- [ ] Decide MODE (interactive or autonomous)
- [ ] If autonomous, verify spec is clear
- [ ] If interactive, be prepared for feedback
- [ ] Choose tool within mode

### During INTERACTIVE Session
- [ ] Give continuous feedback
- [ ] Ask questions when needed
- [ ] Validate important decisions
- [ ] Test incrementally

### During AUTONOMOUS Session
- [ ] Give clear instruction
- [ ] Let it run without interrupting
- [ ] (Optional) Do other things
- [ ] Review result at the end

### After Session
- [ ] Test: `pytest apps/backend/monolith/api/tests/`
- [ ] Commit with complete tag: `[mode:tool-model]`
- [ ] `git push`
- [ ] Switch mode/tool only in new session

---

## Complete Week Example

```bash
# MONDAY
git commit -m "docs: weekly plan [i:cursor-sonnet]"

# TUESDAY - Refinement (interactive)
git commit -m "docs: add order tracking spec [i:cursor-sonnet]"
git commit -m "docs: add risk management spec [i:codex-gpt4]"

# WEDNESDAY - Implementation (autonomous - clear spec)
git commit -m "feat: implement order tracking [a:claude-cli]"

# THURSDAY - Review (interactive) + adjustments
git commit -m "refactor: optimize order queries [i:cursor-sonnet]"
git commit -m "fix: handle missing order edge case [i:cursor-sonnet]"

# FRIDAY - Tests (autonomous) + integration (interactive)
git commit -m "test: generate integration tests [a:cursor-agent-sonnet]"
git commit -m "feat: add Order.get_by_status method [i:cursor-sonnet]"
git commit -m "docs: update architecture docs [i:cursor-sonnet]"
```

---

## Anti-Patterns to Avoid

### ❌ Using Autonomous without Clear Spec
```bash
# BAD
claude-code # without spec, just vague idea
# Result: code is not what you wanted
```

**Solution**: Refine in interactive mode first, then delegate.

### ❌ Using Interactive for Mechanical Task
```bash
# BAD - wasting time in interactive mode
Cursor: "Generate 50 tests for this module"
You: [supervising each test being generated]
```

**Solution**: Use autonomous mode for mechanical tasks.

### ❌ Mixing Modes in Same Commit
```bash
# BAD
git add # files from interactive session
git add # files from autonomous session
git commit -m "feat: stuff [???]"
```

**Solution**: One mode per commit.

### ❌ Not Specifying Model in Tag
```bash
# BAD
git commit -m "feat: implement feature [i:cursor]"
# Which model? Sonnet? GPT-4o?
```

**Solution**: Complete tag `[i:cursor-sonnet]`.

---

## Productivity Metrics

```bash
# Commits by mode
git log --grep="\[i:" --oneline | wc -l  # Interactive
git log --grep="\[a:" --oneline | wc -l  # Autonomous

# Which mode is more productive?
git log --grep="\[a:" --since="1 month ago" --pretty=format:"%H %ai" | wc -l

# Bugs by mode (indicates quality)
git log --grep="fix.*\[i:" --oneline | wc -l
git log --grep="fix.*\[a:" --oneline | wc -l

# Most used tools
git log --grep="cursor" --oneline | wc -l
git log --grep="codex" --oneline | wc -l
git log --grep="claude-cli" --oneline | wc -l
```

---

## Workflow Summary

| Day | Activity | Mode | Typical Tool |
|-----|----------|------|--------------|
| MON | Planning | Interactive | Cursor/Codex |
| TUE | Refinement | Interactive | Cursor/Codex |
| WED | Implementation | Autonomous | Claude CLI or Agent |
| THU | Review/Debug | Interactive | Cursor/Codex |
| FRI | Integration | Hybrid | Both |

**Remember**: Mode first, tool second!
