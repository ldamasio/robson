# Legacy Documentation

This directory contains documentation for the **old architecture** (pre-2026).

## What Changed

### Old Architecture (Docker Hub + Direct Manifests)

```
┌─────────────┐
│   Robson    │
│  (this repo)│
└──────┬──────┘
       │
       ├─ Build images → Docker Hub (ldamasio/rbs-*)
       ├─ Update manifests: infra/k8s/prod/*.yml
       └─ GitOps Sync Checker (safety net for failed updates)
```

**Problems:**
- Images and manifest updates were separate steps (could fail independently)
- Sync checker was needed to detect gaps
- Used Docker Hub (now deprecated for RBX products)
- Manifests stored in same repo as code

### New Architecture (GHCR + External GitOps)

```
┌─────────────┐      ┌─────────────┐
│   Robson    │──────▶│  rbx-infra  │
│  (this repo)│      │  (GitOps)   │
└─────────────┘      └──────┬──────┘
       │                    │
       │                    └─ ArgoCD watches
       │                           │
       ├─ Build images → GHCR      │
       └─ Update manifests ────────┘
          (atomic: same workflow)
```

**Improvements:**
- ✅ Images pushed to GHCR (`ghcr.io/rbxrobotica/robson`)
- ✅ Manifests in separate repo (`rbx-infra`)
- ✅ Atomic updates: if build succeeds, manifest is updated
- ✅ Kustomize-based deploys
- ✅ No sync checker needed (can't get out of sync)

## Migration Timeline

- **Pre-2026**: Docker Hub + inline manifests
- **Jan 2026**: Migrated to GHCR
- **Mar 2026**: Moved manifests to rbx-infra, disabled sync checker

## Using This Documentation

Documents in `legacy/` describe the old system. They are kept for:
- Historical reference
- Understanding past incidents
- Migration context

**For current operations**, see:
- `/home/psyctl/apps/rbx-infra/docs/` (GitOps documentation)
- `/home/psyctl/apps/rbx-infra/CLAUDE.md` (operational guide)
- `/home/psyctl/apps/robson/CLAUDE.md` (build pipeline)

---

**Last Updated**: 2026-03-29
**Reason**: GitOps Sync Checker workflow disabled
