# Robson Bot Documentation Index

**Central navigation hub for all Robson Bot documentation.**

---

## 🚀 Quick Start

**New to Robson v3?** Start here:

1. **[README.md](../README.md)** - Project overview and installation
2. **[onboarding/DEVELOPER-QUICKSTART.md](onboarding/DEVELOPER-QUICKSTART.md)** - Clone → first PR (current)
3. **[architecture/v3-runtime-spec.md](architecture/v3-runtime-spec.md)** - Robsond runtime architecture
4. **[../AGENTS.md](../AGENTS.md)** - Repository-wide rules (canonical)

> **Note:** `DEVELOPER.md` and `AUTH_FLOW.md` describe the v1 Django
> stack and are kept only for historical reference. Robson v3 is the
> canonical version (Rust runtime + SvelteKit frontend).

**For AI Agents?** Start here:

1. **[AGENTS.md](AGENTS.md)** - Comprehensive guide for AI-assisted development
2. **[SKILL.md](SKILL.md)** - AI Skills Framework and prompt engineering guide
3. **[AI-MEMORY-DB.md](AI-MEMORY-DB.md)** - Runtime knowledge store (learns from PRs)
4. **[../CLAUDE.md](../CLAUDE.md)** - Claude Code specific context
5. **[AI_WORKFLOW.md](AI_WORKFLOW.md)** - AI collaboration guidelines

---

## 📚 Documentation Structure

```
docs/
├── INDEX.md (you are here)          # Central navigation hub
├── ARCHITECTURE.md                  # High-level architecture overview
├── DEVELOPER.md                     # Developer workflow and practices
├── AI_WORKFLOW.md                   # AI collaboration guidelines
├── AGENTS.md                        # Comprehensive AI agent guide
├── SKILL.md                         # AI Skills Framework (prompt engineering)
├── AI-MEMORY-DB.md                  # Runtime knowledge store (learns from PRs)
├── AUTH_FLOW.md                     # Authentication flow documentation
├── CONTRIBUTING-ADAPTERS.md         # Adapter development guide
├── STYLE_GUIDE.md                   # Code style conventions
├── LANGUAGE-POLICY.md               # English-only policy and rationale
├── ADRs.md                          # Architecture Decision Records index
├── adr/                             # Architecture Decision Records
│   ├── ADR-TEMPLATE.md
│   ├── ADR-0001 to ADR-0023
├── policies/                        # Organizational policies
│   ├── PRODUCTION-DEPLOYMENT.md     # Production deployment integrity policy
│   ├── UNTRACKED-POSITION-RECONCILIATION.md  # Robson-authored position invariant (ADR-0022)
│   └── SYMBOL-AGNOSTIC-POLICIES.md  # Policies apply to every trading pair (ADR-0023)
├── ai-first/                        # AI-First transformation docs
│   ├── ARCHITECTURE.md
│   ├── DEEPSEEK_GATEWAY.md
│   ├── INGESTION_EVENTS.md
│   └── SQL_SCHEMA.md
├── quality/                         # Code quality tooling
│   ├── README.md                    # Quality guide (pre-commit, SonarLint, SonarQube)
│   └── sonarlint.md                 # SonarLint IDE setup
├── specs/                           # Specifications (TDD/BDD)
│   ├── README.md
│   └── api/openapi.yaml             # REST API (OpenAPI 3.1)
├── requirements/                    # Business requirements
│   ├── README.md
│   └── robson-*-requirements.md
├── runbooks/                        # Operational procedures
│   ├── README.md
│   ├── argocd-initial-setup.md
│   ├── ci-cd-image-tagging.md
│   └── deployment-checklist.md
├── infra/                           # Infrastructure documentation
│   └── K3S-CLUSTER-GUIDE.md         # k3s cluster deployment guide
├── plan/                            # Planning documents
│   ├── infra/                       # Infrastructure deployment plans
│   └── prompts/                     # AI prompts for implementation
├── FEATURES/                        # Feature documentation
│   └── trading-thesis.md            # Trading Thesis feature (v1 Chat)
└── history/                         # Legacy/archived docs
    ├── 2024-12-deployment/          # December 2024 deployment archive
    └── monolith/
```

---

## 👥 Documentation by Role

### For **Developers**

**Getting Started**:
- [Developer Workflow](DEVELOPER.md) - Day-to-day development practices
- [Code Style Guide](STYLE_GUIDE.md) - Coding conventions
- [Language Policy](LANGUAGE-POLICY.md) - English-only requirement
- [Quality Tooling](quality/README.md) - Pre-commit, SonarLint, SonarQube

**Architecture**:
- [System Architecture](ARCHITECTURE.md) - Hexagonal architecture overview
- [ADRs Index](ADRs.md) - All architectural decisions

**Contributing**:
- [CONTRIBUTING.md](../CONTRIBUTING.md) - Contribution guidelines
- [Adapter Development](CONTRIBUTING-ADAPTERS.md) - Building adapters

### For **Operations / SRE**

**Policies** (READ FIRST):
- [Production Deployment Policy](policies/PRODUCTION-DEPLOYMENT.md) - **CRITICAL**: GitOps-only deployments
- [Untracked Position Reconciliation](policies/UNTRACKED-POSITION-RECONCILIATION.md) - **CRITICAL**: every open position must be Robson-authored (ADR-0022)
- [Symbol-Agnostic Policies](policies/SYMBOL-AGNOSTIC-POLICIES.md) - rules apply to every trading pair (ADR-0023)

**Deployment**:
- [K3s Cluster Guide](infra/K3S-CLUSTER-GUIDE.md) - k3s deployment guide
- [Deployment Checklist](runbooks/deployment-checklist.md) - Deployment procedures
- [ArgoCD Setup](runbooks/argocd-initial-setup.md) - ArgoCD configuration
- [CI/CD Image Tagging](runbooks/ci-cd-image-tagging.md) - Image versioning

**Infrastructure**:
- [ADR-0011: GitOps Auto-Deploy](adr/ADR-0011-gitops-automatic-manifest-updates.md)

### For **AI Agents**

**Start Here**:
- **[AGENTS.md](AGENTS.md)** - Comprehensive guide for AI-assisted development
- **[SKILL.md](SKILL.md)** - AI Skills Framework and prompt engineering
- **[AI-MEMORY-DB.md](AI-MEMORY-DB.md)** - Runtime knowledge store (learns from PRs)
- [AI Workflow](AI_WORKFLOW.md) - Collaboration guidelines
- [Language Policy](LANGUAGE-POLICY.md) - English-only requirement

**Context**:
- [Architecture](ARCHITECTURE.md) - System design
- [ADRs Index](ADRs.md) - All architectural decisions
- [Specs](specs/README.md) - Feature specifications

**Tool-Specific**:
- [../CLAUDE.md](../CLAUDE.md) - Claude Code integration
- [../.cursorrules](../.cursorrules) - Cursor AI rules

### For **Traders / Users**

**Getting Started**:
- **[Strategies Guide](STRATEGIES.md)** - Pre-built trading strategies (All In, Rescue Forces, etc.)
- [Position Sizing Golden Rule](requirements/POSITION-SIZING-GOLDEN-RULE.md) - How position sizes are calculated
- [Technical Stop Documentation](requirements/TECHNICAL-STOP.md) - Understanding technical stop-loss

**Trading**:
- [Risk Management](RISK-MANAGEMENT.md) - Risk controls and limits
- [Pattern Detection](PATTERN-DETECTION.md) - Automated pattern recognition

### For **Product / Business**

**Overview**:
- [Project README](../README.md) - What is Robson Bot?
- [Requirements](requirements/README.md) - Business requirements

**API**:
- [API Documentation](specs/api/openapi.yaml) - API capabilities

---

## 🔍 Documentation by Topic

### Architecture

| Topic | Document |
|-------|----------|
| **High-Level Overview** | [ARCHITECTURE.md](ARCHITECTURE.md) |
| **Robson v3 Migration** | [architecture/v3-migration-plan.md](architecture/v3-migration-plan.md) |
| **Robson v3 Runtime** | [architecture/v3-runtime-spec.md](architecture/v3-runtime-spec.md) |
| **Robson v3 Risk Engine** | [architecture/v3-risk-engine-spec.md](architecture/v3-risk-engine-spec.md) |
| **Hexagonal Architecture** | [ADR-0002](adr/ADR-0002-hexagonal-architecture.md) |
| **AI-First Architecture** | [ai-first/ARCHITECTURE.md](ai-first/ARCHITECTURE.md) |
| **ParadeDB Database** | [ADR-0007](adr/ADR-0007-paradedb-primary-database.md) |
| **Transaction Hierarchy** | [architecture/TRANSACTION-HIERARCHY.md](architecture/TRANSACTION-HIERARCHY.md) |
| **GitOps Auto-Deploy** | [ADR-0011](adr/ADR-0011-gitops-automatic-manifest-updates.md) |
| **Production Integrity** | [ADR-0012](adr/ADR-0012-production-deployment-integrity.md) |
| **Opportunity Detection vs Technical Stop** | [ADR-0021](adr/ADR-0021-opportunity-detection-vs-technical-stop-analysis.md) |
| **Robson-Authored Position Invariant** | [ADR-0022](adr/ADR-0022-robson-authored-position-invariant.md) |
| **Symbol-Agnostic Policy Invariant** | [ADR-0023](adr/ADR-0023-symbol-agnostic-policy-invariant.md) |

### Development

| Topic | Document |
|-------|----------|
| **Developer Workflow** | [DEVELOPER.md](DEVELOPER.md) |
| **Code Style** | [STYLE_GUIDE.md](STYLE_GUIDE.md) |
| **Contributing** | [../CONTRIBUTING.md](../CONTRIBUTING.md) |
| **Adapter Development** | [CONTRIBUTING-ADAPTERS.md](CONTRIBUTING-ADAPTERS.md) |

### Code Quality

| Topic | Document |
|-------|----------|
| **Quality Tooling Overview** | [quality/README.md](quality/README.md) |
| **Pre-commit Hooks** | [.pre-commit-config.yaml](../.pre-commit-config.yaml) |
| **SonarLint (IDE)** | [quality/sonarlint.md](quality/sonarlint.md) |
| **SonarQube (CI)** | [sonar-project.properties](../sonar-project.properties) |

### Strategies

| Topic | Document |
|-------|----------|
| **Iron Exit Protocol** | [strategy/IRON_EXIT_PROTOCOL.md](strategy/IRON_EXIT_PROTOCOL.md) |

### Operations

| Topic | Document |
|-------|----------|
| **K3s Cluster Deployment** | [infra/K3S-CLUSTER-GUIDE.md](infra/K3S-CLUSTER-GUIDE.md) |
| **ArgoCD Setup** | [runbooks/argocd-initial-setup.md](runbooks/argocd-initial-setup.md) |
| **CI/CD & Image Tagging** | [runbooks/ci-cd-image-tagging.md](runbooks/ci-cd-image-tagging.md) |
| **Deployment Checklist** | [runbooks/deployment-checklist.md](runbooks/deployment-checklist.md) |
| **Runbooks Overview** | [runbooks/README.md](runbooks/README.md) |
| **First Leveraged Position** | [operations/2025-12-24-first-leveraged-position.md](operations/2025-12-24-first-leveraged-position.md) |
| **Isolated Margin SHORT (BTCUSDC)** | [operations/2026-01-05-isolated-margin-short-btcusdc.md](operations/2026-01-05-isolated-margin-short-btcusdc.md) |

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

## 🤖 AI-First Documentation

Robson Bot is optimized for AI-assisted development:

### Core AI Documents

1. **[AGENTS.md](AGENTS.md)** - Master guide for all AI agents
2. **[SKILL.md](SKILL.md)** - AI Skills Framework and prompt engineering
3. **[AI-MEMORY-DB.md](AI-MEMORY-DB.md)** - Runtime knowledge store (learns from PRs)
4. **[../CLAUDE.md](../CLAUDE.md)** - Claude Code specific context
5. **[AI_WORKFLOW.md](AI_WORKFLOW.md)** - Collaboration rules
6. **[LANGUAGE-POLICY.md](LANGUAGE-POLICY.md)** - Why English only

### AI Tool Configuration

- **Claude Code**: [../CLAUDE.md](../CLAUDE.md)
- **Cursor AI**: [../.cursorrules](../.cursorrules)

---

## 📖 Learning Paths

### Path 1: New Backend Developer

1. [Hexagonal Architecture](ARCHITECTURE.md)
2. [ADR-0002: Hexagonal Architecture](adr/ADR-0002-hexagonal-architecture.md)
3. [Adapter Development](CONTRIBUTING-ADAPTERS.md)
4. [Code Style](STYLE_GUIDE.md)

### Path 2: New Frontend Developer

1. [Frontend README](../apps/frontend/README.md)
2. [API Specs](specs/api/openapi.yaml)

### Path 3: New DevOps Engineer

1. [Production Deployment Policy](policies/PRODUCTION-DEPLOYMENT.md) ⚠️ **READ FIRST**
2. [K3s Cluster Guide](infra/K3S-CLUSTER-GUIDE.md)
3. [ADR-0011: GitOps Auto-Deploy](adr/ADR-0011-gitops-automatic-manifest-updates.md)
4. [ArgoCD Setup](runbooks/argocd-initial-setup.md)
5. [Deployment Checklist](runbooks/deployment-checklist.md)

### Path 4: Understanding Architecture

1. [System Architecture](ARCHITECTURE.md)
2. [All ADRs](ADRs.md)
3. [AI-First Architecture](ai-first/ARCHITECTURE.md)

---

## 🔗 External Resources

### Hexagonal Architecture
- [Netflix - Ready for changes with Hexagonal Architecture](https://netflixtechblog.com/ready-for-changes-with-hexagonal-architecture-b315ec967749)

### Django Best Practices
- [Django REST Framework](https://www.django-rest-framework.org/)

### Kubernetes
- [Traefik Documentation](https://doc.traefik.io/traefik/)

### GitOps
- [ArgoCD Documentation](https://argo-cd.readthedocs.io/)

---

## 📝 Documentation Standards

### Markdown
- Use GitHub Flavored Markdown
- Use relative links for internal references
- Include table of contents for long documents

### Code Samples
- Always include language identifier for syntax highlighting
- Keep examples concise and focused

### Cross-Linking
- Link freely between related documents
- Use relative paths, not absolute
- Update INDEX.md when adding new docs

---

**Last Updated**: 2026-04-18
**Maintained by**: Robson Bot Core Team
**License**: Same as project
