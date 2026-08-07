# Robson Operator CLI

**Status**: Repository-verified on 2026-08-06

Robson has one narrow Rust operator binary at `robson-cli/`. It exists for exceptional recovery workflows. Routine position management uses the SvelteKit dashboard or the authenticated `robsond` API.

The former Bun and TypeScript CLI under `cli/` was removed. It was unpublished, unauthenticated for mutations, and had drifted from the daemon contract. Commands such as `robson arm`, `robson status`, `robson panic`, and `robson credentials` are not supported operator interfaces.

## Supported commands

| Command | Purpose | Operational class |
|---|---|---|
| `robson-cli reconcile-close` | Close a stale local Active position using reviewed exchange evidence | Irreversible recovery mutation |
| `robson-cli income ack` | Acknowledge one unmatched income-ledger item without deleting it | Audited ledger mutation |

Use `robson-cli --help` and the subcommand help for the exact flags supported by the checked-out revision.

## Build and distribution

The repository currently has no versioned, checksummed release artifact for this binary. The production container builds and copies `robsond` only. Do not assume `robson-cli` is installed in a daemon container or on an operator workstation.

For repository validation:

```bash
cargo build --release -p robson-cli
./target/release/robson-cli --help
```

For an operated environment, use only an approved binary built from a reviewed source revision compatible with the deployed `robsond` API. Building an arbitrary branch during an incident is not an acceptable distribution procedure.

## Authentication and connectivity

Both commands call the authenticated `robsond` HTTP API. The default base URL is `http://localhost:8080`.

Set the bearer token in the process environment:

```bash
export ROBSON_API_TOKEN
```

The shell must already hold the value. This documentation intentionally does not show a token value or an inline assignment.

Prefer the environment variable over `--token`. A command-line token may be retained in shell history or exposed in a process listing. For a remote daemon, use an approved secure tunnel or local port forward. Never send the bearer token over plaintext remote HTTP.

`ROBSON_OPERATOR_ID` may supply the default audited actor for `income ack`.

## Stale-Active recovery

Read and execute the operator procedure in [Stale-Active Recovery](runbooks/td-2026-05-05-001-stale-active-recovery.md). The command appends an irreversible terminal event and requires exchange-grade evidence.

Command shape:

```bash
robson-cli reconcile-close \
  --position-id <POSITION_UUID> \
  --evidence-file <REVIEWED_EVIDENCE_JSON> \
  --robsond-url http://localhost:8080
```

Do not use this command for routine exits, untracked exchange positions, or projection-only orphan repair.

## Income-ledger acknowledgement

Read [ADR-0045](adr/ADR-0045-income-ledger-reconciliation.md) and inspect the item before acknowledgement.

Command shape:

```bash
robson-cli income ack <EXCHANGE_INCOME_ID> \
  --reason "<AUDITABLE_REASON>" \
  --actor "<OPERATOR_ID>" \
  --robsond-url http://localhost:8080
```

Acknowledgement preserves the ledger item and records the reason, actor, and timestamp. It is not a deletion or a substitute for reconciliation.

## Safety boundaries

- The CLI does not grant operational authorization.
- Confirm daemon and CLI API compatibility before any mutation.
- Keep tokens out of arguments, files, logs, and screenshots.
- Capture command output and incident evidence in the approved audit channel.
- Stop on authentication, evidence-consistency, or version errors.
- Use the dashboard for supported routine actions and the runbooks for exceptional recovery.

## Distribution follow-up

A separate change should produce a versioned, checksummed `robson-cli` artifact tied to the same source revision as `robsond`. That work is intentionally outside this cleanup because changing runtime packaging affects backend release paths.
