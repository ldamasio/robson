# Robson v2 CLI Reference

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-12
**Status**: Planning Phase

---

## Table of Contents

1. [Installation](#installation)
2. [Commands](#commands)
3. [JSON Output](#json-output)
4. [Examples](#examples)
5. [Configuration](#configuration)
6. [Automation](#automation)

---

## Installation

### Prerequisites

- Bun >= 1.0
- robsond daemon running (localhost or remote)

### Install CLI

```bash
# Install globally
bun install -g robson-cli

# Or install from source
cd cli
bun install
bun link

# Verify installation
robson --version
```

---

## Commands

### `robson init`

Initialize Robson configuration

```bash
robson init

# Interactive prompts:
# - Daemon URL (default: http://localhost:8080)
# - Output format (default: table)
# - Log level (default: info)
```

**Creates**: `~/.robson/config.toml`

---

### `robson arm`

Arm a new position (wait for detector signal to enter)

```bash
robson arm SYMBOL --strategy STRATEGY [OPTIONS]

Arguments:
  SYMBOL              Trading pair (e.g., BTCUSDT)

Options:
  --strategy NAME     Strategy name (required)
                      Options: all-in, custom
  --capital AMOUNT    Capital to allocate (default: from config)
  --leverage N        Leverage multiplier (1-10, default: 3)
  --dry-run           Simulate without real orders

Examples:
  robson arm BTCUSDT --strategy all-in
  robson arm ETHUSDT --strategy all-in --capital 1000 --leverage 2
  robson arm BTCUSDT --strategy all-in --dry-run
```

**Output**:
```
✓ Position armed: pos_01HQZXY123
  Symbol: BTCUSDT
  Strategy: all-in
  Capital: $10,000
  Leverage: 3x
  Status: Armed (waiting for entry signal)

  Next: The detector will scan for entry opportunities.
        Use 'robson status' to monitor.
```

---

### `robson disarm`

Disarm a position (cancel waiting for entry signal)

```bash
robson disarm POSITION_ID

Arguments:
  POSITION_ID         Position ID or symbol

Options:
  --force             Force disarm even if entering/active

Examples:
  robson disarm pos_01HQZXY123
  robson disarm BTCUSDT
  robson disarm BTCUSDT --force
```

**Output**:
```
✓ Position disarmed: pos_01HQZXY123
  Symbol: BTCUSDT
  Status: Armed → Cancelled
```

**Note**: Cannot disarm active positions (use `panic` instead)

---

### `robson status`

Show status of all positions

```bash
robson status [OPTIONS]

Options:
  --symbol SYMBOL     Filter by symbol
  --state STATE       Filter by state (armed/active/closed)
  --json              Output as JSON
  --watch             Continuous monitoring (refresh every 2s)

Examples:
  robson status
  robson status --symbol BTCUSDT
  robson status --state active
  robson status --json
  robson status --watch
```

**Output (table)**:
```
POSITIONS

ID               Symbol    Side   State    Entry      SL         SG         PnL       Leverage
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
pos_01HQZXY123   BTCUSDT   Long   Active   $95,000    $93,500    $98,500    +$156.23  3x
pos_01HQZXY456   ETHUSDT   Long   Armed    -          -          -          -         2x
pos_01HQZXY789   BTCUSDT   Long   Closed   $94,000    $92,500    -          -$100.00  3x

SUMMARY
  Active: 1
  Armed: 1
  Closed today: 1
  Total PnL today: +$56.23
```

**Output (JSON)**:
```json
{
  "positions": [
    {
      "id": "pos_01HQZXY123",
      "symbol": "BTCUSDT",
      "side": "long",
      "state": "active",
      "entry_price": 95000.0,
      "stop_loss": 93500.0,
      "stop_gain": 98500.0,
      "quantity": 0.2001,
      "leverage": 3,
      "unrealized_pnl": 156.23,
      "palma": {
        "distance": 1500.0,
        "distance_pct": 1.58
      },
      "created_at": "2026-01-12T10:30:00Z",
      "entry_filled_at": "2026-01-12T10:32:15Z"
    }
  ],
  "summary": {
    "active_count": 1,
    "armed_count": 1,
    "closed_today_count": 1,
    "total_pnl_today": 56.23
  }
}
```

---

### `robson panic`

Emergency close all positions (market orders)

```bash
robson panic [OPTIONS]

Options:
  --symbol SYMBOL     Only close positions for this symbol
  --confirm           Skip confirmation prompt
  --dry-run           Simulate without real orders

Examples:
  robson panic
  robson panic --symbol BTCUSDT
  robson panic --confirm
```

**Output**:
```
⚠️  PANIC MODE: Close all active positions?

  This will place MARKET orders to exit:
    - pos_01HQZXY123 (BTCUSDT Long, $19,009.50)
    - pos_01HQZXY456 (ETHUSDT Long, $5,000.00)

  Total exposure: $24,009.50

  Continue? (y/N): y

✓ Panic executed:
  - pos_01HQZXY123: Exiting (order_abc123)
  - pos_01HQZXY456: Exiting (order_def456)

  Waiting for fills...

✓ All positions closed:
  - pos_01HQZXY123: Closed at $94,800 (PnL: -$40.02)
  - pos_01HQZXY456: Closed at $3,150 (PnL: +$15.00)
```

---

### `robson history`

Show closed positions history

```bash
robson history [OPTIONS]

Options:
  --days N            Show last N days (default: 7)
  --symbol SYMBOL     Filter by symbol
  --json              Output as JSON
  --csv               Output as CSV

Examples:
  robson history
  robson history --days 30
  robson history --symbol BTCUSDT
  robson history --json
  robson history --csv > trades.csv
```

**Output**:
```
CLOSED POSITIONS (Last 7 days)

Closed At           Symbol    Side   Entry      Exit       PnL        Exit Reason
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
2026-01-12 10:45    BTCUSDT   Long   $95,000    $93,400    -$100.00   Stop Loss
2026-01-11 14:30    ETHUSDT   Short  $3,100     $3,050     +$150.00   Stop Gain
2026-01-11 09:15    BTCUSDT   Long   $94,500    $97,200    +$270.00   Stop Gain

SUMMARY
  Total trades: 3
  Win rate: 66.67% (2/3)
  Total PnL: +$320.00
  Average PnL: +$106.67
  Best trade: +$270.00 (BTCUSDT)
  Worst trade: -$100.00 (BTCUSDT)
```

---

### `robson reconcile`

Force reconciliation with exchange

```bash
robson reconcile [OPTIONS]

Options:
  --position-id ID    Reconcile specific position
  --all               Reconcile all positions
  --json              Output as JSON

Examples:
  robson reconcile --all
  robson reconcile --position-id pos_01HQZXY123
```

**Output**:
```
Reconciling positions...

✓ pos_01HQZXY123 (BTCUSDT): OK
✓ pos_01HQZXY456 (ETHUSDT): OK
⚠️ pos_01HQZXY789 (SOLUSDT): DISCREPANCY

  Issue: Quantity mismatch
    Local: 10.0 SOL
    Exchange: 9.5 SOL

  Corrective action: Update local state to match exchange

Continue? (y/N): y

✓ Reconciliation complete:
  - 2 positions OK
  - 1 position corrected
```

---

### `robson config`

Manage configuration

```bash
robson config [SUBCOMMAND]

Subcommands:
  show                Show current configuration
  set KEY VALUE       Set configuration value
  reset               Reset to defaults

Examples:
  robson config show
  robson config set daemon.url http://localhost:8080
  robson config set output.format json
  robson config reset
```

---

### `robson logs`

Tail daemon logs

```bash
robson logs [OPTIONS]

Options:
  --follow            Follow logs (like tail -f)
  --level LEVEL       Filter by level (debug/info/warn/error)
  --position-id ID    Filter by position ID
  --json              Output as JSON

Examples:
  robson logs --follow
  robson logs --level error
  robson logs --position-id pos_01HQZXY123
```

---

## JSON Output

All commands support `--json` flag for machine-readable output.

### Status JSON Schema

```typescript
interface StatusResponse {
  positions: Position[];
  summary: Summary;
}

interface Position {
  id: string;
  symbol: string;
  side: "long" | "short";
  state: "armed" | "entering" | "active" | "exiting" | "closed" | "error";
  entry_price?: number;
  stop_loss: number;
  stop_gain: number;
  quantity: number;
  leverage: number;
  unrealized_pnl?: number;
  realized_pnl?: number;
  palma?: {
    distance: number;
    distance_pct: number;
  };
  created_at: string;      // ISO 8601
  entry_filled_at?: string;
  closed_at?: string;
}

interface Summary {
  active_count: number;
  armed_count: number;
  closed_today_count: number;
  total_pnl_today: number;
}
```

### Error JSON Schema

```typescript
interface ErrorResponse {
  error: {
    code: string;
    message: string;
    details?: Record<string, unknown>;
  };
}

// Example
{
  "error": {
    "code": "POSITION_NOT_FOUND",
    "message": "Position pos_01HQZXY123 not found",
    "details": {
      "position_id": "pos_01HQZXY123"
    }
  }
}
```

---

## Examples

### Example 1: Basic Workflow

```bash
# 1. Arm position
robson arm BTCUSDT --strategy all-in
# → pos_01HQZXY123 created

# 2. Monitor status
robson status --watch

# 3. When entry signal triggers (automatic):
# Armed → Entering → Active

# 4. When stop loss triggers (automatic):
# Active → Exiting → Closed

# 5. Check result
robson history --days 1
```

### Example 2: Automation Script

```bash
#!/usr/bin/env bash
# arm-positions.sh

SYMBOLS=("BTCUSDT" "ETHUSDT" "SOLUSDT")

for symbol in "${SYMBOLS[@]}"; do
  echo "Arming $symbol..."
  robson arm "$symbol" --strategy all-in --json \
    | jq -r '.position_id' \
    >> armed-positions.txt
done

echo "Armed $(wc -l < armed-positions.txt) positions"
```

### Example 3: Monitoring with JSON

```bash
# Poll status every 10s, alert if loss > $50
while true; do
  robson status --json \
    | jq '.positions[] | select(.unrealized_pnl < -50)' \
    | while read -r position; do
        echo "ALERT: Position loss > $50"
        echo "$position" | jq .
      done

  sleep 10
done
```

### Example 4: Export to CSV

```bash
# Export last 30 days to CSV
robson history --days 30 --csv > trades.csv

# Import to Excel/Google Sheets for analysis
```

---

## Configuration

### Config File Location

`~/.robson/config.toml`

### Config Schema

```toml
[daemon]
url = "http://localhost:8080"
timeout_ms = 5000

[output]
format = "table"  # table | json | csv
color = true

[risk]
default_capital = 10000.0
default_leverage = 3
max_open_positions = 3

[logging]
level = "info"  # debug | info | warn | error
```

### Environment Variables

Override config with env vars:

```bash
export ROBSON_DAEMON_URL="http://localhost:8080"
export ROBSON_OUTPUT_FORMAT="json"
export ROBSON_LOG_LEVEL="debug"

robson status
```

---

## Automation

### Systemd Service (Monitor Daemon)

```ini
# /etc/systemd/system/robson-monitor.service
[Unit]
Description=Robson Position Monitor
After=network.target

[Service]
Type=simple
User=trading
ExecStart=/usr/local/bin/robson status --watch --json
Restart=always

[Install]
WantedBy=multi-user.target
```

### Cron Job (Daily Report)

```cron
# Send daily PnL report at 9 AM
0 9 * * * robson history --days 1 --json | mail -s "Daily Trading Report" user@example.com
```

### Webhook Integration

```bash
# Post status to webhook on change
robson status --json \
  | curl -X POST https://example.com/webhook \
    -H "Content-Type: application/json" \
    -d @-
```

---

## Exit Codes

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | General error |
| 2    | Invalid arguments |
| 3    | Connection error (daemon unreachable) |
| 4    | Authentication error |
| 5    | Position not found |
| 6    | Invalid state transition |
| 7    | Risk limit exceeded |
| 8    | Exchange error |

**Usage**:
```bash
robson arm BTCUSDT --strategy all-in
if [ $? -eq 7 ]; then
  echo "Risk limit exceeded, check config"
fi
```

---

## CLI Architecture

```
┌──────────────────────────────────────┐
│  robson (Bun CLI)                    │
│                                      │
│  ┌────────────────────────────────┐ │
│  │  Commander.js (CLI framework)  │ │
│  └────────────┬───────────────────┘ │
│               │                      │
│  ┌────────────▼───────────────────┐ │
│  │  API Client (HTTP/gRPC)        │ │
│  └────────────┬───────────────────┘ │
│               │                      │
│  ┌────────────▼───────────────────┐ │
│  │  Output Formatter (table/JSON) │ │
│  └────────────────────────────────┘ │
└────────────────┬─────────────────────┘
                 │ HTTP/gRPC
┌────────────────▼─────────────────────┐
│  robsond (Rust Daemon)               │
│  API Server                          │
└──────────────────────────────────────┘
```

---

## Development

### Run from Source

```bash
cd cli
bun install
bun run dev arm BTCUSDT --strategy all-in
```

### Run Tests

```bash
bun test
```

### Build

```bash
bun run build
# Output: dist/robson
```

---

## Troubleshooting

### "Cannot connect to daemon"

```bash
# Check if daemon is running
curl http://localhost:8080/health/live

# Check daemon logs
robson logs --follow
```

### "Position not found"

```bash
# Verify position ID
robson status --json | jq '.positions[].id'

# Reconcile state
robson reconcile --all
```

### "Invalid state transition"

Trying to disarm an active position:

```bash
# Wrong:
robson disarm BTCUSDT  # Error: Position is Active

# Correct:
robson panic --symbol BTCUSDT
```

---

## Next Steps

See:
- [ARCHITECTURE.md](./ARCHITECTURE.md) - System architecture
- [DOMAIN.md](./DOMAIN.md) - Domain model
- [EXECUTION-PLAN.md](./EXECUTION-PLAN.md) - Implementation roadmap

---

**Status**: Ready for implementation
**Implementation**: Bun + TypeScript, ~2000 LOC
