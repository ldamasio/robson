//! Exchange Credentials Domain Types
//!
//! Multi-tenant credential management for exchange API access.
//!
//! # Security Model
//!
//! - Credentials are encrypted at rest with AES-256-GCM
//! - AAD (Additional Authenticated Data) includes tenant context
//! - Each credential is isolated by (tenant_id, user_id, exchange, profile)
//! - Single-user mode uses tenant_id="default", user_id="local"

use crate::context::IdentityScope;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

// =============================================================================
// Credential Identity
// =============================================================================

/// Unique identifier for a set of exchange credentials.
///
/// Composes `IdentityScope` (tenant/user/profile) with `Exchange`.
/// This ensures all credential operations are explicitly scoped.
///
/// # Example
///
/// ```rust,ignore
/// use robson_domain::{IdentityScope, CredentialId, Exchange, CredentialProfile};
///
/// // Single-user mode
/// let scope = IdentityScope::single_user();
/// let id = CredentialId::from_scope(scope, Exchange::Binance);
///
/// // Multi-tenant mode
/// let scope = IdentityScope::new("org123", "user456", "prod");
/// let id = CredentialId::from_scope(scope, Exchange::Binance);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CredentialId {
    /// Identity scope (tenant, user, profile)
    #[serde(flatten)]
    pub scope: IdentityScope,
    /// Exchange name
    pub exchange: Exchange,
}

impl CredentialId {
    /// Create a credential ID from an identity scope and exchange.
    ///
    /// This is the primary constructor - always requires explicit scope.
    pub fn from_scope(scope: IdentityScope, exchange: Exchange) -> Self {
        Self { scope, exchange }
    }

    /// Create a credential ID for single-user mode.
    ///
    /// Convenience wrapper using `IdentityScope::single_user()`.
    pub fn single_user(exchange: Exchange) -> Self {
        Self {
            scope: IdentityScope::single_user(),
            exchange,
        }
    }

    /// Create a credential ID for single-user mode with custom profile.
    pub fn single_user_with_profile(exchange: Exchange, profile: impl Into<String>) -> Self {
        Self {
            scope: IdentityScope::single_user_with_profile(profile),
            exchange,
        }
    }

    /// Get tenant ID.
    pub fn tenant_id(&self) -> &str {
        &self.scope.tenant_id
    }

    /// Get user ID.
    pub fn user_id(&self) -> &str {
        &self.scope.user_id
    }

    /// Get profile.
    pub fn profile(&self) -> &str {
        &self.scope.profile
    }

    /// Get the AAD (Additional Authenticated Data) for encryption.
    ///
    /// Format: "{tenant_id}|{user_id}|{profile}|{exchange}"
    /// Used in AES-GCM to bind the ciphertext to the credential identity.
    pub fn aad(&self) -> Vec<u8> {
        format!(
            "{}|{}|{}|{}",
            self.scope.tenant_id,
            self.scope.user_id,
            self.scope.profile,
            self.exchange.as_str()
        )
        .into_bytes()
    }

    /// Check if this is single-user mode.
    pub fn is_single_user(&self) -> bool {
        self.scope.is_single_user()
    }

    /// Get the identity scope.
    pub fn scope(&self) -> &IdentityScope {
        &self.scope
    }
}

// =============================================================================
// Exchange Type
// =============================================================================

/// Supported exchanges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exchange {
    /// Binance production
    Binance,
    /// Binance testnet (for paper trading/testing)
    BinanceTestnet,
}

impl Exchange {
    /// Get the exchange name as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Exchange::Binance => "binance",
            Exchange::BinanceTestnet => "binance_testnet",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "binance" => Some(Exchange::Binance),
            "binance_testnet" => Some(Exchange::BinanceTestnet),
            _ => None,
        }
    }

    /// Get the base URL for API calls.
    pub fn base_url(&self) -> &'static str {
        match self {
            Exchange::Binance => "https://api.binance.com",
            Exchange::BinanceTestnet => "https://testnet.binance.vision",
        }
    }
}

// =============================================================================
// Credential Profile
// =============================================================================

/// Credential profile (environment type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialProfile {
    /// Default profile
    Default,
    /// Paper trading (testnet)
    Paper,
    /// Production (explicit)
    Prod,
}

impl CredentialProfile {
    /// Get the profile name as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            CredentialProfile::Default => "default",
            CredentialProfile::Paper => "paper",
            CredentialProfile::Prod => "prod",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "default" => Some(CredentialProfile::Default),
            "paper" => Some(CredentialProfile::Paper),
            "prod" => Some(CredentialProfile::Prod),
            _ => None,
        }
    }

    /// Default profile for single-user mode.
    pub fn single_user_default() -> Self {
        CredentialProfile::Default
    }
}

impl Default for CredentialProfile {
    fn default() -> Self {
        Self::Default
    }
}

// =============================================================================
// Credential Status
// =============================================================================

/// Status of stored credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    /// Active and usable
    Active,
    /// Revoked by user
    Revoked,
    /// Expired (if applicable)
    Expired,
}

impl CredentialStatus {
    /// Get the status as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            CredentialStatus::Active => "active",
            CredentialStatus::Revoked => "revoked",
            CredentialStatus::Expired => "expired",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(CredentialStatus::Active),
            "revoked" => Some(CredentialStatus::Revoked),
            "expired" => Some(CredentialStatus::Expired),
            _ => None,
        }
    }
}

// =============================================================================
// Decrypted Credentials
// =============================================================================

/// Decrypted API credentials (in-memory only, never persisted).
///
/// This struct contains the plaintext credentials and should:
/// - Never be logged
/// - Never be serialized to disk
/// - Be zeroized when dropped
#[derive(Debug)]
pub struct ApiCredentials {
    /// API Key (public identifier)
    pub api_key: String,
    /// API Secret (secret key)
    pub api_secret: zeroize::Zeroizing<String>,
}

impl ApiCredentials {
    /// Create new API credentials.
    pub fn new(api_key: impl Into<String>, api_secret: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_secret: zeroize::Zeroizing::new(api_secret.into()),
        }
    }
}

impl zeroize::Zeroize for ApiCredentials {
    fn zeroize(&mut self) {
        self.api_key.zeroize();
        self.api_secret.zeroize();
    }
}

impl Drop for ApiCredentials {
    fn drop(&mut self) {
        self.zeroize();
    }
}

// =============================================================================
// Stored Credential (Encrypted)
// =============================================================================

/// Encrypted credential as stored in the database.
#[derive(Debug, Clone)]
pub struct StoredCredential {
    /// Credential identity
    pub id: CredentialId,
    /// Encrypted API key
    pub api_key_ciphertext: Vec<u8>,
    /// Nonce for API key encryption
    pub api_key_nonce: Vec<u8>,
    /// Encrypted API secret
    pub api_secret_ciphertext: Vec<u8>,
    /// Nonce for API secret encryption
    pub api_secret_nonce: Vec<u8>,
    /// Encryption version
    pub crypto_version: i32,
    /// Key identifier
    pub key_id: String,
    /// Status
    pub status: CredentialStatus,
    /// User-friendly label
    pub label: Option<String>,
    /// When last used
    pub last_used_at: Option<DateTime<Utc>>,
    /// When created
    pub created_at: DateTime<Utc>,
    /// When updated
    pub updated_at: DateTime<Utc>,
}

// =============================================================================
// Errors
// =============================================================================

/// Credential-related errors.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// Credential not found
    #[error("Credential not found for {tenant_id}/{user_id}/{exchange}/{profile}")]
    NotFound {
        tenant_id: String,
        user_id: String,
        exchange: String,
        profile: String,
    },

    /// Credential is not active
    #[error("Credential is {status} (not active)")]
    NotActive { status: String },

    /// Encryption/decryption error
    #[error("Encryption error: {0}")]
    Encryption(String),

    /// Invalid credential format
    #[error("Invalid credential: {0}")]
    InvalidCredential(String),

    /// Access denied (tenant isolation violation)
    #[error("Access denied: credential belongs to different tenant/user")]
    AccessDenied,

    /// Database error
    #[error("Database error: {0}")]
    Database(String),
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credential_id_single_user() {
        let id = CredentialId::single_user(Exchange::Binance);

        assert_eq!(id.tenant_id(), "default");
        assert_eq!(id.user_id(), "local");
        assert_eq!(id.profile(), "default");
        assert_eq!(id.exchange, Exchange::Binance);
        assert!(id.is_single_user());
    }

    #[test]
    fn test_credential_id_single_user_with_profile() {
        let id = CredentialId::single_user_with_profile(Exchange::Binance, "paper");

        assert_eq!(id.tenant_id(), "default");
        assert_eq!(id.user_id(), "local");
        assert_eq!(id.profile(), "paper");
        assert!(id.is_single_user());
    }

    #[test]
    fn test_credential_id_from_scope() {
        let scope = IdentityScope::new("org123", "user456", "prod");
        let id = CredentialId::from_scope(scope, Exchange::Binance);

        assert_eq!(id.tenant_id(), "org123");
        assert_eq!(id.user_id(), "user456");
        assert_eq!(id.profile(), "prod");
        assert!(!id.is_single_user());
    }

    #[test]
    fn test_credential_id_aad() {
        let scope = IdentityScope::new("tenant1", "user1", "prod");
        let id = CredentialId::from_scope(scope, Exchange::Binance);

        let aad = id.aad();
        let aad_str = String::from_utf8(aad).unwrap();

        // Format: tenant_id|user_id|profile|exchange
        assert_eq!(aad_str, "tenant1|user1|prod|binance");
    }

    #[test]
    fn test_credential_id_aad_single_user() {
        let id = CredentialId::single_user(Exchange::Binance);
        let aad_str = String::from_utf8(id.aad()).unwrap();

        assert_eq!(aad_str, "default|local|default|binance");
    }

    #[test]
    fn test_exchange_base_url() {
        assert_eq!(Exchange::Binance.base_url(), "https://api.binance.com");
        assert_eq!(
            Exchange::BinanceTestnet.base_url(),
            "https://testnet.binance.vision"
        );
    }

    #[test]
    fn test_api_credentials_zeroize() {
        let mut creds = ApiCredentials::new("test_key", "test_secret");

        assert_eq!(creds.api_key, "test_key");
        assert_eq!(*creds.api_secret, "test_secret");

        creds.zeroize();

        assert!(creds.api_key.is_empty() || creds.api_key.contains('\0'));
    }

    #[test]
    fn test_aad_prevents_cross_tenant_decrypt() {
        let scope1 = IdentityScope::new("tenant_a", "user1", "default");
        let scope2 = IdentityScope::new("tenant_b", "user1", "default");

        let id1 = CredentialId::from_scope(scope1, Exchange::Binance);
        let id2 = CredentialId::from_scope(scope2, Exchange::Binance);

        // Different tenants should have different AAD
        assert_ne!(id1.aad(), id2.aad());

        let scope3 = IdentityScope::new("tenant_a", "user2", "default");
        let id3 = CredentialId::from_scope(scope3, Exchange::Binance);

        // Different users should have different AAD
        assert_ne!(id1.aad(), id3.aad());

        let scope4 = IdentityScope::new("tenant_a", "user1", "paper");
        let id4 = CredentialId::from_scope(scope4, Exchange::Binance);

        // Different profiles should have different AAD
        assert_ne!(id1.aad(), id4.aad());
    }
}
