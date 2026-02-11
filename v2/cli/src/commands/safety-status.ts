import { Command } from 'commander';
import { RobsonClient } from '../api/client';
import type { SafetyStatusResponse } from '../types';

export function registerSafetyStatusCommand(program: Command) {
  program
    .command('safety-status')
    .description('Show safety net status (rogue position monitoring)')
    .option('--json', 'Output as JSON')
    .action(async (options) => {
      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      try {
        const status = await client.safetyStatus();

        if (options.json) {
          console.log(JSON.stringify(status, null, 2));
          return;
        }

        printSafetyStatus(status);
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to get safety status. Is robsond running?');
        }
        process.exit(1);
      }
    });
}

function printSafetyStatus(status: SafetyStatusResponse) {
  console.log('╔══════════════════════════════════════════════════════════════╗');
  console.log('║                    SAFETY NET STATUS                          ║');
  console.log('╠══════════════════════════════════════════════════════════════╣');
  console.log(`║ Status:     ${status.enabled ? '✅ ENABLED' : '❌ DISABLED'}`);
  console.log(`║ Symbols:    ${status.symbols.join(', ') || 'None'}`);
  console.log(`║ Interval:   ${status.poll_interval_secs}s`);
  console.log(`║ Pending:    ${status.pending_executions} execution(s)`);
  console.log('╚══════════════════════════════════════════════════════════════╝');
  console.log();

  if (status.tracked_positions.length === 0) {
    console.log('No rogue positions detected.');
    console.log();
    console.log('💡 The safety net monitors for positions opened outside Robson v2.');
    console.log('   If you open a position manually on Binance, it will appear here.');
    return;
  }

  console.log('📊 DETECTED ROGUE POSITIONS');
  console.log();
  console.log('ID                        | Symbol    | Side  | Entry      | Stop       | Stop %');
  console.log('--------------------------|-----------|-------|------------|------------|-------');

  for (const pos of status.tracked_positions) {
    const id = pos.id.substring(0, 25);
    const symbol = pos.symbol.padEnd(9);
    const side = pos.side.toUpperCase().padEnd(5);
    const entry = pos.entry_price.toFixed(2).padStart(10);
    const stop = pos.stop_price.toFixed(2).padStart(10);
    const stopPct = `${pos.stop_distance_pct.toFixed(2)}%`.padStart(5);

    console.log(`${id} | ${symbol} | ${side} | ${entry} | ${stop} | ${stopPct}`);
  }

  console.log();
  console.log('⚠️  These positions have calculated stop losses at 2% from entry.');
  console.log('   If price hits the stop, the safety net will execute a market order to exit.');
}
