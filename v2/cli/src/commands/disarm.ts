import { Command } from 'commander';
import { RobsonClient } from '../api/client';

export function registerDisarmCommand(program: Command) {
  program
    .command('disarm <id>')
    .description('Disarm position (cancel before entry signal received)')
    .action(async (id: string) => {
      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      console.log(`Disarming position: ${id}`);

      try {
        await client.disarm(id);
        console.log('✓ Position disarmed successfully');
        console.log('  Detector task cancelled');
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to disarm position');
        }
        process.exit(1);
      }
    });
}
