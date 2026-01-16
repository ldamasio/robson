import { Command } from 'commander';

export function registerPanicCommand(program: Command) {
  program
    .command('panic')
    .description('Emergency close all positions (market orders)')
    .option('--symbol <symbol>', 'Only close positions for this symbol')
    .option('--confirm', 'Skip confirmation prompt')
    .option('--dry-run', 'Simulate without real orders')
    .action(async (options) => {
      console.log('⚠️  PANIC MODE');
      console.log('Options:', options);

      if (!options.confirm) {
        console.log('This will close ALL active positions.');
        console.log('Use --confirm to execute.');
        return;
      }

      // TODO: Implement API call to robsond
      // const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');
      // await client.panic(options);
      // console.log('All positions closed');
    });
}
