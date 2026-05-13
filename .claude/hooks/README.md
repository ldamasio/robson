# Claude Code Hooks for Robson v2

This directory contains hooks that run automatically during Claude Code sessions to ensure code quality and consistency.

## Available Hooks

### 1. `post-tool-use.sh` (Fast Validation)

**Triggers**: After `Write` or `Edit` operations on `.rs`, `.ts`, or `.tsx` files

**What it does**:
- **Rust files**: Runs `cargo fmt --check` (fast formatting validation)
- **TypeScript files**: Runs `bun run tsc --noEmit` (type checking)

**Why**: Provides immediate feedback on formatting/type errors without slowing down the workflow

**Speed**: Very fast (< 1 second for format checks)

### 2. `stop.sh` (Full Validation)

**Triggers**: When ending a Claude Code session (stop/exit command)

**What it does**:
- Runs `./scripts/verify.sh` (full verification)
- Checks formatting, linting, and tests (unless fast mode)
- Provides summary of all validation results

**Why**: Ensures all changes are production-ready before committing

**Speed**: Depends on test suite (2-10 seconds for full verification)

## Configuration

### Enable Hooks

Hooks are enabled by default when this directory exists in the repository.

To configure hooks for your Claude Code session, edit your Claude Code settings:

```bash
# For v2 workspace only (recommended)
cat > /home/psyctl/apps/robson/v2/.claude/settings.json <<EOF
{
  "hooks": {
    "enabled": true,
    "directory": ".claude/hooks"
  }
}
EOF
```

### Disable Hooks Temporarily

Set environment variable before starting Claude Code:

```bash
export CLAUDE_HOOK_DISABLED=1
claude-code
```

Or during a session, hooks will respect the environment variable if already set.

### Fast Mode (Skip Tests)

To skip tests and only run format/lint checks:

```bash
export CLAUDE_HOOK_FAST=1
claude-code
```

This makes the `stop` hook run much faster while still catching formatting and lint errors.

## When to Use Each Mode

### Full Validation (Default)
✅ **Use when**:
- Working on critical business logic
- Before creating a pull request
- Before committing to main branch
- When you have time for comprehensive checks

❌ **Skip when**:
- Rapid prototyping
- Working on documentation only
- Iterating quickly on experimental code

### Fast Mode (`CLAUDE_HOOK_FAST=1`)
✅ **Use when**:
- Iterating quickly on features
- Working on multiple small changes
- Documentation updates
- Refactoring (want quick feedback)

❌ **Skip when**:
- Finalizing feature for PR
- Working on critical financial logic
- Uncertain about correctness of changes

### Disabled (`CLAUDE_HOOK_DISABLED=1`)
✅ **Use when**:
- Working on non-code files only (docs, configs)
- Troubleshooting hook issues
- Need to move fast without interruption
- Experimenting with ideas

❌ **Skip when**:
- Working on production code
- Multiple changes that need validation
- Unsure about code quality

## Hook Behavior

### Non-Blocking Errors

Hooks provide feedback but **do not block** the session from continuing. If validation fails:

1. Hook shows error message
2. Suggests quick fix commands
3. Session continues normally

You should fix errors before committing, but hooks won't prevent you from working.

### File-Type Specific

Hooks are smart about which files to validate:

| File Extension | Validation |
|---------------|------------|
| `.rs` | Rust formatting + clippy |
| `.ts`, `.tsx` | TypeScript type checking |
| `.md`, `.toml`, `.json` | No validation (skip hook) |

This keeps hooks fast and relevant.

## Customization

### Adding Custom Hooks

Create additional hook scripts in this directory:

```bash
# Example: pre-commit hook
touch .claude/hooks/pre-commit.sh
chmod +x .claude/hooks/pre-commit.sh
```

### Modifying Existing Hooks

Edit the `.sh` files directly. Key sections:

```bash
# Example: Add custom validation
if [[ "$FILE_PATH" =~ \.rs$ ]]; then
    # Your custom Rust validation here
    cargo custom-check
fi
```

## Troubleshooting

### Hook Fails Immediately

**Problem**: Hook exits with error even though code looks fine

**Solution**:
1. Check if verification script exists: `ls -la scripts/verify.sh`
2. Run manually: `./scripts/verify.sh`
3. Check environment variables: `echo $CLAUDE_HOOK_DISABLED`

### Hook Runs Too Slowly

**Problem**: Session feels sluggish after edits

**Solution**:
1. Enable fast mode: `export CLAUDE_HOOK_FAST=1`
2. Or disable post-tool-use hook, keep stop hook only
3. Reduce test suite size (if tests are slow)

### TypeScript Hook Fails

**Problem**: TypeScript validation fails even though code compiles

**Solution**:
1. Ensure dependencies are installed: `cd cli && bun install`
2. Check `tsconfig.json` is valid
3. Run manually: `cd cli && bun run tsc --noEmit`

### Hook Not Running

**Problem**: Expected hook to run but nothing happened

**Solution**:
1. Check hooks are executable: `chmod +x .claude/hooks/*.sh`
2. Verify Claude Code settings point to `.claude/hooks`
3. Check if `CLAUDE_HOOK_DISABLED=1` is set

## Integration with CI/CD

These hooks complement (but don't replace) CI/CD checks:

```
┌─────────────────┐     ┌──────────────┐     ┌─────────────┐
│ Claude Code     │────▶│ Git Push     │────▶│ GitHub      │
│ Hooks           │     │              │     │ Actions CI  │
│ (Local)         │     │              │     │ (Remote)    │
└─────────────────┘     └──────────────┘     └─────────────┘
     ▲                                              │
     │                                              │
     │   Catch errors early                         │
     └──────────────────────────────────────────────┘
                    Ensure quality
```

**Local hooks**: Fast feedback, catch obvious errors
**CI/CD**: Comprehensive checks, cross-platform validation, integration tests

Always ensure CI/CD passes before merging, even if hooks pass locally.

## Best Practices

1. **Keep hooks fast** - Slow hooks discourage use
2. **Make errors actionable** - Show exact fix commands
3. **Don't block workflow** - Inform but don't prevent work
4. **Respect user control** - Easy to disable/configure
5. **Validate only changed code** - Don't re-check entire codebase

## Examples

### Typical Session with Hooks

```bash
# Start session
$ claude-code

# Claude writes Rust file
Claude> [Writes domain.rs]
→ Validating Rust formatting...
✓ Rust formatting OK

# Claude edits TypeScript file
Claude> [Edits client.ts]
→ Validating TypeScript types...
✓ TypeScript types OK

# End session
Claude> stop
═══════════════════════════════════════════════════
    Robson v2 - Session Validation
═══════════════════════════════════════════════════

→ Running FULL validation (format, lint, tests)
Running rustfmt (check mode)...
✓ Rust formatting OK
Running clippy (strict mode)...
✓ Clippy passed (no warnings)
Running cargo test...
✓ All Rust tests passed
Running TypeScript type check...
✓ TypeScript types OK

╔════════════════════════════════════════════╗
║  ✓ All validations passed!                ║
║  Your changes are ready to commit.        ║
╚════════════════════════════════════════════╝
```

### Fast Mode Session

```bash
# Enable fast mode
$ export CLAUDE_HOOK_FAST=1
$ claude-code

# ... make changes ...

# End session (no tests run)
Claude> stop
→ Running FAST validation (no tests)
✓ Rust formatting OK
✓ Clippy passed (no warnings)
✓ TypeScript types OK
✓ All validations passed!
```

## Further Reading

- [Claude Code Hooks Documentation](https://docs.anthropic.com/claude-code/hooks)
- [../../../docs/v2/CONTRIBUTING.md](../../../docs/v2/CONTRIBUTING.md) - Contribution guidelines
- [../../scripts/verify.sh](../../scripts/verify.sh) - Full verification script

---

**Version**: 1.0
**Last Updated**: 2026-01-15
