# CI/CD Image Tagging — Robson Runtime

## Overview

Documents the Docker image tagging strategy, CI/CD pipeline behavior, and GitOps flow
for the Robson Rust runtime. The pipeline lives in `.github/workflows/robsond.yml`.

---

## Tag Types

| Tag Pattern | When Created | Use Case | Example |
|-------------|--------------|----------|---------|
| `sha-<8chars>` | Every push to `main` that changes the root Rust workspace or Docker build inputs, and every `workflow_dispatch` on `main` | **Production** | `sha-776a72f9` |
| `latest` | Runs that move the production pin | Dev/local only | `latest` |

### SHA Tags (Golden Standard)

- **Format**: `sha-<first-8-chars-of-commit>`
- **Registry**: `ghcr.io/rbxrobotica/robson-v2`
- **Purpose**: Immutable, traceable, rollback-friendly

---

## Workflow Triggers

The workflow (`robsond.yml`) triggers on:

```yaml
on:
  push:
    branches: ["main"]
    paths:
      - ".cargo/**"
      - ".dockerignore"
      - ".github/workflows/robsond.yml"
      - "Cargo.lock"
      - "Cargo.toml"
      - "Dockerfile"
      - "clippy.toml"
      - "migrations/**"
      - "robson-*/**"
      - "robsond/**"
      - "rust-toolchain.toml"
      - "rustfmt.toml"
  pull_request:
    branches: ["main"]
  workflow_dispatch:
```

`pull_request` has no path filter so that `Rust Tests` can be a required check on `main`,
which needs a report on every PR. `push` keeps the filter because a run on `main` deploys.

> **Important**: Of the files in `.github/workflows/`, only `robsond.yml` triggers this
> workflow on `push`, so merging a change to it publishes an image and restarts the daemon.
> For a deploy with no change to any of these paths, use `workflow_dispatch` manually. It builds
> `main`'s tip and, like a push, only moves the pin forward, so it cannot roll back (see
> [Rollback](#rollback)):
> ```bash
> gh workflow run robsond.yml --repo ldamasio/robson --ref main
> ```

---

## Pipeline Steps

### Job 1: Rust Tests

This is a summary; `robsond.yml` is the source of truth.

1. Cache Rust toolchain and deps (`~/.rustup`, `~/.cargo/registry`, `~/.cargo/git`, `target`)
2. Assert that the `Dockerfile` builder image matches `rust-toolchain.toml`
3. `cargo test --all --no-fail-fast` (no `postgres` feature, uses `MemoryStore`)
4. `cargo test --workspace --exclude robson-connectors --features postgres --no-fail-fast`
   with `DATABASE_URL` unset
5. Reject duplicate migration version prefixes
6. `cargo run -p robsond --features postgres -- db migrate` on a clean PostgreSQL service
7. `cargo test --workspace --exclude robson-connectors --features postgres --no-fail-fast -- --ignored`
   with `DATABASE_URL` set
8. `rustup toolchain install nightly-2026-08-22 --component rustfmt`
9. `cargo +nightly-2026-08-22 fmt --all --check` (see [rustfmt Configuration](#rustfmt-configuration))
10. `cargo clippy --all-targets -- -D clippy::correctness -D clippy::suspicious`

### Job 2: Build & Push Image (main only, after Job 1)

1. Docker Buildx setup
2. Login to GHCR (`ghcr.io`) with `GITOPS_TOKEN`
3. Build from `Dockerfile`, push tag `sha-<8chars>`
4. Clone `rbxrobotica/rbx-infra` and check the pin in `apps/prod/robson/kustomization.yml` with
   `scripts/promotion-guard.sh`. If this commit is older than the pinned one or no longer on
   `main`, leave the pin alone and end green with a `Pin not moved` notice (see
   [the troubleshooting entry](#build--push-is-green-but-the-pin-did-not-move)). Otherwise set
   `images[].newTag` for `ghcr.io/rbxrobotica/robson-v2` with `yq`, fail if the file does not end
   up with the new tag, commit and push. A rejected push starts over from a fresh rbx-infra `main`,
   guard included, up to three pushes; a newer pin that beat the last push is still a skip.
5. Move `latest` to the image, only if the pin moved
6. ArgoCD detects the commit and syncs `robson-prod` automatically

Runs on `main` never cancel each other: each commit has its own concurrency group. This job runs one
at a time across them (job group `robsond-promote`, `queue: max`, so a waiting job is never replaced),
which keeps the pin and `latest` together. Two merges a few minutes apart can each restart the
daemon, in order; a run that reaches the job after a newer commit is pinned skips.

When a dependency of this job fails:

| Fails | Pin | Shows as | Next step |
|-------|-----|----------|-----------|
| GHCR build or push | Unchanged | Red at `Build and push` | Re-run the failed job |
| GitHub: fetching this repository, cloning or pushing rbx-infra | Unchanged, unless a push landed and only its acknowledgement was lost | Red at `Update image tags in rbx-infra`; the summary says the pin is unknown | [Manual Deployment](#manual-deployment-fallback), which checks the pin first |
| GHCR, moving `latest` | **Already moved**, production deploys | Red at `Move latest to this image`; the summary says the pin moved | [Troubleshooting](#build--push-fails-at-move-latest-to-this-image) |
| Nothing: a newer commit is pinned | Unchanged | Green, `Pin not moved` notice | None |

---

## GitOps Flow

```
Push to main (root Rust workspace or Docker build input change)
    │
    ▼
Rust Tests: cargo test + migration check + DB tests + pinned nightly fmt check + clippy
    │
    ▼
Build & Push: ghcr.io/rbxrobotica/robson-v2:sha-XXXXXXXX
    │
    ▼
Bump the pin in rbx-infra:
  apps/prod/robson/kustomization.yml (images[].newTag)
    │
    ▼
ArgoCD syncs robson-prod (namespace: robson):
  1. robsond-db-migrate Job (Sync hook, wave -1)
  2. robsond Deployment, only if the hook succeeds
    │
    ▼
✅ Deploy complete
```

### The Image Pin

`apps/prod/robson/kustomization.yml` in `rbxrobotica/rbx-infra` holds the single pin for every
`robson-v2` image, used by both the Deployment and the migrate Job (Pattern R, see
[Image Promotion](https://github.com/rbxrobotica/rbx-infra/blob/main/docs/infra/IMAGE-PROMOTION.md)):

```yaml
images:
  - name: ghcr.io/rbxrobotica/robson-v2
    newTag: sha-XXXXXXXX
```

`robsond-deploy.yml` and `robsond-db-migrate-job.yml` carry the tag `sha-REPLACE_ME` on purpose.
Kustomize replaces it at render time, and the tag does not exist in GHCR, so a bypassed transformer
fails with `ImagePullBackOff` instead of running an unknown build.

- **Never edit those two image lines.** The rendered manifests still use the pin, so the cluster
  does not change, and the edit corrupts the placeholder.
- **`kubectl set image` does not hold.** `robson-prod` has `selfHeal` on, so ArgoCD reverts it.
- **CI promotes production only.** `apps/testnet/robson/kustomization.yml` has its own pin, which
  this workflow does not touch.

---

## rustfmt Configuration

`rustfmt.toml` uses **nightly-only options** (e.g., `imports_granularity`,
`group_imports`, `wrap_comments`, `format_code_in_doc_comments`). The CI explicitly
installs a pinned nightly (`nightly-2026-08-22`) to run formatting checks, because the output
of these options changes between nightlies. Do not simplify `rustfmt.toml` to stable-only
options. Bumping the pin is a deliberate step, described in a comment in `robsond.yml`.

---

## Rollback

A rollback moves the pin in `apps/prod/robson/kustomization.yml` back to an earlier tag. It is a
production deploy: ArgoCD syncs the commit and the daemon restarts (single replica, `Recreate`).
The comment on the pin says CI bumps it, never by hand. That is the routine path; this section and
[Manual Deployment](#manual-deployment-fallback) are the exceptions. An agent needs the operator's
explicit authorization for this exact operation (`AGENTS.md` in rbx-infra).

Three things can defeat a rollback:

- **A migration the target lacks.** The sync runs `robsond db migrate` from the target image before
  it touches the Deployment. SQLx refuses to run when the database has a migration the image does
  not include (`migration <version> was previously applied but is missing in the resolved
  migrations`). The hook fails, ArgoCD does not apply the Deployment, and the daemon stays on the
  bad image. Step 3 checks for this against `main`, not against the running tag, because a hook can
  apply a migration and then fail, which leaves the Deployment on a tag older than the schema. If
  step 3 prints a file, check whether that version was applied:
  `kubectl -n robson exec deploy/robsond -- sh -c 'RUST_LOG=robson_db=info robsond db status'`.
  If it was, roll forward instead: revert the change on `main`, keep the migration file
  ([applied migrations are immutable](robsond-db-migrations.md)), and let CI promote the new image.
- **New event types in the range.** An older projector returns `MissingHandler` for an event type
  it does not know. If the range wrote new event types in production, read the rollback notes in
  the ADR for that change first.
- **The next promotion.** Any later `build-and-push` run on `main` for a commit newer than the
  target overwrites the pin: a merge that touches a path in `push.paths`, a `workflow_dispatch`, or a
  re-run. The guard does not stop these, because they move the pin forward. Step 2 stops these until
  the fix is ready for `main`; that fix is what makes the recovery last.

```bash
# 1. Find the running image and the earlier tags. A tag is the first 8 chars of a commit in this repo.
kubectl -n robson get deploy robsond -o jsonpath='{.spec.template.spec.containers[0].image}{"\n"}'
gh api 'repos/rbxrobotica/rbx-infra/commits?path=apps/prod/robson/kustomization.yml&per_page=10' \
  --jq '.[] | "\(.commit.author.date[0:16]) \(.commit.message | split("\n")[0])"'
SHA_TAG="sha-<previous>"

# 2. Stop promotions: hold merges, then wait for or cancel (gh run cancel <id>) every run of this
#    workflow on main that has not completed. One sweep reads every run of the workflow, unfiltered,
#    so a run that changes status during the check is still caught, and no search cap applies. The
#    output must be exactly "scan complete"; a failed page leaves that line out. A run GitHub refuses
#    to cancel (409, "has not been queued yet") can still start later and promote, so wait for it.
gh api --paginate 'repos/ldamasio/robson/actions/workflows/robsond.yml/runs?per_page=100' \
  --jq '.workflow_runs[] | select(.head_branch == "main" and .event != "pull_request" and .status != "completed") | "\(.id) \(.head_sha[0:8]) \(.event) \(.status)"' \
  && echo "scan complete"

#    Then wait for ArgoCD, because a queued or running hook can still apply a migration. All three
#    must hold. For a sync stuck on a hook, see "The daemon stays on the old image".
#    - "queued" is empty. Anything there is a sync that has not finished.
#    - "phase" is Succeeded, Failed or Error. A blank phase means a sync is queued.
#    - "revision" matches rbx-infra main, or ArgoCD has not seen the latest commit yet.
kubectl get application robson-prod -n argocd \
  -o jsonpath='queued: {.operation}{"\n"}revision: {.status.sync.revision}{"\n"}phase: {.status.operationState.phase}{"\n"}'
gh api repos/rbxrobotica/rbx-infra/commits/main --jq .sha

# 3. From a checkout of this repo. This must print nothing; see "A migration the target lacks".
git fetch origin
git diff --name-only "${SHA_TAG#sha-}" origin/main -- migrations/

# 4. The tag must exist in GHCR, or the hook sticks in ImagePullBackOff. This must print "exists".
#    It needs a GHCR login with read:packages (podman login ghcr.io).
podman manifest inspect "ghcr.io/rbxrobotica/robson-v2:${SHA_TAG}" > /dev/null && echo exists

# 5. Bump the pin in a fresh clone of rbx-infra
WORK=$(mktemp -d)
git clone https://github.com/rbxrobotica/rbx-infra.git "$WORK"
cd "$WORK"
./scripts/promote-image-tag.sh robson ghcr.io/rbxrobotica/robson-v2 "$SHA_TAG"

# 6. Render it. Both lines must show $SHA_TAG.
kubectl kustomize apps/prod/robson | grep 'image: ghcr.io/rbxrobotica/robson-v2'

# 7. Commit and push. ArgoCD syncs automatically.
git add apps/prod/robson/kustomization.yml
git commit -m "chore(robson-v2): roll back image tag to ${SHA_TAG}"
git push origin main
```

Then [verify the rollout](#verifying-a-pin-bump).

`main` in rbx-infra requires a PR, and only admins can bypass that. Without admin rights the push is
rejected, so open a PR with the same commit. If the push loses a race, run
`git pull --rebase origin main` and push again.

On a host with `docker` instead of `podman`, use `docker manifest inspect`, or put `CHECK_REGISTRY=1`
in front of the script, which runs the same check.

---

## Manual Deployment (Fallback)

If `Update image tags in rbx-infra` fails, the image is already in GHCR, and the pin most likely did
not move. A push can land while its acknowledgement is lost, so check the pin first; if it already
shows the run's tag, the deploy is under way and there is nothing to do here:

```bash
gh api repos/rbxrobotica/rbx-infra/contents/apps/prod/robson/kustomization.yml --jq .content | base64 -d | grep newTag
```

A re-run checks the pin again and leaves it alone if a newer commit is pinned by then. A re-run of a
run whose commit predates `scripts/promotion-guard.sh` has no guard, and neither does the hand bump:
use those only if no newer run has promoted since.

First re-run the failed job:

```bash
gh run rerun <run-id> --repo ldamasio/robson --failed
```

If that fails too, bump the pin by hand. This deploys exactly as a CI promotion does, including the
restart, and the same authorization rule as [Rollback](#rollback) applies.

```bash
# Take the tag from the run, so it cannot be mistyped
RUN_ID="<run-id>"
SHA_TAG="sha-$(gh run view "$RUN_ID" --repo ldamasio/robson --json headSha --jq '.headSha[0:8]')"
echo "$SHA_TAG"

# The tag must exist in GHCR, or the hook sticks in ImagePullBackOff. This must print "exists".
# It needs a GHCR login with read:packages (podman login ghcr.io).
podman manifest inspect "ghcr.io/rbxrobotica/robson-v2:${SHA_TAG}" > /dev/null && echo exists

# Bump the pin in a fresh clone of rbx-infra
WORK=$(mktemp -d)
git clone https://github.com/rbxrobotica/rbx-infra.git "$WORK"
cd "$WORK"
./scripts/promote-image-tag.sh robson ghcr.io/rbxrobotica/robson-v2 "$SHA_TAG"

# Render it. Both lines must show $SHA_TAG.
kubectl kustomize apps/prod/robson | grep 'image: ghcr.io/rbxrobotica/robson-v2'

git add apps/prod/robson/kustomization.yml
git commit -m "chore(robson-v2): manual rollout to ${SHA_TAG}"
git push origin main
```

Then [verify the rollout](#verifying-a-pin-bump).

---

## Verifying a Pin Bump

`git push` does not wait for ArgoCD, which can take a few minutes to see the commit. Until it does,
`rollout status` and the Application status describe the previous rollout, so wait for the new tag
first:

```bash
# Ends once ArgoCD has applied $SHA_TAG to the Deployment
until [ "$(kubectl -n robson get deploy robsond -o jsonpath='{.spec.template.spec.containers[0].image}')" = \
  "ghcr.io/rbxrobotica/robson-v2:${SHA_TAG}" ]; do sleep 10; done

# Ends once the new pod is ready
kubectl -n robson rollout status deploy/robsond

# Must show Synced and Healthy
kubectl get application robson-prod -n argocd -o wide
```

If the first command is still waiting after 10 minutes, the rollout has not happened. Stop it and
see [The daemon stays on the old image](#the-daemon-stays-on-the-old-image-after-a-pin-bump).

---

## Troubleshooting

### Build & Push fails: `kustomize pin was not updated in apps/prod/robson/kustomization.yml`

The `yq` expression matched no image stanza, so the new tag never landed. Check that the file in
`rbxrobotica/rbx-infra` still has an `images` entry named `ghcr.io/rbxrobotica/robson-v2`. If the
file itself moved, the step fails earlier with a `yq` file error. Either way, fix the path or name
in the `Update image tags in rbx-infra` step or in rbx-infra, then follow
[Manual Deployment](#manual-deployment-fallback).

### Build & Push fails: `Failed to push to rbx-infra after 3 attempts`

Every push was rejected. Check the step log, and check that `GITOPS_TOKEN` can still write to
`rbxrobotica/rbx-infra`. Then follow [Manual Deployment](#manual-deployment-fallback).

### Build & Push is green but the pin did not move

The step log and the run summary say `Pin not moved` and why. The guard skipped the bump: this
commit is older than the pinned one (a run that started late, or a re-run), it is no longer on
`main`, or the pin names a commit off `main` and this commit is not the tip. The `sha-<8chars>` image
is in GHCR; `latest` did not move. Nothing needs fixing. Running this commit anyway is a
[Rollback](#rollback).

### Build & Push fails at `Move latest to this image`

The pin has already moved and ArgoCD deploys this commit; the run summary says so. Only `latest`,
which is dev/local only, still names an older image. Production needs nothing, and the next
promotion moves `latest` again.

To fix it sooner, do not copy this run's tag: a newer run may have promoted since. Re-run the failed
job, which waits its turn and skips if a newer commit is pinned, but rebuilds the image and pushes it
again under the tag production runs. Or, while no `Build & Push Image` job is running or waiting,
copy the tag production pins now, with a GHCR login that can write packages:

```bash
SHA_TAG=$(gh api repos/rbxrobotica/rbx-infra/contents/apps/prod/robson/kustomization.yml --jq .content \
  | base64 -d | sed -n 's/^ *newTag: //p')
docker buildx imagetools create --tag ghcr.io/rbxrobotica/robson-v2:latest "ghcr.io/rbxrobotica/robson-v2:${SHA_TAG}"
```

### The daemon stays on the old image after a pin bump

ArgoCD applies the Deployment only after the `robsond-db-migrate` hook succeeds. Start from the sync
state and the Job. Missing pods prove nothing: the Job can fail to create one, and Kubernetes removes
the pod once the Job reaches its backoff limit, so logs exist only during the retries.

```bash
kubectl get application robson-prod -n argocd \
  -o jsonpath='{.status.operationState.phase}{"\n"}{.status.operationState.message}{"\n"}'
kubectl -n robson describe job robsond-db-migrate
kubectl -n robson describe pods -l job-name=robsond-db-migrate
kubectl -n robson logs -l job-name=robsond-db-migrate
```

- `was previously applied but is missing in the resolved migrations`: the image is older than the
  schema. Move the pin to an image that includes every applied migration. See [Rollback](#rollback).
- `ImagePullBackOff` on a `sha-<8chars>` tag: read the pod's events before touching the pin.
  - `not found` or `manifest unknown`: the tag is not in GHCR. Move the pin to a tag CI published.
  - `unauthorized` or `denied`: `ghcr-pull-secret` is the problem, and a different tag will not help.
    Fix the secret.
- `ImagePullBackOff` on `sha-REPLACE_ME`: the images transformer did not apply. From a checkout of
  rbx-infra, run `kubectl kustomize apps/prod/robson | grep 'image: ghcr.io/rbxrobotica/robson-v2'`
  and fix `images[].name` in `kustomization.yml`. Do not put a real tag in the manifests.

Two ArgoCD behaviours can keep the sync stuck after the cause is fixed. Both commands below change
production, so the same authorization rule as [Rollback](#rollback) applies.

- A hook in `ImagePullBackOff` never fails, so the sync can stay `Running` and ignore the next commit.
  If it does, clear it with `argocd app terminate-op robson-prod`, then check that
  `kubectl -n robson get pods -l job-name=robsond-db-migrate` finds nothing before moving the pin.
- Automated sync does not retry a failed sync of the same commit. After fixing something outside Git,
  such as the pull secret, run `argocd app sync robson-prod`. Moving the pin makes a new commit, which
  syncs by itself.

### Formatting check fails locally

Ensure you are using the pinned nightly toolchain:
```bash
rustup toolchain install nightly-2026-08-22 --component rustfmt
cargo +nightly-2026-08-22 fmt --all --check
```

### CI not triggered after workflow file change

On `push`, the workflow only triggers on Rust workspace and Docker build input path changes, and on
`robsond.yml` itself. For a change to any other workflow file, dispatch manually:
```bash
gh workflow run robsond.yml --repo ldamasio/robson --ref main
```

---

## References

- [Workflow](../../.github/workflows/robsond.yml)
- [rustfmt config](../../rustfmt.toml)
- [rbx-infra manifests](https://github.com/rbxrobotica/rbx-infra/tree/main/apps/prod/robson)
- [Image Promotion (rbx-infra)](https://github.com/rbxrobotica/rbx-infra/blob/main/docs/infra/IMAGE-PROMOTION.md)
- [Robsond DB Migrations](robsond-db-migrations.md)
- [ADR-0011: GitOps Automatic Manifest Updates](../adr/ADR-0011-gitops-automatic-manifest-updates.md)
  (historical: its `sed` step predates the kustomize pin)
