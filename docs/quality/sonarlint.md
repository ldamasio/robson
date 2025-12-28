# SonarLint - Local Code Quality Analysis

**SonarLint** is a lightweight IDE extension that provides real-time code quality feedback without requiring a SonarQube server connection. It helps catch bugs, security vulnerabilities, and code smells as you type.

---

## Why SonarLint?

- **Real-time feedback**: Catch issues before committing
- **Zero configuration required**: Works standalone immediately
- **Rule consistency**: Uses the same quality rules as SonarQube (when connected)
- **Multi-language support**: Python, JavaScript/TypeScript, Go, and more
- **No server needed**: Runs entirely in your IDE

---

## Installation

### Visual Studio Code

1. Install the [SonarLint extension](https://marketplace.visualstudio.com/items?itemName=SonarSource.sonarlint-vscode)
2. Reload VS Code when prompted
3. SonarLint will automatically analyze Python, JavaScript, and TypeScript files

**Configuration (Optional)**:

Create `.vscode/settings.json` in your workspace root:

```json
{
  "sonarlint.output.showVerboseLogs": true,
  "sonarlint.output.enableAnalyzerTraces": false,
  "sonarlint.disableTelemetry": true
}
```

### JetBrains IDEs (PyCharm, IntelliJ IDEA, GoLand, WebStorm)

1. Go to **File** → **Settings** → **Plugins**
2. Search for **SonarLint**
3. Click **Install**
4. Restart the IDE
5. SonarLint will automatically activate for supported file types

**Configuration (Optional)**:

Go to **Settings** → **Tools** → **SonarLint**:

- Enable/disable specific rules
- Configure file exclusions
- Adjust analysis scope

---

## Recommended Rule Focus

For the Robson project, SonarLint is configured to prioritize:

### Critical Rules
- **Bugs**: Null pointer dereferences, resource leaks, logic errors
- **Security**: SQL injection, XSS, hardcoded credentials, weak crypto
- **Code Smells**: Complex functions, code duplication, unused code

### Disable/Lower Priority (Optional)
SonarLint may report style issues that conflict with formatters (ruff, black, prettier). These can be safely ignored:

- **Code style**: Indentation, line length, naming conventions (handled by formatters)
- **Minor code smells**: Prefer `const` over `let`, trivial simplifications

To disable specific rules in VS Code, add to `.vscode/settings.json`:

```json
{
  "sonarlint.rules": {
    "python:S117": { "level": "off" },  // local variable naming
    "javascript:S117": { "level": "off" }  // parameter naming
  }
}
```

---

## Connected Mode (Optional)

If you have a SonarQube server, you can connect SonarLint to share rule configurations and quality profiles.

### VS Code Connected Mode

1. Open Command Palette (`Ctrl+Shift+P` / `Cmd+Shift+P`)
2. Select **SonarLint: Connect to Server**
3. Enter your SonarQube server URL and credentials
4. Select the **Robson** project binding

### JetBrains Connected Mode

1. Go to **Settings** → **Tools** → **SonarLint** → **Connected Mode**
2. Click **Add Connection**
3. Enter server URL and credentials
4. Bind the current project to **Robson**

**Benefits of Connected Mode**:
- Team-wide rule consistency
- Synchronized quality profiles
- Issue synchronization with server

**Note**: The Robson project does NOT require connected mode. SonarLint works perfectly standalone.

---

## Common Issues & Fixes

### Issue: Too many false positives

**Solution**: Adjust rule severity in SonarLint settings. Disable style rules that conflict with formatters.

### Issue: Analysis slows down the IDE

**Solution**:
- Limit analysis scope to current file (default)
- Disable verbose logs in settings
- Exclude generated directories (e.g., `node_modules`, `venv`, `dist`)

### Issue: Conflicts with ruff/black/prettier

**Solution**: Let SonarLint focus on **bugs** and **security**. Disable style-related rules. Trust formatters for code style.

### Issue: "Rule not found" errors

**Solution**: Update SonarLint to the latest version. Some rules may have been deprecated or renamed.

---

## Keyboard Shortcuts

### VS Code

| Action | Shortcut |
|--------|----------|
| Show SonarLint issues | `Ctrl+Shift+P` → "SonarLint: Show All Issues" |
| Run analysis on active file | (Automatic, runs on save) |
| Disable rule on current line | Right-click issue → "Disable on this line" |

### JetBrains

| Action | Shortcut |
|--------|----------|
| Show SonarLint tool window | `Alt+7` (Windows/Linux) or `Cmd+7` (Mac) |
| Run analysis on current file | `Ctrl+Shift+K` (Windows/Linux) or `Cmd+Shift+K` (Mac) |
| Navigate to next issue | `F2` |

---

## Integration with Pre-commit

SonarLint runs **in your IDE only** and does **NOT** integrate with pre-commit hooks. This is intentional:

- **Pre-commit**: Fast, automated checks on every commit (ruff, black, prettier)
- **SonarLint**: Deep, interactive analysis while coding (bugs, security)

Use both together for maximum code quality coverage:

1. **Code**: SonarLint warns you in real-time
2. **Commit**: Pre-commit hooks run fast checks
3. **CI**: Full test suite + optional SonarQube scan

---

## Next Steps

- Install SonarLint in your preferred IDE
- Try it on a file in `apps/backend/monolith/api/`
- Review reported issues and fix critical ones
- Adjust rules to match your workflow

For SonarQube server integration (optional), see [README.md](./README.md#sonarqube-optional).
