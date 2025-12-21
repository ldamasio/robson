Robson Bot – Architecture Decision Records (ADRs)

Purpose
- Capture significant architectural decisions and their context, especially when introducing or changing a design pattern, a core dependency, or a cross‑cutting concern.

Project Policy
- When introducing a design pattern (e.g., Singleton, Factory, Strategy), add or update an ADR entry in this document.
- For substantial or long‑form ADRs, create per‑ADR files under `docs/adr/` (e.g., `docs/adr/ADR-000X-<slug>.md`) and list them here.
- Keep ADRs concise, actionable, and dated. Reference code paths and links to PRs when relevant.

ADR Template (inline)
- Title: <short, imperative>
- Status: Proposed | Accepted | Deprecated | Superseded
- Date: YYYY-MM-DD
- Context: What problem or forces led to this decision?
- Decision: What is decided?
- Consequences: Positive, negative, trade‑offs
- Alternatives: Considered options and why not chosen
- Notes: References to code, tests, docs, or PRs

---

Index

| ID | Title | Status | Date |
|----|-------|--------|------|
| ADR-0001 | BinanceService Singleton | Accepted | 2024-11 |
| ADR-0002 | Hexagonal Architecture | Accepted | 2024-11 |
| ADR-0003 | Istio Ambient + Gateway API | Accepted | 2024-11 |
| ADR-0004 | GitOps Preview Environments | Accepted | 2024-11 |
| ADR-0005 | Ansible Bootstrap Hardening | Accepted | 2024-11 |
| ADR-0006 | English-Only Codebase | Accepted | 2024-11 |
| ADR-0007 | ParadeDB Primary Database | Accepted | 2024-12 |
| ADR-0008 | DeepSeek Primary LLM | Accepted | 2024-12 |
| ADR-0009 | RAG Architecture ParadeDB | Accepted | 2024-12 |
| ADR-0010 | GitHub Data Ingestion | Accepted | 2024-12 |

Files located in `docs/adr/`.
