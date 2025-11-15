# Development Guides

**Comprehensive guides for developers contributing to Robson Bot.**

## Purpose

This directory contains detailed guides for:
- **Environment Setup**: Local development configuration
- **Testing Strategy**: How to write and run tests
- **Code Style**: Conventions and best practices
- **Contribution Workflow**: Git workflow and PR process

## Structure

```
development/
├── README.md
├── setup.md                # Local environment setup
├── testing.md              # Testing strategy and guides
├── code-style.md           # Style guide and conventions
└── contributing-workflow.md # Git workflow and PR process
```

## Quick Start

New to Robson Bot? Start here:

1. **[Environment Setup](setup.md)** - Get your local dev environment running
2. **[Code Style](code-style.md)** - Learn our coding conventions
3. **[Testing Guide](testing.md)** - Understand our testing approach
4. **[Contribution Workflow](contributing-workflow.md)** - Make your first PR

## Key Resources

### For Backend Developers

- **Setup**: [setup.md#backend](setup.md#backend-setup)
- **Testing**: [testing.md#backend](testing.md#backend-testing)
- **Style**: [code-style.md#python](code-style.md#python-conventions)
- **Architecture**: [../ARCHITECTURE.md](../ARCHITECTURE.md)

### For Frontend Developers

- **Setup**: [setup.md#frontend](setup.md#frontend-setup)
- **Testing**: [testing.md#frontend](testing.md#frontend-testing)
- **Style**: [code-style.md#javascript](code-style.md#javascript-conventions)
- **Components**: [../../apps/frontend/README.md](../../apps/frontend/README.md)

### For Infrastructure Engineers

- **Setup**: [setup.md#infrastructure](setup.md#infrastructure-setup)
- **K8s Guide**: [../../infra/README.md](../../infra/README.md)
- **ADRs**: [../adr/](../adr/)

## Development Principles

### 1. English Only

**All code, comments, documentation, and commit messages must be in English.**

See [LANGUAGE-POLICY.md](../LANGUAGE-POLICY.md) for rationale.

### 2. Hexagonal Architecture

New backend code follows **Ports & Adapters** pattern.

See [../ARCHITECTURE.md](../ARCHITECTURE.md) for details.

### 3. Test-Driven Development

Write tests before implementation when possible.

See [testing.md](testing.md) for methodology.

### 4. Spec-Driven Development

Features start with specifications.

See [../specs/README.md](../specs/README.md) for approach.

### 5. ADR for Decisions

Document significant decisions in Architecture Decision Records.

See [../adr/ADR-TEMPLATE.md](../adr/ADR-TEMPLATE.md) for template.

## Common Tasks

### Run Tests Locally

```bash
# Backend tests
cd apps/backend/monolith
python manage.py test -v 2

# Frontend tests
cd apps/frontend
npm test

# With coverage
pytest --cov=robson --cov-report=html
```

### Code Quality Checks

```bash
# Format code
black .
isort .

# Lint
flake8
mypy robson/

# Security scan
bandit -r robson/
```

### Build Docker Images

```bash
# Backend
docker build -f apps/backend/monolith/Dockerfile -t robson-backend .

# Frontend
docker build -f apps/frontend/Dockerfile -t robson-frontend .
```

### Deploy to Preview Environment

```bash
# Push to branch
git push origin feature/my-feature

# ArgoCD auto-creates preview at:
# https://h-feature-my-feature.preview.robsonbot.com
```

## Development Workflow

1. **Create Issue**: Describe the change
2. **Create Branch**: `git checkout -b feature/description`
3. **Write Spec**: Create or update feature spec
4. **Write Tests**: TDD approach
5. **Implement**: Write code
6. **Validate**: Run tests + linting
7. **Commit**: Use Conventional Commits
8. **Push**: Triggers CI + preview deployment
9. **Create PR**: Use PR template
10. **Review**: Address feedback
11. **Merge**: Squash and merge

See [contributing-workflow.md](contributing-workflow.md) for details.

## Tools & IDE Setup

### Recommended Tools

- **IDE**: VS Code, PyCharm, Cursor
- **Python**: 3.12 with pyenv
- **Node**: 20 with nvm
- **Docker**: Docker Desktop
- **K8s**: kubectl, k9s, helm
- **Git**: gh CLI

### VS Code Extensions

- Python (Microsoft)
- Pylance
- ESLint
- Prettier
- Docker
- Kubernetes
- GitLens
- Mermaid Preview

### Pre-commit Hooks

Install once:

```bash
pip install pre-commit
pre-commit install
```

Runs automatically on `git commit`:
- Black (formatter)
- isort (import sorting)
- Flake8 (linting)
- Mypy (type checking)
- Bandit (security)

## Troubleshooting

### Common Issues

| Issue | Solution |
|-------|----------|
| Import errors | Check virtual environment activated |
| Database errors | Run migrations: `python manage.py migrate` |
| Port already in use | Stop conflicting service or change port |
| Docker build fails | Clear cache: `docker builder prune` |

### Getting Help

- **Documentation**: Start with this directory
- **Issues**: Search existing GitHub issues
- **Discussions**: GitHub Discussions for questions
- **Slack**: #robson-dev (internal)

## Contributing

We welcome contributions! Please:

1. Read [CONTRIBUTING.md](../../CONTRIBUTING.md)
2. Follow our [Code of Conduct](../../CODE_OF_CONDUCT.md)
3. Review [contributing-workflow.md](contributing-workflow.md)
4. Check [open issues](https://github.com/rbxrobotica/robson/issues)

## Learning Resources

### Hexagonal Architecture

- [Netflix - Ready for changes with Hexagonal Architecture](https://netflixtechblog.com/ready-for-changes-with-hexagonal-architecture-b315ec967749)
- [Alistair Cockburn - Original article](https://alistair.cockburn.us/hexagonal-architecture/)

### Django Best Practices

- [Two Scoops of Django](https://www.feldroy.com/books/two-scoops-of-django-3-x)
- [Django REST Framework Guide](https://www.django-rest-framework.org/)

### React Best Practices

- [React Docs](https://react.dev/)
- [Testing Library](https://testing-library.com/docs/react-testing-library/intro/)

### Kubernetes

- [Kubernetes Patterns](https://k8spatterns.io/)
- [Gateway API](https://gateway-api.sigs.k8s.io/)

---

**Happy coding! 🚀**
