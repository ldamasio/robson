//! Identity Context for Multi-Tenant Credential Resolution
//!
//! Provides explicit context for all credential operations:
//! - tenant_id: Tenant/organization identifier
//! - user_id: User within tenant
//! - profile: Credential profile (default, paper, prod)
//!
//! Single-user mode is an alias: tenant_id="default", user_id="local"
//! This is NEVER implicit in the internal model - always explicit.

use serde::{Deserialize, Serialize};

/// Default tenant ID for single-user mode.
pub const DEFAULT_TENANT_ID: &str = "default";

/// Default user ID for single-user mode.
pub const DEFAULT_USER_ID: &str = "local";

/// Default profile name.
pub const DEFAULT_PROFILE: &str = "default";

/// Identity scope for credential resolution.
///
/// This struct provides explicit context for all operations that need
/// to resolve credentials or tenant-specific data.
///
/// # Single-User Mode
///
/// Use `IdentityScope::single_user()` which sets:
/// - tenant_id = "default"
/// - user_id = "local"
/// - profile = "default"
///
/// # Multi-Tenant Mode
///
/// Use `IdentityScope::new()` with explicit values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdentityScope {
    /// Tenant/organization identifier.
    pub tenant_id: String,
    /// User identifier within tenant.
    pub user_id: String,
    /// Credential profile (default, paper, prod).
    pub profile: String,
}

impl IdentityScope {
    /// Create a new identity scope with explicit values.
    pub fn new(
        tenant_id: impl Into<String>,
        user_id: impl Into<String>,
        profile: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            user_id: user_id.into(),
            profile: profile.into(),
        }
    }

    /// Create identity scope for single-user mode.
    ///
    /// This is an alias for:
    /// - tenant_id = "default"
    /// - user_id = "local"
    /// - profile = "default"
    pub fn single_user() -> Self {
        Self {
            tenant_id: DEFAULT_TENANT_ID.to_string(),
            user_id: DEFAULT_USER_ID.to_string(),
            profile: DEFAULT_PROFILE.to_string(),
        }
    }

    /// Create identity scope for single-user mode with custom profile.
    pub fn single_user_with_profile(profile: impl Into<String>) -> Self {
        Self {
            tenant_id: DEFAULT_TENANT_ID.to_string(),
            user_id: DEFAULT_USER_ID.to_string(),
            profile: profile.into(),
        }
    }

    /// Check if this is single-user mode.
    pub fn is_single_user(&self) -> bool {
        self.tenant_id == DEFAULT_TENANT_ID && self.user_id == DEFAULT_USER_ID
    }

    /// Get the AAD (Additional Authenticated Data) string.
    ///
    /// Format: "{tenant_id}|{user_id}|{profile}"
    /// Used for AES-GCM encryption binding.
    pub fn aad_base(&self) -> String {
        format!("{}|{}|{}", self.tenant_id, self.user_id, self.profile)
    }

    /// Parse from strings (for CLI parsing).
    pub fn from_parts(
        tenant_id: Option<&str>,
        user_id: Option<&str>,
        profile: Option<&str>,
    ) -> Self {
        Self {
            tenant_id: tenant_id
                .map(|s| s.to_string())
                .unwrap_or_else(|| DEFAULT_TENANT_ID.to_string()),
            user_id: user_id
                .map(|s| s.to_string())
                .unwrap_or_else(|| DEFAULT_USER_ID.to_string()),
            profile: profile
                .map(|s| s.to_string())
                .unwrap_or_else(|| DEFAULT_PROFILE.to_string()),
        }
    }
}

impl Default for IdentityScope {
    fn default() -> Self {
        Self::single_user()
    }
}

impl std::fmt::Display for IdentityScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_single_user() {
            write!(f, "single-user(profile={})", self.profile)
        } else {
            write!(
                f,
                "tenant={}/user={}/profile={}",
                self.tenant_id, self.user_id, self.profile
            )
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_user_defaults() {
        let scope = IdentityScope::single_user();

        assert_eq!(scope.tenant_id, "default");
        assert_eq!(scope.user_id, "local");
        assert_eq!(scope.profile, "default");
        assert!(scope.is_single_user());
    }

    #[test]
    fn test_explicit_multi_tenant() {
        let scope = IdentityScope::new("org123", "user456", "prod");

        assert_eq!(scope.tenant_id, "org123");
        assert_eq!(scope.user_id, "user456");
        assert_eq!(scope.profile, "prod");
        assert!(!scope.is_single_user());
    }

    #[test]
    fn test_from_parts_all_none() {
        let scope = IdentityScope::from_parts(None, None, None);

        assert_eq!(scope.tenant_id, "default");
        assert_eq!(scope.user_id, "local");
        assert_eq!(scope.profile, "default");
    }

    #[test]
    fn test_from_parts_partial() {
        let scope = IdentityScope::from_parts(None, None, Some("paper"));

        assert_eq!(scope.tenant_id, "default");
        assert_eq!(scope.user_id, "local");
        assert_eq!(scope.profile, "paper");
    }

    #[test]
    fn test_from_parts_all_explicit() {
        let scope = IdentityScope::from_parts(Some("tenant_a"), Some("user_b"), Some("prod"));

        assert_eq!(scope.tenant_id, "tenant_a");
        assert_eq!(scope.user_id, "user_b");
        assert_eq!(scope.profile, "prod");
    }

    #[test]
    fn test_aad_base_format() {
        let scope = IdentityScope::new("tenant1", "user2", "prod");
        assert_eq!(scope.aad_base(), "tenant1|user2|prod");

        let single = IdentityScope::single_user();
        assert_eq!(single.aad_base(), "default|local|default");
    }

    #[test]
    fn test_display_single_user() {
        let scope = IdentityScope::single_user();
        assert_eq!(format!("{}", scope), "single-user(profile=default)");

        let scope_paper = IdentityScope::single_user_with_profile("paper");
        assert_eq!(format!("{}", scope_paper), "single-user(profile=paper)");
    }

    #[test]
    fn test_display_multi_tenant() {
        let scope = IdentityScope::new("org123", "user456", "prod");
        assert_eq!(
            format!("{}", scope),
            "tenant=org123/user=user456/profile=prod"
        );
    }

    #[test]
    fn test_default_is_single_user() {
        let scope = IdentityScope::default();
        assert!(scope.is_single_user());
    }
}
