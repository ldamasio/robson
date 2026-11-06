//! ADR-0056 step 4: replaying historical `EntryFilled` payloads through the
//! store keeps the venue position id, and the rebuilt `Position` serializes it
//! under the legacy key. Payloads are the golden fixtures captured before the
//! naming cleanup.

use robson_domain::{Event, Position, Side, Symbol};
use robson_store::{MemoryStore, PositionRepository, Store};
use uuid::Uuid;

const WIRE_KEY: &str = "binance_position_id";
const VENUE_POSITION: &str = "8389765512345";

const WITH_ID: &str =
    include_str!("../../robson-domain/tests/fixtures/adr0056_step4/event_entry_filled_value.json");
const HISTORICAL: &str = include_str!(
    "../../robson-domain/tests/fixtures/adr0056_step4/event_entry_filled_historical.input.json"
);
const MISSING: &str = include_str!(
    "../../robson-domain/tests/fixtures/adr0056_step4/event_entry_filled_missing.input.json"
);

async fn replay(payloads: &[&str]) -> serde_json::Value {
    let store = MemoryStore::new();
    let mut position_id = None;

    for payload in payloads {
        let event: Event = serde_json::from_str(payload).unwrap();
        let Event::EntryFilled { position_id: id, .. } = &event else {
            panic!("fixture is not an EntryFilled event");
        };
        if position_id.is_none() {
            let mut position =
                Position::new(Uuid::now_v7(), Symbol::from_pair("BTCUSDT").unwrap(), Side::Long);
            position.id = *id;
            PositionRepository::save(&store, &position).await.unwrap();
            position_id = Some(*id);
        }
        store.apply_event(&event).unwrap();
    }

    let position = PositionRepository::find_by_id(&store, position_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    serde_json::to_value(&position).unwrap()
}

#[tokio::test]
async fn replay_keeps_the_id_under_the_legacy_key() {
    for payload in [WITH_ID, HISTORICAL] {
        assert_eq!(replay(&[payload]).await[WIRE_KEY], VENUE_POSITION);
    }
}

#[tokio::test]
async fn replay_without_the_field_leaves_null() {
    // `get`, because indexing a missing key also reads as null.
    assert_eq!(replay(&[MISSING]).await.get(WIRE_KEY), Some(&serde_json::Value::Null));
}

#[tokio::test]
async fn replay_of_a_later_event_without_the_field_does_not_erase_the_id() {
    assert_eq!(replay(&[WITH_ID, MISSING]).await[WIRE_KEY], VENUE_POSITION);
}
