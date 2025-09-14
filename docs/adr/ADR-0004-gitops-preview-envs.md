ADR-0004: GitOps Preview Environments per Branch

Status: Accepted
Date: 2025-09-14

Context
- Every non-main branch should create an isolated homolog environment for testing/QA.
- We need reproducible, automatic creation and teardown of environments driven by Git.

Decision
- Use ArgoCD ApplicationSet (Git generator) to create a namespace and application stack per branch (excluding `main`).
- Naming: `h-<branch>` namespace and DNS host `h-<branch>.robson.rbx.ia.br`.
- Deploy applications via Helm charts with per-branch values (image tags, hosts, secrets references).

Consequences
- Positive: fast feedback, safe merges, consistent infra; automatic cleanup on branch deletion.
- Trade-offs: cluster capacity planning; DNS/certs automation required.

Implementation Notes
- DNS: external-dns manages records for Gateway IP/LoadBalancer.
- TLS: cert-manager issues per-host certificates; Gateway references TLS secrets.
- Gateway API: per-branch `Gateway` and `HTTPRoute` resources; Istio Ambient enabled at namespace level.
- CI: build/push images tagged `<branch>-<sha>`; pass values to ApplicationSet or use ArgoCD Image Updater.

Related
- ADR-0003 Istio Ambient + Gateway API
- infra/README.md; docs/MIGRATION_PLAN.md

