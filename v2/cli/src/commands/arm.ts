import { Command } from 'commander';
import { RobsonClient } from '../api/client';

export function registerArmCommand(program: Command) {
  program
    .command('arm <symbol>')
    .description('Arm position for a symbol (creates detector, waits for entry signal)')
    .requiredOption('--capital <amount>', 'Capital to allocate (e.g., 1000)')
    .option('--risk <percent>', 'Risk per trade in percent (default: 1)', '1')
    .option('--side <side>', 'Position side: long or short (default: long)', 'long')
    .action(async (symbol: string, options) => {
      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      const capital = parseFloat(options.capital);
      const riskPercent = parseFloat(options.risk);
      const side = options.side.toLowerCase() as 'long' | 'short';

      if (isNaN(capital) || capital <= 0) {
        console.error('Error: --capital must be a positive number');
        process.exit(1);
      }

      if (isNaN(riskPercent) || riskPercent <= 0 || riskPercent > 10) {
        console.error('Error: --risk must be between 0 and 10');
        process.exit(1);
      }

      if (side !== 'long' && side !== 'short') {
        console.error('Error: --side must be "long" or "short"');
        process.exit(1);
      }

      console.log(`Arming ${symbol.toUpperCase()} ${side.toUpperCase()}`);
      console.log(`  Capital: ${capital} USDT`);
      console.log(`  Risk: ${riskPercent}%`);
      console.log(`  Leverage: 10x (fixed)`);
      console.log();

      try {
        const result = await client.arm({
          symbol: symbol.toUpperCase(),
          side,
          capital,
          risk_percent: riskPercent,
        });

        console.log('✓ Position armed successfully');
        console.log(`  ID: ${result.position_id}`);
        console.log(`  Symbol: ${result.symbol}`);
        console.log(`  Side: ${result.side}`);
        console.log(`  State: ${result.state}`);
        console.log();
        console.log('Detector is now monitoring for entry signal...');
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to arm position. Is robsond running?');
        }
        process.exit(1);
      }
    });
}
