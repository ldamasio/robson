# bronze-c1 · Canonical byte profile (normative)

**Status**: Proposed (Phase B prerequisite of bronze-v1 §3.3)
**Version**: c1 (any change is a new profile id and storage prefix)
**Owner**: bronze-v1 data contract (Robson)
**Golden vectors**: `bronze-c1-golden/` (committed; regeneration script included)

This document defines "byte-identical" for bronze objects. The exporter
implementation conforms to this specification, not the other way around; an
exporter lockfile pins library versions but never defines the bytes.

## 1. Row model

A bronze line is one JSON object per source row, LF-terminated (`0x0A`),
including the final line of a part. No other whitespace anywhere.

### 1.1 `event_log` row: 18 envelope fields, in exactly this order

```
event_id, tenant_id, stream_key, seq, event_type, payload,
payload_schema_version, occurred_at, ingested_at, idempotency_key,
trace_id, causation_id, command_id, workflow_id, actor_type, actor_id,
prev_hash, hash
```

All 18 keys are always present; SQL `NULL` becomes JSON `null`.

### 1.2 `income_ledger` row: 9 fields, in exactly this order

```
id, exchange_income_id, symbol, income_type, amount, asset,
exchange_trade_id, income_time, created_at
```

All 9 keys always present; SQL `NULL` becomes JSON `null`.

## 2. Scalar encodings

| SQL type | JSON encoding |
|---|---|
| `UUID` | lowercase hyphenated string |
| `TIMESTAMPTZ` | string `YYYY-MM-DDTHH:MM:SS.ffffffZ`, UTC, always exactly 6 fractional digits |
| `NUMERIC` | JSON **string** containing PostgreSQL `numeric::text` output (exact stored precision, no normalization) |
| `BIGINT` / `INT` | JSON number, decimal, no exponent |
| `TEXT` / `VARCHAR` | JSON string |
| SQL `NULL` | JSON `null` |

## 3. `payload` (JSONB) re-emission

- The stored JSONB value is parsed and re-emitted; the runtime Rust types
  are never involved (bronze-v1 §2.1 raw-JSONB rule).
- Object keys at **every** level inside `payload` are sorted by byte order
  of their UTF-8 encoding. (The top-level row object is the §1 exception:
  fixed column order.)
- JSONB scalars re-emit as: strings per §4; numbers as the PostgreSQL
  `jsonb` canonical numeric text for the stored value (jsonb already
  normalizes numeric storage; c1 freezes its text form, including preserved
  trailing zeros); booleans `true`/`false`; `null` as `null`. Duplicate
  keys cannot occur (jsonb keeps last-writer only).
- Redaction (bronze-v1 §2.3 + registry) is applied **before**
  canonicalization and hashing; a redacted value is the JSON string
  `"[redacted-bronze-v1]"`.

## 4. String escaping

UTF-8 throughout; non-ASCII characters are emitted raw (no `\u` escaping).
Escaped are exactly: `"` as `\"`, `\` as `\\`, and control characters
U+0000..U+001F using the short forms `\b \t \n \f \r` where they exist and
`\u00XX` (lowercase hex) otherwise. The solidus `/` is never escaped.

## 5. Parts (deterministic split)

- Rows in canonical cursor order (bronze-v1 §2: `(ingested_at, event_id)`
  ascending; `(created_at, exchange_income_id COLLATE "C")` ascending).
- `MAX_PART_BYTES = 536870912` (512 MiB) of uncompressed canonical NDJSON.
  Greedy split: rows are appended to the current part in cursor order; if
  appending the next row would make the part exceed `MAX_PART_BYTES`, the
  current part is closed and the row starts the next part. A single row
  larger than `MAX_PART_BYTES` forms a part of its own (never split inside
  a row). This yields exactly one valid partition of any row sequence.
- An empty window has zero parts (marker only, `rows: 0`).
- Part object names: `part-00000.ndjson.zst`, `part-00001.ndjson.zst`, ...
  (five-digit zero-padded ascending index, maximum index 99999; a window
  needing more parts is a fatal error) under the window prefix of
  bronze-v1 §3.1 (which names parts as `part-<index>.ndjson.zst`).

## 6. Compression (fixed tuple; any change = new profile)

`c1` fixes the complete zstd tuple; the exporter implements it and MUST NOT
derive any parameter from library defaults:

- libzstd version: **1.5.7** (the normative encoder for `c1`);
- `compressionLevel = 19`;
- `windowLog = 23` (set explicitly, never level-derived);
- `checksumFlag = 1` (content checksum present);
- `contentSizeFlag = 1` with the exact uncompressed size declared in the
  frame header (single-shot compression of the fully materialized part);
- `nbWorkers = 0` (single-thread mode);
- standard frame, no dictionary, one frame per part, single finalization,
  no intermediate flush;
- **fresh compression context per part** (new `ZSTD_CCtx` or a full
  `ZSTD_CCtx_reset` of parameters and session): sticky parameters from a
  previous part are prohibited. Every parameter not listed in this tuple
  takes the libzstd 1.5.7 internal resolution for level 19 with the
  content size known; manually setting any other parameter is prohibited.

Changing any element of this tuple (including the libzstd version) is a
new profile id and a new storage prefix. **zstd frame golden vectors are
committed in the exporter repository (rbx-data)** against exactly this
tuple; the vectors in this repository freeze the canonical (uncompressed)
byte layer, which hashing verifies first.

## 7. Hashes

SHA-256, lowercase hex. Every part records two hashes in the commit marker:
`sha256_ndjson` (uncompressed canonical bytes) and `sha256_stored`
(compressed object as stored).

## 8. `commit.json` canonical encoding

Same scalar/escaping rules as §2/§4; the marker has a **fixed key order**
(recursive sorting does not apply):

```
table, env, window_start, window_end, rows, profile,
payload_schema_version_min, payload_schema_version_max, parts
```

- `table` ∈ {`event_log`, `income_ledger`}; `env` ∈ {`prod`, `testnet`}.
- `window_start`/`window_end`: §2 timestamp format (UTC, 6 fractional
  digits), half-open bounds.
- `rows`: non-negative integer; equals the sum of part `rows`.
- `profile`: the literal string `c1`. The zstd tuple, including libzstd
  1.5.7, is a constant of the profile and is NOT repeated in the marker.
- **No per-run and no per-build values**: `run_id`, exporter build/version
  and hostnames live in exporter logs only. Identical window content
  yields a byte-identical marker.
- `payload_schema_version_min/max` per bronze-v1 §3.1: numeric or both
  `null` for `event_log`; keys entirely omitted for `income_ledger`.
- `parts`: array ordered by ascending part index; each element is an
  object with fixed key order `name`, `rows`, `bytes_ndjson`,
  `bytes_stored`, `sha256_ndjson`, `sha256_stored` (`rows`/`bytes_*`
  non-negative integers, hashes lowercase hex).
- Single LF-terminated line; no compression for the marker.

## 9. Golden vectors

`bronze-c1-golden/` contains, per vector: `input.json` (parsed source row,
deliberately unordered keys) and `expected.ndjson` (exact canonical bytes),
plus `SHA256SUMS`. `regen.py` re-derives every `expected.ndjson` from
`input.json` per this spec; CI or a reviewer can run it and diff. Vectors:

1. `vector-01-position-armed`: envelope with nulls, nested payload with
   shuffled keys, non-ASCII text.
2. `vector-02-entry-execution-rejected`: redaction applied to `$.reason`.
3. `vector-03-income-ledger`: 9-column row, NUMERIC-as-string, SQL NULL.
4. `vector-04-jsonb-edge-cases`: JSONB numbers (decimal with preserved
   trailing zeros, exponent normalization `1.230e-5`, integer beyond 64
   bits), arrays, objects inside arrays, non-ASCII keys, surviving
   escapes (`"`,`\`, C0 controls) outside redacted fields.
5. `vector-05-array-redaction`: `reasons[*]` wildcard redaction with a
   `null` element preserved, alongside a machine-code exception field kept
   verbatim.
6. `vector-06-multiline`: two rows in cursor order in one part (LF after
   every line, including the last).
7. `vector-07-markers`: canonical `commit.json` fixtures for a non-empty
   `event_log` window, an empty `event_log` window and an `income_ledger`
   window.

An exporter claiming `profile: c1` MUST reproduce all `expected.*` bytes
exactly and MUST pass cross-run golden tests (same input, same bytes,
across processes and hosts).
