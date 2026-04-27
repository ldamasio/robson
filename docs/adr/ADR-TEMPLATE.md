ADR-XXXX: <Title>

Status: Proposed | Accepted | Deprecated | Superseded
Date: YYYY-MM-DD

Context
- What problem or forces led to this decision?

Needs Input  
- <Input 1> — description of what is missing  
- <Input 2> — description of what is missing  
*(If the inputs above are not provided, the recommendation should not be made.)*

Decision
- What do we decide and how does it work?

Consequences
- Positive
- Negative/Trade‑offs

Alternatives
- Option A — why not chosen
- Option B — why not chosen

Implementation Notes
- Code paths, modules, patterns
- Tests and how they validate the decision
- Related docs/PRs/issues

---

## Directory Guide

### docs/adr/ADR-NNNN-slug.md
Architectural Decision Records. One file per decision. Permanent, append-only (status changes to Superseded, never deleted). Numbered sequentially without reusing gaps.

**When to create**: a decision that constrains future implementation choices, has non-obvious tradeoffs, or was actively debated before adoption.

### docs/architecture/v3-*.md
Architecture descriptions, migration plans, runtime specs, control loop docs. Living documents updated as the system evolves. They REFERENCE ADRs by number but do not replace them.

**When to update**: when the target architecture, migration scope, or runtime behaviour changes.

### docs/implementation/YYYY-MM-DD-milestone-name.md
Per-milestone implementation guides. Step-by-step instructions tied to a specific MIG-vN#N or QE-PN identifier. Created at session start, closed at session end. Not updated retroactively.

**When to create**: at the start of every implementation session for a named milestone.

### docs/plan/
Roadmaps, execution plans, prompts. Forward-looking. Archived when done.

**When to create**: when planning a new phase or producing orchestration prompts.

### docs/guides/
How-to guides that are tool/process-specific and not tied to one milestone. Examples: migration event sourcing, diff review checklists.

**When to create**: when a reusable process or tool guide is needed.

