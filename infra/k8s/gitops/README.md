# GitOps

Components
- App of Apps: root ArgoCD Applications (to be added under app-of-apps/)
- ApplicationSet: `applicationsets/branches.yaml` generates preview envs per branch (non-main)
- Charts: `../../charts` contains Helm charts used by Applications

Notes
- Replace placeholders (OWNER/REPO) with the actual repository settings.
- Gateway API is used instead of Ingress; ensure Istio Ambient and CRDs are installed.
- App of Apps: apply `app-of-apps/root.yaml` after ArgoCD install.
- Preview images: `.github/workflows/preview-images.yml` builds images tagged `<branch>-<short_sha>`; ApplicationSet values should reference this tag.
