/**
 * Identity Context Resolution for Robson CLI.
 *
 * Provides explicit context resolution for all operations that need scope.
 *
 * Resolution order:
 * 1. CLI flags (highest priority)
 * 2. Config file (~/.robson/config.toml)
 * 3. Defaults (lowest priority)
 *
 * Single-user mode is an alias: tenant_id="default", user_id="local", profile="default"
 */

import { loadConfig, DEFAULT_SCOPE } from './config.js';

// =============================================================================
// Types
// =============================================================================

/**
 * Resolved identity scope.
 *
 * All fields are guaranteed to be set (no undefined).
 */
export interface Scope {
  /** Tenant identifier (default: "default") */
  tenantId: string;
  /** User identifier within tenant (default: "local") */
  userId: string;
  /** Credential/runtime profile (default: "default") */
  profile: string;
}

/**
 * Options for resolving scope.
 *
 * All fields are optional - missing fields fall back to config or defaults.
 */
export interface ScopeOptions {
  /** Override tenant ID */
  tenant?: string;
  /** Override user ID */
  user?: string;
  /** Override profile */
  profile?: string;
}

// =============================================================================
// Scope Resolution
// =============================================================================

/**
 * Resolve identity scope from CLI options, config file, and defaults.
 *
 * Priority: opts > config > defaults
 *
 * @param opts - CLI options (may be partial)
 * @returns Fully resolved scope (no undefined values)
 *
 * @example
 * ```typescript
 * // No options -> defaults
 * resolveScope({}) // { tenantId: "default", userId: "local", profile: "default" }
 *
 * // With profile override
 * resolveScope({ profile: "paper" }) // { tenantId: "default", userId: "local", profile: "paper" }
 *
 * // Full multi-tenant
 * resolveScope({ tenant: "org123", user: "user456", profile: "prod" })
 * // { tenantId: "org123", userId: "user456", profile: "prod" }
 * ```
 */
export function resolveScope(opts: ScopeOptions = {}): Scope {
  // Load config (may be empty if file doesn't exist)
  const config = loadConfig();

  // Resolution: opts > config > defaults
  return {
    tenantId: opts.tenant ?? config.scope?.tenant_id ?? DEFAULT_SCOPE.tenantId,
    userId: opts.user ?? config.scope?.user_id ?? DEFAULT_SCOPE.userId,
    profile: opts.profile ?? config.scope?.profile ?? DEFAULT_SCOPE.profile,
  };
}

/**
 * Check if scope is single-user mode (default/local).
 */
export function isSingleUser(scope: Scope): boolean {
  return (
    scope.tenantId === DEFAULT_SCOPE.tenantId &&
    scope.userId === DEFAULT_SCOPE.userId
  );
}

/**
 * Format scope for display.
 */
export function formatScope(scope: Scope): string {
  if (isSingleUser(scope)) {
    return `single-user(profile=${scope.profile})`;
  }
  return `tenant=${scope.tenantId}/user=${scope.userId}/profile=${scope.profile}`;
}

/**
 * Create a scope key for caching/indexing.
 *
 * Format: "{tenantId}|{userId}|{profile}"
 */
export function scopeKey(scope: Scope): string {
  return `${scope.tenantId}|${scope.userId}|${scope.profile}`;
}

/**
 * Scope to plain object (for API calls).
 */
export function scopeToApiFormat(scope: Scope): {
  tenant_id: string;
  user_id: string;
  profile: string;
} {
  return {
    tenant_id: scope.tenantId,
    user_id: scope.userId,
    profile: scope.profile,
  };
}

// =============================================================================
// CLI Helpers
// =============================================================================

/**
 * Common scope options for Commander.js commands.
 *
 * Usage:
 * ```typescript
 * program
 *   .command('credentials set')
 *   .option('--tenant <id>', 'Tenant ID (advanced)')
 *   .option('--user <id>', 'User ID (advanced)')
 *   .option('--profile <name>', 'Credential profile (default, paper, prod)')
 *   .action(async (options) => {
 *     const scope = resolveScope({
 *       tenant: options.tenant,
 *       user: options.user,
 *       profile: options.profile,
 *     });
 *   });
 * ```
 */
export const SCOPE_OPTIONS = {
  tenant: {
    description: 'Tenant ID (advanced, for multi-tenant setups)',
    default: undefined,
  },
  user: {
    description: 'User ID (advanced, for multi-tenant setups)',
    default: undefined,
  },
  profile: {
    description: 'Credential profile (default, paper, prod)',
    default: undefined,
  },
} as const;

/**
 * Extract scope options from Commander.js options object.
 */
export function extractScopeOptions(options: Record<string, unknown>): ScopeOptions {
  return {
    tenant: options.tenant as string | undefined,
    user: options.user as string | undefined,
    profile: options.profile as string | undefined,
  };
}

// =============================================================================
// Exports
// =============================================================================

export default {
  resolveScope,
  isSingleUser,
  formatScope,
  scopeKey,
  scopeToApiFormat,
  SCOPE_OPTIONS,
  extractScopeOptions,
  DEFAULT_SCOPE,
};
