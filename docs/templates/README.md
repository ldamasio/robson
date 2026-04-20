# Analysis Document Templates

Operational standards for analysis, diagnosis, and execution planning.

## Available Templates

### `analysis-document-template.md`

Standard structure for technical analysis documents with autonomous agent executability.

**Use when**:
- Investigating bugs, performance issues, or unexpected behavior
- Planning feature implementations or migrations
- Documenting gaps and remediation plans
- Creating executable runbooks for multi-step operations

**Structure**:
1. **Executive Summary** — Problem, findings, recommended action, effort
2. **Current State** — System overview, observed vs expected behavior, root cause
3. **Gaps** — Documentation, code, infrastructure issues with priorities
4. **Priority Tracks** — Organized work streams with dependencies
5. **Execution Selector** — Quick reference: objective → entry point
6. **Entry Points** — Executable tasks with preconditions, steps, verification
7. **Verification Commands** — Reusable check patterns
8. **Rollback Notes** — Recovery procedures for each change type

**Key Principles**:
- **Executable first** — Entry points must run without interpretation
- **Verifiable outcomes** — Every step has concrete pass/fail criteria
- **Autonomous-agent ready** — No ambiguous language, no "figure it out"
- **Exit code semantics** — Preconditions return 0 (met) or 1 (abort)
- **Rollback always defined** — Every change has documented recovery

**Example Usage**:
```bash
# Copy template
cp docs/templates/analysis-document-template.md \
   docs/analysis/$(date +%Y-%m-%d)-my-analysis.md

# Fill sections
# Commit to repo when complete
git add docs/analysis/$(date +%Y-%m-%d)-my-analysis.md
git commit -m "docs(analysis): add [topic] analysis"
```

**Cross-Project Compatibility**:

Template designed for use across:
- **robson** (this project)
- **éden** (portfolio/risk aggregation)
- **strategos** (trading strategy research)
- **rbx-infra** (infrastructure)

Adapt section names as needed (e.g., "Gaps" → "Issues", "Entry Points" → "Runbooks") but maintain the core structure: diagnosis → plan → executable steps → verification.

---

## Contributing

When creating new templates:
1. Follow existing naming conventions (`lowercase-with-dashes.md`)
2. Include "Use when" and "Key Principles" sections in README
3. Provide example usage
4. Ensure autonomous agent executability
5. Add changelog section to template itself
