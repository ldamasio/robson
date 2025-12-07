# AI-First Transformation History

**Status**: In Progress
**Started**: 2025-12-07
**Last Updated**: 2025-12-07

---

## Overview

This document tracks the AI-First transformation of Robson Bot, which adds self-hosted RAG (Retrieval-Augmented Generation) capabilities for project knowledge queries.

**Goal**: Enable developers to ask natural language questions about Robson Bot's architecture, decisions, and codebase, with answers grounded in actual project data (PRs, issues, docs).

---

## Key Decisions

### **Database: ParadeDB**
- **Rationale**: Unified OLTP + search database eliminates need for separate search infrastructure (Elasticsearch, Qdrant).
- **ADR**: ADR-0007
- **Benefits**: Hybrid search (BM25 + vectors), PostgreSQL wire compatibility, ACID guarantees.

### **LLM: DeepSeek**
- **Rationale**: Self-hosted LLM aligns with open-source philosophy, reduces cost, ensures privacy.
- **ADR**: ADR-0008
- **Model**: DeepSeek-R1-Distill-Qwen-1.5B (CPU-friendly, upgradable to larger models).

### **RAG Architecture**
- **Rationale**: Pure LLM approaches hallucinate; RAG grounds answers in factual project data.
- **ADR**: ADR-0009
- **Components**: Indexer (ingest GitHub data) + Retriever (hybrid search + LLM generation).

### **Ingestion Strategy**
- **Rationale**: Event-driven (webhooks) + batch (scheduled) ensures fresh data with reliability.
- **ADR**: ADR-0010
- **Triggers**: GitHub webhooks (real-time) + daily CronJob (backfill).

---

## Implementation Timeline

| Phase | Duration | Status | Notes |
|-------|----------|--------|-------|
| **Planning & Documentation** | Week 1 | ✅ Completed | ADRs, SQL schema, event formats, execution plans |
| **Infrastructure** | Week 2 | 🔄 Pending | Deploy ParadeDB, DeepSeek Gateway |
| **Indexing** | Week 3-4 | 🔄 Pending | Implement indexer, backfill data |
| **Retrieval** | Week 5-6 | 🔄 Pending | Implement retriever, integrate with Django |
| **Observability** | Week 7 | 🔄 Pending | Metrics, dashboards, alerts |
| **Production Launch** | Week 8 | 🔄 Pending | Beta rollout, feedback collection |

---

## Execution Plans

All implementation work is organized into 6 sequential execution plans:

1. **Plan 01: ParadeDB Infrastructure** - Deploy ParadeDB to k3s and docker-compose
2. **Plan 02: Django ParadeDB Migration** - Migrate Django to use ParadeDB
3. **Plan 03: DeepSeek Gateway Setup** - Deploy self-hosted LLM service
4. **Plan 04: RAG Indexer Implementation** - Ingest GitHub data into ParadeDB
5. **Plan 05: RAG Retriever Integration** - Implement hybrid search + LLM generation
6. **Plan 06: Observability & Metrics** - Add monitoring and alerting

Each plan is designed to be executed independently via Claude Code CLI.

---

## Documentation Created

### **Architecture Decision Records**
- ADR-0007: Adopt ParadeDB as Primary Database
- ADR-0008: Adopt DeepSeek as Primary Local LLM
- ADR-0009: Robson RAG Architecture using ParadeDB
- ADR-0010: GitHub Data Ingestion Strategy

### **Technical Specifications**
- AI-First Architecture Overview
- ParadeDB SQL Schema
- Ingestion Event Format
- DeepSeek Gateway Protocol

### **Execution Plans**
- Execution Plans Index
- 6 detailed .prompt files for step-by-step implementation

---

## Success Metrics

| Metric | Target | How to Measure |
|--------|--------|----------------|
| **Query Latency (p95)** | <2s | Prometheus rag_query_latency_seconds |
| **Indexing Freshness** | <1 hour lag | Prometheus rag_ingestion_lag_seconds |
| **Answer Quality** | >80% thumbs-up | User feedback in UI |
| **Developer Adoption** | 50+ queries/week | Django logs, analytics |
| **Cost Reduction** | $0 external API spend | Zero OpenAI/Anthropic bills |

---

## Future Enhancements

1. **Re-ranking**: Add cross-encoder model for top-K re-ranking (improve precision).
2. **Multi-modal**: Index diagrams (CLIP embeddings) and images (OCR).
3. **Fine-tuning**: Fine-tune DeepSeek on Robson-specific data (trading domain).
4. **Multi-repo**: Index dependencies (e.g., python-binance docs).
5. **Agentic RAG**: Multi-step reasoning (e.g., "compare two strategies").
6. **GPU Acceleration**: Add GPU node to k3s for faster inference.

---

**Maintainers**: Robson Bot Core Team
**Related**: docs/plan/, docs/adr/ADR-0007.md to ADR-0010.md
