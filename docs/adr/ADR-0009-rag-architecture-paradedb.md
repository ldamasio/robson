ADR-0009: Robson RAG Architecture using ParadeDB

Status: Proposed
Date: 2025-12-07

Context
- Robson Bot's AI-First transformation requires a **Retrieval-Augmented Generation (RAG)** system to answer developer queries using project knowledge.
- Knowledge sources include:
  - GitHub PRs (code changes, discussions, decisions).
  - GitHub Issues (bug reports, feature requests, user feedback).
  - Documentation (ADRs, specs, runbooks, CLAUDE.md).
  - Code chunks (functions, classes, critical logic).
- **Pure LLM approaches fail** because:
  - LLM context windows are limited (cannot fit entire repo history).
  - LLMs hallucinate without grounding in factual data.
  - LLM knowledge is static (outdated for recent PRs/issues).
- **RAG solves this** by:
  - Indexing knowledge into a searchable database.
  - Retrieving relevant chunks for a given query.
  - Augmenting LLM prompts with retrieved context.
  - Generating grounded, factual answers.
- **ParadeDB** (ADR-0007) provides hybrid search (BM25 + vectors) in PostgreSQL, eliminating need for separate search infrastructure.
- **DeepSeek** (ADR-0008) provides local LLM for answer generation.

Needs Input
- Chunk size strategy (target: 512 tokens per chunk, with 128-token overlap).
- Embedding model selection (DeepSeek embeddings vs sentence-transformers).
- Retrieval strategy: Top-K (K=5? K=10?), re-ranking enabled or disabled.
- Index refresh frequency: Real-time (webhook-triggered) vs batch (hourly/daily).

*(If inputs are not provided, default to: 512-token chunks, DeepSeek embeddings, Top-5 retrieval, daily batch indexing.)*

Decision
- **Implement RAG architecture for Robson Bot** with the following components:

1. **Knowledge Store** (ParadeDB):
   - **Table**: `rag_knowledge_entries` (schema in `docs/ai-first/SQL_SCHEMA.md`).
   - **Columns**: `id`, `source_type`, `source_id`, `repo`, `chunk_text`, `chunk_embedding`, `metadata`, `indexed_at`.
   - **Indexes**: BM25 full-text index (`pg_search`), vector similarity index (`pgvector`), metadata GIN index.

2. **Ingestion Pipeline**:
   - **Trigger**: GitHub webhooks (PR merged, issue closed) + scheduled batch (daily).
   - **Processor**: `rag-indexer` service (Python) ingests events, chunks content, generates embeddings, writes to ParadeDB.
   - **Event Format**: JSON schema defined in `docs/ai-first/INGESTION_EVENTS.md`.

3. **Retriever Service**:
   - **Query flow**: User query → BM25 search + vector search → hybrid ranking → top-K chunks.
   - **Re-ranking**: Optional (future): Use cross-encoder model to re-rank top-K results.
   - **Output**: List of `RetrievedChunk` objects (text, metadata, score).

4. **LLM Integration**:
   - **Prompt template**: `You are Robson Bot assistant. Use the following context to answer: [chunks]. Question: [query]`.
   - **Context injection**: Retrieved chunks inserted into prompt.
   - **Generation**: DeepSeek gateway generates answer with streaming.

5. **API Endpoint**:
   - **Endpoint**: `POST /api/v1/knowledge/query` (Django REST).
   - **Request**: `{"query": "How do I add a new strategy?"}`
   - **Response**: `{"answer": "...", "sources": [...]}`

6. **Data Lifecycle**:
   - **Ingestion**: GitHub → ingestion event → rag-indexer → ParadeDB.
   - **Retrieval**: User query → retriever → ParadeDB → ranked chunks.
   - **Generation**: Chunks + query → DeepSeek gateway → answer.

Consequences
- Positive
  - **Grounded answers**: LLM responses backed by actual project data (no hallucinations).
  - **Up-to-date knowledge**: Index refreshes capture recent PRs/issues.
  - **Source attribution**: Users see which PRs/docs informed the answer.
  - **Hybrid search quality**: BM25 (keyword) + vectors (semantic) improves recall.
  - **Unified DB**: ParadeDB eliminates need for separate search infrastructure.
  - **Developer productivity**: Faster onboarding, self-service answers to common questions.
- Negative/Trade-offs
  - **Indexing latency**: New PRs not immediately queryable (batch delay).
  - **Storage overhead**: Embeddings consume ~3KB per chunk (768-dim floats).
  - **Chunking complexity**: Poor chunking strategy degrades retrieval quality.
  - **LLM dependency**: Answers only as good as retrieved chunks + LLM reasoning.
  - **Maintenance burden**: Index drift, stale data, schema evolution.

Alternatives
- **No RAG, LLM-only**
  - Pros: Simpler architecture.
  - Cons: Hallucinations, outdated knowledge, no source attribution.
  - Why not chosen: Unacceptable for production knowledge base.

- **Vector search only (no BM25)**
  - Pros: Pure semantic search, no keyword tuning.
  - Cons: Poor recall for exact keyword queries (e.g., function names, error codes).
  - Why not chosen: Hybrid search outperforms vector-only in benchmarks.

- **BM25 only (no vectors)**
  - Pros: Fast, no embedding overhead.
  - Cons: Misses semantic relationships (synonyms, paraphrases).
  - Why not chosen: Semantic search critical for natural language queries.

- **External RAG service (Pinecone, Weaviate Cloud)**
  - Pros: Managed infrastructure, zero ops.
  - Cons: Cost, privacy, vendor lock-in.
  - Why not chosen: Violates self-hosting principle.

Implementation Notes
- **Code paths**:
  - Indexer: `apps/backend/rag-indexer/` (new Python service).
  - Retriever: `apps/backend/core/application/rag_retriever.py` (new use case).
  - API: `apps/backend/monolith/api/views/knowledge_views.py` (new endpoint).
  - Schema: `apps/backend/monolith/api/migrations/XXXX_rag_schema.py` (new migration).

- **Infrastructure**:
  - Indexer: `infra/k8s/apps/rag-indexer/deployment.yaml` (CronJob or Deployment with webhook listener).
  - Retriever: Part of Django monolith (no separate service).
  - ParadeDB: ADR-0007 infrastructure.

- **Dependencies**:
  - Python: `transformers`, `sentence-transformers`, `psycopg2`, `numpy`.
  - ParadeDB extensions: `pg_search`, `pgvector`.

- **Testing**:
  - Unit tests: Mock ParadeDB, verify chunking logic, retrieval ranking.
  - Integration tests: End-to-end query flow with real ParadeDB.
  - Quality tests: Evaluate retrieval quality (precision@K, recall@K) on labeled queries.

- **Related**:
  - ADR-0007: ParadeDB (foundation for knowledge store).
  - ADR-0008: DeepSeek (LLM for answer generation).
  - ADR-0010: Ingestion Strategy (event-driven indexing).
  - Execution Plan: `docs/plan/04-rag-indexer-implementation.prompt`.
  - Execution Plan: `docs/plan/05-rag-retriever-integration.prompt`.
  - Schema: `docs/ai-first/SQL_SCHEMA.md`.
  - Events: `docs/ai-first/INGESTION_EVENTS.md`.

- **Metrics**:
  - **Indexing**: Chunks indexed/day, ingestion lag, failures.
  - **Retrieval**: Query latency (p50, p95), cache hit rate.
  - **Quality**: User feedback (thumbs up/down), answer accuracy (manual eval).

- **Future Enhancements**:
  - **Re-ranking**: Add cross-encoder model for top-K re-ranking.
  - **Multi-modal**: Index images (diagrams, screenshots) with CLIP embeddings.
  - **Feedback loop**: Use user ratings to fine-tune retrieval and LLM prompts.
  - **Contextual chunking**: Use AST-based chunking for code (vs naive sliding window).

References
- RAG Paper: https://arxiv.org/abs/2005.11401
- ParadeDB Hybrid Search: https://docs.paradedb.com/search/hybrid
- LlamaIndex: https://www.llamaindex.ai (inspiration for chunking strategies)
