# Robson CLI (robson-go)

Go-based CLI for the Robson cryptocurrency trading platform.

## Architecture

```
┌─────────────┐
│   robson    │  ← C router (main.c) - preserves historical continuity
│   (main.c)  │    Translates legacy flags → subcommands
└──────┬──────┘    Delegates via execvp()
       │
       ▼
┌─────────────┐
│  robson-go  │  ← Go implementation (this directory)
│   (main.go) │    All business logic lives here
└─────────────┘    Clean subcommand architecture
```

## Design Philosophy

**CLI as a CONTRACT OF INTENT**

Just as in trading, where we separate:
- Idea formulation
- Validation
- Execution

We separate at the CLI level:
- `plan` - Create execution blueprint
- `validate` - Verify before committing
- `execute` - Act on validated intent

## Project Structure

```
cli/
├── go.mod              # Go module definition
├── main.go             # Entry point
├── cmd/
│   ├── root.go         # Root command + global flags
│   ├── legacy.go       # Legacy commands (help, report, say, buy, sell)
│   ├── agentic.go      # Agentic workflow (plan, validate, execute)
│   └── margin.go       # Margin trading (status, positions, margin-buy)
└── README.md           # This file
```

## Building

### Prerequisites

- Go 1.23 or later
- GCC or Clang (for building main.c)

### Build robson-go (Go binary)

```bash
cd cli
go mod download
go build -o robson-go -ldflags="-X main.Version=0.2.0 -X main.BuildTime=$(date -u +%Y-%m-%dT%H:%M:%SZ)" .
```

This creates the `robson-go` binary.

### Build robson (C router)

```bash
cd ..
gcc -o robson main.c
```

This creates the `robson` binary that delegates to `robson-go`.

### Install to PATH

```bash
# Option 1: Copy to system bin
sudo cp robson /usr/local/bin/
sudo cp cli/robson-go /usr/local/bin/

# Option 2: Add to PATH
export PATH=$PATH:$(pwd):$(pwd)/cli
```

## Usage

### Legacy Commands (backward compatible)

```bash
# Using legacy flags (translated by C router)
robson --help
robson --report
robson --say "Hello, world!"
robson --buy BTCUSDT 0.001 50000
robson --sell BTCUSDT 0.001 55000

# Using modern subcommands (direct to robson-go)
robson help
robson report
robson say "Hello, world!"
robson buy BTCUSDT 0.001 50000
robson sell BTCUSDT 0.001 55000
```

### Agentic Workflow

```bash
# STEP 1: Plan
robson plan buy BTCUSDT 0.001 --limit 50000
# Returns: Plan ID (e.g., abc123def456)

# STEP 2: Validate
robson validate abc123def456
# Checks: balance, market conditions, risk

# STEP 3: Execute
robson execute abc123def456
# Only if validated successfully
```

### Margin Trading Commands

```bash
# Account status overview
robson status                      # Quick summary
robson status --detailed           # With position details

# View positions
robson positions                   # Open positions
robson positions --live            # With real-time prices
robson positions --all             # Include closed positions
robson positions --json            # JSON output for scripts

# Open leveraged position (Golden Rule enforced)
robson margin-buy --capital 100 --stop-percent 2 --leverage 3
robson margin-buy --capital 100 --stop-price 85000 --leverage 5 --live --confirm
```

### JSON Output (for automation)

All commands support `--json` flag for machine-readable output:

```bash
robson help --json
robson plan buy BTCUSDT 0.001 --json
robson validate abc123def456 --json
robson execute abc123def456 --json
robson positions --json            # Position data for scripts
```

## Smoke Tests

### Test 1: Basic help

```bash
robson help
# Expected: Help screen with all commands listed
```

### Test 2: Legacy flag translation

```bash
robson --help
# Expected: Same output as "robson help"
```

### Test 3: Say command

```bash
robson say "Testing CLI"
# Expected: "Robson says: Testing CLI"
```

### Test 4: JSON output

```bash
robson help --json | jq .
# Expected: Valid JSON structure
```

### Test 5: Agentic workflow

```bash
# Create plan
PLAN_ID=$(robson plan buy BTCUSDT 0.001 --json | jq -r '.planID')
echo "Plan ID: $PLAN_ID"

# Validate plan
robson validate $PLAN_ID

# Execute plan (simulated)
robson execute $PLAN_ID
```

### Test 6: Version info

```bash
robson version
# Expected: Version and build time
```

### Quick smoke test script

```bash
#!/bin/bash
set -e

echo "=== Robson CLI Smoke Tests ==="

echo "Test 1: Help command"
robson help > /dev/null && echo "✓ PASSED" || echo "✗ FAILED"

echo "Test 2: Legacy flag translation"
robson --help > /dev/null && echo "✓ PASSED" || echo "✗ FAILED"

echo "Test 3: Say command"
OUTPUT=$(robson say "test")
[[ "$OUTPUT" == *"Robson says: test"* ]] && echo "✓ PASSED" || echo "✗ FAILED"

echo "Test 4: JSON output"
robson help --json | jq . > /dev/null && echo "✓ PASSED" || echo "✗ FAILED"

echo "Test 5: Plan creation"
robson plan buy BTCUSDT 0.001 > /dev/null && echo "✓ PASSED" || echo "✗ FAILED"

echo "Test 6: Version"
robson version > /dev/null && echo "✓ PASSED" || echo "✗ FAILED"

echo ""
echo "=== All tests completed ==="
```

Save as `smoke-test.sh` and run: `bash smoke-test.sh`

## Development

### Adding a new command

1. Create command in `cmd/` directory:

```go
var myCmd = &cobra.Command{
    Use:   "mycommand",
    Short: "Short description",
    Long:  "Long description",
    RunE: func(cmd *cobra.Command, args []string) error {
        if jsonOutput {
            return outputJSON(map[string]interface{}{
                "command": "mycommand",
                "status":  "success",
            })
        }

        fmt.Println("Human-readable output")
        return nil
    },
}
```

2. Register in `cmd/root.go`:

```go
func init() {
    // ...
    rootCmd.AddCommand(myCmd)
}
```

3. Rebuild:

```bash
go build -o robson-go .
```

### Testing

```bash
# Run unit tests
go test ./...

# Test specific command
./robson-go help
./robson-go plan buy BTCUSDT 0.001

# Test via C router
./robson --help
./robson help
```

## Integration with Backend

**TODO**: These commands currently output simulated data.

Next steps:
1. Add configuration file support (`~/.robson/config.yaml`)
2. Integrate with Django backend API
3. Implement actual order execution
4. Add plan persistence (SQLite or JSON files)
5. Implement real validation logic

## Error Handling

The CLI uses Go's standard error handling:

```go
return fmt.Errorf("validation failed: %w", err)
```

Errors are displayed to stderr and exit with code 1.

## Dependencies

- `github.com/spf13/cobra` - CLI framework
- Standard library only (no external runtime deps)

## Version History

- **v0.2.0** - Agentic workflow (plan/validate/execute)
- **v0.1.0** - Legacy commands ported to Go
- **v0.0.1** - Original C implementation

## License

Same as parent Robson project (see LICENSE in repository root)
