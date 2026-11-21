//! Inert control-delivery identity data. These types grant no receipt/send
//! authority.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationState {
    Ready,
    Evaluating,
    Accepted,
    Claimed,
    Held,
    Spent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryDeliveryStatus {
    Accepted,
    Claimed,
    Held,
    Terminal,
}

/// Validated identity relation only; neither ownership nor persistence
/// evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryIdentity {
    generation: Uuid,
    epoch: i64,
    query: Uuid,
}

impl DeliveryIdentity {
    pub fn new(
        generation: Uuid,
        epoch: i64,
        query: Uuid,
        signal: Uuid,
        cycle: Uuid,
    ) -> Option<Self> {
        (epoch > 0 && !generation.is_nil() && !query.is_nil() && query == signal && query == cycle)
            .then_some(Self { generation, epoch, query })
    }

    pub fn generation(self) -> Uuid {
        self.generation
    }
    pub fn epoch(self) -> i64 {
        self.epoch
    }
    pub fn query(self) -> Uuid {
        self.query
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_rejects_every_mismatched_or_empty_component() {
        let generation = Uuid::from_u128(1);
        let query = Uuid::from_u128(2);
        let other = Uuid::from_u128(3);
        let identity = DeliveryIdentity::new(generation, 1, query, query, query).unwrap();
        assert_eq!(
            (identity.generation(), identity.epoch(), identity.query()),
            (generation, 1, query)
        );
        for (g, e, q, s, c) in [
            (Uuid::nil(), 1, query, query, query),
            (generation, 0, query, query, query),
            (generation, -1, query, query, query),
            (generation, 1, Uuid::nil(), Uuid::nil(), Uuid::nil()),
            (generation, 1, other, query, query),
            (generation, 1, query, other, query),
            (generation, 1, query, query, other),
        ] {
            assert!(DeliveryIdentity::new(g, e, q, s, c).is_none());
        }
        assert!(DeliveryIdentity::new(generation, i64::MAX, query, query, query).is_some());
    }

    #[test]
    fn state_vocabulary_is_exact_and_unknown_values_are_not_authority() {
        for (state, name) in [
            (GenerationState::Ready, "ready"),
            (GenerationState::Evaluating, "evaluating"),
            (GenerationState::Accepted, "accepted"),
            (GenerationState::Claimed, "claimed"),
            (GenerationState::Held, "held"),
            (GenerationState::Spent, "spent"),
        ] {
            let encoded = serde_json::to_value(state).unwrap();
            assert_eq!(encoded, name);
            assert_eq!(serde_json::from_value::<GenerationState>(encoded).unwrap(), state);
        }
        for (state, name) in [
            (QueryDeliveryStatus::Accepted, "accepted"),
            (QueryDeliveryStatus::Claimed, "claimed"),
            (QueryDeliveryStatus::Held, "held"),
            (QueryDeliveryStatus::Terminal, "terminal"),
        ] {
            let encoded = serde_json::to_value(state).unwrap();
            assert_eq!(encoded, name);
            assert_eq!(serde_json::from_value::<QueryDeliveryStatus>(encoded).unwrap(), state);
        }
        for invalid in ["null", "1", "\"unknown\"", "\"Accepted\""] {
            assert!(serde_json::from_str::<GenerationState>(invalid).is_err());
            assert!(serde_json::from_str::<QueryDeliveryStatus>(invalid).is_err());
        }
    }
}
