import { Command } from 'commander';

export function registerStatusCommand(program: Command) {
  program
    .command('status')
    .description('Show position status')
    .option('--json', 'Output as JSON')
    .option('--symbol <symbol>', 'Filter by symbol')
    .option('--state <state>', 'Filter by state (armed/active/closed)')
    .option('--watch', 'Continuous monitoring (refresh every 2s)')
    .action(async (options) => {
      console.log('Status command (not implemented yet)');
      console.log('Options:', options);

      // TODO: Implement API call to robsond
      // const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');
      // const status = await client.status();
      // if (options.json) {
      //   console.log(JSON.stringify(status, null, 2));
      // } else {
      //   printTable(status);
      // }
    });
}
