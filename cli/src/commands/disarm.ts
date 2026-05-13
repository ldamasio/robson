import { Command } from 'commander';
import { RobsonClient } from '../api/client';

export function registerDisarmCommand(program: Command) {
  program
    .command('disarm <id>')
    .description('Cancel an Armed position or manually exit an Active position')
    .action(async (id: string) => {
      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      console.log(`Cancelling or closing position: ${id}`);

      try {
        await client.disarm(id);
        console.log('✓ Position cancelled or closed successfully');
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to cancel or close position');
        }
        process.exit(1);
      }
    });
}
