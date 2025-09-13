Robson Bot – Style Guide

Language Policy
- English only for all code (identifiers), comments, docstrings, commit messages, PRs, and documentation.
- Avoid slang or ambiguous phrasing; prefer concise, descriptive wording.
- For user-facing text, keep tone neutral and professional.

Naming
- Python: `snake_case` for variables/functions, `PascalCase` for classes, `UPPER_SNAKE_CASE` for constants.
- Files and modules: `snake_case`.
- Avoid abbreviations unless widely understood (e.g., `id`, `url`).

Comments & Docstrings
- Explain “why”, not “what” (code should express “what”).
- Public functions/methods: add short docstrings describing purpose, parameters, and return when helpful.
- Keep comments up-to-date; remove stale notes and TODOs promptly or track them as issues.

Structure & Imports
- Import order: stdlib → third‑party → local.
- Avoid circular dependencies. Extract shared logic to common modules where needed.

Migrations & DB
- Prefer explicit schema changes; avoid ambiguous auto‑renames.
- Use `migrations.RenameField` and `RunPython` for data fixes when necessary.
- For development, if there’s no data to preserve, consider full reset (`make dev-reset-api`).

Testing
- Co-locate tests under `api/tests/` following module names.
- Write focused, deterministic tests; avoid external calls.
- Use flags/config to disable integrations in tests (e.g., `TRADING_ENABLED=False`).

Tooling
- Recommended: Black (format), isort (imports), Flake8 (lint), Mypy (types).
- Prefer `python -m pip` for installs; pin versions in `requirements.txt`.

Commits & PRs
- Use Conventional Commits for messages (e.g., `feat:`, `fix:`, `chore:`, `docs:`, `ci:`).
- Keep PRs small and focused; include migration notes and test coverage.

