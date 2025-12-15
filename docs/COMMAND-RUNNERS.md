# Command Runners - Architectural Guidelines

**Quick reference for which tool to use when.**

---

## The Three-Tool Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Task Categories                      │
├──────────────┬──────────────────┬───────────────────────┤
│  BUILD       │   DEVELOPMENT    │    DOMAIN ACTIONS     │
│  (Artifacts) │   (Daily work)   │    (Trading)          │
├──────────────┼──────────────────┼───────────────────────┤
│     make     │      just        │       robson          │
└──────────────┴──────────────────┴───────────────────────┘
```

---

## Responsibility Matrix

| Category | Tool | Examples | Why This Tool? |
|----------|------|----------|----------------|
| **Build artifacts** | `make` | Compile C, build Go, generate files | Make's traditional strength; widely understood |
| **Install to system** | `make` | Install CLI to /usr/local/bin | Sudo operations; system-wide changes |
| **Daily development** | `just` | Run tests, start servers, migrations | Better DX; discoverability; guardrails |
| **Database operations** | `just` | Migrate, reset, seed data | Needs confirmations; frequent use |
| **Code quality** | `just` | Lint, format, validate | Development workflow task |
| **Infrastructure** | `just` | K9s, logs, deployments | Orchestration with helpers |
| **Trading operations** | `robson` | plan, validate, execute | Domain logic; business operations |
| **Vendor/external** | `make` | Sync git submodules, download deps | Traditional build-system task |

---

## Decision Rules

### Use `make` when:
- ✅ Compiling code (C, Go binaries)
- ✅ Installing to system paths (requires sudo)
- ✅ Generating artifacts (binaries, packages)
- ✅ Managing external dependencies (git submodules, vendor)
- ✅ Traditional build-system tasks

### Use `just` when:
- ✅ Daily development workflow (test, run, debug)
- ✅ Need discoverability (`just --list`)
- ✅ Need guardrails/confirmations
- ✅ Orchestrating multiple tools
- ✅ Database operations (migrate, reset, seed)
- ✅ Code quality (lint, format)

### Use `robson` when:
- ✅ Trading operations (buy, sell, plan, execute)
- ✅ Business domain logic
- ✅ Need JSON output for automation

### Use direct CLI when:
- ✅ CI/CD (most stable, explicit)
- ✅ Advanced operations not in just/make
- ✅ Debugging/troubleshooting

---

## Practical Examples

### ✅ Correct Usage

```bash
# Build (Make's strength)
make build-cli          # Compile C + Go
make install-cli        # Install to system

# Development (just's strength)
just setup              # Install dependencies
just test               # Run all tests
just db-migrate         # Apply migrations
just dev-backend        # Start dev server

# Domain operations (robson's purpose)
robson plan buy BTCUSDT 0.001
robson validate <plan-id> --client-id 1
robson execute <plan-id> --client-id 1

# Direct CLI (when needed)
kubectl get pods
python manage.py shell
```

### Cooperation Example

```bash
# Typical workflow uses ALL tools together:

# 1. Build artifacts (Make)
make build-cli

# 2. Setup development (just)
just setup
just db-up
just db-migrate

# 3. Run tests (just)
just test

# 4. Install to system (Make)
make install-cli

# 5. Use domain CLI (robson)
robson plan buy BTCUSDT 0.001
```

---

## Anti-Patterns to Avoid

### ❌ Duplication

```bash
# DON'T duplicate in both tools
make test              # ❌
just test              # ❌
# Pick one: just test (better for daily dev)

# DON'T re-implement domain logic
just plan-buy          # ❌ Use robson
make execute-plan      # ❌ Use robson
```

### ❌ Wrong Tool for the Job

```bash
# DON'T use just for system installation
just install-cli       # ❌ Use make (needs sudo, system-wide)

# DON'T use make for daily dev workflow
make run-all-tests     # ❌ Use just (better DX)

# DON'T use robson for dev tasks
robson migrate         # ❌ Use just
```

---

## Integration Points

### just can call make

```just
# In justfile: delegate build to make
build:
    make build-cli
    @echo "✅ Build complete"
```

### make can suggest just

```makefile
# In Makefile: hint at better tool
test:
	@echo "Tip: Use 'just test' for better test workflows"
	cd apps/backend/monolith && python manage.py test
```

### Both can call robson

```just
# justfile wrapper with guardrails
[confirm("Execute in LIVE mode?")]
execute-live PLAN_ID CLIENT_ID:
    ./robson execute {{PLAN_ID}} --client-id {{CLIENT_ID}} --live --acknowledge-risk
```

```makefile
# Makefile can also wrap robson (if needed)
.PHONY: smoke-test-trading
smoke-test-trading:
	./robson plan buy BTCUSDT 0.001 --json | jq -r '.planID'
```

---

## Quick Reference Card

**I want to...**

| Task | Command |
|------|---------|
| Compile the CLI | `make build-cli` |
| Install CLI system-wide | `make install-cli` |
| Run all tests | `just test` |
| Start dev database | `just db-up` |
| Reset database | `just db-reset` |
| Start dev server | `just dev-backend` |
| Lint code | `just lint` |
| Open K9s | `just k9s` |
| Create trading plan | `robson plan buy BTCUSDT 0.001` |
| Execute plan | `robson execute <id> --client-id 1` |
| See all dev tasks | `just --list` |
| See all make targets | `make help` or read Makefile |
| See trading commands | `robson --help` |

---

## Implementation Strategy

### Now (Immediate)

1. Keep Makefile focused on **build + install**
2. Create justfile focused on **dev workflow**
3. No duplication - clear boundaries
4. Both tools coexist peacefully

### Refactoring Steps

```bash
# 1. Create justfile (new)
touch justfile

# 2. Move dev tasks from Makefile to justfile
#    - dev-db-*, dev-test → just
#    - k9s-* → just
#    - Keep: build-*, install-*, sync-*

# 3. Test both work
make build-cli     # Still works (build)
just test          # New (dev)
robson --help      # Unchanged (domain)

# 4. Update docs
#    - README: mention both make and just
#    - Show when to use each
```

---

## Summary

**Make and just are complementary, not competitive:**

- **Make** = Build system (artifacts, installation, vendor)
- **just** = Dev workflow (test, run, orchestrate, database)
- **robson** = Domain operations (trading)

**No migration needed** - they work together!

---

**Last Updated:** 2025-12-14
