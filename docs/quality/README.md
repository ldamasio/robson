# Code Quality Guide - Robson Bot

This guide explains the code-quality tooling stack for the Robson project. All tools are **additive, reversible, and optional** at different levels.

---

## Quick Start

```bash
# 1. Install pre-commit (one-time setup)
pip install pre-commit
make pre-commit-install

# 2. Run all quality checks manually
make pre-commit-run

# 3. Update hook versions periodically
make pre-commit-update
```

---

## Tooling Overview

| Tool | Scope | When It Runs | Purpose | Required |
|------|-------|--------------|---------|----------|
| **Pre-commit** | All files | Before every `git commit` | Fast automated checks | ✅ Yes (recommended) |
| **Ruff** | Python | Pre-commit + manual | Fast linter & formatter | ✅ Yes |
| **Black** | Python | Pre-commit + manual | Code formatter | ✅ Yes |
| **isort** | Python | Pre-commit + manual | Import sorter | ✅ Yes |
| **MyPy** | Python | Pre-commit + manual | Type checker | ⚠️ Core domain only |
| **Bandit** | Python | Pre-commit + manual | Security linter | ✅ Yes |
| **gofmt** | Go | Pre-commit + manual | Code formatter | ✅ Yes |
| **Prettier** | JS/JSX | Pre-commit + manual | Code formatter | ✅ Yes |
| **ESLint** | JS/JSX | Pre-commit + manual | Linter | ✅ Yes |
| **SonarLint** | All languages | In IDE only | Real-time feedback | ⚠️ Optional |
| **SonarQube** | All languages | CI only | Deep analysis & metrics | ❌ Optional |

---

## Pre-commit Hooks (Recommended)

**Pre-commit** is the first line of defense. It runs fast, deterministic checks before every commit.

### Installation

```bash
# One-time setup
pip install pre-commit
make pre-commit-install
```

### How It Works

1. You run `git commit`
2. Pre-commit hooks automatically run on staged files
3. Failed hooks block the commit (fix issues and try again)
4. Skipped files pass through unchanged

### Running Manually

```bash
# Run on all files (useful for first-time setup)
make pre-commit-run

# Run on specific files
pre-commit run --files apps/backend/monolith/api/views.py

# Skip hooks (emergency only)
git commit --no-verify

# Skip specific hook
SKIP=mypy git commit
```

### Updating Hooks

```bash
# Update to latest versions
make pre-commit-update

# Review changes in .pre-commit-config.yaml
git diff .pre-commit-config.yaml
```

### Hook Categories

#### Always-On Hooks (Fast, Deterministic)
- Trailing whitespace trimmer
- End-of-file fixer
- YAML/JSON/TOML syntax validation
- Large file detector (max 500 KB)
- Merge conflict detector
- Private key detector

#### Python Hooks
- **ruff**: Lint and fix (`apps/backend/`)
- **ruff-format**: Format code (`apps/backend/`)
- **black**: Format code (line length 100)
- **isort**: Sort imports
- **mypy**: Type check (`apps/backend/core/` only)
- **bandit**: Security check

#### Go Hooks
- **gofmt**: Format code (`cli/`)

#### JavaScript/React Hooks
- **prettier**: Format code (`apps/frontend/`)
- **eslint**: Lint code

#### Documentation Hooks
- **markdownlint**: Lint Markdown files
- **yamllint**: Lint YAML files

#### Custom Hooks
- **English-only policy**: Detect non-ASCII characters in code
- **No Django in core**: Enforce hexagonal architecture

### Configuration File

See [`.pre-commit-config.yaml`](../../.pre-commit-config.yaml) for all hook configurations.

---

## Makefile Targets

The [`Makefile`](../../Makefile) provides convenient targets for running quality checks:

```bash
# Pre-commit management
make pre-commit-install   # Install hooks
make pre-commit-run       # Run on all files
make pre-commit-update    # Update hook versions

# Language-specific formatting
make format-python        # Format Python code (black + isort)
make format-go            # Format Go code (gofmt)
make format-js            # Format JS/JSX (prettier)

# Language-specific linting
make lint-python          # Lint Python (ruff)
make lint-go              # Lint Go (gofmt -l)
make lint-js              # Lint JS/JSX (eslint)

# Run all quality checks
make quality-all          # Format + lint all languages
```

---

## SonarLint (Optional, IDE Only)

**SonarLint** provides **real-time code quality feedback in your IDE** without requiring a server.

### Installation

- **VS Code**: [SonarLint extension](https://marketplace.visualstudio.com/items?itemName=SonarSource.sonarlint-vscode)
- **JetBrains**: Plugin marketplace (PyCharm, IntelliJ, GoLand, WebStorm)

### Key Features

- **Real-time analysis**: Catch bugs, security vulnerabilities, and code smells as you type
- **Zero config**: Works standalone, no server needed
- **Multi-language**: Python, JavaScript/TypeScript, Go

### Documentation

See [SonarLint Guide](./sonarlint.md) for detailed setup and usage.

---

## SonarQube (Optional, CI Only)

**SonarQube** provides **deep code analysis, quality gates, and technical debt tracking** for CI pipelines. It is **NOT required for local development**.

### When to Use SonarQube

- ✅ You want centralized quality metrics and dashboards
- ✅ You want enforced quality gates on pull requests
- ✅ You want to track technical debt and code coverage trends
- ❌ You just want fast local feedback (use pre-commit + SonarLint instead)

### Setup (Optional)

1. **Deploy SonarQube server** (or use SonarCloud)
2. **Configure scanner**: See [`sonar-project.properties`](../../sonar-project.properties)
3. **Add CI job**: See `.github/workflows/sonarqube.yml` (to be created)

### Running the Scanner

```bash
# Set authentication token
export SONAR_TOKEN=your-token

# Run analysis (after running tests with coverage)
sonar-scanner
```

### CI Integration (Example)

```yaml
# .github/workflows/sonarqube.yml
name: SonarQube Analysis

on:
  push:
    branches: [main]
  pull_request:

jobs:
  sonarqube:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Run tests with coverage
        run: |
          cd apps/backend/monolith
          pytest --cov --cov-report=xml --cov-report=lcov

      - name: SonarQube Scan
        uses: sonarsource/sonarqube-scan-action@master
        env:
          SONAR_TOKEN: ${{ secrets.SONAR_TOKEN }}
          SONAR_HOST_URL: ${{ secrets.SONAR_HOST_URL }}
```

**Note**: This workflow is **not enabled by default**. Create it only when you have a SonarQube server ready.

### Configuration File

See [`sonar-project.properties`](../../sonar-project.properties) for:
- Source directories
- Test report paths
- Exclusions (migrations, tests, generated files)
- Quality gate thresholds

---

## Local vs CI Checks

### Local (Pre-commit + SonarLint)
- **Fast** (seconds)
- **Runs on every commit**
- **Blocks broken code from entering repo**
- **No infrastructure required**

### CI (Full Test Suite + SonarQube)
- **Comprehensive** (minutes)
- **Runs on every push/PR**
- **Enforces quality gates**
- **Generates metrics and dashboards**

---

## Rollback Procedure

If you need to remove quality tooling:

### Disable Pre-commit Hooks

```bash
# Uninstall hooks
pre-commit uninstall
pre-commit uninstall --hook-type commit-msg

# Remove pre-commit entirely
pip uninstall pre-commit
rm .pre-commit-config.yaml
```

### Disable SonarLint

- Uninstall the extension from your IDE
- No code changes required

### Disable SonarQube

- Delete `sonar-project.properties`
- Remove CI job (if created)
- No code changes required

---

## Troubleshooting

### Pre-commit: Hook failed

```bash
# Identify which hook failed
pre-commit run --all-files --verbose

# Run specific hook manually
pre-commit run ruff --all-files

# Update hooks (may fix compatibility issues)
make pre-commit-update
```

### Pre-commit: Slow performance

```bash
# Skip expensive hooks temporarily
SKIP=mypy,badit git commit

# Run less frequently
# In .pre-commit-config.yaml, change hook "files" pattern
```

### MyPy: Type errors

```bash
# Run mypy standalone for detailed errors
cd apps/backend/monolith
mypy apps/backend/core/

# Add type ignore comment (for false positives)
# type: ignore[reason]
```

### Coverage: Missing reports

```bash
# Generate coverage reports first
cd apps/backend/monolith
pytest --cov --cov-report=xml --cov-report=lcov
```

### SonarQube: "Project not found"

- Create the project in SonarQube UI first
- Match `sonar.projectKey` in `sonar-project.properties`

### SonarQube: Permission denied

- Check `SONAR_TOKEN` has "Execute Analysis" permission
- Generate new token in SonarQube UI (My Account → Security)

---

## Best Practices

1. **Install pre-commit on day one**: It prevents bad code from entering the repo
2. **Fix pre-commit issues immediately**: Don't skip hooks habitually
3. **Use SonarLint in your IDE**: Catch issues before committing
4. **Review SonarQube reports weekly**: Track technical debt trends
5. **Tighten quality gates gradually**: Start relaxed, improve over time
6. **Don't duplicate tools**: Trust formatters for style, linters for bugs

---

## Further Reading

- [Pre-commit Documentation](https://pre-commit.com/)
- [Ruff Documentation](https://docs.astral.sh/ruff/)
- [SonarLint Setup Guide](./sonarlint.md)
- [SonarQube Documentation](https://docs.sonarqube.org/)
- [Development Workflow](../DEVELOPER.md)

---

**Last Updated**: 2025-12-28
**Maintained By**: Development Team
