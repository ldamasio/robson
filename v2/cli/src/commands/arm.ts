import { Command } from 'commander';

export function registerArmCommand(program: Command) {
  program
    .command('arm <symbol>')
    .description('Arm position for symbol')
    .option('--strategy <name>', 'Strategy name (required)', 'all-in')
    .option('--capital <amount>', 'Capital to allocate')
    .option('--leverage <n>', 'Leverage multiplier (1-10)', '3')
    .option('--dry-run', 'Simulate without real orders')
    .action(async (symbol, options) => {
      console.log(`Arming ${symbol} with strategy "${options.strategy}"`);
      console.log('Options:', options);

      // TODO: Implement API call to robsond
      // const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');
      // const result = await client.arm({ symbol, ...options });
      // console.log('Position armed:', result.position_id);
    });
}
