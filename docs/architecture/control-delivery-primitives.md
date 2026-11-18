# ADR-0056 step 8: control-delivery identity primitives

Status: inert data definitions; operational rollout and durable delivery remain pending.

`robson_eventlog::EventCoordinates` holds event/tenant/stream identity, a positive
sequence, and original occurrence/ingestion timestamps. Fields are public and
untrusted. Explicit `validate()` checks shape only: nonnil event/tenant UUIDs,
nonblank stream and positive sequence. It does not contact storage, authenticate
the reference, prove a commit, or establish freshness. The ingestion-month helper
uses the UTC `ingested_at` timestamp because event-log partitions use ingestion
time; a delayed event's `occurred_at` may belong to another month. Neither this
helper nor the data type authorizes deletion or retention of any partition.

`robsond::control_delivery::DeliveryIdentity` checks a nonnil generation, positive
epoch and a single nonnil UUID shared by query, signal and cycle. Its private
fields preserve that relation after construction. It proves neither ownership
of a generation nor a durable disposition. `GenerationState` and
`QueryDeliveryStatus` define the reviewed serialized vocabulary, not a state
machine or admission decision. There is no trusted receipt or send-token type,
conversion, implicit clock read, background task, database call or HTTP request.
Constructing and checking these values performs 0 SQL and 0 HTTP requests.

Runtime producer, consumer, projector, detector, recovery, admission and venue
paths do not call these definitions. Existing USD-M behavior and financial
policy are unchanged. Tests cover malformed identity/epoch/sequence, exact enum
vocabulary and an event crossing an ingestion-month boundary; runtime and
cross-process delivery properties remain later integration work.

The separately planned storage slice owns migration 43, catalog verification
and its bounded migrator route. No migration or dispatch change is included
here. Its application and PostgreSQL acceptance require actual approved
migrations 38–42 and the real composed adapter; placeholders, fabricated
migration metadata and out-of-order application are forbidden. The atomic
writer, immutable original signal payload, committed receipt, projection,
rebuild, retention and activation have their own subsequent review gates.
