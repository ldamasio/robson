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

Since ADR-0054, the CLI authenticates as the operator's Google account via
the OAuth 2.0 Device Authorization Grant — there is no bearer token to
export or pass on the command line.

Sign in once per machine:

```bash
robson-cli auth login
```

This prints a short user code and a Google verification URL. Open the URL
on any device, enter the code, and approve. `robson-cli` polls until
approval, then caches `{id_token, refresh_token, expiry}` at
`~/.config/robson/credentials.json` (or under `$XDG_CONFIG_HOME` if set)
with `0600` permissions. `reconcile-close` and `income ack` read this
cache automatically and refresh it transparently as it nears expiry — no
further flags are needed.

Other `auth` subcommands:

```bash
robson-cli auth status        # print the cached email/expiry — no network call
robson-cli auth logout        # delete the cached credential
robson-cli auth print-token   # print the current (refreshed) ID token, for scripting/curl
```

If neither command finds a cached credential, it fails with `run
\`robson auth login\` first` rather than silently falling back to an
unauthenticated request. For a remote daemon, use an approved secure
tunnel or local port forward — never send the bearer credential over
plaintext remote HTTP.

`ROBSON_OPERATOR_ID` may supply the default audited actor for `income ack`.

**Legacy note:** `ROBSON_API_TOKEN` / `--token` are removed. Setting
`ROBSON_API_TOKEN` on the `robsond` side now fails startup loudly (see
ADR-0054) rather than being silently accepted; a temporary rollout bridge
exists under the separately-named `ROBSON_LEGACY_API_TOKEN` for
production continuity during the Google-auth cutover, but it is not a
CLI-facing option and is removed once the repo owner confirms Google
auth is stable in production.

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
- Keep credentials out of arguments, logs, and screenshots. The cached
  `~/.config/robson/credentials.json` (mode `0600`) holds a live
  refresh token — treat it like the old bearer token: do not copy it
  into tickets, chat, or shared machines.
- Capture command output and incident evidence in the approved audit channel.
- Stop on authentication, evidence-consistency, or version errors.
- Use the dashboard for supported routine actions and the runbooks for exceptional recovery.

## Distribution follow-up

A separate change should produce a versioned, checksummed `robson-cli` artifact tied to the same source revision as `robsond`. That work is intentionally outside this cleanup because changing runtime packaging affects backend release paths.
