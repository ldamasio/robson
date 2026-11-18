//! RBX Identity authentication and Robson role authorization.
//!
//! Human callers authenticate with Google through ZITADEL. `robsond` trusts
//! only the configured RBX issuer, audience, client IDs, canonical subjects,
//! and organization-scoped project roles. Static API tokens and direct Google
//! ID tokens are deliberately not accepted.

use std::{collections::BTreeSet, sync::Arc};

use serde::Serialize;
use uuid::Uuid;

use crate::{
    config::OidcConfig,
    oidc_auth::{RbxOidcError, RbxOidcVerifier},
};

pub const ROLE_OBSERVER: &str = "rbx:robson:observer";
pub const ROLE_OPERATOR: &str = "rbx:robson:operator";
pub const ROLE_FUNDING: &str = "rbx:robson:funding";
pub const ROLE_EMERGENCY: &str = "rbx:robson:emergency";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredPermission {
    Observer,
    Operator,
    Funding,
    Emergency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    Disabled,
    RbxOidc,
}

impl AuthMethod {
    /// Value accepted by the existing recovery-decision audit schema.
    ///
    /// RBX OIDC proves the operator identity cryptographically. The disabled
    /// development mode is retained only for explicit loopback development
    /// and therefore remains an asserted identity in recovery records.
    pub fn decision_auth_kind(self) -> &'static str {
        match self {
            Self::Disabled => "asserted",
            Self::RbxOidc => "verified",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub method: AuthMethod,
    pub issuer: String,
    pub subject: String,
    pub client_id: Option<String>,
    pub tenant_id: Option<Uuid>,
    pub roles: BTreeSet<String>,
    grants_all: bool,
}

impl AuthContext {
    fn disabled(tenant_id: Option<Uuid>) -> Self {
        Self {
            method: AuthMethod::Disabled,
            issuer: "robsond-development".to_owned(),
            subject: "development-operator".to_owned(),
            client_id: None,
            tenant_id,
            roles: BTreeSet::new(),
            grants_all: true,
        }
    }

    fn allows(&self, required: RequiredPermission) -> bool {
        if self.grants_all {
            return true;
        }

        match required {
            RequiredPermission::Observer => self.roles.iter().any(|role| {
                matches!(
                    role.as_str(),
                    ROLE_OBSERVER | ROLE_OPERATOR | ROLE_FUNDING | ROLE_EMERGENCY
                )
            }),
            RequiredPermission::Operator => self.roles.contains(ROLE_OPERATOR),
            RequiredPermission::Funding => self.roles.contains(ROLE_FUNDING),
            RequiredPermission::Emergency => self.roles.contains(ROLE_EMERGENCY),
        }
    }

    pub fn permissions(&self) -> Vec<&'static str> {
        [
            RequiredPermission::Observer,
            RequiredPermission::Operator,
            RequiredPermission::Funding,
            RequiredPermission::Emergency,
        ]
        .into_iter()
        .filter(|permission| self.allows(*permission))
        .map(|permission| match permission {
            RequiredPermission::Observer => "observer",
            RequiredPermission::Operator => "operator",
            RequiredPermission::Funding => "funding",
            RequiredPermission::Emergency => "emergency",
        })
        .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing or invalid Authorization header")]
    MissingBearer,
    #[error("bearer token is invalid")]
    InvalidToken,
    #[error("authenticated identity lacks the required Robson role")]
    Forbidden,
    #[error("identity provider is temporarily unavailable")]
    IdentityUnavailable,
}

#[derive(Clone)]
pub struct AuthService {
    verifier: Option<Arc<RbxOidcVerifier>>,
    development_tenant_id: Option<Uuid>,
}

impl AuthService {
    pub fn new(oidc: Option<OidcConfig>, tenant_id: Option<Uuid>) -> Self {
        Self {
            verifier: oidc.map(|config| Arc::new(RbxOidcVerifier::new(config))),
            development_tenant_id: tenant_id,
        }
    }

    pub async fn authenticate(
        &self,
        authorization: Option<&str>,
        required: RequiredPermission,
    ) -> Result<AuthContext, AuthError> {
        let Some(verifier) = &self.verifier else {
            return Ok(AuthContext::disabled(self.development_tenant_id));
        };

        let token = authorization
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty())
            .ok_or(AuthError::MissingBearer)?;

        let identity = verifier.verify(token).await.map_err(|error| match error {
            RbxOidcError::InvalidToken => AuthError::InvalidToken,
            RbxOidcError::IdentityUnavailable => AuthError::IdentityUnavailable,
        })?;
        let context = AuthContext {
            method: AuthMethod::RbxOidc,
            issuer: identity.issuer,
            subject: identity.subject,
            client_id: Some(identity.client_id),
            tenant_id: Some(identity.tenant_id),
            roles: identity.roles,
            grants_all: false,
        };

        if context.allows(required) {
            Ok(context)
        } else {
            Err(AuthError::Forbidden)
        }
    }

    #[cfg(test)]
    pub(crate) fn from_verifier_for_test(verifier: RbxOidcVerifier) -> Self {
        Self {
            verifier: Some(Arc::new(verifier)),
            development_tenant_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_are_independent_and_observer_access_is_implied() {
        let tenant_id = Uuid::now_v7();
        let context = AuthContext {
            method: AuthMethod::RbxOidc,
            issuer: "https://auth.merovelis.com".to_owned(),
            subject: "rbx-user-1".to_owned(),
            client_id: Some("robson-android".to_owned()),
            tenant_id: Some(tenant_id),
            roles: BTreeSet::from([ROLE_OPERATOR.to_owned()]),
            grants_all: false,
        };

        assert!(context.allows(RequiredPermission::Observer));
        assert!(context.allows(RequiredPermission::Operator));
        assert!(!context.allows(RequiredPermission::Funding));
        assert!(!context.allows(RequiredPermission::Emergency));
        assert_eq!(context.tenant_id, Some(tenant_id));
    }

    #[tokio::test]
    async fn development_without_a_verifier_remains_explicitly_disabled() {
        let tenant_id = Uuid::now_v7();
        let context = AuthService::new(None, Some(tenant_id))
            .authenticate(None, RequiredPermission::Emergency)
            .await
            .unwrap();

        assert_eq!(context.method, AuthMethod::Disabled);
        assert_eq!(context.tenant_id, Some(tenant_id));
        assert_eq!(context.permissions(), vec!["observer", "operator", "funding", "emergency"]);
        assert_eq!(context.method.decision_auth_kind(), "asserted");
    }

    #[test]
    fn rbx_oidc_is_a_verified_recovery_identity() {
        assert_eq!(AuthMethod::RbxOidc.decision_auth_kind(), "verified");
    }
}
