/**
 * Configuration management for Robson CLI.
 *
 * Config file location: ~/.robson/config.toml
 *
 * Schema:
 * ```toml
 * [scope]
 * tenant_id = "default"
 * user_id = "local"
 * profile = "default"
 * ```
 */

import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';
import * as TOML from 'smol-toml';

// =============================================================================
// Types
// =============================================================================

/**
 * Scope configuration from config file.
 */
export interface ScopeConfig {
  tenant_id?: string;
  user_id?: string;
  profile?: string;
}

/**
 * Full configuration schema.
 */
export interface RobsonConfig {
  scope?: ScopeConfig;
}

/**
 * Default values for scope (single-user alias).
 */
export const DEFAULT_SCOPE = {
  tenantId: 'default',
  userId: 'local',
  profile: 'default',
} as const;

// =============================================================================
// Config File Path
// =============================================================================

/**
 * Get the path to the config file.
 *
 * Priority:
 * 1. ROBSON_CONFIG_PATH env var (if set)
 * 2. ~/.robson/config.toml
 */
export function getConfigPath(): string {
  const envPath = process.env.ROBSON_CONFIG_PATH;
  if (envPath) {
    return envPath;
  }

  const homeDir = os.homedir();
  return path.join(homeDir, '.robson', 'config.toml');
}

/**
 * Get the config directory path.
 */
export function getConfigDir(): string {
  return path.dirname(getConfigPath());
}

// =============================================================================
// Config Loading
// =============================================================================

/**
 * Load configuration from file.
 *
 * Returns empty config if file doesn't exist.
 * Falls back to defaults if parsing fails.
 */
export function loadConfig(): RobsonConfig {
  const configPath = getConfigPath();

  if (!fs.existsSync(configPath)) {
    return {};
  }

  try {
    const content = fs.readFileSync(configPath, 'utf-8');
    const parsed = TOML.parse(content) as Record<string, unknown>;

    const config: RobsonConfig = {};

    if (parsed.scope && typeof parsed.scope === 'object') {
      const scopeData = parsed.scope as Record<string, unknown>;
      config.scope = {
        tenant_id: scopeData.tenant_id as string | undefined,
        user_id: scopeData.user_id as string | undefined,
        profile: scopeData.profile as string | undefined,
      };
    }

    return config;
  } catch (error) {
    // If config file is invalid, warn and return defaults
    const message = error instanceof Error ? error.message : String(error);
    console.error(`Warning: Failed to parse config file (${configPath}): ${message}`);
    console.error('Using default configuration.');
    return {};
  }
}

/**
 * Save configuration to file.
 *
 * Creates directory if it doesn't exist.
 */
export function saveConfig(config: RobsonConfig): void {
  const configPath = getConfigPath();
  const configDir = getConfigDir();

  // Ensure directory exists
  if (!fs.existsSync(configDir)) {
    fs.mkdirSync(configDir, { recursive: true });
  }

  // Build TOML content
  const lines: string[] = [];

  if (config.scope) {
    lines.push('[scope]');
    if (config.scope.tenant_id) {
      lines.push(`tenant_id = "${config.scope.tenant_id}"`);
    }
    if (config.scope.user_id) {
      lines.push(`user_id = "${config.scope.user_id}"`);
    }
    if (config.scope.profile) {
      lines.push(`profile = "${config.scope.profile}"`);
    }
  }

  fs.writeFileSync(configPath, lines.join('\n') + '\n', 'utf-8');
}

/**
 * Set a scope value in config.
 */
export function setScopeValue(
  key: keyof ScopeConfig,
  value: string
): void {
  const config = loadConfig();
  config.scope = config.scope || {};
  config.scope[key] = value;
  saveConfig(config);
}

// =============================================================================
// Exports
// =============================================================================

export default {
  loadConfig,
  saveConfig,
  setScopeValue,
  getConfigPath,
  getConfigDir,
  DEFAULT_SCOPE,
};
