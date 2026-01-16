import { Command } from 'commander';

export function registerDisarmCommand(program: Command) {
  program
    .command('disarm <id>')
    .description('Disarm position (cancel waiting for entry signal)')
    .option('--force', 'Force disarm even if entering/active')
    .action(async (id, options) => {
      console.log(`Disarming position: ${id}`);
      console.log('Options:', options);

      // TODO: Implement API call to robsond
      // const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');
      // await client.disarm(id, options.force);
      // console.log('Position disarmed');
    });
}
