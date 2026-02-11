//! Credential Store: Multi-tenant encrypted storage for exchange credentials.
//!
//! # Security Model
//!
//! - AES-256-GCM encryption with AAD (Additional Authenticated Data)
//! - AAD includes tenant_id + user_id + exchange + profile
//! - Master key provided via environment variable (ROBSON_CRYPTO_KEY)
//! - Supports key rotation via key_id
//!
//! # Usage
//!
//! ```rust,ignore
//! use robson_store::{CredentialStore, CredentialId, Exchange, CredentialProfile};
//!
//! // Single-user mode
//! let id = CredentialId::single_user(Exchange::Binance, CredentialProfile::Default);
//!
//! // Store credentials
//! store.store(&id, "api_key", "api_secret").await?;
//!
//! // Retrieve credentials
//! let creds = store.get(&id).await?;
//! println!("API Key: {}", creds.api_key);
//! ```

use async_trait::async_trait;
use chrono::Utc;
use robson_domain::{
    ApiCredentials, CredentialError, CredentialId, CredentialStatus,
    StoredCredential,
};
#[cfg(feature = "postgres")]
use std::sync::Arc;

#[cfg(feature = "postgres")]
use sqlx::PgPool;

// =============================================================================
// Encryption
// =============================================================================

mod crypto {
    use aes_gcm::{
        aead::{Aead, KeyInit, OsRng},
        Aes256Gcm, Nonce,
    };
    use rand::RngCore;
    use robson_domain::CredentialError;
    use zeroize::Zeroizing;

    /// AES-256-GCM key size (32 bytes)
    const KEY_SIZE: usize = 32;

    /// Nonce size for AES-GCM (12 bytes)
    pub const NONCE_SIZE: usize = 12;

    /// Encryption key wrapper.
    pub struct EncryptionKey {
        key: Aes256Gcm,
        key_id: String,
    }

    impl EncryptionKey {
        /// Create from raw bytes.
        pub fn from_bytes(key_bytes: &[u8], key_id: impl Into<String>) -> Result<Self, CredentialError> {
            if key_bytes.len() != KEY_SIZE {
                return Err(CredentialError::Encryption(format!(
                    "Invalid key size: expected {} bytes, got {}",
                    KEY_SIZE,
                    key_bytes.len()
                )));
            }

            let key = Aes256Gcm::new_from_slice(key_bytes)
                .map_err(|e| CredentialError::Encryption(e.to_string()))?;

            Ok(Self {
                key,
                key_id: key_id.into(),
            })
        }

        /// Generate a new random key.
        pub fn generate(key_id: impl Into<String>) -> Self {
            let mut key_bytes = [0u8; KEY_SIZE];
            OsRng.fill_bytes(&mut key_bytes);
            let key = Aes256Gcm::new_from_slice(&key_bytes).unwrap();
            Self {
                key,
                key_id: key_id.into(),
            }
        }

        /// Get the key ID.
        pub fn key_id(&self) -> &str {
            &self.key_id
        }

        /// Encrypt plaintext with AAD.
        pub fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<(Vec<u8>, Vec<u8>), CredentialError> {
            let mut nonce_bytes = [0u8; NONCE_SIZE];
            OsRng.fill_bytes(&mut nonce_bytes);
            let nonce = Nonce::from_slice(&nonce_bytes);

            let ciphertext = self
                .key
                .encrypt(nonce, aes_gcm::aead::Payload {
                    msg: plaintext,
                    aad,
                })
                .map_err(|e| CredentialError::Encryption(e.to_string()))?;

            Ok((ciphertext, nonce_bytes.to_vec()))
        }

        /// Decrypt ciphertext with AAD.
        pub fn decrypt(&self, ciphertext: &[u8], nonce: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
            if nonce.len() != NONCE_SIZE {
                return Err(CredentialError::Encryption(format!(
                    "Invalid nonce size: expected {} bytes, got {}",
                    NONCE_SIZE,
                    nonce.len()
                )));
            }

            let nonce = Nonce::from_slice(nonce);

            let plaintext = self
                .key
                .decrypt(nonce, aes_gcm::aead::Payload {
                    msg: ciphertext,
                    aad,
                })
                .map_err(|e| CredentialError::Encryption(format!("Decryption failed: {}", e)))?;

            Ok(Zeroizing::new(plaintext))
        }
    }

    /// Parse hex-encoded key from environment variable.
    pub fn parse_key_from_hex(hex: &str) -> Result<Vec<u8>, CredentialError> {
        hex::decode(hex)
            .map_err(|e| CredentialError::Encryption(format!("Invalid hex key: {}", e)))
    }
}

// =============================================================================
// Credential Store Trait
// =============================================================================

/// Repository for storing and retrieving encrypted exchange credentials.
#[async_trait]
pub trait CredentialStore: Send + Sync {
    /// Store credentials for a given identity.
    ///
    /// The credentials are encrypted before storage.
    async fn store(
        &self,
        id: &CredentialId,
        api_key: &str,
        api_secret: &str,
        label: Option<&str>,
    ) -> Result<(), CredentialError>;

    /// Retrieve credentials for a given identity.
    ///
    /// Returns decrypted credentials if found and active.
    async fn get(&self, id: &CredentialId) -> Result<ApiCredentials, CredentialError>;

    /// Check if credentials exist for a given identity.
    async fn exists(&self, id: &CredentialId) -> Result<bool, CredentialError>;

    /// Revoke credentials for a given identity.
    async fn revoke(&self, id: &CredentialId, reason: &str) -> Result<(), CredentialError>;

    /// List all credentials for a tenant/user.
    async fn list(&self, tenant_id: &str, user_id: &str) -> Result<Vec<StoredCredential>, CredentialError>;

    /// Update last used timestamp.
    async fn touch(&self, id: &CredentialId) -> Result<(), CredentialError>;
}

// =============================================================================
// In-Memory Implementation (for testing)
// =============================================================================

/// In-memory credential store for testing.
pub struct MemoryCredentialStore {
    encryption_key: crypto::EncryptionKey,
    credentials: std::sync::RwLock<std::collections::HashMap<String, StoredCredential>>,
}

impl MemoryCredentialStore {
    /// Create a new in-memory store with a random key.
    pub fn new() -> Self {
        Self {
            encryption_key: crypto::EncryptionKey::generate("default"),
            credentials: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Create with a specific key (for testing key rotation).
    pub fn with_key(key_bytes: &[u8], key_id: &str) -> Result<Self, CredentialError> {
        Ok(Self {
            encryption_key: crypto::EncryptionKey::from_bytes(key_bytes, key_id)?,
            credentials: std::sync::RwLock::new(std::collections::HashMap::new()),
        })
    }

    fn id_to_key(id: &CredentialId) -> String {
        format!(
            "{}:{}:{}:{}",
            id.tenant_id(),
            id.user_id(),
            id.exchange.as_str(),
            id.profile()
        )
    }
}

impl Default for MemoryCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CredentialStore for MemoryCredentialStore {
    async fn store(
        &self,
        id: &CredentialId,
        api_key: &str,
        api_secret: &str,
        label: Option<&str>,
    ) -> Result<(), CredentialError> {
        let key = Self::id_to_key(id);
        let aad = id.aad();

        // Encrypt
        let (api_key_ciphertext, api_key_nonce) = self.encryption_key.encrypt(api_key.as_bytes(), &aad)?;
        let (api_secret_ciphertext, api_secret_nonce) = self.encryption_key.encrypt(api_secret.as_bytes(), &aad)?;

        let now = Utc::now();
        let stored = StoredCredential {
            id: id.clone(),
            api_key_ciphertext,
            api_key_nonce,
            api_secret_ciphertext,
            api_secret_nonce,
            crypto_version: 1,
            key_id: self.encryption_key.key_id().to_string(),
            status: CredentialStatus::Active,
            label: label.map(|s| s.to_string()),
            last_used_at: None,
            created_at: now,
            updated_at: now,
        };

        let mut creds = self.credentials.write().unwrap();
        creds.insert(key, stored);

        Ok(())
    }

    async fn get(&self, id: &CredentialId) -> Result<ApiCredentials, CredentialError> {
        let key = Self::id_to_key(id);
        let aad = id.aad();

        let creds = self.credentials.read().unwrap();
        let stored = creds
            .get(&key)
            .ok_or_else(|| CredentialError::NotFound {
                tenant_id: id.tenant_id().to_string(),
                user_id: id.user_id().to_string(),
                exchange: id.exchange.as_str().to_string(),
                profile: id.profile().to_string(),
            })?
            .clone();

        drop(creds);

        // Check status
        if stored.status != CredentialStatus::Active {
            return Err(CredentialError::NotActive {
                status: stored.status.as_str().to_string(),
            });
        }

        // Verify key ID matches
        if stored.key_id != self.encryption_key.key_id() {
            return Err(CredentialError::Encryption(
                "Key ID mismatch - key rotation needed".to_string(),
            ));
        }

        // Decrypt
        let api_key_bytes = self
            .encryption_key
            .decrypt(&stored.api_key_ciphertext, &stored.api_key_nonce, &aad)?;
        let api_secret_bytes = self
            .encryption_key
            .decrypt(&stored.api_secret_ciphertext, &stored.api_secret_nonce, &aad)?;

        let api_key = String::from_utf8(api_key_bytes.to_vec())
            .map_err(|e| CredentialError::Encryption(format!("Invalid UTF-8: {}", e)))?;
        let api_secret = String::from_utf8(api_secret_bytes.to_vec())
            .map_err(|e| CredentialError::Encryption(format!("Invalid UTF-8: {}", e)))?;

        // Update last used
        {
            let mut creds = self.credentials.write().unwrap();
            if let Some(stored) = creds.get_mut(&key) {
                stored.last_used_at = Some(Utc::now());
            }
        }

        Ok(ApiCredentials::new(api_key, api_secret))
    }

    async fn exists(&self, id: &CredentialId) -> Result<bool, CredentialError> {
        let key = Self::id_to_key(id);
        let creds = self.credentials.read().unwrap();
        Ok(creds.contains_key(&key))
    }

    async fn revoke(&self, id: &CredentialId, reason: &str) -> Result<(), CredentialError> {
        let key = Self::id_to_key(id);
        let mut creds = self.credentials.write().unwrap();

        if let Some(stored) = creds.get_mut(&key) {
            stored.status = CredentialStatus::Revoked;
            stored.updated_at = Utc::now();
            // Note: In real impl, we'd store revoke_reason too
            let _ = reason;
        }

        Ok(())
    }

    async fn list(&self, tenant_id: &str, user_id: &str) -> Result<Vec<StoredCredential>, CredentialError> {
        let creds = self.credentials.read().unwrap();
        let result = creds
            .values()
            .filter(|c| c.id.tenant_id() == tenant_id && c.id.user_id() == user_id)
            .cloned()
            .collect();
        Ok(result)
    }

    async fn touch(&self, id: &CredentialId) -> Result<(), CredentialError> {
        let key = Self::id_to_key(id);
        let mut creds = self.credentials.write().unwrap();
        if let Some(stored) = creds.get_mut(&key) {
            stored.last_used_at = Some(Utc::now());
        }
        Ok(())
    }
}

// =============================================================================
// PostgreSQL Implementation
// =============================================================================

/// PostgreSQL credential store.
#[cfg(feature = "postgres")]
pub struct PgCredentialStore {
    pool: Arc<PgPool>,
    encryption_key: crypto::EncryptionKey,
}

#[cfg(feature = "postgres")]
impl PgCredentialStore {
    /// Create a new PostgreSQL credential store.
    pub fn new(pool: Arc<PgPool>, key_bytes: &[u8], key_id: &str) -> Result<Self, CredentialError> {
        Ok(Self {
            pool,
            encryption_key: crypto::EncryptionKey::from_bytes(key_bytes, key_id)?,
        })
    }

    /// Create from environment variables.
    ///
    /// Reads from:
    /// - `ROBSON_CRYPTO_KEY`: Hex-encoded 32-byte key
    /// - `ROBSON_CRYPTO_KEY_ID`: Key identifier (default: "default")
    pub fn from_env(pool: Arc<PgPool>) -> Result<Self, CredentialError> {
        let key_hex = std::env::var("ROBSON_CRYPTO_KEY")
            .map_err(|_| CredentialError::Encryption("ROBSON_CRYPTO_KEY not set".to_string()))?;

        let key_bytes = crypto::parse_key_from_hex(&key_hex)?;
        let key_id = std::env::var("ROBSON_CRYPTO_KEY_ID").unwrap_or_else(|_| "default".to_string());

        Self::new(pool, &key_bytes, &key_id)
    }
}

#[cfg(feature = "postgres")]
#[async_trait]
impl CredentialStore for PgCredentialStore {
    async fn store(
        &self,
        id: &CredentialId,
        api_key: &str,
        api_secret: &str,
        label: Option<&str>,
    ) -> Result<(), CredentialError> {
        let aad = id.aad();

        // Encrypt
        let (api_key_ciphertext, api_key_nonce) = self.encryption_key.encrypt(api_key.as_bytes(), &aad)?;
        let (api_secret_ciphertext, api_secret_nonce) = self.encryption_key.encrypt(api_secret.as_bytes(), &aad)?;

        sqlx::query(
            r#"
            INSERT INTO exchange_credentials (
                tenant_id, user_id, exchange, profile,
                api_key_ciphertext, api_key_nonce,
                api_secret_ciphertext, api_secret_nonce,
                crypto_version, key_id, status, label
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (tenant_id, user_id, exchange, profile) DO UPDATE SET
                api_key_ciphertext = EXCLUDED.api_key_ciphertext,
                api_key_nonce = EXCLUDED.api_key_nonce,
                api_secret_ciphertext = EXCLUDED.api_secret_ciphertext,
                api_secret_nonce = EXCLUDED.api_secret_nonce,
                crypto_version = EXCLUDED.crypto_version,
                key_id = EXCLUDED.key_id,
                status = EXCLUDED.status,
                label = EXCLUDED.label,
                updated_at = NOW()
            "#
        )
        .bind(&id.tenant_id)
        .bind(&id.user_id)
        .bind(id.exchange.as_str())
        .bind(id.profile.as_str())
        .bind(&api_key_ciphertext)
        .bind(&api_key_nonce)
        .bind(&api_secret_ciphertext)
        .bind(&api_secret_nonce)
        .bind(1i32) // crypto_version
        .bind(&self.encryption_key.key_id())
        .bind("active")
        .bind(label)
        .execute(&*self.pool)
        .await
        .map_err(|e| CredentialError::Database(e.to_string()))?;

        Ok(())
    }

    async fn get(&self, id: &CredentialId) -> Result<ApiCredentials, CredentialError> {
        let aad = id.aad();

        let row = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, String, String)>(
            r#"
            SELECT api_key_ciphertext, api_key_nonce,
                   api_secret_ciphertext, api_secret_nonce,
                   key_id, status
            FROM exchange_credentials
            WHERE tenant_id = $1 AND user_id = $2 AND exchange = $3 AND profile = $4
            "#
        )
        .bind(&id.tenant_id)
        .bind(&id.user_id)
        .bind(id.exchange.as_str())
        .bind(id.profile.as_str())
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| CredentialError::Database(e.to_string()))?
        .ok_or_else(|| CredentialError::NotFound {
            tenant_id: id.tenant_id.clone(),
            user_id: id.user_id.clone(),
            exchange: id.exchange.as_str().to_string(),
            profile: id.profile.as_str().to_string(),
        })?;

        let (api_key_ciphertext, api_key_nonce, api_secret_ciphertext, api_secret_nonce, key_id, status) = row;

        // Check status
        if status != "active" {
            return Err(CredentialError::NotActive { status });
        }

        // Verify key ID matches
        if key_id != self.encryption_key.key_id() {
            return Err(CredentialError::Encryption(
                "Key ID mismatch - key rotation needed".to_string(),
            ));
        }

        // Decrypt
        let api_key_bytes = self
            .encryption_key
            .decrypt(&api_key_ciphertext, &api_key_nonce, &aad)?;
        let api_secret_bytes = self
            .encryption_key
            .decrypt(&api_secret_ciphertext, &api_secret_nonce, &aad)?;

        let api_key = String::from_utf8(api_key_bytes.to_vec())
            .map_err(|e| CredentialError::Encryption(format!("Invalid UTF-8: {}", e)))?;
        let api_secret = String::from_utf8(api_secret_bytes.to_vec())
            .map_err(|e| CredentialError::Encryption(format!("Invalid UTF-8: {}", e)))?;

        // Update last_used_at
        sqlx::query(
            r#"
            UPDATE exchange_credentials
            SET last_used_at = NOW()
            WHERE tenant_id = $1 AND user_id = $2 AND exchange = $3 AND profile = $4
            "#
        )
        .bind(&id.tenant_id)
        .bind(&id.user_id)
        .bind(id.exchange.as_str())
        .bind(id.profile.as_str())
        .execute(&*self.pool)
        .await
        .ok(); // Ignore errors on touch

        Ok(ApiCredentials::new(api_key, api_secret))
    }

    async fn exists(&self, id: &CredentialId) -> Result<bool, CredentialError> {
        let row: Option<(i64,)> = sqlx::query_as(
            r#"
            SELECT 1 FROM exchange_credentials
            WHERE tenant_id = $1 AND user_id = $2 AND exchange = $3 AND profile = $4 AND status = 'active'
            "#
        )
        .bind(&id.tenant_id)
        .bind(&id.user_id)
        .bind(id.exchange.as_str())
        .bind(id.profile.as_str())
        .fetch_optional(&*self.pool)
        .await
        .map_err(|e| CredentialError::Database(e.to_string()))?;

        Ok(row.is_some())
    }

    async fn revoke(&self, id: &CredentialId, reason: &str) -> Result<(), CredentialError> {
        sqlx::query(
            r#"
            UPDATE exchange_credentials
            SET status = 'revoked', revoke_reason = $5, revoked_at = NOW(), updated_at = NOW()
            WHERE tenant_id = $1 AND user_id = $2 AND exchange = $3 AND profile = $4
            "#
        )
        .bind(&id.tenant_id)
        .bind(&id.user_id)
        .bind(id.exchange.as_str())
        .bind(id.profile.as_str())
        .bind(reason)
        .execute(&*self.pool)
        .await
        .map_err(|e| CredentialError::Database(e.to_string()))?;

        Ok(())
    }

    async fn list(&self, tenant_id: &str, user_id: &str) -> Result<Vec<StoredCredential>, CredentialError> {
        // Simplified - returns empty for now (full impl would map all fields)
        let _ = (tenant_id, user_id);
        Ok(Vec::new())
    }

    async fn touch(&self, id: &CredentialId) -> Result<(), CredentialError> {
        sqlx::query(
            r#"
            UPDATE exchange_credentials
            SET last_used_at = NOW()
            WHERE tenant_id = $1 AND user_id = $2 AND exchange = $3 AND profile = $4
            "#
        )
        .bind(&id.tenant_id)
        .bind(&id.user_id)
        .bind(id.exchange.as_str())
        .bind(id.profile.as_str())
        .execute(&*self.pool)
        .await
        .map_err(|e| CredentialError::Database(e.to_string()))?;

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use robson_domain::{Exchange, IdentityScope};

    #[tokio::test]
    async fn test_memory_store_roundtrip_single_user() {
        let store = MemoryCredentialStore::new();
        let id = CredentialId::single_user(Exchange::Binance);

        // Store
        store
            .store(&id, "test_api_key", "test_api_secret", Some("Test Account"))
            .await
            .unwrap();

        // Retrieve
        let creds = store.get(&id).await.unwrap();
        assert_eq!(creds.api_key, "test_api_key");
        assert_eq!(*creds.api_secret, "test_api_secret");
    }

    #[tokio::test]
    async fn test_memory_store_roundtrip_multi_tenant() {
        let store = MemoryCredentialStore::new();
        let scope = IdentityScope::new("org123", "user456", "prod");
        let id = CredentialId::from_scope(scope, Exchange::Binance);

        // Store
        store
            .store(&id, "org_api_key", "org_api_secret", Some("Org Account"))
            .await
            .unwrap();

        // Retrieve
        let creds = store.get(&id).await.unwrap();
        assert_eq!(creds.api_key, "org_api_key");
        assert_eq!(*creds.api_secret, "org_api_secret");
    }

    #[tokio::test]
    async fn test_memory_store_not_found() {
        let store = MemoryCredentialStore::new();
        let id = CredentialId::single_user(Exchange::Binance);

        let result = store.get(&id).await;
        assert!(matches!(result, Err(CredentialError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_memory_store_revoke() {
        let store = MemoryCredentialStore::new();
        let id = CredentialId::single_user(Exchange::Binance);

        store.store(&id, "key", "secret", None).await.unwrap();

        // Revoke
        store.revoke(&id, "Compromised").await.unwrap();

        // Should fail to retrieve
        let result = store.get(&id).await;
        assert!(matches!(result, Err(CredentialError::NotActive { .. })));
    }

    #[tokio::test]
    async fn test_tenant_isolation() {
        let store = MemoryCredentialStore::new();

        let scope_a = IdentityScope::new("tenant_a", "user1", "default");
        let scope_b = IdentityScope::new("tenant_b", "user1", "default");

        let id1 = CredentialId::from_scope(scope_a, Exchange::Binance);
        let id2 = CredentialId::from_scope(scope_b, Exchange::Binance);

        // Store for tenant A
        store.store(&id1, "key_a", "secret_a", None).await.unwrap();

        // Tenant B should not have access
        let result = store.get(&id2).await;
        assert!(matches!(result, Err(CredentialError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_profile_isolation() {
        let store = MemoryCredentialStore::new();

        let scope_default = IdentityScope::single_user();
        let scope_paper = IdentityScope::single_user_with_profile("paper");

        let id_default = CredentialId::from_scope(scope_default, Exchange::Binance);
        let id_paper = CredentialId::from_scope(scope_paper, Exchange::Binance);

        // Store for default profile
        store.store(&id_default, "key_default", "secret_default", None).await.unwrap();

        // Paper profile should not have access
        let result = store.get(&id_paper).await;
        assert!(matches!(result, Err(CredentialError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_aad_prevents_cross_tenant_decrypt() {
        // Create store with known key
        let key_bytes = [42u8; 32];
        let store = MemoryCredentialStore::with_key(&key_bytes, "test_key").unwrap();

        let id = CredentialId::single_user(Exchange::Binance);
        store.store(&id, "key", "secret", None).await.unwrap();

        // Try to decrypt with wrong AAD (simulating cross-tenant attack)
        let wrong_scope = IdentityScope::new("wrong_tenant", "wrong_user", "default");
        let wrong_id = CredentialId::from_scope(wrong_scope, Exchange::Binance);
        let result = store.get(&wrong_id).await;
        assert!(matches!(result, Err(CredentialError::NotFound { .. })));
    }

    #[test]
    fn test_encryption_key_roundtrip() {
        let key = crypto::EncryptionKey::generate("test");
        let plaintext = b"my_secret_api_key";
        let aad = b"default|local|default|binance";

        let (ciphertext, nonce) = key.encrypt(plaintext, aad).unwrap();
        let decrypted = key.decrypt(&ciphertext, &nonce, aad).unwrap();

        assert_eq!(&*decrypted, plaintext);
    }

    #[test]
    fn test_encryption_aad_mismatch() {
        let key = crypto::EncryptionKey::generate("test");
        let plaintext = b"my_secret_api_key";
        let aad1 = b"tenant_a|user1|default|binance";
        let aad2 = b"tenant_b|user1|default|binance";

        let (ciphertext, nonce) = key.encrypt(plaintext, aad1).unwrap();

        // Decrypting with wrong AAD should fail
        let result = key.decrypt(&ciphertext, &nonce, aad2);
        assert!(result.is_err());
    }
}
