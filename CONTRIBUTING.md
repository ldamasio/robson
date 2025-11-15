# Contributing to Robson Bot

Thank you for your interest in contributing to Robson Bot! This document provides guidelines and best practices for contributing to this open-source cryptocurrency trading platform.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Language Policy](#language-policy)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Coding Standards](#coding-standards)
- [Testing Requirements](#testing-requirements)
- [Commit Message Guidelines](#commit-message-guidelines)
- [Pull Request Process](#pull-request-process)
- [Architecture Guidelines](#architecture-guidelines)
- [Documentation](#documentation)
- [Getting Help](#getting-help)

---

## Code of Conduct

We are committed to providing a welcoming and inclusive environment. Please be respectful and constructive in all interactions.

### Our Standards

- **Be respectful**: Treat all contributors with respect
- **Be constructive**: Provide helpful feedback
- **Be inclusive**: Welcome contributors of all backgrounds
- **Be professional**: Keep discussions focused on technical merits

### Unacceptable Behavior

- Harassment, trolling, or inflammatory comments
- Personal attacks or insults
- Publishing others' private information
- Other conduct inappropriate for a professional setting

---

## Language Policy

**CRITICAL**: All technical content in Robson Bot MUST be in English.

### What Must Be in English

- ✅ **Source code**: Variables, functions, classes, methods
- ✅ **Comments**: Inline, block, docstrings
- ✅ **Documentation**: README, ADRs, specs, guides
- ✅ **Git**: Commit messages, branch names, PR descriptions
- ✅ **Issues & PRs**: Titles, descriptions, comments
- ✅ **Code reviews**: All feedback and discussions
- ✅ **Configuration**: YAML, JSON, environment variables
- ✅ **API contracts**: OpenAPI, AsyncAPI, GraphQL schemas
- ✅ **Tests**: Test names, assertions, fixtures
- ✅ **Logs**: Application logs, error messages

### Why English Only?

1. **International Positioning**: Robson Bot targets global open-source community
2. **AI Compatibility**: AI assistants are 40% more effective with English-only codebases
3. **Team Scaling**: Enables hiring internationally
4. **Documentation Quality**: Technical terminology is more precise in English
5. **Industry Standard**: Top open-source projects are 100% English

Read the full rationale: [docs/LANGUAGE-POLICY.md](docs/LANGUAGE-POLICY.md)

### Enforcement

- Pre-commit hooks detect non-English characters
- CI checks enforce language policy
- PRs with non-English content will be requested to update

### Support for Non-Native Speakers

We welcome contributors from all language backgrounds! Tips:

- Use Grammarly or LanguageTool for writing assistance
- Focus on clarity over perfect grammar
- Ask for help with technical English in PR comments
- We're patient and will help with language during code reviews

---

## Getting Started

### Prerequisites

- **Python**: 3.12+
- **Node.js**: 20+ (for frontend)
- **Docker**: Latest version
- **Git**: Latest version
- **IDE**: VS Code, PyCharm, or Cursor recommended

### Fork and Clone

1. Fork the repository on GitHub
2. Clone your fork:
   ```bash
   git clone https://github.com/YOUR-USERNAME/robson.git
   cd robson
   ```

3. Add upstream remote:
   ```bash
   git remote add upstream https://github.com/rbxrobotica/robson.git
   ```

### Local Development Setup

#### Backend Setup

```bash
cd apps/backend/monolith

# Create virtual environment
python -m venv .venv
source .venv/bin/activate  # On Windows: .venv\Scripts\activate

# Install dependencies
pip install -r requirements.txt

# Copy environment file
cp .env.development.example .env

# Start local PostgreSQL
cd ../..
make dev-db-up
cd apps/backend/monolith

# Run migrations
python manage.py migrate

# Run tests
python manage.py test -v 2

# Start dev server
python manage.py runserver
```

#### Frontend Setup

```bash
cd apps/frontend

# Install dependencies
npm install

# Copy environment file
cp .env.example .env

# Start dev server
npm run dev
```

### Install Pre-commit Hooks

```bash
pip install pre-commit
pre-commit install
pre-commit install --hook-type commit-msg
```

Pre-commit hooks will automatically:
- Format code (Black, Prettier)
- Sort imports (isort)
- Lint code (Ruff, ESLint)
- Type check (MyPy)
- Security check (Bandit)
- Validate commit messages
- Check language policy

---

## Development Workflow

### 1. Create an Issue

Before starting work, create or comment on an issue to discuss your proposed changes.

### 2. Create a Branch

```bash
git checkout -b feature/my-feature
# or
git checkout -b fix/bug-description
```

**Branch naming convention**:
- `feature/description` - New features
- `fix/description` - Bug fixes
- `docs/description` - Documentation only
- `refactor/description` - Code refactoring
- `test/description` - Test additions/fixes

### 3. Make Changes

- Write code following our [coding standards](#coding-standards)
- Add tests for new functionality
- Update documentation as needed
- Run tests and linters locally

### 4. Commit Changes

Follow [Conventional Commits](#commit-message-guidelines):

```bash
git add .
git commit -m "feat(trading): add stop-loss order support

Implement automatic stop-loss order execution.
Includes risk validation and exchange integration.

Closes #123"
```

### 5. Push and Create PR

```bash
git push origin feature/my-feature
```

Create a Pull Request on GitHub with:
- Clear title following Conventional Commits
- Description of changes
- Link to related issue(s)
- Screenshots/GIFs for UI changes
- Test results

### 6. Code Review

- Address reviewer feedback
- Keep commits clean (squash if needed)
- Ensure CI passes

### 7. Merge

Once approved and CI passes, maintainers will merge your PR.

---

## Coding Standards

### Python

**Style**:
- Follow PEP 8
- Line length: 100 characters (enforced by Black)
- Use type hints for all functions
- Docstrings for public functions/classes (Google style)

**Naming**:
- `snake_case` for variables and functions
- `PascalCase` for classes
- `UPPER_SNAKE_CASE` for constants
- Prefix private methods with `_`

**Example**:
```python
from typing import Protocol
from decimal import Decimal

class OrderRepository(Protocol):
    """Repository interface for order persistence."""

    def save(self, order: Order) -> Order:
        """
        Save order to persistent storage.

        Args:
            order: Order entity to save

        Returns:
            Saved order with generated ID

        Raises:
            ValidationError: If order is invalid
        """
        ...
```

### JavaScript/React

**Style**:
- Use ES6+ features
- Functional components with hooks
- PropTypes for type checking
- Line length: 100 characters

**Naming**:
- `camelCase` for variables and functions
- `PascalCase` for components
- `UPPER_SNAKE_CASE` for constants

**Example**:
```javascript
import React, { useState } from 'react';
import PropTypes from 'prop-types';

/**
 * Order form component for placing trades.
 *
 * @param {Function} onSubmit - Callback when form is submitted
 * @returns {JSX.Element}
 */
const OrderForm = ({ onSubmit }) => {
  const [quantity, setQuantity] = useState('');

  const handleSubmit = (e) => {
    e.preventDefault();
    onSubmit({ quantity: parseFloat(quantity) });
  };

  return <form onSubmit={handleSubmit}>{/* ... */}</form>;
};

OrderForm.propTypes = {
  onSubmit: PropTypes.func.isRequired,
};

export default OrderForm;
```

### Architecture Principles

**Hexagonal Architecture** (Ports & Adapters):

1. **Domain Layer** (`apps/backend/core/domain/`):
   - Pure entities, no framework dependencies
   - Business logic only
   - Immutable value objects

2. **Application Layer** (`apps/backend/core/application/`):
   - Use cases orchestrate domain logic
   - Port definitions (interfaces)
   - No framework dependencies

3. **Adapters** (`apps/backend/core/adapters/`):
   - Concrete implementations of ports
   - Framework-specific code allowed
   - Django ORM, Binance API client, etc.

**Rules**:
- ❌ NO Django imports in `core/domain/` or `core/application/`
- ✅ Use dependency injection
- ✅ Depend on abstractions (ports), not concretions

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for details.

---

## Testing Requirements

### Coverage Target

- **Minimum**: 80% overall
- **Domain layer**: 95%+ (critical business logic)
- **Application layer**: 90%+ (use cases)
- **Adapters**: 70%+ (integration tests)

### Test Types

**Unit Tests** (fast, isolated):
```python
def test_order_total_value():
    """Test order total value calculation."""
    order = Order(
        symbol=Symbol("BTCUSDT"),
        quantity=Decimal("0.5"),
        price=Decimal("50000"),
    )
    assert order.total_value == Decimal("25000")
```

**Integration Tests** (with database):
```python
@pytest.mark.django_db
def test_save_order(client, user):
    """Test saving order via API."""
    client.force_authenticate(user=user)
    response = client.post('/api/orders/', {
        'symbol': 'BTCUSDT',
        'quantity': '0.5',
        'price': '50000',
    })
    assert response.status_code == 201
```

### Running Tests

```bash
# Backend (from apps/backend/monolith)
python manage.py test -v 2

# With coverage
pytest --cov --cov-report=html

# Frontend (from apps/frontend)
npm test

# Watch mode
npm test -- --watch
```

---

## Commit Message Guidelines

We use **Conventional Commits** for clear, semantic version control.

### Format

```
<type>(<scope>): <subject>

<body>

<footer>
```

### Types

- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation only
- `style`: Code style (formatting, no logic change)
- `refactor`: Code restructuring (no behavior change)
- `perf`: Performance improvement
- `test`: Add/update tests
- `chore`: Build process, dependencies, tooling

### Scope

Component affected: `trading`, `risk`, `api`, `frontend`, `infra`, etc.

### Examples

```
feat(trading): add stop-loss order support

Implement automatic stop-loss execution when price
drops below threshold. Includes risk validation.

Closes #123
```

```
fix(api): prevent race condition in order placement

Use database transaction to ensure order atomicity.
Adds retry logic for transient API failures.

Fixes #456
```

```
docs(readme): update installation instructions

Add Docker setup steps and troubleshooting section.
```

### Enforcement

Commit messages are validated by pre-commit hook. Invalid messages will be rejected.

---

## Pull Request Process

### Before Creating PR

- [ ] All tests pass locally
- [ ] Code is formatted (Black, Prettier)
- [ ] No linting errors (Ruff, ESLint)
- [ ] Type checking passes (MyPy)
- [ ] Security scan passes (Bandit)
- [ ] Coverage meets 80% threshold
- [ ] Documentation updated
- [ ] CHANGELOG updated (if applicable)

### PR Title

Use Conventional Commits format:

```
feat(trading): add stop-loss orders
```

### PR Description Template

```markdown
## Summary
Brief description of changes.

## Related Issue
Closes #123

## Type of Change
- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Documentation update

## Changes Made
- Added stop-loss order type
- Updated risk validation
- Added integration tests

## Testing
- [ ] Unit tests added/updated
- [ ] Integration tests added/updated
- [ ] Manual testing performed

## Screenshots
(if applicable)

## Checklist
- [ ] Code follows project style
- [ ] All code is in English
- [ ] Tests pass locally
- [ ] Documentation updated
- [ ] No breaking changes (or documented)
```

### Review Process

1. **Automated Checks**: CI runs tests, linting, security scans
2. **Code Review**: Maintainer reviews code quality and architecture
3. **Feedback**: Address reviewer comments
4. **Approval**: At least one maintainer approval required
5. **Merge**: Squash and merge (default) or rebase

### Preview Environment

Every PR automatically gets a preview environment:
- **URL**: `https://h-<branch-name>.preview.robsonbot.com`
- **Created**: On PR creation
- **Updated**: On each push
- **Deleted**: When PR is closed/merged

Use this to test your changes in a production-like environment.

---

## Architecture Guidelines

### Adding a New Use Case

1. **Define domain entity** (if new):
   ```python
   # apps/backend/core/domain/my_entity.py
   @dataclass(frozen=True)
   class MyEntity:
       id: str
       # ... fields
   ```

2. **Define port**:
   ```python
   # apps/backend/core/application/ports.py
   class MyEntityRepository(Protocol):
       def save(self, entity: MyEntity) -> MyEntity: ...
   ```

3. **Implement use case**:
   ```python
   # apps/backend/core/application/my_use_case.py
   class MyUseCase:
       def __init__(self, repo: MyEntityRepository):
           self._repo = repo

       def execute(self, command: MyCommand) -> MyEntity:
           # Business logic
           pass
   ```

4. **Implement adapter**:
   ```python
   # apps/backend/core/adapters/driven/persistence/django_my_repository.py
   class DjangoMyRepository:
       def save(self, entity: MyEntity) -> MyEntity:
           # Django ORM operations
           pass
   ```

5. **Write tests** (test use case with test doubles)

See [docs/AGENTS.md](docs/AGENTS.md) for more patterns.

### Adding a REST Endpoint

1. Create Django view in `apps/backend/monolith/api/views/`
2. Add URL route
3. Update OpenAPI spec (`docs/specs/api/openapi.yaml`)
4. Write integration tests
5. Update documentation

### Frontend Component

1. Create component in `apps/frontend/src/components/`
2. Follow ports & adapters pattern
3. Write unit tests
4. Add PropTypes
5. Document props in JSDoc

---

## Documentation

### When to Update Docs

- **Always**: When adding features, changing APIs, or modifying behavior
- **ADRs**: For significant architectural decisions
- **Specs**: For new features (TDD/BDD approach)
- **README**: For setup or usage changes
- **OpenAPI**: For API endpoint changes

### Documentation Standards

- **Markdown**: GitHub Flavored Markdown, linted with markdownlint
- **Diagrams**: Mermaid format (version controlled)
- **Code Examples**: Include language identifier, tested examples
- **Links**: Use relative paths for internal references

### Creating an ADR

Use the template in `docs/adr/ADR-TEMPLATE.md`:

```bash
cp docs/adr/ADR-TEMPLATE.md docs/adr/ADR-XXXX-my-decision.md
```

Fill out:
- Context (problem)
- Decision drivers
- Considered options
- Decision outcome
- Consequences

---

## Getting Help

### Resources

- **Documentation**: [docs/INDEX.md](docs/INDEX.md)
- **Developer Guide**: [docs/DEVELOPER.md](docs/DEVELOPER.md)
- **Architecture**: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- **AI Guide**: [docs/AGENTS.md](docs/AGENTS.md)

### Communication

- **GitHub Issues**: Bug reports, feature requests
- **GitHub Discussions**: Questions, ideas, general discussion
- **Pull Requests**: Code review, implementation discussion

### Common Questions

**Q: I found a bug, what should I do?**
A: Create an issue with steps to reproduce, expected vs. actual behavior, and your environment.

**Q: I want to add a feature, where do I start?**
A: Create an issue to discuss the feature first. Once approved, follow the development workflow.

**Q: My PR is failing CI, what do I do?**
A: Check the CI logs, fix the issues locally, and push again. Run `pre-commit run --all-files` locally before pushing.

**Q: I'm not sure about code architecture, can I get help?**
A: Yes! Ask in the PR comments or create a discussion. We're here to help.

**Q: Can I contribute without coding?**
A: Absolutely! Documentation, testing, bug reports, and design are all valuable contributions.

---

## Recognition

Contributors are recognized in:
- **CONTRIBUTORS.md**: All contributors listed
- **Release Notes**: Significant contributions highlighted
- **GitHub**: Contributor badge on profile

Thank you for contributing to Robson Bot! 🚀

---

**Last Updated**: 2025-11-14
**Version**: 1.0
