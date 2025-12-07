ADR-0010: GitHub Data Ingestion Strategy

Status: Proposed
Date: 2025-12-07

Context
- Robson Bot's RAG system (ADR-0009) requires indexed knowledge from GitHub:
  - **PRs**: Code changes, review comments, merge decisions.
  - **Issues**: Bug reports, feature requests, discussions.
  - **Docs**: Markdown files (ADRs, specs, README, CLAUDE.md).
  - **Code**: Functions, classes, critical logic (selective indexing).
- **GitHub remains the Source of Truth**; ParadeDB is a **derived index** for search.
- **Ingestion challenges**:
  - **Volume**: Large repos (1000+ PRs) require batch backfill + incremental updates.
  - **Rate limits**: GitHub API has strict rate limits (5000 req/hour authenticated).
  - **Data freshness**: New PRs should be queryable within reasonable time (target: <1 hour).
  - **Idempotency**: Re-ingesting the same PR should not create duplicates.
  - **Error handling**: Network failures, malformed data, API errors must not block ingestion.
- **Triggers**:
  - **Webhooks**: Real-time notifications when PRs are merged, issues closed.
  - **Scheduled batch**: Daily/weekly sweep to catch missed events (webhook reliability not guaranteed).
- **What to store**:
  - **Normalized text**: PR description, issue body, doc content (cleaned Markdown).
  - **Chunks**: Split text into ~512-token chunks with overlap.
  - **Embeddings**: Dense vectors (768-dim) for semantic search.
  - **Metadata**: Repo, PR#, author, timestamp, labels, status.
- **What NOT to store**:
  - Full repository clones (GitHub is SoT).
  - Binary files (images, videos).
  - Secrets or credentials (filter via `.env`, `credentials.json` patterns).

Needs Input
- Webhook endpoint authentication strategy (shared secret? GitHub App?).
- Backpressure handling: If ingestion is slow, should webhooks be queued or dropped?
- Backfill strategy: Index all historical PRs or only recent (e.g., last 6 months)?
- Code indexing scope: All Python files? Only `core/domain/`? Only public APIs?

*(If inputs are not provided, default to: GitHub App auth, queue webhooks (Redis), backfill last 12 months, index Python files under `apps/backend/core/` and `docs/`.)*

Decision
- **Implement event-driven ingestion pipeline** with the following components:

1. **Ingestion Sources**:
   - **GitHub Webhooks**: Listen for `pull_request.closed`, `issues.closed`, `push` (to `docs/` or `main` branch).
   - **GitHub API Polling**: Scheduled batch (daily) to catch missed events, backfill historical data.

2. **Event Processor** (`rag-indexer` service):
   - **Input**: JSON events (schema: `docs/ai-first/INGESTION_EVENTS.md`).
   - **Processing**:
     - Parse event (extract PR#, issue#, file paths).
     - Fetch full content from GitHub API (if not in webhook payload).
     - Clean and normalize text (remove HTML tags, code fences, excessive whitespace).
     - Chunk text (512 tokens, 128-token overlap).
     - Generate embeddings via DeepSeek gateway (or sentence-transformers).
     - Write chunks to ParadeDB (`rag_knowledge_entries`).
   - **Idempotency**: Use `source_type + source_id` as deduplication key (upsert).

3. **Event Format** (JSON):
   ```json
   {
     "event_type": "github.pr",
     "repo": "ldamasio/robson",
     "pr_number": 128,
     "title": "feat: add RAG architecture",
     "author": "ldamasio",
     "merged_at": "2025-12-07T10:30:00Z",
     "content_blocks": [
       {
         "kind": "description",
         "text": "This PR implements the RAG architecture..."
       },
       {
         "kind": "diff_summary",
         "text": "Added 3 new files: rag_indexer.py, ..."
       }
     ],
     "metadata": {
       "labels": ["enhancement", "ai"],
       "reviewers": ["user1", "user2"]
     }
   }
   ```

4. **Webhook Endpoint**:
   - **Path**: `POST /api/v1/webhooks/github` (Django view).
   - **Auth**: GitHub webhook secret (HMAC signature verification).
   - **Processing**: Validate payload, queue event (Redis/Celery), return 200 OK immediately (async processing).

5. **Scheduled Batch**:
   - **Trigger**: Kubernetes CronJob (daily at 2 AM UTC).
   - **Process**: Query GitHub API for merged PRs in last 24 hours, emit events, process.

6. **Error Handling**:
   - **Retry logic**: Exponential backoff for API failures (3 retries max).
   - **Dead letter queue**: Failed events logged and flagged for manual review.
   - **Alerts**: Slack/email notification if ingestion lag exceeds 2 hours.

7. **Data Lifecycle**:
   - **Ingestion**: GitHub event → webhook/batch → rag-indexer → ParadeDB.
   - **Update**: Re-ingesting same PR (e.g., edited description) updates existing chunks (upsert by `source_id`).
   - **Deletion**: Closed/deleted PRs marked as `deleted=true` in metadata (soft delete, not removed from index).

Consequences
- Positive
  - **Real-time updates**: Webhooks ensure new PRs are indexed within minutes.
  - **Reliability**: Scheduled batch catches missed webhooks.
  - **Idempotency**: Re-processing same event is safe (no duplicates).
  - **Error resilience**: Retries and DLQ prevent data loss.
  - **GitHub as SoT**: No risk of divergence; index is rebuildable from GitHub.
  - **Scalability**: Queue-based processing handles burst traffic (webhook spikes).
- Negative/Trade-offs
  - **Webhook dependency**: If GitHub webhooks fail silently, batch job catches but with delay.
  - **Rate limits**: Aggressive backfill can exhaust GitHub API quota (mitigated with throttling).
  - **Indexing lag**: Batch delay means some queries see stale data (acceptable for non-critical use case).
  - **Operational complexity**: Webhook endpoint, queue, cron jobs require monitoring.
  - **Storage growth**: Historical PRs accumulate; no automatic pruning (manual cleanup needed).

Alternatives
- **Polling-only (no webhooks)**
  - Pros: Simpler, no webhook endpoint to maintain.
  - Cons: Higher latency (1+ hour), wastes API quota on redundant polls.
  - Why not chosen: Real-time updates critical for developer productivity.

- **GitHub App with GraphQL API**
  - Pros: Higher rate limits, richer data access.
  - Cons: More complex auth, requires app installation on org/repo.
  - Why not chosen: REST API sufficient for MVP; can upgrade later.

- **Stream entire repo (git clone + watch)**
  - Pros: Full history, no API dependency.
  - Cons: Massive storage, complex diffing logic, doesn't capture issues/PRs.
  - Why not chosen: Violates "GitHub is SoT" principle.

- **Manual ingestion (no automation)**
  - Pros: Zero ops.
  - Cons: Knowledge becomes stale immediately; unacceptable UX.
  - Why not chosen: RAG is useless without fresh data.

Implementation Notes
- **Code paths**:
  - Webhook handler: `apps/backend/monolith/api/views/webhook_views.py` (new).
  - Event processor: `apps/backend/rag-indexer/processor.py` (new service).
  - Batch job: `apps/backend/rag-indexer/batch.py` (new script).
  - Event schema: `apps/backend/rag-indexer/schemas.py` (Pydantic models).

- **Infrastructure**:
  - Webhook: Django view (part of monolith).
  - Queue: Redis + Celery (or RabbitMQ if already in stack).
  - Batch: Kubernetes CronJob (`infra/k8s/apps/rag-indexer/cronjob.yaml`).
  - Secrets: GitHub webhook secret, API token (stored in k8s Secret).

- **Environment Variables**:
  - `GITHUB_WEBHOOK_SECRET`: HMAC secret for webhook validation.
  - `GITHUB_API_TOKEN`: Personal access token for API requests.
  - `GITHUB_ORG`: Organization name (e.g., `ldamasio`).
  - `GITHUB_REPO`: Repository name (e.g., `robson`).

- **Testing**:
  - Unit tests: Mock GitHub API responses, verify chunking and embedding.
  - Integration tests: Send test webhook payloads, verify ParadeDB writes.
  - End-to-end tests: Trigger real webhook (test repo), query indexed data via RAG.
  - Load tests: Simulate 100 concurrent webhook events, measure throughput.

- **Monitoring**:
  - Metrics: Events ingested/hour, ingestion lag (event time → indexed time), failures.
  - Logs: Event payloads (sanitized), API errors, retry attempts.
  - Alerts: Ingestion lag >2 hours, API rate limit approaching, DLQ growing.

- **Related**:
  - ADR-0009: RAG Architecture (consumes ingested data).
  - ADR-0007: ParadeDB (storage backend).
  - Execution Plan: `docs/plan/04-rag-indexer-implementation.prompt`.
  - Event Format: `docs/ai-first/INGESTION_EVENTS.md`.

- **Future Enhancements**:
  - **Differential sync**: Only fetch changed files (use GitHub `commits` API with SHA comparison).
  - **Multi-repo support**: Index multiple repos (not just `ldamasio/robson`).
  - **Code chunking**: Use tree-sitter for AST-based code chunking (vs naive sliding window).
  - **Image indexing**: Extract text from diagrams (OCR) and index with CLIP embeddings.

References
- GitHub Webhooks: https://docs.github.com/en/webhooks
- GitHub REST API: https://docs.github.com/en/rest
- Celery: https://docs.celeryproject.org
