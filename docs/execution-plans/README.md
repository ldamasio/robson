# Execution Plans

This directory contains **transparent, auditable execution plans** for major initiatives and transformations in Robson Bot.

## Purpose

Execution plans provide:
- **Clarity**: Clear objectives and success criteria
- **Transparency**: Visible progress tracking
- **Accountability**: Assigned ownership and timelines
- **Risk Management**: Identified risks and mitigations
- **Communication**: Stakeholder alignment

## Structure

```
execution-plans/
├── README.md
├── template.md             # Standard plan template
└── 2025-Q4/
    ├── ai-first-transformation.md
    └── tdd-implementation.md
```

Plans are organized by quarter/year for historical tracking.

## Plan Format

Each execution plan follows this structure:

```markdown
# [Title]

**Status**: [Draft | In Progress | Completed | Blocked]
**Owner**: [Team/Person]
**Created**: YYYY-MM-DD
**Target**: YYYY-MM-DD

## Objective
[Clear, measurable objective]

## Success Criteria
- [ ] Criterion 1
- [ ] Criterion 2

## Context
[Background and motivation]

## Detailed Tasks
### Phase 1: [Name]
- [ ] Task 1.1
- [ ] Task 1.2

## Dependencies
- Dependency 1

## Risks & Mitigations
| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| ... | ... | ... | ... |

## Timeline
[Milestones]

## Progress Tracking
[Link to project board]
```

## Status Definitions

- **Draft**: Plan under development, not yet approved
- **In Progress**: Actively executing tasks
- **Completed**: All success criteria met
- **Blocked**: Waiting on dependency or decision

## Best Practices

1. **Start Small**: Break large initiatives into phases
2. **Measurable**: Define clear success criteria
3. **Realistic**: Set achievable timelines
4. **Transparent**: Update progress regularly
5. **Learn**: Document lessons learned

## Integration with ADRs

Execution plans often trigger Architecture Decision Records:
- Plans describe WHAT and WHEN
- ADRs describe WHY and HOW (technical decisions)

Example:
- Plan: "AI-First Transformation Q4 2025"
- ADR: "ADR-0006: English-Only Codebase"

## Tools

- GitHub Projects for task tracking
- Markdown for plan documentation
- Mermaid for Gantt charts
- Regular review meetings

## Examples

See `2025-Q4/ai-first-transformation.md` for a comprehensive example.
