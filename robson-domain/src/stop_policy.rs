//! Stop-policy versioning (ADR-0050 §3/§4, ADR-0052).
//!
//! A position's stop derivation is pinned at arm time and never changes for
//! the lifetime of the position: a deploy must never retroact on live
//! positions. Positions armed before versioning existed replay as
//! [`StopPolicy::LegacyUncapped`] (missing field = legacy); an UNKNOWN
//! persisted value is a deserialization failure, never a silent legacy
//! fallback.

use serde::{Deserialize, Serialize};

use crate::value_objects::DomainError;

/// Stop-policy version pinned to a position at arm time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopPolicy {
    /// Historical derivation: uncapped ADR-0041 buffer over the guard-aware
    /// basis, no domain-side tick quantization (the exchange adapter aligns
    /// the trigger at placement). Every position armed before stop-policy
    /// versioning replays as this.
    #[default]
    LegacyUncapped,
    /// ADR-0052 derivation: the immutable, persisted executable span drives
    /// the trailing ladder while the buffer remains capped at 0.25 x the
    /// cap-basis distance. The single adversely tick-quantized trigger is
    /// consumed by every execution surface.
    ExecutableSpan,
}

impl StopPolicy {
    /// Canonical wire/DB string for this policy.
    pub fn as_str(&self) -> &'static str {
        match self {
            StopPolicy::LegacyUncapped => "legacy_uncapped",
            StopPolicy::ExecutableSpan => "executable_span",
        }
    }

    /// Parse a persisted policy string. Unknown values are an error:
    /// a corrupted or future policy must fail loudly, never demote to
    /// legacy behavior on capital-real paths.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "legacy_uncapped" => Ok(StopPolicy::LegacyUncapped),
            "executable_span" => Ok(StopPolicy::ExecutableSpan),
            other => Err(DomainError::InvalidStopPolicy(format!(
                "Unknown stop policy '{other}' (expected legacy_uncapped or executable_span)"
            ))),
        }
    }
}

impl std::fmt::Display for StopPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_snake_case() {
        assert_eq!(serde_json::to_value(StopPolicy::LegacyUncapped).unwrap(), "legacy_uncapped");
        assert_eq!(serde_json::to_value(StopPolicy::ExecutableSpan).unwrap(), "executable_span");
    }

    #[test]
    fn parse_accepts_known_and_rejects_unknown() {
        assert_eq!(StopPolicy::parse("legacy_uncapped").unwrap(), StopPolicy::LegacyUncapped);
        assert_eq!(StopPolicy::parse("executable_span").unwrap(), StopPolicy::ExecutableSpan);
        assert!(StopPolicy::parse("executable_span_v2").is_err());
        assert!(StopPolicy::parse("").is_err());
    }

    #[test]
    fn unknown_wire_value_fails_deserialization_never_defaults() {
        // serde must NOT have an `other`-style fallback: an unknown version
        // is a hard failure, only a MISSING field means legacy.
        let err = serde_json::from_value::<StopPolicy>(serde_json::json!("future_stop_policy"));
        assert!(err.is_err());
    }

    #[test]
    fn default_is_legacy() {
        assert_eq!(StopPolicy::default(), StopPolicy::LegacyUncapped);
    }
}
