# Argo CD Configuration Examples

Minimal, complete YAML examples for RBX Systems GitOps patterns. No secrets are included.

---

## 1. App-of-Apps Bootstrap

The root Application that bootstraps all other Applications and ApplicationSets.

```yaml
# infra/k8s/gitops/app-of-apps/root.yml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: rbx-root
  namespace: argocd
  labels:
    rbx.product: platform
    rbx.env: production
    rbx.agent_id: human
    app.kubernetes.io/part-of: rbx-systems
spec:
  project: default
  source:
    repoURL: https://github.com/ldamasio/robson.git
    targetRevision: main
    path: infra/k8s/gitops/app-of-apps
  destination:
    server: https://kubernetes.default.svc
    namespace: argocd
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
    syncOptions:
      - CreateNamespace=true
    retry:
      limit: 5
      backoff:
        duration: 5s
        factor: 2
        maxDuration: 3m
```

The `app-of-apps/` directory contains child Application YAMLs. When a new YAML is added to that directory and merged, the root Application syncs and creates the child.

### Child Application Example

```yaml
# infra/k8s/gitops/app-of-apps/cert-manager.yml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: platform-cert-manager
  namespace: argocd
  labels:
    rbx.product: platform
    rbx.env: production
    rbx.agent_id: human
  finalizers:
    - resources-finalizer.argocd.argoproj.io
spec:
  project: default
  source:
    repoURL: https://github.com/ldamasio/robson.git
    targetRevision: main
    path: infra/k8s/platform/cert-manager
  destination:
    server: https://kubernetes.default.svc
    namespace: cert-manager
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
    syncOptions:
      - CreateNamespace=true
```

---

## 2. ApplicationSet: Git Directory Generator

Generates one Application per subdirectory under `infra/k8s/products/`. When a new product directory is added, a new Application is created automatically.

```yaml
# infra/k8s/gitops/applicationsets/products.yml
apiVersion: argoproj.io/v1alpha1
kind: ApplicationSet
metadata:
  name: rbx-products
  namespace: argocd
  labels:
    rbx.product: platform
    rbx.env: production
    rbx.agent_id: human
spec:
  goTemplate: true
  goTemplateOptions: ["missingkey=error"]
  generators:
    - git:
        repoURL: https://github.com/ldamasio/robson.git
        revision: main
        directories:
          - path: infra/k8s/products/*
  template:
    metadata:
      name: "rbx-{{ .path.basename }}"
      namespace: argocd
      labels:
        rbx.product: "{{ .path.basename }}"
        rbx.env: production
        rbx.agent_id: ci-bot
        app.kubernetes.io/part-of: rbx-systems
    spec:
      project: default
      source:
        repoURL: https://github.com/ldamasio/robson.git
        targetRevision: main
        path: "{{ .path.path }}"
      destination:
        server: https://kubernetes.default.svc
        namespace: "{{ .path.basename }}"
      syncPolicy:
        automated:
          prune: true
          selfHeal: true
        syncOptions:
          - CreateNamespace=true
        retry:
          limit: 3
          backoff:
            duration: 5s
            factor: 2
            maxDuration: 2m
```

**Directory structure that feeds this generator**:

```
infra/k8s/products/
├── robson/
│   ├── deployment.yml
│   ├── service.yml
│   └── ingress.yml
├── strategos/
│   ├── deployment.yml
│   ├── service.yml
│   └── ingress.yml
└── thalamus/
    ├── deployment.yml
    └── service.yml
```

Each subdirectory produces one Application: `rbx-robson`, `rbx-strategos`, `rbx-thalamus`.

---

## 3. ApplicationSet: List Generator for Environments

Generates one Application per environment from an explicit list. Useful when environments have different cluster endpoints or namespace conventions.

```yaml
# infra/k8s/gitops/applicationsets/environments.yml
apiVersion: argoproj.io/v1alpha1
kind: ApplicationSet
metadata:
  name: rbx-robson-envs
  namespace: argocd
  labels:
    rbx.product: robson
    rbx.agent_id: human
spec:
  goTemplate: true
  goTemplateOptions: ["missingkey=error"]
  generators:
    - list:
        elements:
          - env: production
            namespace: robson
            cluster: https://kubernetes.default.svc
            values_file: values-prod.yaml
          - env: staging
            namespace: robson-staging
            cluster: https://kubernetes.default.svc
            values_file: values-staging.yaml
  template:
    metadata:
      name: "robson-{{ .env }}"
      namespace: argocd
      labels:
        rbx.product: robson
        rbx.env: "{{ .env }}"
        rbx.agent_id: human
        app.kubernetes.io/part-of: rbx-systems
    spec:
      project: default
      source:
        repoURL: https://github.com/ldamasio/robson.git
        targetRevision: main
        path: infra/charts/robson-backend
        helm:
          valueFiles:
            - "{{ .values_file }}"
      destination:
        server: "{{ .cluster }}"
        namespace: "{{ .namespace }}"
      syncPolicy:
        automated:
          prune: true
          selfHeal: true
        syncOptions:
          - CreateNamespace=true
        retry:
          limit: 3
          backoff:
            duration: 5s
            factor: 2
            maxDuration: 2m
```

This produces two Applications: `robson-production` and `robson-staging`, each pointing to the same Helm chart but with different values files.

---

## 4. Recommended Repository Layout

```
infra/
├── k8s/
│   ├── gitops/                           # Argo CD resource definitions
│   │   ├── app-of-apps/
│   │   │   ├── root.yml                  # Root Application (entry point)
│   │   │   ├── cert-manager.yml          # Platform: cert-manager
│   │   │   ├── istio-ambient.yml         # Platform: Istio
│   │   │   ├── gateway-api-crds.yml      # Platform: Gateway API
│   │   │   ├── argocd.yml               # Platform: Argo CD self-management
│   │   │   ├── robson-prod.yml           # Product: robson (until Phase 3)
│   │   │   └── applicationsets.yml       # References the applicationsets/ dir
│   │   │
│   │   ├── applicationsets/
│   │   │   ├── branches.yml              # PR preview generator
│   │   │   ├── products.yml              # (Phase 3) Git dir generator
│   │   │   └── environments.yml          # (Phase 4) List generator
│   │   │
│   │   ├── policies/
│   │   │   └── sync-windows.yml          # Maintenance windows (optional)
│   │   │
│   │   └── rbac/
│   │       ├── project-robson.yml        # AppProject for robson
│   │       └── project-platform.yml      # AppProject for platform
│   │
│   ├── platform/                         # Platform component manifests
│   │   ├── argocd/
│   │   ├── cert-manager/
│   │   ├── gateway-api-crds/
│   │   └── istio-ambient/
│   │
│   ├── prod/                             # Robson production manifests
│   ├── staging/                          # Robson staging manifests
│   │
│   └── products/                         # (Phase 3) Per-product manifests
│       ├── robson/
│       ├── strategos/
│       └── thalamus/
│
├── charts/                               # Helm charts (source for Helm Applications)
│   ├── robson-backend/
│   │   ├── Chart.yaml
│   │   ├── values.yaml                   # Default values
│   │   ├── values-prod.yaml              # Production overrides
│   │   ├── values-staging.yaml           # Staging overrides
│   │   └── templates/
│   └── robson-frontend/
│
├── apps/                                 # Infrastructure applications (DNS, etc.)
│
└── docs/
    └── gitops/
        ├── ARGOCD-STRATEGY.md
        ├── ARGOCD-DECISION-RECORD.md
        ├── EXAMPLES.md                   # This file
        ├── RUNBOOK-GITOPS-CHANGES.md
        └── GLOSSARY.md
```

### Key Points

- `gitops/app-of-apps/` contains explicit Application YAMLs for the root and its platform children.
- `gitops/applicationsets/` contains ApplicationSet definitions. The root App-of-Apps has a child that points to this directory.
- `platform/` contains the actual manifests for infrastructure services.
- `prod/` and `staging/` contain product deployment manifests (current approach).
- `products/` is the future home for per-product directories consumed by the git directory generator.
- `charts/` contains Helm charts referenced by Applications and ApplicationSets.

---

## References

- [ARGOCD-STRATEGY.md](./ARGOCD-STRATEGY.md)
- [ARGOCD-DECISION-RECORD.md](./ARGOCD-DECISION-RECORD.md)
- [Argo CD ApplicationSet Generators](https://argo-cd.readthedocs.io/en/stable/operator-manual/applicationset/Generators/)
