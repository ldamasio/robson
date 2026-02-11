import { Command } from 'commander';
import { RobsonClient } from '../api/client';
import { addScopeFlags, withScope } from '../flags';
import { formatScope, scopeToApiFormat } from '../context';
import type { SafetyTestResponse } from '../types';

interface SafetyTestOptions {
  json?: boolean;
  verbose?: boolean;
  tenant?: string;
  user?: string;
  profile?: string;
}

export function registerSafetyTestCommand(program: Command) {
  const cmd = program
    .command('safety-test')
    .description('Test safety net connection and show current positions');

  // Add scope flags (this command needs credentials to test Binance connection)
  addScopeFlags(cmd)
    .option('--json', 'Output as JSON')
    .option('--verbose', 'Show detailed output including scope')
    .action(withScope<SafetyTestOptions>(async (scope, options) => {
      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      try {
        // Show scope only in verbose mode
        if (options.verbose && !options.json) {
          console.log(`Scope: ${formatScope(scope)}`);
          console.log();
        }

        // Pass scope to backend (backend may ignore if not yet supported)
        const result = await client.safetyTest(scopeToApiFormat(scope));

        if (options.json) {
          console.log(JSON.stringify(result, null, 2));
          return;
        }

        printTestResult(result);
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to test safety net. Is robsond running?');
        }
        process.exit(1);
      }
    }));
}

function printTestResult(result: SafetyTestResponse) {
  console.log('╔══════════════════════════════════════════════════════════════╗');
  console.log('║                    SAFETY NET TEST                            ║');
  console.log('╠══════════════════════════════════════════════════════════════╣');

  if (result.success) {
    console.log('║ Status:     ✅ SUCCESS');
  } else {
    console.log('║ Status:     ❌ FAILED');
  }

  console.log('╚══════════════════════════════════════════════════════════════╝');
  console.log();
  console.log(result.message);
  console.log();

  if (result.positions && result.positions.length > 0) {
    console.log('📊 POSITIONS FROM BINANCE');
    console.log();
    console.log('Symbol    | Side  | Quantity      | Entry        | Calculated Stop');
    console.log('----------|-------|---------------|--------------|----------------');

    for (const pos of result.positions) {
      const symbol = pos.symbol.padEnd(9);
      const side = pos.side.toUpperCase().padEnd(5);
      const qty = pos.quantity.toFixed(6).padStart(13);
      const entry = pos.entry_price.toFixed(2).padStart(12);
      const stop = pos.calculated_stop.toFixed(2).padStart(14);

      console.log(`${symbol} | ${side} | ${qty} | ${entry} | ${stop}`);
    }

    console.log();
    console.log('💡 The "Calculated Stop" shows what the safety net would use (2% from entry).');
  }
}
