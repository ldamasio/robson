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

Since ADR-0057, the CLI authenticates with Google through RBX Identity/ZITADEL
using the OAuth 2.0 Device Authorization Grant. `robsond` never accepts a
Google-issued token or a static API token directly.

Configure the registered RBX public client before signing in:

```bash
export ROBSON_CLI_OIDC_ISSUER=https://auth.merovelis.com
export ROBSON_CLI_OIDC_CLIENT_ID=<registered-public-client-id>
export ROBSON_CLI_OIDC_SCOPES='openid profile email offline_access urn:zitadel:iam:org:id:<organization-id> urn:zitadel:iam:org:project:id:<api-project-id>:aud'
```

The client ID, organization ID, and API project ID are public registration
metadata, not secrets. The organization scope selects the
organization-specific login policy and role context. The registered public
client must assert project roles in its access tokens. Do not substitute
guessed values.

The CLI is a ZITADEL public device client. It has no client secret: login and
refresh use only the registered public client ID and the scopes above. Google
is selected inside the RBX Identity login policy and is never called directly
by the CLI.

Sign in once per machine:

```bash
robson-cli auth login
```

This prints a short user code and an RBX Identity verification URL. Open the
URL on any device, choose Google, enter the code, and approve. `robson-cli`
polls until approval, then caches `{issuer, client_id, access_token,
refresh_token, email, expiry}` at
`~/.config/robson/credentials.json` (or under `$XDG_CONFIG_HOME` if set)
with `0600` permissions. `reconcile-close` and `income ack` read this
cache automatically and refresh it transparently as it nears expiry — no
further flags are needed.

Other `auth` subcommands:

```bash
robson-cli auth status        # print the cached email/expiry — no network call
robson-cli auth logout        # delete the cached credential
robson-cli auth print-token   # print the current RBX access token, for scripting/curl
```

If neither command finds a cached credential, it fails with `run
\`robson-cli auth login\` first` rather than silently falling back to an
unauthenticated request. For a remote daemon, use an approved secure
tunnel or local port forward — never send the bearer credential over
plaintext remote HTTP.

`ROBSON_OPERATOR_ID` may supply the default actor for `income ack` only in the
explicit loopback development-auth mode. In an authenticated deployment the
API ignores that client-supplied actor and records the canonical OIDC subject.

**Legacy note:** `ROBSON_API_TOKEN`, `ROBSON_LEGACY_API_TOKEN`, direct Google
client configuration, and `--token` are removed. Setting any legacy human-auth
variable on `robsond` fails startup loudly; there is no dual-authentication
bridge.

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
