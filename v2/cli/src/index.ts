#!/usr/bin/env bun

import { Command } from 'commander';
import { registerStatusCommand } from './commands/status';
import { registerArmCommand } from './commands/arm';
import { registerDisarmCommand } from './commands/disarm';
import { registerPanicCommand } from './commands/panic';

const program = new Command();

program
  .name('robson')
  .description('Robson v2 - Trading automation platform')
  .version('2.0.0-alpha');

// Register commands
registerStatusCommand(program);
registerArmCommand(program);
registerDisarmCommand(program);
registerPanicCommand(program);

program.parse();
