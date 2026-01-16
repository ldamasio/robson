# Robson CLI v2

Command-line interface for Robson v2 trading platform.

## Installation

```bash
bun install
```

## Development

```bash
# Run CLI
bun run dev --help
bun run dev status
bun run dev arm BTCUSDT --strategy all-in

# Build
bun run build

# Test
bun test
```

## Usage

```bash
robson <command> [options]

Commands:
  status              Show position status
  arm <symbol>        Arm position for symbol
  disarm <id>         Disarm position
  panic               Emergency close all positions
  help [command]      Display help for command
```

## Environment Variables

```bash
export ROBSON_DAEMON_URL="http://localhost:8080"  # Default
```

## Examples

### Check Status

```bash
robson status
robson status --json
robson status --watch
```

### Arm Position

```bash
robson arm BTCUSDT --strategy all-in
robson arm ETHUSDT --strategy all-in --capital 1000 --leverage 2
```

### Emergency Close

```bash
robson panic
robson panic --symbol BTCUSDT
robson panic --confirm
```

## Development Status

- [x] CLI structure
- [x] Command stubs
- [x] API client stub
- [ ] Full API integration (pending robsond implementation)
- [ ] Table formatting
- [ ] Watch mode
