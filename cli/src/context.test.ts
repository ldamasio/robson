/**
 * Tests for context resolution and config loading.
 */

import { describe, test, expect, beforeEach, afterEach } from 'bun:test';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';
import { resolveScope, isSingleUser, formatScope, scopeKey, scopeToApiFormat, type Scope } from './context';
import { loadConfig, saveConfig, DEFAULT_SCOPE } from './config';

// =============================================================================
// Test Fixtures
// =============================================================================

const TEST_CONFIG_DIR = path.join(os.tmpdir(), 'robson-test-' + Date.now());
const TEST_CONFIG_PATH = path.join(TEST_CONFIG_DIR, 'config.toml');

function setupTestConfig(content?: string): void {
  if (!fs.existsSync(TEST_CONFIG_DIR)) {
    fs.mkdirSync(TEST_CONFIG_DIR, { recursive: true });
  }
  if (content !== undefined) {
    fs.writeFileSync(TEST_CONFIG_PATH, content, 'utf-8');
  }
}

function cleanupTestConfig(): void {
  if (fs.existsSync(TEST_CONFIG_DIR)) {
    fs.rmSync(TEST_CONFIG_DIR, { recursive: true, force: true });
  }
}

// =============================================================================
// Context Resolution Tests
// =============================================================================

describe('resolveScope', () => {
  const originalEnv = process.env.ROBSON_CONFIG_PATH;

  beforeEach(() => {
    // Reset to no config
    delete process.env.ROBSON_CONFIG_PATH;
  });

  afterEach(() => {
    // Restore env
    if (originalEnv !== undefined) {
      process.env.ROBSON_CONFIG_PATH = originalEnv;
    } else {
      delete process.env.ROBSON_CONFIG_PATH;
    }
    cleanupTestConfig();
  });

  test('returns defaults when no options and no config', () => {
    const scope = resolveScope({});
    expect(scope.tenantId).toBe('default');
    expect(scope.userId).toBe('local');
    expect(scope.profile).toBe('default');
  });

  test('profile override works with defaults for tenant/user', () => {
    const scope = resolveScope({ profile: 'paper' });
    expect(scope.tenantId).toBe('default');
    expect(scope.userId).toBe('local');
    expect(scope.profile).toBe('paper');
  });

  test('tenant override works with defaults for user/profile', () => {
    const scope = resolveScope({ tenant: 'org123' });
    expect(scope.tenantId).toBe('org123');
    expect(scope.userId).toBe('local');
    expect(scope.profile).toBe('default');
  });

  test('user override works with defaults for tenant/profile', () => {
    const scope = resolveScope({ user: 'user456' });
    expect(scope.tenantId).toBe('default');
    expect(scope.userId).toBe('user456');
    expect(scope.profile).toBe('default');
  });

  test('full multi-tenant override', () => {
    const scope = resolveScope({
      tenant: 'org123',
      user: 'user456',
      profile: 'prod',
    });
    expect(scope.tenantId).toBe('org123');
    expect(scope.userId).toBe('user456');
    expect(scope.profile).toBe('prod');
  });

  test('opts override config values', () => {
    // Setup config with non-default values
    setupTestConfig(`
[scope]
tenant_id = "config_tenant"
user_id = "config_user"
profile = "config_profile"
`);
    process.env.ROBSON_CONFIG_PATH = TEST_CONFIG_PATH;

    // Override only profile
    const scope = resolveScope({ profile: 'override_profile' });

    expect(scope.tenantId).toBe('config_tenant'); // From config
    expect(scope.userId).toBe('config_user'); // From config
    expect(scope.profile).toBe('override_profile'); // From opts
  });
});

// =============================================================================
// Scope Helper Tests
// =============================================================================

describe('isSingleUser', () => {
  test('returns true for default scope', () => {
    const scope: Scope = { tenantId: 'default', userId: 'local', profile: 'default' };
    expect(isSingleUser(scope)).toBe(true);
  });

  test('returns true for single-user with different profile', () => {
    const scope: Scope = { tenantId: 'default', userId: 'local', profile: 'paper' };
    expect(isSingleUser(scope)).toBe(true);
  });

  test('returns false for multi-tenant', () => {
    const scope: Scope = { tenantId: 'org123', userId: 'user456', profile: 'prod' };
    expect(isSingleUser(scope)).toBe(false);
  });

  test('returns false for non-default tenant', () => {
    const scope: Scope = { tenantId: 'org123', userId: 'local', profile: 'default' };
    expect(isSingleUser(scope)).toBe(false);
  });
});

describe('formatScope', () => {
  test('formats single-user scope', () => {
    const scope: Scope = { tenantId: 'default', userId: 'local', profile: 'default' };
    expect(formatScope(scope)).toBe('single-user(profile=default)');
  });

  test('formats single-user with custom profile', () => {
    const scope: Scope = { tenantId: 'default', userId: 'local', profile: 'paper' };
    expect(formatScope(scope)).toBe('single-user(profile=paper)');
  });

  test('formats multi-tenant scope', () => {
    const scope: Scope = { tenantId: 'org123', userId: 'user456', profile: 'prod' };
    expect(formatScope(scope)).toBe('tenant=org123/user=user456/profile=prod');
  });
});

describe('scopeKey', () => {
  test('generates pipe-separated key', () => {
    const scope: Scope = { tenantId: 'a', userId: 'b', profile: 'c' };
    expect(scopeKey(scope)).toBe('a|b|c');
  });

  test('generates key for default scope', () => {
    const scope: Scope = { tenantId: 'default', userId: 'local', profile: 'default' };
    expect(scopeKey(scope)).toBe('default|local|default');
  });
});

describe('scopeToApiFormat', () => {
  test('converts to snake_case format', () => {
    const scope: Scope = { tenantId: 'org123', userId: 'user456', profile: 'prod' };
    const api = scopeToApiFormat(scope);
    expect(api).toEqual({
      tenant_id: 'org123',
      user_id: 'user456',
      profile: 'prod',
    });
  });
});

// =============================================================================
// Config File Tests
// =============================================================================

describe('loadConfig', () => {
  const originalEnv = process.env.ROBSON_CONFIG_PATH;

  beforeEach(() => {
    delete process.env.ROBSON_CONFIG_PATH;
  });

  afterEach(() => {
    if (originalEnv !== undefined) {
      process.env.ROBSON_CONFIG_PATH = originalEnv;
    } else {
      delete process.env.ROBSON_CONFIG_PATH;
    }
    cleanupTestConfig();
  });

  test('returns empty config when file does not exist', () => {
    process.env.ROBSON_CONFIG_PATH = '/nonexistent/path/config.toml';
    const config = loadConfig();
    expect(config).toEqual({});
  });

  test('loads scope from config file', () => {
    setupTestConfig(`
[scope]
tenant_id = "org123"
user_id = "user456"
profile = "prod"
`);
    process.env.ROBSON_CONFIG_PATH = TEST_CONFIG_PATH;

    const config = loadConfig();
    expect(config.scope).toBeDefined();
    expect(config.scope?.tenant_id).toBe('org123');
    expect(config.scope?.user_id).toBe('user456');
    expect(config.scope?.profile).toBe('prod');
  });

  test('handles partial scope config', () => {
    setupTestConfig(`
[scope]
profile = "paper"
`);
    process.env.ROBSON_CONFIG_PATH = TEST_CONFIG_PATH;

    const config = loadConfig();
    expect(config.scope?.profile).toBe('paper');
    expect(config.scope?.tenant_id).toBeUndefined();
    expect(config.scope?.user_id).toBeUndefined();
  });
});

describe('saveConfig', () => {
  const originalEnv = process.env.ROBSON_CONFIG_PATH;

  beforeEach(() => {
    delete process.env.ROBSON_CONFIG_PATH;
    setupTestConfig();
  });

  afterEach(() => {
    if (originalEnv !== undefined) {
      process.env.ROBSON_CONFIG_PATH = originalEnv;
    } else {
      delete process.env.ROBSON_CONFIG_PATH;
    }
    cleanupTestConfig();
  });

  test('creates config file with scope', () => {
    process.env.ROBSON_CONFIG_PATH = TEST_CONFIG_PATH;

    saveConfig({
      scope: {
        tenant_id: 'org123',
        user_id: 'user456',
        profile: 'prod',
      },
    });

    const content = fs.readFileSync(TEST_CONFIG_PATH, 'utf-8');
    expect(content).toContain('[scope]');
    expect(content).toContain('tenant_id = "org123"');
    expect(content).toContain('user_id = "user456"');
    expect(content).toContain('profile = "prod"');
  });
});
