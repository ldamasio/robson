Robson Bot – AI Collaboration Workflow

Purpose
- Define clear, repeatable guidelines for AI assistants collaborating on this repository.

Core Rules
- Language: English only for code, comments, docstrings, commit messages, PRs, and documentation.
- Conventional Commits: Always use semantic commit messages (e.g., `feat:`, `fix:`, `chore:`, `docs:`, `ci:`).
- Commit Suggestions: After every modification (file add/update/delete) or directory change, the AI must propose an English semantic commit message to the user.
- Tests/CI: Prefer keeping the test suite green; when changing behavior, add or update tests accordingly.
- Migrations: Prefer explicit schema migrations. If no data needs preserving in dev, `make dev-reset-api` is permitted.
- Tooling: Respect pre-commit hooks (black, isort, core checks, English‑only checker). Run `make lint` before suggesting a commit when feasible.

Operational Guidance
- Small, focused changes: Make targeted patches and propose a concise commit message.
- Documentation updates: Whenever behavior or developer UX changes, update relevant docs (README, DEVELOPER.md, MIGRATION_GUIDE.md, STYLE_GUIDE.md) and include those docs files in the proposed commit.
- English‑only enforcement: Avoid introducing Portuguese or other non‑English text in code or docs. If encountered, translate as part of the change.

Suggested Commit Message Template
- `<type>(<scope>): <short imperative summary>`
- Body: list notable changes succinctly (bullets are OK)
- Footer (optional): BREAKING CHANGE or issue references

Examples
- `feat(api): add trade duration property and winner logic`
- `fix(ci): make Order.price nullable in migrations to satisfy tests`
- `docs(dev): add pre-commit usage and style guide link`

