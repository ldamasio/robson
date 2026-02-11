/**
 * Credentials command for managing exchange API credentials.
 *
 * Commands:
 * - `robson credentials set` - Store encrypted credentials
 * - `robson credentials list` - List stored credentials (metadata only, no secrets)
 * - `robson credentials revoke` - Revoke credentials
 *
 * Security:
 * - API Secret is prompted with hidden input (no echo)
 * - Never logs or prints secrets
 * - Only metadata is shown in output
 */

import { Command } from 'commander';
import * as readline from 'readline';
import { RobsonClient } from '../api/client';
import { addScopeFlags, withScope } from '../flags';
import { formatScope, resolveScope, scopeToApiFormat } from '../context';

// =============================================================================
// Types
// =============================================================================

interface CredentialsSetOptions {
  exchange?: string;
  label?: string;
  verbose?: boolean;
  tenant?: string;
  user?: string;
  profile?: string;
}

interface CredentialsListOptions {
  tenant?: string;
  user?: string;
  json?: boolean;
  verbose?: boolean;
}

interface CredentialsRevokeOptions {
  exchange?: string;
  verbose?: boolean;
  tenant?: string;
  user?: string;
  profile?: string;
  reason?: string;
}

// =============================================================================
// Hidden Input Helper
// =============================================================================

/**
 * Prompt for hidden input (no echo).
 *
 * Uses raw stdin to disable echo. Falls back to regular input if TTY is not available.
 */
async function promptHidden(promptText: string): Promise<string> {
  return new Promise((resolve) => {
    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout,
    });

    // Hide input
    const stdin = process.stdin;
    const wasRaw = stdin.isTTY ? stdin.isRaw : false;

    if (stdin.isTTY) {
      stdin.setRawMode(true);
    }

    process.stdout.write(promptText);

    let input = '';

    const onData = (char: Buffer) => {
      const c = char.toString('utf8');

      switch (c) {
        case '\n':
        case '\r':
        case '\u0004': // Ctrl+D
          if (stdin.isTTY) {
            stdin.setRawMode(wasRaw);
          }
          stdin.removeListener('data', onData);
          rl.close();
          process.stdout.write('\n');
          resolve(input);
          break;
        case '\u0003': // Ctrl+C
          process.exit();
          break;
        case '\u007F': // Backspace
          input = input.slice(0, -1);
          break;
        default:
          input += c;
          break;
      }
    };

    stdin.on('data', onData);
  });
}

/**
 * Prompt for visible input.
 */
async function promptVisible(promptText: string): Promise<string> {
  return new Promise((resolve) => {
    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stdout,
    });

    rl.question(promptText, (answer) => {
      rl.close();
      resolve(answer.trim());
    });
  });
}

// =============================================================================
// Register Commands
// =============================================================================

export function registerCredentialsCommand(program: Command) {
  const credentials = program
    .command('credentials')
    .description('Manage exchange API credentials');

  // credentials set
  registerCredentialsSetCommand(credentials);

  // credentials list
  registerCredentialsListCommand(credentials);

  // credentials revoke
  registerCredentialsRevokeCommand(credentials);
}

// =============================================================================
// credentials set
// =============================================================================

function registerCredentialsSetCommand(program: Command) {
  const cmd = program
    .command('set')
    .description('Store exchange API credentials (encrypted)');

  addScopeFlags(cmd)
    .option('--exchange <name>', 'Exchange name (default: binance)')
    .option('--label <text>', 'Optional label for this credential')
    .option('--verbose', 'Show detailed output including scope')
    .action(withScope<CredentialsSetOptions>(async (scope, options) => {
      const exchange = options.exchange || 'binance';

      // Show scope only in verbose mode
      if (options.verbose) {
        console.log(`Storing credentials for exchange=${exchange}`);
        console.log(`Scope: ${formatScope(scope)}`);
        console.log();
      }

      // Prompt for API Key (visible)
      const apiKey = await promptVisible('API Key: ');
      if (!apiKey) {
        console.error('Error: API Key is required');
        process.exit(1);
      }

      // Prompt for API Secret (hidden)
      const apiSecret = await promptHidden('API Secret: ');
      if (!apiSecret) {
        console.error('Error: API Secret is required');
        process.exit(1);
      }

      // Validate inputs (basic)
      if (apiKey.length < 10) {
        console.error('Error: API Key seems too short');
        process.exit(1);
      }

      if (apiSecret.length < 10) {
        console.error('Error: API Secret seems too short');
        process.exit(1);
      }

      // Call API to store credentials
      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      try {
        await client.setCredentials({
          tenant_id: scope.tenantId,
          user_id: scope.userId,
          profile: scope.profile,
          exchange,
          api_key: apiKey,
          api_secret: apiSecret,
          label: options.label,
        });

        console.log();
        console.log('✅ Credentials saved successfully');
        console.log();
        console.log('Metadata:');
        console.log(`  Exchange: ${exchange}`);
        console.log(`  Tenant:   ${scope.tenantId}`);
        console.log(`  User:     ${scope.userId}`);
        console.log(`  Profile:  ${scope.profile}`);
        if (options.label) {
          console.log(`  Label:    ${options.label}`);
        }
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to store credentials. Is robsond running?');
        }
        process.exit(1);
      }
    }));
}

// =============================================================================
// credentials list
// =============================================================================

function registerCredentialsListCommand(program: Command) {
  program
    .command('list')
    .description('List stored credentials (metadata only, no secrets)')
    .option('--tenant <id>', 'Filter by tenant (default: from config or "default")')
    .option('--user <id>', 'Filter by user (default: from config or "local")')
    .option('--json', 'Output as JSON')
    .option('--verbose', 'Show detailed output including resolved scope')
    .action(async (options: CredentialsListOptions) => {
      // Resolve scope from flags/config/defaults for filtering
      const scope = resolveScope({
        tenant: options.tenant,
        user: options.user,
        // profile not used for list filtering
      });

      if (options.verbose && !options.json) {
        console.log(`Scope: ${formatScope(scope)}`);
        console.log();
      }

      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      try {
        const credentials = await client.listCredentials({
          tenant_id: scope.tenantId,
          user_id: scope.userId,
        });

        if (options.json) {
          console.log(JSON.stringify(credentials, null, 2));
          return;
        }

        if (credentials.length === 0) {
          console.log('No credentials stored.');
          console.log();
          console.log('Use `robson credentials set` to add credentials.');
          return;
        }

        console.log('📋 STORED CREDENTIALS');
        console.log();
        console.log('Exchange   | Tenant    | User      | Profile   | Status  | Label');
        console.log('-----------|-----------|-----------|-----------|---------|-------');

        for (const cred of credentials) {
          const exchange = (cred.exchange || 'binance').padEnd(10);
          const tenant = (cred.tenant_id || 'default').substring(0, 9).padEnd(9);
          const user = (cred.user_id || 'local').substring(0, 9).padEnd(9);
          const profile = (cred.profile || 'default').padEnd(9);
          const status = cred.status === 'active' ? '✅ active' : '❌ revoked';

          console.log(`${exchange} | ${tenant} | ${user} | ${profile} | ${status} | ${cred.label || '-'}`);
        }

        console.log();
        console.log('💡 Only metadata is shown. Secrets are never exposed.');
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to list credentials. Is robsond running?');
        }
        process.exit(1);
      }
    });
}

// =============================================================================
// credentials revoke
// =============================================================================

function registerCredentialsRevokeCommand(program: Command) {
  const cmd = program
    .command('revoke')
    .description('Revoke stored credentials');

  addScopeFlags(cmd)
    .option('--exchange <name>', 'Exchange name (default: binance)')
    .option('--reason <text>', 'Reason for revocation')
    .option('--verbose', 'Show detailed output including scope')
    .action(withScope<CredentialsRevokeOptions>(async (scope, options) => {
      const exchange = options.exchange || 'binance';
      const reason = options.reason || 'User requested';

      // Show scope only in verbose mode
      if (options.verbose) {
        console.log(`Revoking credentials for exchange=${exchange}`);
        console.log(`Scope: ${formatScope(scope)}`);
        console.log();
      }

      const client = new RobsonClient(process.env.ROBSON_DAEMON_URL || 'http://localhost:8080');

      try {
        await client.revokeCredentials({
          tenant_id: scope.tenantId,
          user_id: scope.userId,
          profile: scope.profile,
          exchange,
          reason,
        });

        console.log('✅ Credentials revoked');
        console.log();
        console.log('Metadata:');
        console.log(`  Exchange: ${exchange}`);
        console.log(`  Tenant:   ${scope.tenantId}`);
        console.log(`  User:     ${scope.userId}`);
        console.log(`  Profile:  ${scope.profile}`);
        console.log(`  Reason:   ${reason}`);
      } catch (error) {
        if (error instanceof Error) {
          console.error(`Error: ${error.message}`);
        } else {
          console.error('Failed to revoke credentials. Is robsond running?');
        }
        process.exit(1);
      }
    }));
}

// =============================================================================
// Exports
// =============================================================================

export default {
  registerCredentialsCommand,
};
