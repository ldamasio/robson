# GitOps

Components
- App of Apps: root ArgoCD Applications (to be added under app-of-apps/)
- ApplicationSet: `applicationsets/branches.yaml` generates preview envs per branch (non-main)
- Charts: `../../charts` contains Helm charts used by Applications

Notes
- Replace placeholders (OWNER/REPO) with the actual repository settings.
- Gateway API is used instead of Ingress; ensure Istio Ambient and CRDs are installed.

