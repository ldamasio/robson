# Requirements Documentation

**Purpose**: This directory contains requirements for Robson Bot, organized into **current implementation** and **future/planned** requirements.

## Organization

Requirements are separated into three categories:

1. **Core Requirements** (`robson-core-requirements.md`)
   - Product-level capabilities
   - Target users and personas
   - Business constraints and success criteria

2. **Domain Requirements** (`robson-domain-requirements.md`)
   - Entity behaviors, validations, invariants
   - Business rules and calculations
   - State transitions and workflows

3. **API Requirements** (`robson-api-requirements.md`)
   - API endpoint behaviors
   - Request/response contracts
   - Authentication and authorization rules

## Requirement ID Conventions

### Current Implementation Requirements

Requirements that reflect the **actual behavior** of the system today:

- `REQ-CUR-CORE-###` - Core product requirements (implemented)
- `REQ-CUR-DOMAIN-###` - Domain model requirements (implemented)
- `REQ-CUR-API-###` - API requirements (implemented)

**Source of truth**: Existing code, models, tests, API behavior

### Future / Planned Requirements

Requirements for **desired** functionality not yet implemented:

- `REQ-FUT-CORE-###` - Planned core product features
- `REQ-FUT-DOMAIN-###` - Planned domain model enhancements
- `REQ-FUT-API-###` - Planned API features

**Source of truth**: Product vision, architectural direction, roadmap

## Traceability

Every requirement must be traceable to:

1. **Specifications** (`docs/specs/`)
   - `SPEC-*-###` references `REQ-*-###`

2. **Code** (for current requirements)
   - Model classes: `apps/backend/monolith/api/models/`
   - Domain classes: `apps/backend/core/domain/`
   - API views: `apps/backend/monolith/api/views/`

3. **Tests** (for current requirements)
   - Unit tests: `apps/backend/monolith/api/tests/`
   - Integration tests

4. **OpenAPI** (for API requirements)
   - `docs/specs/api/openapi.yaml`

5. **ADRs** (for architectural requirements)
   - `docs/adr/ADR-*.md`

See [TRACEABILITY.md](../development/TRACEABILITY.md) for detailed mapping.

## Requirement Structure

Each requirements document follows this structure:

```markdown
# 1. Current Implementation Requirements
(REQ-CUR-* IDs - what exists today)

## 1.1 Category
REQ-CUR-XXX-001: Description
- Rationale: Why this exists
- Source: Model/code reference
- Constraints: Limitations
- Acceptance criteria: How to verify

# 2. Future / Planned Requirements
(REQ-FUT-* IDs - what we want)

## 2.1 Category
REQ-FUT-XXX-001: Description
- Rationale: Why we need this
- Dependencies: What must exist first
- Priority: High/Medium/Low
- Estimated complexity: Simple/Moderate/Complex

# 3. Known Gaps or Unclear Behavior
(Issues, inconsistencies, missing validations)

# 4. Traceability
(Mapping to code, specs, tests)
```

## How to Use Requirements

### For Developers

1. **Understanding existing behavior**:
   - Read `REQ-CUR-*` requirements
   - Trace to code via model/view references
   - Check tests for verification

2. **Implementing new features**:
   - Check if `REQ-FUT-*` exists
   - Create spec referencing requirement
   - Implement following spec
   - Write tests referencing requirement ID
   - Update requirement status to current

3. **Modifying existing behavior**:
   - Check `REQ-CUR-*` for current contract
   - Assess impact on dependents
   - Create ADR if architectural change
   - Update requirement if behavior changes

### For Product Managers

1. **Planning features**:
   - Add `REQ-FUT-*` requirements
   - Prioritize based on business value
   - Link to execution plans

2. **Validating implementation**:
   - Verify `REQ-CUR-*` matches behavior
   - Check acceptance criteria
   - File gaps as issues

### For AI Agents

1. **Code generation**:
   - Read `REQ-CUR-*` to understand existing behavior
   - Check `REQ-FUT-*` for planned features
   - Reference requirement IDs in code comments
   - Generate tests with requirement traceability

2. **Requirements analysis**:
   - Extract requirements from code
   - Identify gaps between code and requirements
   - Suggest missing requirements
   - Propose future requirements based on patterns

## Requirement Lifecycle

```
[Identified] → [Documented as REQ-FUT] → [Specified] →
[Implemented] → [Tested] → [Promoted to REQ-CUR] → [Maintained]
```

### States

- **Identified**: Need recognized but not yet documented
- **Documented**: `REQ-FUT-*` created with rationale
- **Specified**: `SPEC-*` created referencing requirement
- **Implemented**: Code written satisfying requirement
- **Tested**: Tests verify requirement
- **Promoted**: `REQ-FUT-*` → `REQ-CUR-*` (requirement becomes current)
- **Maintained**: `REQ-CUR-*` kept in sync with code evolution

## Validation Rules

### For Current Requirements (REQ-CUR-*)

- ✅ Must reference existing code (model, view, function)
- ✅ Must have at least one test verifying behavior
- ✅ Must be consistent with OpenAPI (for API requirements)
- ✅ Must reflect actual behavior, not idealized behavior

### For Future Requirements (REQ-FUT-*)

- ✅ Must have clear rationale
- ✅ Must specify priority
- ✅ Must identify dependencies
- ✅ Should have estimated complexity
- ❌ Must NOT be referenced in OpenAPI as implemented

## Examples

### Current Requirement Example

```markdown
**REQ-CUR-DOMAIN-003**: Order P&L Calculation

**Description**: Orders must calculate profit and loss based on fill price and current price.

**Rationale**: Traders need real-time P&L to make informed decisions.

**Source**: `apps/backend/monolith/api/models/trading.py:Order.calculate_pnl()`

**Constraints**:
- Uses avg_fill_price if available, otherwise order price
- BUY side: P&L = (current_price - fill_price) * quantity
- SELL side: P&L = (fill_price - current_price) * quantity

**Acceptance Criteria**:
- ✓ BUY order with current price > fill price shows positive P&L
- ✓ SELL order with current price < fill price shows positive P&L
- ✓ P&L is Decimal type with 8 decimal places

**Tests**: `apps/backend/monolith/api/tests/test_models.py::test_order_pnl`
```

### Future Requirement Example

```markdown
**REQ-FUT-API-012**: Real-time Position Updates via WebSocket

**Description**: System should push position updates to connected clients when P&L changes.

**Rationale**: Manual polling is inefficient; real-time updates improve UX.

**Dependencies**:
- REQ-CUR-API-005 (Position list endpoint)
- WebSocket infrastructure (partially implemented)

**Priority**: High

**Estimated Complexity**: Moderate

**Acceptance Criteria** (when implemented):
- [ ] Client receives update when position quantity changes
- [ ] Client receives update when unrealized P&L changes > 0.5%
- [ ] Updates include position ID, symbol, quantity, unrealized_pnl
- [ ] Rate limiting: max 1 update per second per position
```

## References

- [Core Requirements](robson-core-requirements.md)
- [Domain Requirements](robson-domain-requirements.md)
- [API Requirements](robson-api-requirements.md)
- [Specifications](../specs/README.md)
- [Traceability Matrix](../development/TRACEABILITY.md)
- [AI-First Strategy](../development/AI-FIRST-STRATEGY.md)

---

**Maintained by**: Robson Bot Core Team
**Last Updated**: 2025-11-14
**Version**: 1.0
