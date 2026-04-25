import { Command } from 'commander';
import { RobsonClient } from '../api/client';
import type { PositionSummary } from '../types';

export function registerStatusCommand(program: Command) {
  program
    .command('status')
    .description('Show position status')
    .option('--json', 'Output as JSON')
    .action(async (options) => {
      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      try {
        const status = await client.status();

        if (options.json) {
          console.log(JSON.stringify(status, null, 2));
          return;
        }

        console.log(`Active Positions: ${status.active_positions}`);
        console.log();

        if (status.positions.length === 0) {
          console.log('No positions.');
          return;
        }

        // Print table header
        console.log('ID                                   | Symbol    | Side  | State        | Entry    | Stop     | PnL');
        console.log('-------------------------------------|-----------|-------|--------------|----------|----------|--------');

        // Print each position
        for (const pos of status.positions) {
          const id = pos.id.substring(0, 36);
          const symbol = pos.symbol.padEnd(9);
          const side = pos.side.padEnd(5);
          const state = pos.state.substring(0, 12).padEnd(12);
          const entry = pos.entry_price ? pos.entry_price.toFixed(2).padStart(8) : '       -';
          const stop = pos.trailing_stop ? pos.trailing_stop.toFixed(2).padStart(8) : '       -';
          const pnl = pos.pnl !== undefined ? pos.pnl.toFixed(2).padStart(8) : '       -';

          console.log(`${id} | ${symbol} | ${side} | ${state} | ${entry} | ${stop} | ${pnl}`);
        }
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to get status. Is robsond running?');
        }
        process.exit(1);
      }
    });
}
