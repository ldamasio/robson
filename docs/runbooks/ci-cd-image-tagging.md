# CI/CD Image Tagging — Robson Runtime

## Overview

Documents the Docker image tagging strategy, CI/CD pipeline behavior, and GitOps flow
for the Robson Rust runtime. The pipeline lives in `.github/workflows/robsond.yml`.

---

## Tag Types

| Tag Pattern | When Created | Use Case | Example |
|-------------|--------------|----------|---------|
| `sha-<8chars>` | Every push to `main` that changes the root Rust workspace or Docker build inputs | **Production** | `sha-776a72f9` |
| `latest` | Every push to `main` | Dev/local only | `latest` |

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
  workflow_dispatch:
```

> **Important**: Changes exclusively in `.github/workflows/` do **not** trigger the
> workflow automatically due to the Rust workspace path filter. Use `workflow_dispatch` manually:
> ```bash
> gh workflow run robsond.yml --repo ldamasio/robson --ref main
> ```

---

## Pipeline Steps

### Job 1: Rust Tests

1. Cache Rust toolchain and deps (`~/.rustup`, `~/.cargo`, `target`)
2. `cargo test --all --no-fail-fast`
3. `rustup toolchain install nightly --component rustfmt`
4. `cargo +nightly fmt --all --check` (nightly required for options in `rustfmt.toml`)
5. `cargo clippy --all-targets -- -D clippy::correctness -D clippy::suspicious`

### Job 2: Build & Push Image (main only, after Job 1)

1. Docker Buildx setup
2. Login to GHCR (`ghcr.io`) with `GITOPS_TOKEN`
3. Build from `Dockerfile`, push tags `sha-<8chars>` and `latest`
4. Clone `rbxrobotica/rbx-infra`, set `images[].newTag` in the prod overlay with
   `yq`, commit and push
5. ArgoCD detects manifest change and syncs automatically

---

## GitOps Flow

```
Push to main (root Rust workspace or Docker build input change)
    │
    ▼
Rust Tests: cargo test + nightly fmt check + clippy
    │
    ▼
Build & Push: ghcr.io/rbxrobotica/robson-v2:sha-XXXXXXXX
    │
    ▼
Update rbx-infra:
  apps/prod/robson/kustomization.yml (images[].newTag)
    │
    ▼
ArgoCD syncs (namespace: robson)
    │
    ▼
✅ Deploy complete
```

---

## rustfmt Configuration

`rustfmt.toml` uses **nightly-only options** (e.g., `imports_granularity`,
`group_imports`, `wrap_comments`, `format_code_in_doc_comments`). The CI explicitly
installs the nightly toolchain to run formatting checks. Do not simplify `rustfmt.toml`
to stable-only options.

---

## Rollback

The deployed tag lives in ONE place: `images[].newTag` in the prod overlay.
Editing `image:` in `robsond-deploy.yml` has no effect, because the kustomize
images transformer overrides it (Pattern R; the manifests carry a
`sha-REPLACE_ME` placeholder that is deliberately not a real tag).

**Do not read image tags out of `git log` commit hashes.** A rbx-infra commit
hash and a `sha-<hex>` image tag are both 8 hex characters and are trivially
confused under pressure. Only the file history is authoritative.

```bash
set -euo pipefail

WORK=$(mktemp -d)                      # never a fixed path: a stale /tmp clone
git clone https://github.com/rbxrobotica/rbx-infra.git "$WORK/rbx-infra"
cd "$WORK/rbx-infra"
K=apps/prod/robson/kustomization.yml

# 1. Read the ACTUAL promoted tags, newest first. These are image tags,
#    not commit hashes.
git log -20 --format='%h %ad %s' --date=short -- "$K"
git log -20 -p -- "$K" | grep -E '^\+\s+newTag:' | head

PREV=sha-xxxxxxxx                      # pick from the output above

# 2. Prove the tag exists in GHCR BEFORE pointing production at it
docker manifest inspect "ghcr.io/rbxrobotica/robson-v2:${PREV}" >/dev/null \
  && echo "tag exists" || { echo "TAG DOES NOT EXIST, stop here"; exit 1; }

# 3. Roll the pin back
PREV="$PREV" yq -i \
  '(.images[] | select(.name == "ghcr.io/rbxrobotica/robson-v2") | .newTag) = strenv(PREV)' "$K"

# 4. Prove the render resolves to exactly that tag, on BOTH workloads,
#    before pushing. Expect two identical lines.
kubectl kustomize apps/prod/robson | grep 'image: ghcr.io/rbxrobotica/robson-v2'

# 5. Push. ArgoCD syncs automatically.
git commit -am "chore(robson-v2): roll back to ${PREV}"
git push origin main
```

### Verify the rollback actually landed

`git push` proves nothing. ArgoCD may be degraded, the PreSync migration Job
may be stuck, or the bad pod may still be serving. With strategy `Recreate`,
`Synced` alone does not imply available either. Check all four:

```bash
# a. ArgoCD converged
kubectl get application -n argocd robson-prod \
  -o jsonpath='{.status.sync.status}{" "}{.status.health.status}{"\n"}'

# b. The pod is running the tag you intended
kubectl get pod -n robson -l app.kubernetes.io/name=robsond \
  -o jsonpath='{.items[*].spec.containers[0].image}{"\n"}'

# c. The pod is actually up, not ImagePullBackOff or CrashLoop
kubectl get pod -n robson -l app.kubernetes.io/name=robsond

# d. The daemon answers and still holds the book you expect
curl -s https://api.robson.rbx.ia.br/health
curl -s https://api.robson.rbx.ia.br/status
```

If a position is open, (d) is the one that matters: the exchange insurance
stop covers the gap while the daemon is down (ADR-0039), but you want to see
the daemon back and reconciled, with `reconciliation_blockers` empty.

---

## Manual Deployment (Fallback)

If the GitOps update fails:

```bash
set -euo pipefail

# The tag you want to deploy. It MUST be exported: `yq`'s strenv() reads the
# environment of the yq process, and a bare assignment silently yields an
# empty string, writing `newTag: ""` into the overlay.
export SHA_TAG="sha-776a72f9"

docker manifest inspect "ghcr.io/rbxrobotica/robson-v2:${SHA_TAG}" >/dev/null \
  && echo "tag exists" || { echo "TAG DOES NOT EXIST, stop here"; exit 1; }

WORK=$(mktemp -d)
git clone https://github.com/rbxrobotica/rbx-infra.git "$WORK/rbx-infra"
cd "$WORK/rbx-infra"
K=apps/prod/robson/kustomization.yml
yq -i '(.images[] | select(.name == "ghcr.io/rbxrobotica/robson-v2") | .newTag) = strenv(SHA_TAG)' "$K"

# Both the daemon and the migrate hook resolve from the same pin. Expect two
# identical lines carrying ${SHA_TAG}, and stop if you do not get them.
kubectl kustomize apps/prod/robson | grep 'image: ghcr.io/rbxrobotica/robson-v2'

git commit -am "chore(robson-v2): manual rollout to ${SHA_TAG}"
git push origin main
```

Then run the four verification checks from the Rollback section above. The
procedure is not finished at `git push`.

---

## Troubleshooting

### Build & Push fails: `must have exactly one images entry`

The prod overlay lost its `robson-v2` pin, or gained a duplicate. The promotion
step refuses to guess. Inspect and repair the overlay:

```bash
yq '.images' apps/prod/robson/kustomization.yml
```

There must be exactly one entry with
`name: ghcr.io/rbxrobotica/robson-v2`. See rbx-infra
`docs/infra/IMAGE-PROMOTION.md`.

### Build & Push fails: `yq is not available on this runner`

`yq` is preinstalled on GitHub's `ubuntu-latest` images. If it disappears from
a future runner image, install it explicitly in the workflow before the
promotion step.

### Formatting check fails locally

Ensure you are using the nightly toolchain:
```bash
rustup toolchain install nightly --component rustfmt
cargo +nightly fmt --all --check
```

### CI not triggered after workflow file change

The workflow only triggers on Rust workspace and Docker build input path changes. For workflow-only changes,
dispatch manually:
```bash
gh workflow run robsond.yml --repo ldamasio/robson --ref main
```

---

## References

- [Workflow](../../.github/workflows/robsond.yml)
- [rustfmt config](rustfmt.toml)
- [rbx-infra manifests](https://github.com/rbxrobotica/rbx-infra/tree/main/apps/prod/robson)
- [ADR-0011: GitOps Automatic Manifest Updates](../adr/ADR-0011-gitops-automatic-manifest-updates.md)
