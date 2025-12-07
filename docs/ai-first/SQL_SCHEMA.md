# ParadeDB RAG Schema Specification

**Last Updated**: 2025-12-07
**Related ADR**: ADR-0007, ADR-0009
**Status**: Proposed

---

## Overview

This document defines the SQL schema for Robson Bot's RAG (Retrieval-Augmented Generation) knowledge store in ParadeDB. The schema supports:
- **Hybrid search**: BM25 (keyword) + vector (semantic) search.
- **Source attribution**: Track which PR/issue/doc each chunk came from.
- **Metadata filtering**: Filter by repo, author, date, labels.
- **Idempotent updates**: Upsert by `source_type + source_id`.

---

## Schema Design Principles

1. **Separation of Concerns**: RAG tables prefixed with `rag_*` to avoid collision with Django ORM tables.
2. **Denormalization**: Store metadata as JSONB (flexible, queryable).
3. **Indexing Strategy**: BM25 index on `chunk_text`, vector index on `chunk_embedding`, GIN index on `metadata`.
4. **Soft Deletes**: Use `deleted` flag instead of hard deletes (preserve history).
5. **Timestamping**: Track `indexed_at` for monitoring ingestion lag.

---

## Table: `rag_knowledge_entries`

### **Purpose**
Store chunked knowledge from GitHub (PRs, issues, docs, code) with embeddings for hybrid search.

### **Schema**

```sql
CREATE TABLE rag_knowledge_entries (
    -- Primary Key
    id                  BIGSERIAL PRIMARY KEY,

    -- Source Identification (composite unique key)
    source_type         VARCHAR(50) NOT NULL,  -- 'github.pr', 'github.issue', 'github.doc', 'github.code'
    source_id           VARCHAR(255) NOT NULL, -- 'ldamasio/robson#128', 'docs/adr/ADR-0007.md'
    repo                VARCHAR(255) NOT NULL, -- 'ldamasio/robson'

    -- Content
    chunk_text          TEXT NOT NULL,         -- Cleaned, chunked text (512 tokens ~2000 chars)
    chunk_index         INTEGER NOT NULL,      -- Chunk position in original document (0-indexed)
    chunk_hash          VARCHAR(64),           -- SHA256 of chunk_text (for deduplication)

    -- Embeddings
    chunk_embedding     VECTOR(768),           -- Dense embedding (768-dim for DeepSeek)

    -- Metadata (JSONB for flexibility)
    metadata            JSONB NOT NULL DEFAULT '{}',

    -- Lifecycle
    indexed_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted             BOOLEAN NOT NULL DEFAULT FALSE,

    -- Constraints
    CONSTRAINT unique_source_chunk UNIQUE (source_type, source_id, chunk_index)
);
```

### **Indexes**

#### **1. BM25 Full-Text Index** (pg_search)
```sql
CREATE INDEX idx_rag_knowledge_bm25
ON rag_knowledge_entries
USING bm25 (chunk_text)
WITH (key_field='id');
```

#### **2. Vector Similarity Index** (pgvector)
```sql
CREATE INDEX idx_rag_knowledge_embedding
ON rag_knowledge_entries
USING ivfflat (chunk_embedding vector_cosine_ops)
WITH (lists = 100);
```

#### **3. Metadata GIN Index** (for filtering)
```sql
CREATE INDEX idx_rag_knowledge_metadata
ON rag_knowledge_entries
USING gin (metadata jsonb_path_ops);
```

#### **4. Composite Index** (for common queries)
```sql
CREATE INDEX idx_rag_knowledge_repo_active
ON rag_knowledge_entries (repo, deleted, indexed_at DESC);
```

#### **5. Source Lookup Index**
```sql
CREATE INDEX idx_rag_knowledge_source
ON rag_knowledge_entries (source_type, source_id);
```

---

## Storage Estimates

### **Assumptions**
- Average chunk: 512 tokens ≈ 2KB text.
- Embedding: 768 floats × 4 bytes = 3KB.
- Metadata: ~500 bytes (JSON).
- Total per chunk: ~5.5KB.

### **Projections**
| Data Volume | Chunks | Storage | Notes |
|-------------|--------|---------|-------|
| **100 PRs** | ~1,000 | 5.5 MB | Initial backfill |
| **1,000 PRs** | ~10,000 | 55 MB | 1 year of history |
| **10,000 PRs** | ~100,000 | 550 MB | 5+ years of history |

**Index overhead**: ~50% additional (BM25, vector indexes).

**Total for 1,000 PRs**: ~80 MB (well within VPS capacity).

---

## References

- **ParadeDB pg_search**: https://docs.paradedb.com/search/overview
- **pgvector**: https://github.com/pgvector/pgvector
- **BM25 Algorithm**: https://en.wikipedia.org/wiki/Okapi_BM25

---

**Maintainers**: Robson Bot Core Team
**License**: Same as project
