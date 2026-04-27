# Robson Documentation Index

Central navigation hub for all Robson documentation.

---

## Quick Start

**New to Robson v3?** Start here:

1. **[README.md](../README.md)** - Project overview and installation
2. **[onboarding/DEVELOPER-QUICKSTART.md](onboarding/DEVELOPER-QUICKSTART.md)** - Clone to first PR
3. **[architecture/v3-runtime-spec.md](architecture/v3-runtime-spec.md)** - Robsond runtime architecture
4. **[../AGENTS.md](../AGENTS.md)** - Repository-wide rules (canonical)

**For AI Agents?** Start here:

1. **[../AGENTS.md](../AGENTS.md)** - Repository-wide AI instructions (canonical)
2. **[../CLAUDE.md](../CLAUDE.md)** - Claude Code adapter
3. **[AI_WORKFLOW.md](AI_WORKFLOW.md)** - AI collaboration guidelines

---

## Documentation Structure

```
docs/
├── INDEX.md (you are here)          # Central navigation hub
├── ARCHITECTURE.md                  # Architecture overview (legacy + v3)
├── DEVELOPER.md                     # v1 developer guide (archived)
├── AI_WORKFLOW.md                   # AI collaboration guidelines
├── AGENTIC-TRADING.md               # Trading/agentic lifecycle philosophy
├── LANGUAGE-POLICY.md               # English-only policy and rationale
├── STYLE_GUIDE.md                   # Code style conventions (Python-era)
├── ADRs.md                          # Architecture Decision Records index
├── adr/                             # Architecture Decision Records
│   ├── ADR-TEMPLATE.md
│   ├── ADR-0001 to ADR-0034
├── policies/                        # Organizational policies
│   ├── PRODUCTION-DEPLOYMENT.md     # Production deployment integrity policy
│   ├── UNTRACKED-POSITION-RECONCILIATION.md  # Robson-authored position invariant (ADR-0022)
│   └── SYMBOL-AGNOSTIC-POLICIES.md  # Policies apply to every trading pair (ADR-0023)
├── architecture/                    # Architecture documents
│   ├── v3-migration-plan.md         # v2.5 → v3 migration plan
│   ├── v3-runtime-spec.md           # v3 runtime specification
│   ├── v3-query-query-engine.md     # Query engine design
│   ├── v3-control-loop.md           # Control loop specification
│   ├── v3-architectural-decisions.md # Architectural decisions log
│   ├── v3-risk-engine-spec.md       # Risk engine specification
│   ├── v3-tron-evaluation.md        # TRON/TRC-20 evaluation
│   ├── INSTITUTIONAL_READINESS_REPORT_v2.md  # v2 institutional readiness snapshot
│   ├── OPERATION-LIFECYCLE.md        # Operation status state machine
│   ├── TRANSACTION-HIERARCHY.md      # Strategy → Operation → Movement hierarchy
│   └── README.md
├── quality/                         # Code quality tooling
│   ├── README.md                    # Quality guide (pre-commit, SonarLint)
│   └── sonarlint.md                 # SonarLint IDE setup
├── specs/                           # Specifications (TDD/BDD)
│   ├── README.md
│   └── api/openapi.yaml             # REST API (OpenAPI 3.1)
├── requirements/                    # Business requirements
│   ├── README.md
│   └── robson-*-requirements.md
├── runbooks/                        # Operational procedures
│   ├── README.md
│   ├── val-001-testnet-e2e-validation.md  # VAL-001 E2E testnet validation
│   ├── val-002-real-capital-activation.md  # VAL-002 real capital activation
│   ├── argocd-initial-setup.md
│   ├── ci-cd-image-tagging.md
│   ├── deployment-checklist.md
│   ├── deep-storage.md
│   ├── frontend-deploy.md
│   ├── FRONTEND-NGINX-TROUBLESHOOTING.md
│   └── rabbitmq-operations.md
├── implementation/                  # Implementation closeouts
│   ├── 2026-04-27-mig-v3-12-monthly-state-persistence.md
│   ├── AGENTIC-WORKFLOW-FRONTEND-IMPLEMENTATION.md
│   ├── FE-P1-FRONTEND-MVP.md
│   ├── GATE-4-OPERATION-CREATION.md
│   └── entry-policy-strategy-engine.md
├── infra/                           # Infrastructure documentation
│   └── K3S-CLUSTER-GUIDE.md         # k3s cluster deployment guide
├── k8s/frontend/                    # Frontend K8s manifests
├── ops/                             # Operations documentation
│   ├── GITOPS-GUIDE.md
│   ├── OBSERVABILITY-HARDENED.md
│   └── POST-MORTEM-2025-12-31-probe-redirect.md
├── agents/                          # Agent session closeouts
├── analysis/                        # Analysis documents
├── audits/                          # Audit reports
├── entry-gate/                      # Entry gate session logs
├── history/                         # Legacy/archived docs
│   ├── AI_FIRST_TRANSFORMATION.md
│   ├── MIGRATION_PLAN.md
│   └── monolith/MIGRATION_GUIDE.md
├── operations/                      # Production operation logs
├── plan/                            # Planning documents
│   ├── README.md
│   └── infra/README.md
├── sessions/                        # Session closeout notes
├── specs/                           # Feature specifications
├── strategy/                        # Strategy documentation
│   ├── HAND_SPAN_TRAILING_STOP.md
│   ├── IRON_EXIT_PROTOCOL.md
│   └── IMPLEMENTATION_SUMMARY.md (archived to archive/)
├── onboarding/                      # Onboarding guides
│   └── DEVELOPER-QUICKSTART.md
└── archive/                         # Archived (superseded) docs
    ├── ai-first/                    # ParadeDB/DeepSeek RAG (never built for v3)
    ├── strategy/                    # Django pattern engine implementations
    ├── guides/                      # Django migration guides
    ├── market-context/              # Django market research
    ├── plan/                        # Django-era planning docs and prompts
    ├── testing/                     # Django-era test plans
    ├── ops/                         # Portuguese Django observability
    ├── features/                    # Django v1 features
    └── ...                          # Individual archived files
```

---

## Documentation by Role

### For Developers

**Getting Started**:
- [Developer Quickstart](onboarding/DEVELOPER-QUICKSTART.md) - Current onboarding
- [Code Style Guide](STYLE_GUIDE.md) - Coding conventions
- [Language Policy](LANGUAGE-POLICY.md) - English-only requirement
- [Quality Tooling](quality/README.md) - Pre-commit, SonarLint

**Architecture**:
- [System Architecture](ARCHITECTURE.md) - Architecture overview
- [ADRs Index](ADRs.md) - All architectural decisions

**Contributing**:
- [CONTRIBUTING.md](../CONTRIBUTING.md) - Contribution guidelines

### For Operations / SRE

**Policies** (READ FIRST):
- [Production Deployment Policy](policies/PRODUCTION-DEPLOYMENT.md) - GitOps-only deployments
- [Untracked Position Reconciliation](policies/UNTRACKED-POSITION-RECONCILIATION.md) - ADR-0022
- [Symbol-Agnostic Policies](policies/SYMBOL-AGNOSTIC-POLICIES.md) - ADR-0023

**Deployment**:
- [K3s Cluster Guide](infra/K3S-CLUSTER-GUIDE.md) - k3s deployment guide
- [Deployment Checklist](runbooks/deployment-checklist.md) - Deployment procedures
- [ArgoCD Setup](runbooks/argocd-initial-setup.md) - ArgoCD configuration
- [CI/CD Image Tagging](runbooks/ci-cd-image-tagging.md) - Image versioning

**Infrastructure**:
- [ADR-0011: GitOps Auto-Deploy](adr/ADR-0011-gitops-automatic-manifest-updates.md)

### For AI Agents

**Start Here**:
- **[../AGENTS.md](../AGENTS.md)** - Repository-wide AI instructions (canonical)
- **[../CLAUDE.md](../CLAUDE.md)** - Claude Code adapter
- [AI Workflow](AI_WORKFLOW.md) - Collaboration guidelines
- [Language Policy](LANGUAGE-POLICY.md) - English-only requirement

**Context**:
- [Architecture](ARCHITECTURE.md) - System design
- [ADRs Index](ADRs.md) - All architectural decisions
- [Specs](specs/README.md) - Feature specifications

### For Traders / Users

**Getting Started**:
- [Position Sizing Golden Rule](requirements/POSITION-SIZING-GOLDEN-RULE.md) - How position sizes are calculated
- [Technical Stop Documentation](requirements/technical-stop-requirements.md) - Technical stop-loss

---

## Documentation by Topic

### Architecture

| Topic | Document |
|-------|----------|
| **High-Level Overview** | [ARCHITECTURE.md](ARCHITECTURE.md) |
| **Robson v3 Migration** | [architecture/v3-migration-plan.md](architecture/v3-migration-plan.md) |
| **Robson v3 Runtime** | [architecture/v3-runtime-spec.md](architecture/v3-runtime-spec.md) |
| **Robson v3 Risk Engine** | [architecture/v3-risk-engine-spec.md](architecture/v3-risk-engine-spec.md) |
| **Hexagonal Architecture** | [ADR-0002](adr/ADR-0002-hexagonal-architecture.md) |
| **Transaction Hierarchy** | [architecture/TRANSACTION-HIERARCHY.md](architecture/TRANSACTION-HIERARCHY.md) |
| **GitOps Auto-Deploy** | [ADR-0011](adr/ADR-0011-gitops-automatic-manifest-updates.md) |
| **Production Integrity** | [ADR-0029](adr/ADR-0029-production-deployment-integrity.md) |
| **Opportunity Detection vs Technical Stop** | [ADR-0021](adr/ADR-0021-opportunity-detection-vs-technical-stop-analysis.md) |
| **Robson-Authored Position Invariant** | [ADR-0022](adr/ADR-0022-robson-authored-position-invariant.md) |
| **Symbol-Agnostic Policy Invariant** | [ADR-0023](adr/ADR-0023-symbol-agnostic-policy-invariant.md) |
| **Institutional Readiness (v2)** | [architecture/INSTITUTIONAL_READINESS_REPORT_v2.md](architecture/INSTITUTIONAL_READINESS_REPORT_v2.md) |
| **TRON Evaluation** | [architecture/v3-tron-evaluation.md](architecture/v3-tron-evaluation.md) |

### Operations

| Topic | Document |
|-------|----------|
| **K3s Cluster Deployment** | [infra/K3S-CLUSTER-GUIDE.md](infra/K3S-CLUSTER-GUIDE.md) |
| **ArgoCD Setup** | [runbooks/argocd-initial-setup.md](runbooks/argocd-initial-setup.md) |
| **CI/CD & Image Tagging** | [runbooks/ci-cd-image-tagging.md](runbooks/ci-cd-image-tagging.md) |
| **Deployment Checklist** | [runbooks/deployment-checklist.md](runbooks/deployment-checklist.md) |
| **VAL-001 Testnet E2E** | [runbooks/val-001-testnet-e2e-validation.md](runbooks/val-001-testnet-e2e-validation.md) |
| **VAL-002 Real Capital** | [runbooks/val-002-real-capital-activation.md](runbooks/val-002-real-capital-activation.md) |
| **GitOps Guide** | [ops/GITOPS-GUIDE.md](ops/GITOPS-GUIDE.md) |
| **Observability Hardened** | [ops/OBSERVABILITY-HARDENED.md](ops/OBSERVABILITY-HARDENED.md) |

### API & Specs

| Topic | Document |
|-------|----------|
| **REST API** | [specs/api/openapi.yaml](specs/api/openapi.yaml) |
| **Spec-Driven Development** | [specs/README.md](specs/README.md) |
| **Requirements** | [requirements/README.md](requirements/README.md) |

### Policies & Governance

| Topic | Document |
|-------|----------|
| **Production Deployment Policy** | [policies/PRODUCTION-DEPLOYMENT.md](policies/PRODUCTION-DEPLOYMENT.md) |
| **Untracked Position Reconciliation** (ADR-0022) | [policies/UNTRACKED-POSITION-RECONCILIATION.md](policies/UNTRACKED-POSITION-RECONCILIATION.md) |
| **Symbol-Agnostic Policies** (ADR-0023) | [policies/SYMBOL-AGNOSTIC-POLICIES.md](policies/SYMBOL-AGNOSTIC-POLICIES.md) |
| **Language Policy** | [LANGUAGE-POLICY.md](LANGUAGE-POLICY.md) |

---

## Archive

Superseded documentation is moved to `docs/archive/`. These files are kept for historical reference only and should not be used for current development. Key archived collections:

- `archive/ai-first/` — ParadeDB/DeepSeek RAG architecture (never built for v3)
- `archive/plan/` — Django-era planning documents, prompts, and execution plans
- `archive/strategy/` — Django pattern engine and trailing stop implementation summaries
- `archive/guides/` — Django migration and event-sourcing guides
- Individual archived files: `INITIAL-AUDIT.md`, `AUTH_FLOW.md`, `PRODUCTION_TRADING.md`, `STRATEGIES.md`, `STATIC-FILES-ARCHITECTURE.md`, etc.

---

**Last Updated**: 2026-04-27
