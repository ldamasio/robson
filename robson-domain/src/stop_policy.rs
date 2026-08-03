//! Stop-policy versioning (issue #154, ADR-0050 §3/§4).
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
    /// ADR-0050 §3/§4 derivation: span-capped buffer
    /// (`min(configured, 0.25 x span)`), single tick-quantized executable
    /// trigger consumed by every surface, adverse-fill costing. Requires
    /// runtime [`crate::trading_rules::SymbolTradingRules`].
    SpanCappedV1,
}

impl StopPolicy {
    /// Canonical wire/DB string for this policy.
    pub fn as_str(&self) -> &'static str {
        match self {
            StopPolicy::LegacyUncapped => "legacy_uncapped",
            StopPolicy::SpanCappedV1 => "span_capped_v1",
        }
    }

    /// Parse a persisted policy string. Unknown values are an error:
    /// a corrupted or future policy must fail loudly, never demote to
    /// legacy behavior on capital-real paths.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "legacy_uncapped" => Ok(StopPolicy::LegacyUncapped),
            "span_capped_v1" => Ok(StopPolicy::SpanCappedV1),
            other => Err(DomainError::InvalidStopPolicy(format!(
                "Unknown stop policy '{other}' (expected legacy_uncapped or span_capped_v1)"
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
        assert_eq!(serde_json::to_value(StopPolicy::SpanCappedV1).unwrap(), "span_capped_v1");
    }

    #[test]
    fn parse_accepts_known_and_rejects_unknown() {
        assert_eq!(StopPolicy::parse("legacy_uncapped").unwrap(), StopPolicy::LegacyUncapped);
        assert_eq!(StopPolicy::parse("span_capped_v1").unwrap(), StopPolicy::SpanCappedV1);
        assert!(StopPolicy::parse("span_capped_v2").is_err());
        assert!(StopPolicy::parse("").is_err());
    }

    #[test]
    fn unknown_wire_value_fails_deserialization_never_defaults() {
        // serde must NOT have an `other`-style fallback: an unknown version
        // is a hard failure, only a MISSING field means legacy.
        let err = serde_json::from_value::<StopPolicy>(serde_json::json!("span_capped_v9"));
        assert!(err.is_err());
    }

    #[test]
    fn default_is_legacy() {
        assert_eq!(StopPolicy::default(), StopPolicy::LegacyUncapped);
    }
}
