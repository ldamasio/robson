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
images transformer overrides it (Pattern R; the manifests carry the `:latest`
placeholder).

```bash
# 1. Find the previous working SHA from rbx-infra history
gh api repos/rbxrobotica/rbx-infra/commits \
  --jq '.[0:10] | .[] | {sha: .sha[0:8], message: .commit.message[0:60]}'

# 2. Roll the pin back in rbx-infra
git clone https://github.com/rbxrobotica/rbx-infra.git /tmp/rbx-infra
cd /tmp/rbx-infra
K=apps/prod/robson/kustomization.yml
yq -i '(.images[] | select(.name == "ghcr.io/rbxrobotica/robson-v2") | .newTag) = "sha-<previous>"' "$K"

# 3. Confirm the render resolves to the intended tag BEFORE pushing
kubectl kustomize apps/prod/robson | grep 'image: ghcr.io/rbxrobotica/robson-v2'

# 4. Commit and push to rbx-infra. ArgoCD syncs automatically
git commit -am "chore(robson-v2): roll back to sha-<previous>"
git push origin main
```

---

## Manual Deployment (Fallback)

If the GitOps update fails:

```bash
# Get the SHA tag you want to deploy
SHA_TAG="sha-776a72f9"

# Clone rbx-infra and move the pin manually
git clone https://github.com/rbxrobotica/rbx-infra.git /tmp/rbx-infra
cd /tmp/rbx-infra
K=apps/prod/robson/kustomization.yml
yq -i '(.images[] | select(.name == "ghcr.io/rbxrobotica/robson-v2") | .newTag) = strenv(SHA_TAG)' "$K"

# Verify the render before pushing (both the daemon and the migrate hook
# resolve from the same pin)
kubectl kustomize apps/prod/robson | grep 'image: ghcr.io/rbxrobotica/robson-v2'

git commit -am "chore(robson-v2): manual rollout to ${SHA_TAG}"
git push origin main
```

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
