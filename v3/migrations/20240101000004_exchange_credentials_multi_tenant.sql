-- Migration 005: Exchange Credentials (Multi-Tenant)
-- Implements secure storage for exchange API credentials with tenant isolation
--
-- Security Model:
-- - Credentials are encrypted with AES-256-GCM
-- - AAD (Additional Authenticated Data) includes tenant_id + user_id + exchange + profile
-- - Each credential is isolated by (tenant_id, user_id, exchange, profile)
-- - Single-user mode uses tenant_id='default', user_id='local'

BEGIN;

-- =============================================================================
-- EXCHANGE CREDENTIALS (Multi-Tenant)
-- =============================================================================

CREATE TABLE exchange_credentials (
    -- Identity (composite key for multi-tenant isolation)
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    exchange VARCHAR(20) NOT NULL CHECK (exchange IN ('binance', 'binance_testnet')),
    profile VARCHAR(20) NOT NULL DEFAULT 'default' CHECK (profile IN ('default', 'paper', 'prod')),

    -- Encrypted Credentials
    api_key_ciphertext BYTEA NOT NULL,
    api_secret_ciphertext BYTEA NOT NULL,
    api_key_nonce BYTEA NOT NULL,          -- AES-GCM nonce (12 bytes)
    api_secret_nonce BYTEA NOT NULL,       -- AES-GCM nonce (12 bytes)

    -- Encryption Metadata
    crypto_version INT NOT NULL DEFAULT 1, -- Allows key rotation
    key_id TEXT NOT NULL,                  -- References which encryption key was used

    -- Status
    status VARCHAR(20) NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked', 'expired')),
    revoked_at TIMESTAMPTZ,
    revoke_reason TEXT,

    -- Metadata
    label TEXT,                            -- User-friendly name (e.g., "Main Binance Account")
    last_used_at TIMESTAMPTZ,
    permissions TEXT[] DEFAULT ARRAY[]::TEXT[], -- e.g., ['read', 'trade', 'withdraw']

    -- Audit
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Constraints
    CONSTRAINT pk_exchange_credentials PRIMARY KEY (tenant_id, user_id, exchange, profile),
    CONSTRAINT uk_exchange_credentials_key UNIQUE (tenant_id, user_id, exchange, profile)
);

-- Indexes for common queries
CREATE INDEX idx_exchange_credentials_tenant ON exchange_credentials(tenant_id);
CREATE INDEX idx_exchange_credentials_tenant_user ON exchange_credentials(tenant_id, user_id);
CREATE INDEX idx_exchange_credentials_status ON exchange_credentials(status) WHERE status = 'active';
CREATE INDEX idx_exchange_credentials_key_id ON exchange_credentials(key_id);

-- Comments for documentation
COMMENT ON TABLE exchange_credentials IS 'Multi-tenant encrypted storage for exchange API credentials';
COMMENT ON COLUMN exchange_credentials.tenant_id IS 'Tenant identifier (use "default" for single-user mode)';
COMMENT ON COLUMN exchange_credentials.user_id IS 'User identifier within tenant (use "local" for single-user mode)';
COMMENT ON COLUMN exchange_credentials.exchange IS 'Exchange name (binance, binance_testnet)';
COMMENT ON COLUMN exchange_credentials.profile IS 'Profile name (default, paper, prod)';
COMMENT ON COLUMN exchange_credentials.api_key_ciphertext IS 'AES-256-GCM encrypted API key';
COMMENT ON COLUMN exchange_credentials.api_secret_ciphertext IS 'AES-256-GCM encrypted API secret';
COMMENT ON COLUMN exchange_credentials.api_key_nonce IS '12-byte nonce for API key encryption';
COMMENT ON COLUMN exchange_credentials.api_secret_nonce IS '12-byte nonce for API secret encryption';
COMMENT ON COLUMN exchange_credentials.crypto_version IS 'Encryption version for future key rotation';
COMMENT ON COLUMN exchange_credentials.key_id IS 'Identifier of the encryption key used';
COMMENT ON COLUMN exchange_credentials.status IS 'Credential status: active, revoked, expired';
COMMENT ON COLUMN exchange_credentials.permissions IS 'Granted permissions: read, trade, withdraw';

-- =============================================================================
-- ENCRYPTION KEYS (Key Management)
-- =============================================================================

CREATE TABLE encryption_keys (
    key_id TEXT PRIMARY KEY,
    key_ciphertext BYTEA,                  -- Encrypted master key (if using KMS)
    key_version INT NOT NULL DEFAULT 1,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    rotated_from_key_id TEXT,

    CONSTRAINT fk_encryption_keys_rotated_from
        FOREIGN KEY (rotated_from_key_id)
        REFERENCES encryption_keys(key_id)
        ON DELETE SET NULL
);

COMMENT ON TABLE encryption_keys IS 'Encryption key metadata (actual keys stored securely)';
COMMENT ON COLUMN encryption_keys.key_ciphertext IS 'Encrypted key material (if using external KMS)';

-- =============================================================================
-- CREDENTIAL ACCESS AUDIT LOG
-- =============================================================================

CREATE TABLE credential_access_log (
    log_id BIGSERIAL PRIMARY KEY,

    -- Who accessed
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,

    -- What was accessed
    exchange VARCHAR(20) NOT NULL,
    profile VARCHAR(20) NOT NULL,

    -- Access details
    access_type VARCHAR(20) NOT NULL CHECK (access_type IN ('read', 'write', 'decrypt', 'revoke')),
    actor_type VARCHAR(20) NOT NULL CHECK (actor_type IN ('user', 'system', 'worker')),
    actor_id TEXT,

    -- Result
    success BOOLEAN NOT NULL,
    error_message TEXT,

    -- Context
    ip_address INET,
    user_agent TEXT,

    -- Audit
    accessed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_credential_access_tenant ON credential_access_log(tenant_id, accessed_at DESC);
CREATE INDEX idx_credential_access_exchange ON credential_access_log(tenant_id, exchange, accessed_at DESC);
CREATE INDEX idx_credential_access_failed ON credential_access_log(success, accessed_at DESC)
    WHERE success = FALSE;

COMMENT ON TABLE credential_access_log IS 'Audit log for all credential access attempts';

-- =============================================================================
-- HELPER FUNCTIONS
-- =============================================================================

-- Function to check if credentials exist for a tenant/user/exchange
CREATE OR REPLACE FUNCTION has_exchange_credentials(
    p_tenant_id TEXT,
    p_user_id TEXT,
    p_exchange VARCHAR,
    p_profile VARCHAR DEFAULT 'default'
) RETURNS BOOLEAN AS $$
BEGIN
    RETURN EXISTS (
        SELECT 1 FROM exchange_credentials
        WHERE tenant_id = p_tenant_id
          AND user_id = p_user_id
          AND exchange = p_exchange
          AND profile = p_profile
          AND status = 'active'
    );
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

-- Function to get active credentials count for a tenant
CREATE OR REPLACE FUNCTION get_active_credentials_count(
    p_tenant_id TEXT
) RETURNS INT AS $$
DECLARE
    count INT;
BEGIN
    SELECT COUNT(*) INTO count
    FROM exchange_credentials
    WHERE tenant_id = p_tenant_id AND status = 'active';
    RETURN count;
END;
$$ LANGUAGE plpgsql SECURITY DEFINER;

-- Trigger for updated_at
CREATE TRIGGER exchange_credentials_updated_at
    BEFORE UPDATE ON exchange_credentials
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- =============================================================================
-- DEFAULT SINGLE-USER PLACEHOLDER
-- =============================================================================
-- Note: Actual credentials should be inserted via CLI/API, never hardcoded

-- Insert placeholder to indicate migration ran
-- Real credentials will be added by user via: robson credentials set ...

COMMIT;

-- Post-migration notes:
-- 1. Run: robson credentials set --exchange binance --api-key XXX --secret YYY
-- 2. For single-user: tenant_id='default', user_id='local' (automatic)
-- 3. Encryption key should be set via ROBSON_CRYPTO_KEY env var
-- 4. Key rotation: INSERT INTO encryption_keys, then re-encrypt credentials
