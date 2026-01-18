//! Idempotency Key Computation
//!
//! Computes deterministic hash of event payload for deduplication.
//! Ignores non-deterministic fields (timestamps, actor IDs).

use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Compute idempotency key from event metadata
///
/// Formula: SHA256(tenant_id + stream_key + command_id + normalized_payload)
///
/// # Arguments
/// * `tenant_id` - Tenant UUID
/// * `stream_key` - Stream key (e.g., "position:uuid")
/// * `command_id` - Optional command UUID
/// * `payload` - Event payload (will be normalized)
///
/// # Returns
/// Idempotency key as hex string prefixed with "idem_"
pub fn compute_idempotency_key(
    tenant_id: Uuid,
    stream_key: &str,
    command_id: Option<Uuid>,
    payload: &serde_json::Value,
) -> String {
    let normalized_payload = normalize_payload(payload);

    let mut hasher = Sha256::new();
    hasher.update(tenant_id.as_bytes());
    hasher.update(stream_key.as_bytes());

    if let Some(cmd_id) = command_id {
        hasher.update(cmd_id.as_bytes());
    }

    hasher.update(normalized_payload.as_bytes());

    let hash = hasher.finalize();
    format!("idem_{}", hex::encode(hash))
}

/// Normalize payload by removing non-deterministic fields
///
/// Removes:
/// - Timestamps (occurred_at, ingested_at, created_at, updated_at, *_at)
/// - Actor identity (actor_id, user_id)
/// - Request IDs (request_id, trace_id unless semantic)
///
/// Keeps:
/// - All business data (prices, quantities, symbols, etc.)
/// - Command parameters
/// - Entity IDs (position_id, order_id, etc.)
fn normalize_payload(payload: &serde_json::Value) -> String {
    let normalized = match payload {
        serde_json::Value::Object(map) => {
            let mut normalized_map = serde_json::Map::new();

            for (key, value) in map {
                // Skip timestamp fields
                if key.ends_with("_at") || key == "timestamp" {
                    continue;
                }

                // Skip actor identity
                if key == "actor_id" || key == "actor_type" {
                    continue;
                }

                // Skip non-semantic request IDs
                if key == "request_id" {
                    continue;
                }

                // Recursively normalize nested objects
                let normalized_value = match value {
                    serde_json::Value::Object(_) => {
                        serde_json::from_str(&normalize_payload(value)).unwrap_or(value.clone())
                    }
                    _ => value.clone(),
                };

                normalized_map.insert(key.clone(), normalized_value);
            }

            serde_json::Value::Object(normalized_map)
        }
        other => other.clone(),
    };

    // Serialize with sorted keys for determinism
    serde_json::to_string(&normalized).unwrap_or_else(|_| payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_compute_idempotency_key_deterministic() {
        let tenant_id = Uuid::new_v4();
        let stream_key = "position:test";
        let command_id = Some(Uuid::new_v4());
        let payload = json!({
            "position_id": "pos123",
            "symbol": "BTCUSDT",
            "price": "95000.00"
        });

        let key1 = compute_idempotency_key(tenant_id, stream_key, command_id, &payload);
        let key2 = compute_idempotency_key(tenant_id, stream_key, command_id, &payload);

        assert_eq!(key1, key2, "Idempotency key should be deterministic");
        assert!(key1.starts_with("idem_"), "Key should have idem_ prefix");
    }

    #[test]
    fn test_normalize_payload_removes_timestamps() {
        let payload = json!({
            "position_id": "pos123",
            "symbol": "BTCUSDT",
            "price": "95000.00",
            "occurred_at": "2024-01-15T10:00:00Z",
            "created_at": "2024-01-15T10:00:00Z"
        });

        let normalized = normalize_payload(&payload);

        assert!(normalized.contains("position_id"));
        assert!(normalized.contains("symbol"));
        assert!(normalized.contains("price"));
        assert!(!normalized.contains("occurred_at"));
        assert!(!normalized.contains("created_at"));
    }

    #[test]
    fn test_normalize_payload_removes_actor() {
        let payload = json!({
            "order_id": "order123",
            "actor_type": "CLI",
            "actor_id": "user456"
        });

        let normalized = normalize_payload(&payload);

        assert!(normalized.contains("order_id"));
        assert!(!normalized.contains("actor_type"));
        assert!(!normalized.contains("actor_id"));
    }

    #[test]
    fn test_idempotency_key_different_for_different_payloads() {
        let tenant_id = Uuid::new_v4();
        let stream_key = "position:test";
        let command_id = Some(Uuid::new_v4());

        let payload1 = json!({"position_id": "pos123", "price": "95000"});
        let payload2 = json!({"position_id": "pos123", "price": "96000"});

        let key1 = compute_idempotency_key(tenant_id, stream_key, command_id, &payload1);
        let key2 = compute_idempotency_key(tenant_id, stream_key, command_id, &payload2);

        assert_ne!(key1, key2, "Different payloads should have different keys");
    }

    #[test]
    fn test_idempotency_key_ignores_timestamp_changes() {
        let tenant_id = Uuid::new_v4();
        let stream_key = "position:test";
        let command_id = Some(Uuid::new_v4());

        let payload1 = json!({
            "position_id": "pos123",
            "price": "95000",
            "occurred_at": "2024-01-15T10:00:00Z"
        });
        let payload2 = json!({
            "position_id": "pos123",
            "price": "95000",
            "occurred_at": "2024-01-15T11:00:00Z"  // Different timestamp
        });

        let key1 = compute_idempotency_key(tenant_id, stream_key, command_id, &payload1);
        let key2 = compute_idempotency_key(tenant_id, stream_key, command_id, &payload2);

        assert_eq!(key1, key2, "Timestamp changes should not affect idempotency key");
    }
}
