# Robson Bot Documentation Index

**Central navigation hub for all Robson Bot documentation.**

Welcome! This index helps you find the right documentation quickly, whether you're a developer, operator, AI agent, or contributor.

---

## 🚀 Quick Start

**New to Robson Bot?** Start here:

1. **[README.md](../README.md)** - Project overview and installation
2. **[ARCHITECTURE.md](ARCHITECTURE.md)** - System architecture and design principles
3. **[development/setup.md](development/setup.md)** - Local development environment setup
4. **[DEVELOPER.md](DEVELOPER.md)** - Development workflow and practices

**For AI Agents?** Start here:

1. **[AGENTS.md](AGENTS.md)** - Comprehensive guide for AI-assisted development
2. **[../CLAUDE.md](../CLAUDE.md)** - Claude Code specific context
3. **[AI_WORKFLOW.md](AI_WORKFLOW.md)** - AI collaboration guidelines

---

## 📚 Documentation Structure

```
docs/
├── INDEX.md (you are here)          # Central navigation hub
├── INITIAL-AUDIT.md                 # Repository transformation baseline
├── LANGUAGE-POLICY.md               # English-only policy and rationale
├── ARCHITECTURE.md                  # High-level architecture overview
├── DEVELOPER.md                     # Developer workflow and practices
├── AI_WORKFLOW.md                   # AI collaboration guidelines
├── AGENTS.md                        # Comprehensive AI agent guide
├── AUTH_FLOW.md                     # Authentication flow documentation
├── CONTRIBUTING-ADAPTERS.md         # Adapter development guide
├── STYLE_GUIDE.md                   # Code style conventions
├── ADRs.md                          # Architecture Decision Records index
├── adr/                             # Architecture Decision Records
│   ├── ADR-TEMPLATE.md
│   ├── ADR-0001-binance-service-singleton.md
│   ├── ADR-0002-hexagonal-architecture.md
│   ├── ADR-0003-istio-ambient-gateway-api.md
│   ├── ADR-0004-gitops-preview-envs.md
│   ├── ADR-0005-ansible-bootstrap-hardening.md
│   └── ADR-0006-english-only-codebase.md
├── specs/                           # Specifications (TDD/BDD)
│   ├── README.md
│   ├── features/                    # Feature specifications
│   └── api/                         # API specifications
│       ├── openapi.yaml             # REST API (OpenAPI 3.1)
│       └── asyncapi.yaml            # WebSocket/Events
├── execution-plans/                 # Transparent roadmaps
│   ├── README.md
│   ├── template.md
│   └── 2025-Q4/
│       └── ai-first-transformation.md
├── architecture/                    # Architecture documentation
│   ├── README.md
│   ├── system-overview.md           # C4 Model Level 1-2
│   ├── data-flow.md                 # Data flow diagrams
│   ├── tech-stack.md                # Technology stack
│   ├── security-model.md            # Security architecture
│   ├── deployment.md                # K8s deployment architecture
│   └── diagrams/                    # Mermaid diagrams
│       └── c4-context.mmd
├── runbooks/                        # Operational procedures
│   ├── README.md
│   ├── deployment.md
│   ├── troubleshooting.md
│   ├── monitoring.md
│   └── incident-response.md
├── development/                     # Development guides
│   ├── README.md
│   ├── setup.md                     # Local environment setup
│   ├── testing.md                   # Testing strategy
│   ├── code-style.md                # Style guide
│   └── contributing-workflow.md     # Git workflow and PR process
└── history/                         # Legacy/archived docs
    └── monolith/
```

---

## 👥 Documentation by Role

### For **Developers**

**Getting Started**:
- [Development Setup](development/setup.md) - Environment configuration
- [Developer Workflow](DEVELOPER.md) - Day-to-day development practices
- [Code Style Guide](development/code-style.md) - Coding conventions
- [Testing Guide](development/testing.md) - How to write and run tests

**Architecture**:
- [System Architecture](ARCHITECTURE.md) - Hexagonal architecture overview
- [System Overview](architecture/system-overview.md) - C4 model diagrams
- [Tech Stack](architecture/tech-stack.md) - Technologies and rationale
- [Data Flow](architecture/data-flow.md) - How data moves through the system

**Contributing**:
- [Contributing Workflow](development/contributing-workflow.md) - Git workflow and PRs
- [CONTRIBUTING.md](../CONTRIBUTING.md) - Contribution guidelines
- [Adapter Development](CONTRIBUTING-ADAPTERS.md) - Building adapters

### For **Operations / SRE**

**Deployment**:
- [Infrastructure README](../infra/README.md) - Infrastructure overview
- [Deployment Runbook](runbooks/deployment.md) - Deployment procedures
- [Deployment Architecture](architecture/deployment.md) - K8s topology

**Operations**:
- [Troubleshooting](runbooks/troubleshooting.md) - Common issues and fixes
- [Monitoring](runbooks/monitoring.md) - Observability and alerting
- [Incident Response](runbooks/incident-response.md) - Emergency procedures
- [K9s Operations](../infra/K9S-OPERATIONS.md) - Terminal UI for cluster debugging

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
- [../.github/copilot-instructions.md](../.github/copilot-instructions.md) - GitHub Copilot

### For **Product / Business**

**Overview**:
- [Project README](../README.md) - What is Robson Bot?
- [System Overview](architecture/system-overview.md) - High-level architecture
- [Execution Plans](execution-plans/) - Roadmaps and progress

**Features**:
- [Feature Specs](specs/features/) - Detailed feature descriptions
- [API Documentation](specs/api/openapi.yaml) - API capabilities

**Decisions**:
- [ADRs Index](ADRs.md) - Why we made certain technical choices

### For **Security**

**Security Model**:
- [Security Architecture](architecture/security-model.md) - Threat model and controls
- [Authentication Flow](AUTH_FLOW.md) - How auth works
- [ADR-0003: Istio mTLS](adr/ADR-0003-istio-ambient-gateway-api.md)

**Operations**:
- [Incident Response](runbooks/incident-response.md) - Security incidents
- [Deployment Security](architecture/deployment.md) - Network isolation

---

## 🔍 Documentation by Topic

### Architecture

| Topic | Document |
|-------|----------|
| **High-Level Overview** | [ARCHITECTURE.md](ARCHITECTURE.md) |
| **C4 Model Diagrams** | [architecture/system-overview.md](architecture/system-overview.md) |
| **Hexagonal Architecture** | [ADR-0002](adr/ADR-0002-hexagonal-architecture.md) |
| **Data Flow** | [architecture/data-flow.md](architecture/data-flow.md) |
| **Technology Stack** | [architecture/tech-stack.md](architecture/tech-stack.md) |
| **Security Model** | [architecture/security-model.md](architecture/security-model.md) |
| **Deployment** | [architecture/deployment.md](architecture/deployment.md) |

### Development

| Topic | Document |
|-------|----------|
| **Environment Setup** | [development/setup.md](development/setup.md) |
| **Developer Workflow** | [DEVELOPER.md](DEVELOPER.md) |
| **Code Style** | [development/code-style.md](development/code-style.md) |
| **Testing** | [development/testing.md](development/testing.md) |
| **Contributing** | [development/contributing-workflow.md](development/contributing-workflow.md) |
| **Adapter Development** | [CONTRIBUTING-ADAPTERS.md](CONTRIBUTING-ADAPTERS.md) |

### Operations

| Topic | Document |
|-------|----------|
| **Deployment Procedures** | [runbooks/deployment.md](runbooks/deployment.md) |
| **Troubleshooting** | [runbooks/troubleshooting.md](runbooks/troubleshooting.md) |
| **Monitoring** | [runbooks/monitoring.md](runbooks/monitoring.md) |
| **Incident Response** | [runbooks/incident-response.md](runbooks/incident-response.md) |
| **K9s Operations** | [../infra/K9S-OPERATIONS.md](../infra/K9S-OPERATIONS.md) |
| **Infrastructure** | [../infra/README.md](../infra/README.md) |

### API & Specs

| Topic | Document |
|-------|----------|
| **REST API** | [specs/api/openapi.yaml](specs/api/openapi.yaml) |
| **WebSocket API** | [specs/api/asyncapi.yaml](specs/api/asyncapi.yaml) |
| **Feature Specs** | [specs/features/](specs/features/) |
| **Spec-Driven Development** | [specs/README.md](specs/README.md) |

### Planning & Decisions

| Topic | Document |
|-------|----------|
| **ADRs Index** | [ADRs.md](ADRs.md) |
| **All ADRs** | [adr/](adr/) |
| **Execution Plans** | [execution-plans/](execution-plans/) |
| **Initial Audit** | [INITIAL-AUDIT.md](INITIAL-AUDIT.md) |

---

## 🤖 AI-First Documentation

Robson Bot is optimized for AI-assisted development:

### Core AI Documents

1. **[AGENTS.md](AGENTS.md)** - Master guide for all AI agents
   - Project vision and architecture
   - Directory structure and patterns
   - Domain glossary
   - Common tasks and troubleshooting
   - Testing philosophy
   - Security model

2. **[../CLAUDE.md](../CLAUDE.md)** - Claude Code specific
   - Repository context
   - Code patterns
   - Testing approach
   - Common workflows

3. **[AI_WORKFLOW.md](AI_WORKFLOW.md)** - Collaboration rules
   - English-only requirement
   - Conventional Commits
   - Semantic commit messages

4. **[LANGUAGE-POLICY.md](LANGUAGE-POLICY.md)** - Why English only
   - International positioning
   - Team collaboration
   - AI compatibility

### AI Tool Configuration

- **Claude Code**: [../CLAUDE.md](../CLAUDE.md)
- **Cursor AI**: [../.cursorrules](../.cursorrules)
- **GitHub Copilot**: [../.github/copilot-instructions.md](../.github/copilot-instructions.md)

---

## 📖 Learning Paths

### Path 1: New Backend Developer

1. [Development Setup](development/setup.md#backend-setup)
2. [Hexagonal Architecture](ARCHITECTURE.md)
3. [ADR-0002: Hexagonal Architecture](adr/ADR-0002-hexagonal-architecture.md)
4. [Backend Testing](development/testing.md#backend-testing)
5. [Adapter Development](CONTRIBUTING-ADAPTERS.md)
6. [Code Style](development/code-style.md#python-conventions)

### Path 2: New Frontend Developer

1. [Development Setup](development/setup.md#frontend-setup)
2. [Frontend Architecture](../apps/frontend/README.md)
3. [Frontend Testing](development/testing.md#frontend-testing)
4. [Code Style](development/code-style.md#javascript-conventions)
5. [API Specs](specs/api/openapi.yaml)

### Path 3: New DevOps Engineer

1. [Infrastructure Overview](../infra/README.md)
2. [Deployment Architecture](architecture/deployment.md)
3. [ADR-0003: Istio Ambient](adr/ADR-0003-istio-ambient-gateway-api.md)
4. [ADR-0004: GitOps Previews](adr/ADR-0004-gitops-preview-envs.md)
5. [Deployment Runbook](runbooks/deployment.md)
6. [Troubleshooting](runbooks/troubleshooting.md)

### Path 4: Understanding Architecture

1. [System Overview](architecture/system-overview.md)
2. [Hexagonal Architecture](ARCHITECTURE.md)
3. [Data Flow](architecture/data-flow.md)
4. [Tech Stack](architecture/tech-stack.md)
5. [All ADRs](ADRs.md)

---

## 🔗 External Resources

### Hexagonal Architecture

- [Netflix - Ready for changes with Hexagonal Architecture](https://netflixtechblog.com/ready-for-changes-with-hexagonal-architecture-b315ec967749)
- [Alistair Cockburn - Original article](https://alistair.cockburn.us/hexagonal-architecture/)

### Django Best Practices

- [Two Scoops of Django](https://www.feldroy.com/books/two-scoops-of-django-3-x)
- [Django REST Framework](https://www.django-rest-framework.org/)

### Kubernetes

- [Kubernetes Patterns](https://k8spatterns.io/)
- [Gateway API](https://gateway-api.sigs.k8s.io/)
- [Istio Ambient Mode](https://istio.io/latest/docs/ambient/)

### GitOps

- [ArgoCD Documentation](https://argo-cd.readthedocs.io/)
- [GitOps Principles](https://opengitops.dev/)

---

## 📝 Documentation Standards

### Markdown

- Use GitHub Flavored Markdown
- Lint with markdownlint
- Use relative links for internal references
- Include table of contents for long documents

### Diagrams

- Prefer Mermaid format (version controlled)
- Store in `architecture/diagrams/`
- Include both `.mmd` source and rendered view
- Use C4 model for architecture diagrams

### Code Samples

- Always include language identifier for syntax highlighting
- Keep examples concise and focused
- Test code samples before documenting
- Include expected output where relevant

### Cross-Linking

- Link freely between related documents
- Use relative paths, not absolute
- Verify links don't break when moving files
- Update INDEX.md when adding new docs

---

## 🔄 Keeping Documentation Updated

Documentation is only valuable if it's current. We follow these practices:

1. **Update with Code**: Documentation changes in same PR as code changes
2. **ADRs for Decisions**: Significant decisions get ADR documentation
3. **Quarterly Review**: Review all docs quarterly, update stale content
4. **Link Validation**: CI checks for broken links
5. **Feedback Welcome**: Open issues for documentation improvements

---

## 🆘 Getting Help

- **Can't find what you need?** Open an issue with the `documentation` label
- **Found an error?** Submit a PR to fix it
- **Need clarification?** Ask in GitHub Discussions

---

## 📊 Documentation Metrics

| Metric | Target | Current |
|--------|--------|---------|
| **Documentation Coverage** | 90% | TBD |
| **Broken Links** | 0 | TBD |
| **Stale Docs (>6 months)** | <10% | TBD |
| **ADRs** | All major decisions | 6 |

---

**Last Updated**: 2025-11-14
**Maintained by**: Robson Bot Core Team
**License**: Same as project

---

**Ready to dive in? Pick a learning path above or explore the structure!** 🚀
