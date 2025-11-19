# Specifications

This directory contains **executable specifications** for Robson Bot features and APIs, following spec-driven development practices.

## Purpose

Specifications serve as:
- **Living Documentation**: Always in sync with implementation
- **Test Blueprints**: Drive TDD/BDD test development
- **Communication Tool**: Bridge between business requirements and code
- **Contract Definitions**: Define API contracts and behaviors

## Structure

```
specs/
├── features/           # Feature specifications (BDD-style)
│   ├── risk-management.spec.md
│   ├── trading-strategies.spec.md
│   ├── signal-distribution.spec.md
│   └── multi-tenant-isolation.spec.md
└── api/                # API specifications
    ├── openapi.yaml    # REST API (OpenAPI 3.1)
    ├── asyncapi.yaml   # WebSocket/Events (AsyncAPI)
    └── README.md
```

## Spec-Driven Development Workflow

**Mode-First Governance**: See `.ai-agents/MODES.md` for complete guidance.

### Creating Specs (INTERACTIVE MODE)
1. **Write Spec First**: Define feature behavior in markdown
   - Mode: INTERACTIVE (Cursor Chat or Codex)
   - Input: Requirement from `docs/requirements/`
   - Output: Technical spec in `docs/specs/`
   - Tag: `docs: add X spec [i:cursor-sonnet]`

### Implementing Specs (AUTONOMOUS MODE)
2. **Generate Tests**: Create test scaffolding from specs
3. **Implement**: Write code to satisfy specs
   - Mode: AUTONOMOUS (Cursor Agent or Claude CLI)
   - Input: Complete spec from `docs/specs/`
   - Output: Code + tests
   - Tag: `feat: implement X [a:claude-cli]`

### Validation
4. **Validate**: Automated tests ensure compliance
5. **Update**: Keep specs in sync with implementation

## Feature Specification Format

Feature specs follow **Given-When-Then** format:

```markdown
## Feature: Risk Management Position Limits

**As a** trader
**I want** position size limits based on risk profile
**So that** I can prevent excessive losses

### Scenario: Position size exceeds maximum allowed

**Given** a trader with risk profile "Conservative"
**And** maximum position size is $1000
**When** trader attempts to place order for $1500
**Then** order is rejected with error "Position size exceeds limit"
**And** current position remains unchanged
```

## API Specification Standards

- **REST API**: OpenAPI 3.1 specification
- **WebSocket**: AsyncAPI 2.x specification
- **Validation**: Automated with openapi-spec-validator
- **Generation**: Use drf-spectacular for Django REST Framework

## Linking Specs to Tests

Tests reference specs using markers/tags:

```python
@pytest.mark.spec("risk-management.spec.md#scenario-position-size-exceeds-maximum-allowed")
def test_position_size_limit_exceeded():
    # Test implementation
    pass
```

## Spec Coverage

Track which specs are covered by tests using custom reporting:

```bash
pytest --spec-coverage
```

## Contributing

When adding new features:

1. Create spec file in `features/` or update existing
2. Link spec to tracking issue/ADR
3. Generate tests from spec scenarios
4. Implement feature
5. Ensure all spec scenarios pass

## Tools

- **pytest-bdd**: BDD framework for Python
- **behave**: Alternative BDD tool
- **openapi-spec-validator**: OpenAPI validation
- **drf-spectacular**: DRF to OpenAPI generator

## References

- [Specification by Example](https://gojko.net/books/specification-by-example/) - Gojko Adzic
- [OpenAPI Specification](https://spec.openapis.org/oas/latest.html)
- [AsyncAPI Specification](https://www.asyncapi.com/docs/reference/specification/latest)
