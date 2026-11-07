# Robsond DB Migrations

Robsond owns the schema definition and migration files in `migrations`.
Production applies them through the `rbx-infra` ArgoCD application.

## Ownership

- `robson`: migration SQL files, `robsond db migrate`, known migration-state
  repairs, and CI validation against a clean PostgreSQL database.
- `rbx-infra`: Kubernetes Job, ArgoCD hook ordering, image tag wiring, secrets,
  and environment-specific execution.

## Rules

- Already-applied migration files are immutable.
- Schema corrections after a migration has shipped must be new migrations.
- `robsond db migrate` is the only deploy-time migration entrypoint.
- Migration repair logic must be narrow, explicit, schema-validated, and logged.
- A failed migration job must block rollout of a new `robsond` image.

## Current Historical Repairs

`robsond db migrate` first runs a constrained repair for known historical drift:

- `20240101000001_event_log_phase9`: normalizes legacy index names and updates
  the SQLx checksum from the pre-rename file to the current file checksum.
- `20240101000008_create_monthly_state`: replaces the historical zero checksum
  with the current file checksum after validating `monthly_state`.
- `20240101000010_add_realized_loss_trades_opened`: records the migration as
  applied when both columns already exist from the production hotfix.

Any other checksum mismatch remains a hard failure.

## Deployment Flow

1. The Robson CI builds and publishes the `robsond` image.
2. The CI bumps the single image pin, `images[].newTag` in
   `apps/prod/robson/kustomization.yml` in `rbxrobotica/rbx-infra`. The
   `robsond` Deployment and the migrate Job both take their tag from it, and
   their manifests keep a `sha-REPLACE_ME` placeholder on purpose.
3. ArgoCD runs `robsond-db-migrate` as a Sync hook at wave `-1`.
4. Only after the migration hook succeeds does ArgoCD apply the `robsond`
   Deployment in the default wave.

## Operator Checks

Check ArgoCD:

```bash
kubectl get application robson-prod -n argocd -o wide
```

Check the migration hook logs during a deploy, while its pod exists:

```bash
kubectl logs -n robson -l job-name=robsond-db-migrate --tail=-1
```

The pod does not outlive the hook, so `No resources found` after a deploy says
nothing about the result:

- On success, ArgoCD deletes the Job and its pod (`HookSucceeded`).
- On failure, Kubernetes removes the pod when the Job reaches its backoff limit
  (`restartPolicy: OnFailure`, `backoffLimit: 5`). The Job stays until the next
  sync replaces it (`BeforeHookCreation`).

After that, read the hook's output in Grafana, from the Loki data source:

```text
{namespace="robson", pod=~"robsond-db-migrate-.*"}
```

Check migration status from the running `robsond` pod:

```bash
kubectl -n robson exec deploy/robsond -- sh -c 'RUST_LOG=robson_db=info robsond db status'
```

This lists the latest 10 migrations, newest first, so a version older than those
can be applied and still not appear. Keep the `RUST_LOG` override: `status()`
reports through `info!`, and the production ConfigMap's `RUST_LOG` omits
`robson_db`, so a plain `robsond db status` exits 0 and prints no versions.

## CI Guard

The `Robsond CI/CD` workflow starts a clean PostgreSQL service and runs:

```bash
DATABASE_URL=postgresql://postgres:postgres@localhost:5432/robson_migration_ci \
  cargo run -p robsond --features postgres -- db migrate
```

This catches non-idempotent DDL, duplicate index names, and broken migration
ordering before an image can be published.
