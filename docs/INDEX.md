# Robson Bot Documentation Index

**Central navigation hub for all Robson Bot documentation.**

---

## 🚀 Quick Start

**New to Robson Bot?** Start here:

1. **[README.md](../README.md)** - Project overview and installation
2. **[ARCHITECTURE.md](ARCHITECTURE.md)** - System architecture and design principles
3. **[DEVELOPER.md](DEVELOPER.md)** - Development workflow and practices

**For AI Agents?** Start here:

1. **[AGENTS.md](AGENTS.md)** - Comprehensive guide for AI-assisted development
2. **[../CLAUDE.md](../CLAUDE.md)** - Claude Code specific context
3. **[AI_WORKFLOW.md](AI_WORKFLOW.md)** - AI collaboration guidelines

---

## 📚 Documentation Structure

```
docs/
├── INDEX.md (you are here)          # Central navigation hub
├── ARCHITECTURE.md                  # High-level architecture overview
├── DEVELOPER.md                     # Developer workflow and practices
├── AI_WORKFLOW.md                   # AI collaboration guidelines
├── AGENTS.md                        # Comprehensive AI agent guide
├── AUTH_FLOW.md                     # Authentication flow documentation
├── CONTRIBUTING-ADAPTERS.md         # Adapter development guide
├── STYLE_GUIDE.md                   # Code style conventions
├── LANGUAGE-POLICY.md               # English-only policy and rationale
├── ADRs.md                          # Architecture Decision Records index
├── adr/                             # Architecture Decision Records
│   ├── ADR-TEMPLATE.md
│   ├── ADR-0001 to ADR-0010
├── ai-first/                        # AI-First transformation docs
│   ├── ARCHITECTURE.md
│   ├── DEEPSEEK_GATEWAY.md
│   ├── INGESTION_EVENTS.md
│   └── SQL_SCHEMA.md
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

**Architecture**:
- [System Architecture](ARCHITECTURE.md) - Hexagonal architecture overview
- [ADRs Index](ADRs.md) - All architectural decisions

**Contributing**:
- [CONTRIBUTING.md](../CONTRIBUTING.md) - Contribution guidelines
- [Adapter Development](CONTRIBUTING-ADAPTERS.md) - Building adapters

### For **Operations / SRE**

**Deployment**:
- [Infrastructure README](../infra/README.md) - Infrastructure overview
- [K3s Cluster Guide](infra/K3S-CLUSTER-GUIDE.md) - k3s deployment guide
- [Deployment Checklist](runbooks/deployment-checklist.md) - Deployment procedures
- [ArgoCD Setup](runbooks/argocd-initial-setup.md) - ArgoCD configuration
- [CI/CD Image Tagging](runbooks/ci-cd-image-tagging.md) - Image versioning

**Infrastructure**:
- [ADR-0003: Istio Ambient](adr/ADR-0003-istio-ambient-gateway-api.md)
- [ADR-0004: GitOps Previews](adr/ADR-0004-gitops-preview-envs.md)
- [ADR-0005: Ansible Bootstrap](adr/ADR-0005-ansible-bootstrap-hardening.md)

### For **AI Agents**

**Start Here**:
- **[AGENTS.md](AGENTS.md)** - Comprehensive guide for AI-assisted development
- [AI Workflow](AI_WORKFLOW.md) - Collaboration guidelines
- [Language Policy](LANGUAGE-POLICY.md) - English-only requirement

**Context**:
- [Architecture](ARCHITECTURE.md) - System design
- [ADRs Index](ADRs.md) - All architectural decisions
- [Specs](specs/README.md) - Feature specifications

**Tool-Specific**:
- [../CLAUDE.md](../CLAUDE.md) - Claude Code integration
- [../.cursorrules](../.cursorrules) - Cursor AI rules

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
| **Hexagonal Architecture** | [ADR-0002](adr/ADR-0002-hexagonal-architecture.md) |
| **AI-First Architecture** | [ai-first/ARCHITECTURE.md](ai-first/ARCHITECTURE.md) |
| **ParadeDB Database** | [ADR-0007](adr/ADR-0007-paradedb-primary-database.md) |
| **Transaction Hierarchy** | [architecture/TRANSACTION-HIERARCHY.md](architecture/TRANSACTION-HIERARCHY.md) |
| **GitOps Auto-Deploy** | [ADR-0011](adr/ADR-0011-gitops-automatic-manifest-updates.md) |

### Development

| Topic | Document |
|-------|----------|
| **Developer Workflow** | [DEVELOPER.md](DEVELOPER.md) |
| **Code Style** | [STYLE_GUIDE.md](STYLE_GUIDE.md) |
| **Contributing** | [../CONTRIBUTING.md](../CONTRIBUTING.md) |
| **Adapter Development** | [CONTRIBUTING-ADAPTERS.md](CONTRIBUTING-ADAPTERS.md) |

### Operations

| Topic | Document |
|-------|----------|
| **K3s Cluster Deployment** | [infra/K3S-CLUSTER-GUIDE.md](infra/K3S-CLUSTER-GUIDE.md) |
| **ArgoCD Setup** | [runbooks/argocd-initial-setup.md](runbooks/argocd-initial-setup.md) |
| **CI/CD & Image Tagging** | [runbooks/ci-cd-image-tagging.md](runbooks/ci-cd-image-tagging.md) |
| **Deployment Checklist** | [runbooks/deployment-checklist.md](runbooks/deployment-checklist.md) |
| **Infrastructure** | [../infra/README.md](../infra/README.md) |
| **First Leveraged Position** | [operations/2025-12-24-first-leveraged-position.md](operations/2025-12-24-first-leveraged-position.md) |

### API & Specs

| Topic | Document |
|-------|----------|
| **REST API** | [specs/api/openapi.yaml](specs/api/openapi.yaml) |
| **Spec-Driven Development** | [specs/README.md](specs/README.md) |
| **Requirements** | [requirements/README.md](requirements/README.md) |

---

## 🤖 AI-First Documentation

Robson Bot is optimized for AI-assisted development:

### Core AI Documents

1. **[AGENTS.md](AGENTS.md)** - Master guide for all AI agents
2. **[../CLAUDE.md](../CLAUDE.md)** - Claude Code specific context
3. **[AI_WORKFLOW.md](AI_WORKFLOW.md)** - Collaboration rules
4. **[LANGUAGE-POLICY.md](LANGUAGE-POLICY.md)** - Why English only

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

1. [Infrastructure Overview](../infra/README.md)
2. [K3s Cluster Guide](infra/K3S-CLUSTER-GUIDE.md)
3. [ADR-0004: GitOps Previews](adr/ADR-0004-gitops-preview-envs.md)
4. [ArgoCD Setup](runbooks/argocd-initial-setup.md)

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
- [Gateway API](https://gateway-api.sigs.k8s.io/)

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

**Last Updated**: 2024-12-24
**Maintained by**: Robson Bot Core Team
**License**: Same as project
