/**
 * Scope-related CLI flags for Commander.js.
 *
 * These flags should ONLY be added to commands that need identity scope:
 * - `robson credentials set` ✓
 * - `robson credentials revoke` ✓
 * - `robson safety test` ✓
 * - Commands that arm/execute automation ✓
 *
 * NOT added to:
 * - `robson safety status` ✗ (observability only, no credentials)
 * - `robson status` ✗ (status only)
 */

import { Command } from 'commander';
import { resolveScope, extractScopeOptions, type Scope } from './context.js';

// =============================================================================
// Types
// =============================================================================

/**
 * Options that include scope flags.
 */
export interface ScopeFlagOptions {
  tenant?: string;
  user?: string;
  profile?: string;
}

/**
 * Command action handler with scope.
 */
export type ScopedActionHandler<T extends ScopeFlagOptions> = (
  scope: Scope,
  options: T
) => Promise<void> | void;

// =============================================================================
// Flag Definitions
// =============================================================================

/**
 * Add scope flags to a Commander command.
 *
 * Usage:
 * ```typescript
 * program
 *   .command('credentials set')
 *   .description('Set exchange credentials')
 *   .addScopeFlags()
 *   .requiredOption('--exchange <name>', 'Exchange name (binance)')
 *   .action(withScope(async (scope, options) => {
 *     // scope is resolved
 *     // options has all flags
 *   }));
 * ```
 */
export function addScopeFlags(command: Command): Command {
  return command
    .option('--tenant <id>', 'Tenant ID (advanced, for multi-tenant setups)')
    .option('--user <id>', 'User ID (advanced, for multi-tenant setups)')
    .option('--profile <name>', 'Credential profile (default, paper, prod)');
}

/**
 * Helper to extend Command with addScopeFlags method.
 */
declare module 'commander' {
  interface Command {
    addScopeFlags(): this;
  }
}

// Extend Command prototype
Command.prototype.addScopeFlags = function (): Command {
  return addScopeFlags(this);
};

// =============================================================================
// Action Wrapper
// =============================================================================

/**
 * Wrap a command action handler to automatically resolve scope.
 *
 * This ensures scope is resolved from flags > config > defaults
 * before the action handler is called.
 *
 * @example
 * ```typescript
 * program
 *   .command('credentials set')
 *   .addScopeFlags()
 *   .requiredOption('--exchange <name>', 'Exchange')
 *   .action(withScope(async (scope, options) => {
 *     console.log('Scope:', scope);
 *     console.log('Exchange:', options.exchange);
 *   }));
 * ```
 */
export function withScope<T extends ScopeFlagOptions>(
  handler: ScopedActionHandler<T>
): (options: T) => Promise<void> {
  return async (options: T) => {
    const scopeOpts = extractScopeOptions(options);
    const scope = resolveScope(scopeOpts);
    await handler(scope, options);
  };
}

// =============================================================================
// Exports
// =============================================================================

export default {
  addScopeFlags,
  withScope,
};
