import { Command } from 'commander';
import { RobsonClient } from '../api/client';

export function registerPanicCommand(program: Command) {
  program
    .command('panic')
    .description('Emergency close all positions (market orders)')
    .option('--confirm', 'Skip confirmation prompt')
    .action(async (options) => {
      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      console.log('⚠️  PANIC MODE');

      if (!options.confirm) {
        console.log();
        console.log('This will close ALL active positions immediately.');
        console.log('Use --confirm to execute.');
        return;
      }

      try {
        const result = await client.panic();

        console.log();
        console.log(`✓ Panic executed: ${result.count} position(s) closed`);

        if (result.closed_positions.length > 0) {
          console.log();
          console.log('Closed positions:');
          for (const id of result.closed_positions) {
            console.log(`  - ${id}`);
          }
        }
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to execute panic');
        }
        process.exit(1);
      }
    });
}
