ADR-0007: Adopt ParadeDB as Primary Database

Status: Proposed
Date: 2025-12-07

Context
- Robson Bot currently uses standard PostgreSQL for Django ORM persistence and would require a separate search/vector database for RAG (Retrieval-Augmented Generation) capabilities.
- AI-First transformation requires hybrid search (BM25 + dense vectors) over GitHub data (PRs, issues, docs, ADRs, code chunks).
- Running separate databases (Postgres for OLTP, Elasticsearch/Qdrant/Weaviate for search) increases operational complexity, costs, and data synchronization overhead.
- ParadeDB extends PostgreSQL with native BM25 full-text search and vector similarity search, providing a unified database for both transactional and AI workloads.
- ParadeDB is wire-compatible with PostgreSQL, allowing Django ORM to work without changes while adding search capabilities.

Needs Input
- Performance benchmarks for ParadeDB under combined OLTP + search workloads on Contabo VPS (limited resources).
- Storage requirements for vector embeddings (768-dimensional for DeepSeek models).
- Query latency targets for RAG retrieval (< 200ms p95 acceptable).

*(If performance is unacceptable or storage exceeds capacity, consider splitting OLTP and search workloads.)*

Decision
- **Adopt ParadeDB as the single primary database for Robson Bot**, replacing standard PostgreSQL.
- Use ParadeDB for:
  - **Django ORM**: All existing transactional workloads (users, orders, strategies, positions).
  - **RAG knowledge store**: GitHub PR/issue/doc data with BM25 + vector search.
- **Dev/Prod parity**: Use ParadeDB Docker image locally (`paradedb/paradedb:latest`) matching production version.
- **Schema design**: Separate Django tables from RAG tables (prefix: `rag_*`) to maintain clear boundaries.
- **Connection management**: Django uses standard `django.db.backends.postgresql_psycopg2` engine (ParadeDB is PostgreSQL-compatible).

Decision
- Deploy ParadeDB as a StatefulSet in k3s with persistent volumes (production).
- Use ParadeDB Docker container for local development (docker-compose).
- Enable ParadeDB extensions: `pg_search` (BM25), `pgvector` (dense vectors), `pgembedding` (optional).
- Configure Django settings to point to ParadeDB (no code changes required for ORM).
- Create dedicated schema/tables for RAG knowledge with proper indexing strategy.

Consequences
- Positive
  - **Unified infrastructure**: Single database simplifies operations, backups, monitoring.
  - **Cost reduction**: No need for separate search infrastructure (Elasticsearch cluster, etc.).
  - **ACID guarantees**: Transactional consistency between business data and RAG knowledge.
  - **PostgreSQL compatibility**: Leverage existing Django ORM, backup tools (pg_dump), monitoring.
  - **Hybrid search**: Native BM25 + vector similarity in SQL queries.
  - **Simpler dev environment**: One docker-compose service instead of multiple databases.
- Negative/Trade-offs
  - **Resource contention**: OLTP writes compete with search queries on same node (mitigated with connection pooling, query timeouts).
  - **Scaling limitations**: Cannot independently scale OLTP vs search workloads (acceptable for current VPS constraints).
  - **ParadeDB maturity**: Newer project compared to PostgreSQL (mitigated by PostgreSQL foundation).
  - **Vendor lock-in**: ParadeDB-specific search features (BM25, hybrid search) not portable to standard Postgres without migration.

Alternatives
- **Standard PostgreSQL + Separate Search DB (Elasticsearch/Qdrant)**
  - Pros: Independent scaling, mature search ecosystems.
  - Cons: Operational complexity, data sync overhead, higher cost, dual schema management.
  - Why not chosen: Violates simplicity principle for MVP; premature optimization.

- **PostgreSQL + pgvector only (no BM25)**
  - Pros: Standard Postgres extension, widely adopted.
  - Cons: No native BM25 full-text search (only vectors), lower recall for keyword queries.
  - Why not chosen: Hybrid search (BM25 + vectors) is essential for RAG quality.

- **SQLite with FTS5 + vector extension**
  - Pros: Zero-ops, embedded.
  - Cons: Single-writer, not suitable for production Django app with concurrent users.
  - Why not chosen: Not viable for multi-tenant trading platform.

Implementation Notes
- **Code paths**:
  - Django settings: `apps/backend/monolith/backend/settings.py` (update `DATABASES` to point to ParadeDB).
  - Environment variables: `RBS_PG_HOST`, `RBS_PG_PORT`, `RBS_PG_DATABASE`, `RBS_PG_USER`, `RBS_PG_PASSWORD`.
  - No Django ORM code changes required (PostgreSQL wire compatibility).

- **Infrastructure**:
  - Production: `infra/k8s/apps/paradedb/statefulset.yaml` (new).
  - Production: `infra/k8s/apps/paradedb/service.yaml` (new).
  - Production: `infra/k8s/apps/paradedb/pvc.yaml` (new).
  - Dev: `docker-compose.yml` (replace `postgres` image with `paradedb/paradedb:latest`).
  - ArgoCD: New Application manifest for ParadeDB.

- **Schema**:
  - RAG tables: `docs/ai-first/SQL_SCHEMA.md` (defines `rag_knowledge_entries`, indexes).
  - Migration: Django migration to create RAG schema (separate from Django ORM tables).

- **Testing**:
  - Verify Django ORM compatibility: Run existing `./bin/dj test` suite against ParadeDB.
  - Verify search extensions: SQL tests for BM25 and vector queries.
  - Performance tests: Measure p95 latency for combined OLTP + search workload.

- **Related**:
  - ADR-0009: RAG Architecture (depends on this ADR).
  - ADR-0010: Ingestion Strategy (depends on this ADR).
  - Execution Plan: `docs/plan/01-paradedb-infrastructure.prompt`.
  - Execution Plan: `docs/plan/02-django-paradedb-migration.prompt`.

- **Rollback Plan**:
  - If ParadeDB proves unstable or performance is unacceptable, revert to standard PostgreSQL for OLTP.
  - Migrate RAG workload to dedicated Qdrant/Weaviate instance.
  - Django code requires no changes (wire-compatible).

References
- ParadeDB: https://paradedb.com
- ParadeDB GitHub: https://github.com/paradedb/paradedb
- pg_search extension: https://docs.paradedb.com/search/overview
- pgvector extension: https://github.com/pgvector/pgvector
