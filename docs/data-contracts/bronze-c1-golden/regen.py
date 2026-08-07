#!/usr/bin/env python3
"""Regenerate bronze-c1 golden vectors from their input.json files.

Implements bronze-c1.md sections 1-4 and 8 (canonical byte layer; the
part-split algorithm of section 5 and the zstd frames of section 6 are
exporter territory). Fail-closed: unknown tables, invalid scalar kinds
and unsupported constructs abort.

    python3 regen.py          # rewrites expected files + SHA256SUMS
    python3 regen.py --check  # verifies without writing; exit 1 on drift
"""
import hashlib
import json
import re
import sys
from decimal import Decimal
from pathlib import Path

EVENT_LOG_COLUMNS = [
    "event_id", "tenant_id", "stream_key", "seq", "event_type", "payload",
    "payload_schema_version", "occurred_at", "ingested_at",
    "idempotency_key", "trace_id", "causation_id", "command_id",
    "workflow_id", "actor_type", "actor_id", "prev_hash", "hash",
]
INCOME_COLUMNS = [
    "id", "exchange_income_id", "symbol", "income_type", "amount", "asset",
    "exchange_trade_id", "income_time", "created_at",
]
MARKER_KEYS = [
    "table", "env", "window_start", "window_end", "rows", "profile",
    "payload_schema_version_min", "payload_schema_version_max", "parts",
]
PART_KEYS = ["name", "rows", "bytes_ndjson", "bytes_stored", "sha256_ndjson", "sha256_stored"]
REDACTED = "[redacted-bronze-v1]"
TS_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z$")
HASH_RE = re.compile(r"^[0-9a-f]{64}$")
PART_NAME_RE = re.compile(r"^part-\d{5}\.ndjson\.zst$")


def is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def req_str(row, col, nullable=False):
    v = row[col]
    if v is None and nullable:
        return
    if not isinstance(v, str):
        die(f"{col} must be a JSON string" + (" or null" if nullable else ""))

_SHORT = {0x08: "\\b", 0x09: "\\t", 0x0A: "\\n", 0x0C: "\\f", 0x0D: "\\r"}


def die(msg: str):
    raise SystemExit(f"regen: {msg}")


def esc(s: str) -> str:
    out = []
    for ch in s:
        o = ord(ch)
        if ch == '"':
            out.append('\\"')
        elif ch == "\\":
            out.append("\\\\")
        elif o < 0x20:
            out.append(_SHORT.get(o, f"\\u{o:04x}"))
        else:
            out.append(ch)
    return "".join(out)


def emit_number(value) -> str:
    # c1 §3: PostgreSQL jsonb numeric text: plain decimal, no exponent,
    # trailing zeros of the stored value preserved. Decimal keeps the
    # coefficient digits; '{:f}' never uses scientific notation.
    if isinstance(value, int):
        return str(value)
    if isinstance(value, Decimal):
        return "{:f}".format(value)
    die(f"float leaked into number path: {value!r}")


def emit(value, sort_keys: bool) -> str:
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    if isinstance(value, str):
        return f'"{esc(value)}"'
    if isinstance(value, (int, Decimal)):
        return emit_number(value)
    if isinstance(value, float):
        die("IEEE-754 float encountered; inputs must parse numbers as Decimal")
    if isinstance(value, list):
        return "[" + ",".join(emit(v, sort_keys) for v in value) + "]"
    if isinstance(value, dict):
        keys = sorted(value.keys(), key=lambda k: k.encode("utf-8")) if sort_keys else list(value.keys())
        return "{" + ",".join(f'"{esc(k)}":{emit(value[k], True)}' for k in keys) + "}"
    die(f"unsupported type: {type(value)}")


def redact_path(node, parts):
    """Dotted paths with [*] array wildcard; strings are replaced, null kept."""
    if not parts:
        return
    head, rest = parts[0], parts[1:]
    if head.endswith("[*]"):
        key = head[:-3]
        arr = node.get(key) if isinstance(node, dict) else None
        if isinstance(arr, list):
            if rest:
                for item in arr:
                    redact_path(item, rest)
            else:
                for i, item in enumerate(arr):
                    if isinstance(item, str):
                        arr[i] = REDACTED
                    elif item is not None:
                        die(f"redaction wildcard hit non-string non-null: {item!r}")
        return
    if not isinstance(node, dict) or head not in node:
        return
    if rest:
        redact_path(node[head], rest)
    else:
        if isinstance(node[head], str):
            node[head] = REDACTED
        elif node[head] is not None:
            die(f"redaction path hit non-string non-null: {node[head]!r}")


def apply_redaction(payload: dict, paths):
    for p in paths:
        if not p.startswith("$."):
            die(f"unsupported redaction path: {p}")
        redact_path(payload, p[2:].split("."))
    return payload


def validate_row(table: str, row: dict, columns):
    if set(row.keys()) != set(columns):
        die(f"{table}: key set mismatch: missing={set(columns)-set(row)} extra={set(row)-set(columns)}")
    if table == "event_log":
        if not is_int(row["seq"]):
            die("event_log.seq must be an integer")
        if not is_int(row["payload_schema_version"]):
            die("payload_schema_version must be an integer")
        for c in ("event_id", "tenant_id", "stream_key", "event_type", "idempotency_key"):
            req_str(row, c)
        for c in ("trace_id", "causation_id", "command_id", "workflow_id",
                  "actor_type", "actor_id", "prev_hash", "hash"):
            req_str(row, c, nullable=True)
        if not isinstance(row["payload"], dict):
            die("payload must be a JSON object")
        for c in ("occurred_at", "ingested_at"):
            if not (isinstance(row[c], str) and TS_RE.fullmatch(row[c])):
                die(f"{c} must match the c1 timestamp format")
    else:
        # c1 §2: NUMERIC (amount) is a JSON string; ids/types are strings;
        # symbol and exchange_trade_id are nullable strings.
        for c in ("id", "exchange_income_id", "income_type", "amount", "asset"):
            req_str(row, c)
        for c in ("symbol", "exchange_trade_id"):
            req_str(row, c, nullable=True)
        for c in ("income_time", "created_at"):
            if not (isinstance(row[c], str) and TS_RE.fullmatch(row[c])):
                die(f"{c} must match the c1 timestamp format")


def canonical_row(table: str, row: dict, redact) -> bytes:
    if table == "event_log":
        columns = EVENT_LOG_COLUMNS
    elif table == "income_ledger":
        columns = INCOME_COLUMNS
    else:
        die(f"unknown table: {table!r}")
    validate_row(table, row, columns)
    if table == "event_log":
        row["payload"] = apply_redaction(row["payload"], redact)
    fields = ",".join(f'"{esc(c)}":{emit(row[c], c == "payload")}' for c in columns)
    return ("{" + fields + "}\n").encode("utf-8")


def canonical_marker(marker: dict) -> bytes:
    keys = list(marker.keys())
    expected = [k for k in MARKER_KEYS if k in marker]
    if keys != expected:
        die(f"marker keys must follow the fixed order; got {keys}")
    if marker["table"] not in ("event_log", "income_ledger"):
        die("marker table must be event_log or income_ledger")
    if marker["env"] not in ("prod", "testnet"):
        die("marker env must be prod or testnet")
    if marker["profile"] != "c1":
        die("marker profile must be the literal c1")
    if not is_int(marker["rows"]) or marker["rows"] < 0:
        die("marker rows must be a non-negative integer")
    for c in ("window_start", "window_end"):
        if not (isinstance(marker[c], str) and TS_RE.fullmatch(marker[c])):
            die(f"marker {c} must match the c1 timestamp format")
    for k in ("payload_schema_version_min", "payload_schema_version_max"):
        if k in marker and marker[k] is not None and not is_int(marker[k]):
            die(f"marker {k} must be an integer or null")
    if marker["table"] == "income_ledger":
        if "payload_schema_version_min" in marker or "payload_schema_version_max" in marker:
            die("income_ledger marker must omit payload_schema_version_*")
    else:
        for k in ("payload_schema_version_min", "payload_schema_version_max"):
            if k not in marker:
                die(f"event_log marker must carry {k} (numeric or null)")
    for idx, part in enumerate(marker["parts"]):
        if list(part.keys()) != PART_KEYS:
            die(f"part keys must be exactly {PART_KEYS}")
        if not (isinstance(part["name"], str) and PART_NAME_RE.fullmatch(part["name"])):
            die("part name must match part-NNNNN.ndjson.zst")
        if part["name"] != f"part-{idx:05d}.ndjson.zst":
            die("part names must be ascending from part-00000")
        for k in ("rows", "bytes_ndjson", "bytes_stored"):
            if not is_int(part[k]) or part[k] < 0:
                die(f"part {k} must be a non-negative integer")
        for k in ("sha256_ndjson", "sha256_stored"):
            if not (isinstance(part[k], str) and HASH_RE.fullmatch(part[k])):
                die(f"part {k} must be 64 lowercase hex chars")
    if marker["rows"] != sum(p["rows"] for p in marker["parts"]):
        die("marker rows must equal the sum of part rows")
    fields = ",".join(f'"{esc(k)}":{emit(marker[k], False)}' for k in marker)
    return ("{" + fields + "}\n").encode("utf-8")


def load_input(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"), parse_float=Decimal)


def derive(spec: dict) -> list[tuple[str, bytes]]:
    kind = spec.get("kind", "rows")
    if kind == "rows":
        table = spec["table"]
        rows = spec["rows"] if "rows" in spec else [spec["row"]]
        redact = spec.get("redact", [])
        out = b"".join(canonical_row(table, r, redact) for r in rows)
        return [("expected.ndjson", out)]
    if kind == "markers":
        return [
            (f"expected-{name}.json", canonical_marker(marker))
            for name, marker in spec["markers"].items()
        ]
    die(f"unknown vector kind: {kind!r}")


def main() -> int:
    check = "--check" in sys.argv
    here = Path(__file__).parent
    sums = []
    drift = False
    for d in sorted(p for p in here.iterdir() if p.is_dir()):
        spec = load_input(d / "input.json")
        for fname, out in derive(spec):
            target = d / fname
            if check:
                if not target.exists() or target.read_bytes() != out:
                    print(f"DRIFT: {target}")
                    drift = True
            else:
                target.write_bytes(out)
            sums.append(f"{hashlib.sha256(out).hexdigest()}  {d.name}/{fname}")
    sums_text = "\n".join(sums) + "\n"
    sums_file = here / "SHA256SUMS"
    if check:
        if not sums_file.exists() or sums_file.read_text() != sums_text:
            print("DRIFT: SHA256SUMS")
            drift = True
        print("golden vectors: " + ("DRIFT DETECTED" if drift else "OK"))
        return 1 if drift else 0
    sums_file.write_text(sums_text)
    print(f"regenerated {len(sums)} artifacts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
