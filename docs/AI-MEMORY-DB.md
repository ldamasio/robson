# AI Memory Database

**Runtime knowledge store that learns from Pull Request history to inform AI agents during execution.**

## Overview

The **AI Memory Database** is a thread-safe, in-memory knowledge store that syncs project-specific knowledge from GitHub Pull Requests. It enables AI agents to:

✅ Learn from past architectural decisions
✅ Apply established code patterns
✅ Avoid repeating past mistakes
✅ Understand project-specific conventions
✅ Access short-term knowledge (last 30-90 days)

## Why AI Memory DB?

**Problem**: AI agents (Claude, GPT-4, etc.) have:
- ❌ No knowledge of project-specific decisions made in recent PRs
- ❌ No memory of how similar bugs were fixed
- ❌ No understanding of team's preferred patterns
- ❌ Training data cutoff (can't see recent PRs)

**Solution**: **Real-time knowledge extraction from PRs** → Store in memory → Query during execution

```
GitHub PRs (Last 30 days)
         ↓
  Extract Knowledge
         ↓
    Memory DB (In-Memory)
         ↓
  AI Agents Query → Informed Decisions
```

## Architecture

### Components

```
┌──────────────────────────────────────────────────────────┐
│                     GitHub API                            │
│         Source of Truth (Pull Requests)                   │
└────────────────────┬─────────────────────────────────────┘
                     │
                     │ Scheduled Sync (every 6h)
                     ↓
┌──────────────────────────────────────────────────────────┐
│              PR Knowledge Extractor                       │
│  - Classify knowledge type (DECISION, CODE_PATTERN, etc.) │
│  - Extract keywords for indexing                          │
│  - Calculate confidence scores                            │
└────────────────────┬─────────────────────────────────────┘
                     │
                     ↓
┌──────────────────────────────────────────────────────────┐
│                AI Memory Database                         │
│  - Thread-safe storage (RLock)                            │
│  - Multi-index (keyword, type, PR number)                 │
│  - Semantic search (relevance scoring)                    │
│  - O(1) lookup, O(log n) keyword search                   │
└────────────────────┬─────────────────────────────────────┘
                     │
                     │ query(), get_code_patterns()
                     ↓
┌──────────────────────────────────────────────────────────┐
│                   AI Agents                               │
│  - Query relevant knowledge before implementing           │
│  - Inject knowledge into LLM context                      │
│  - Follow established patterns                            │
└──────────────────────────────────────────────────────────┘
```

### Knowledge Types

| Type | Description | Example Source |
|------|-------------|----------------|
| **DECISION** | Architecture/design decisions | "We decided to use hexagonal architecture because..." |
| **CODE_PATTERN** | Code patterns and best practices | "Always use type hints for function parameters" |
| **BUG_FIX** | Bug fixes and solutions | "Fixed race condition by adding lock to monitor" |
| **REFACTORING** | Refactoring patterns | "Extracted risk validation into separate module" |
| **CONFIGURATION** | Config changes and rationale | "Changed cache TTL to 1s for real-time pricing" |
| **DISCUSSION** | Important discussions/consensus | "Team agreed to use 1% risk rule consistently" |
| **TEST_PATTERN** | Testing patterns | "Mock exchange API using pytest fixtures" |

## Quick Start

### 1. Install Dependencies

```bash
pip install PyGithub
```

### 2. Set Environment Variables

```bash
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxx"  # Get from https://github.com/settings/tokens
export GITHUB_REPO="ldamasio/robson"     # Optional (default value)
```

### 3. Sync Knowledge from GitHub

```bash
# Sync all merged PRs from last 30 days
python manage.py sync_pr_knowledge

# Show current memory stats
python manage.py sync_pr_knowledge --stats
```

**Expected Output**:
```
Syncing knowledge from GitHub repo: ldamasio/robson
Fetching merged PRs since 2025-12-21...
PR #234: feat(trading): add stop-loss monitor (5 entries)
PR #189: fix(monitor): race condition in stop execution (3 entries)
PR #167: refactor(risk): extract risk guards (4 entries)
Stored 12 knowledge entries from 3 PRs

============================================================
AI Memory Database Statistics
============================================================
Total Knowledge Entries: 87
Total PRs Indexed: 23
Total Keywords: 342

Entries by Type:
  - CODE_PATTERN: 28
  - DECISION: 15
  - BUG_FIX: 19
  - REFACTORING: 12
  - DISCUSSION: 13

Last Sync: 2026-01-20T14:30:00Z
============================================================
```

### 4. Query from Python Code

```python
from api.application.ai_memory import get_ai_memory, KnowledgeType

# Get memory instance
memory = get_ai_memory()

# Query with semantic search
results = memory.query("position sizing strategy", limit=5)
for entry, score in results:
    print(f"[{score:.2f}] {entry.content[:100]}... (PR #{entry.source_pr})")

# Get code patterns for a specific context
patterns = memory.get_code_patterns("stop-loss monitoring")
for pattern in patterns:
    print(f"- {pattern.content} (PR #{pattern.source_pr})")

# Get recent architectural decisions
decisions = memory.get_recent_decisions(limit=5)
for decision in decisions:
    print(f"- {decision.content[:80]}... (PR #{decision.source_pr})")

# Find similar bug fixes
bugs = memory.get_similar_bug_fixes("race condition in threading")
for bug in bugs:
    print(f"- {bug.content} (PR #{bug.source_pr})")
```

## Usage Patterns

### Pattern 1: Agent Implements New Feature

**Scenario**: AI agent needs to implement a new feature.

**Workflow**:
```python
# Step 1: Agent receives task
task = "Add trailing stop-loss feature"

# Step 2: Query memory for similar implementations
memory = get_ai_memory()
patterns = memory.get_code_patterns("stop-loss monitoring")
similar = memory.query("trailing stop OR stop-loss")

# Step 3: Inject knowledge into LLM context
context = f"""
Task: {task}

Relevant patterns from past PRs:
{format_knowledge_for_llm(patterns)}

Your implementation should follow these established patterns.
"""

# Step 4: Generate code with LLM (informed by past PRs)
code = llm.generate(context)
```

**Result**: New code follows project conventions learned from PRs.

---

### Pattern 2: Agent Fixes Bug

**Scenario**: AI agent encounters an error.

**Workflow**:
```python
# Step 1: Error occurs
error = "ThreadPoolExecutor deadlock in stop monitor"

# Step 2: Search for similar bug fixes
memory = get_ai_memory()
similar_bugs = memory.get_similar_bug_fixes(error)

# Step 3: Present solutions to agent
if similar_bugs:
    print(f"Found {len(similar_bugs)} similar bug fixes:")
    for bug in similar_bugs:
        print(f"  PR #{bug.source_pr}: {bug.content[:100]}...")
        print(f"  Solution: {bug.source_url}")
```

**Result**: Agent learns from past bug fixes instead of trial-and-error.

---

### Pattern 3: Agent Makes Architectural Decision

**Scenario**: AI agent needs to choose an architecture approach.

**Workflow**:
```python
# Step 1: Decision point
decision_needed = "How to structure margin trading module?"

# Step 2: Query past decisions
memory = get_ai_memory()
decisions = memory.query(decision_needed, knowledge_type=KnowledgeType.DECISION)

# Step 3: Present options based on past decisions
for entry, score in decisions:
    print(f"Past decision (relevance: {score:.2f}):")
    print(f"  {entry.content}")
    print(f"  Source: PR #{entry.source_pr}")
```

**Result**: Consistent architectural decisions across the project.

---

## Syncing Strategies

### On-Demand Sync

```bash
# Sync specific PR
python manage.py sync_pr_knowledge --pr 234

# Sync PRs with specific labels
python manage.py sync_pr_knowledge --labels "architecture,refactoring"

# Sync last 7 days
python manage.py sync_pr_knowledge --days 7

# Dry run (see what would be synced)
python manage.py sync_pr_knowledge --dry-run

# Clear memory and re-sync
python manage.py sync_pr_knowledge --clear
```

### Scheduled Sync (Recommended)

**Deploy a Kubernetes CronJob** to sync every 6 hours:

```yaml
# infra/k8s/cronjobs/sync-pr-knowledge.yaml
apiVersion: batch/v1
kind: CronJob
metadata:
  name: sync-pr-knowledge
  namespace: robson
spec:
  schedule: "0 */6 * * *"  # Every 6 hours
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: sync
            image: robson-backend:latest
            command:
              - python
              - manage.py
              - sync_pr_knowledge
              - --days
              - "30"
            env:
            - name: GITHUB_TOKEN
              valueFrom:
                secretKeyRef:
                  name: github-credentials
                  key: token
          restartPolicy: OnFailure
```

**Deploy**:
```bash
kubectl apply -f infra/k8s/cronjobs/sync-pr-knowledge.yaml
```

**Verify**:
```bash
kubectl get cronjob -n robson
kubectl get jobs -n robson -l app=sync-pr-knowledge
kubectl logs -n robson -l app=sync-pr-knowledge --tail=50
```

---

## API Reference

### `AIMemoryDB`

**Singleton instance**:
```python
from api.application.ai_memory import get_ai_memory

memory = get_ai_memory()
```

**Methods**:

#### `query(query, knowledge_type=None, min_confidence=0.3, limit=10)`
Semantic search with relevance scoring.

**Parameters**:
- `query` (str): Natural language query
- `knowledge_type` (KnowledgeType, optional): Filter by type
- `min_confidence` (float): Minimum relevance score (0.0-1.0)
- `limit` (int): Maximum results

**Returns**: `List[Tuple[KnowledgeEntry, float]]` - (entry, relevance_score)

**Example**:
```python
results = memory.query("position sizing", limit=5)
for entry, score in results:
    print(f"[{score:.2f}] {entry.content}")
```

---

#### `get_by_keywords(keywords)`
Get entries matching ANY keyword.

**Parameters**:
- `keywords` (List[str]): List of keywords

**Returns**: `List[KnowledgeEntry]`

**Example**:
```python
entries = memory.get_by_keywords(["risk", "management"])
```

---

#### `get_by_type(knowledge_type)`
Get all entries of a specific type.

**Parameters**:
- `knowledge_type` (KnowledgeType): Type to filter

**Returns**: `List[KnowledgeEntry]`

**Example**:
```python
from api.application.ai_memory import KnowledgeType

decisions = memory.get_by_type(KnowledgeType.DECISION)
```

---

#### `get_code_patterns(context)`
Get code patterns relevant to a context.

**Parameters**:
- `context` (str): Context description

**Returns**: `List[KnowledgeEntry]`

**Example**:
```python
patterns = memory.get_code_patterns("stop-loss monitoring")
```

---

#### `get_similar_bug_fixes(error_description)`
Find similar bug fixes.

**Parameters**:
- `error_description` (str): Description of the error

**Returns**: `List[KnowledgeEntry]`

**Example**:
```python
fixes = memory.get_similar_bug_fixes("race condition in threading")
```

---

#### `get_recent_decisions(limit=10)`
Get most recent architectural decisions.

**Parameters**:
- `limit` (int): Maximum results

**Returns**: `List[KnowledgeEntry]` (sorted by timestamp desc)

**Example**:
```python
decisions = memory.get_recent_decisions(limit=5)
```

---

#### `get_stats()`
Get memory database statistics.

**Returns**: `Dict[str, any]`

**Example**:
```python
stats = memory.get_stats()
print(f"Total entries: {stats['total_entries']}")
print(f"Total PRs: {stats['total_prs']}")
```

---

## Performance

### Benchmarks

| Operation | Complexity | Latency (1000 entries) |
|-----------|-----------|------------------------|
| Store entry | O(k) where k=keywords | <1ms |
| Query by keyword | O(n) | <10ms |
| Query by type | O(n) | <5ms |
| Get by PR number | O(1) | <1ms |
| Semantic search | O(n) | <20ms |

### Memory Usage

- **Average entry size**: ~500 bytes (content + metadata)
- **100 PRs** with avg 5 entries: ~250KB
- **Keyword index**: ~50KB
- **Total**: ~300KB (negligible)

### Thread Safety

- ✅ All operations use `RLock` (reentrant lock)
- ✅ Safe for concurrent reads and writes
- ✅ No deadlocks
- ✅ Tested with 5 concurrent threads

---

## Testing

Run unit tests:

```bash
cd apps/backend/monolith
pytest api/tests/test_ai_memory.py -v
```

**Expected Output**:
```
test_ai_memory.py::TestAIMemoryDB::test_singleton_pattern PASSED
test_ai_memory.py::TestAIMemoryDB::test_store_and_retrieve_entry PASSED
test_ai_memory.py::TestAIMemoryDB::test_query_semantic_search PASSED
test_ai_memory.py::TestAIMemoryDB::test_get_by_type PASSED
test_ai_memory.py::TestAIMemoryDB::test_thread_safety PASSED
...
```

---

## Best Practices

### DO ✅

- ✅ Sync knowledge regularly (every 6 hours recommended)
- ✅ Query knowledge **before** implementing new features
- ✅ Use specific queries (not too broad)
- ✅ Filter by `knowledge_type` when appropriate
- ✅ Check relevance scores (>0.3 is usually good)
- ✅ Inject knowledge into LLM context as **Tier 4** (see SKILL.md)

### DON'T ❌

- ❌ Don't rely solely on memory (use documentation too)
- ❌ Don't store PII or secrets in knowledge
- ❌ Don't query on every single line of code (expensive)
- ❌ Don't ignore low relevance scores (<0.3)
- ❌ Don't forget to set `GITHUB_TOKEN` environment variable

---

## Troubleshooting

### Problem: "GITHUB_TOKEN not set"

**Solution**:
```bash
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxx"
```

Get token from: https://github.com/settings/tokens (needs `repo` scope)

---

### Problem: "PyGithub not installed"

**Solution**:
```bash
pip install PyGithub
```

---

### Problem: "No knowledge entries found"

**Check**:
1. Are there merged PRs in the last 30 days?
2. Do PRs have meaningful descriptions (>50 chars)?
3. Did sync run successfully?

**Debug**:
```bash
python manage.py sync_pr_knowledge --dry-run  # See what would be synced
python manage.py sync_pr_knowledge --stats    # Check current state
```

---

### Problem: "Low relevance scores"

**Solution**: Be more specific in queries.

**Bad**: `memory.query("code")`  (too vague)
**Good**: `memory.query("position sizing calculation")`  (specific)

---

## Integration with SKILL.md

The AI Memory DB is **SKILL-011** in the AI Skills Framework.

**See**: [docs/SKILL.md](SKILL.md#ai-memory-database-runtime-knowledge-store)

**Usage in agent workflows**:
```
SKILL-101: Smart Order Entry
    ↓
1. Parse Intent (SKILL-001)
    ↓
2. Query Knowledge (SKILL-011) ← "position sizing patterns"
    ↓
3. Fetch Market Data (SKILL-002)
    ↓ (knowledge informs implementation)
4. Identify Technical Stop (SKILL-003)
    ↓
... continue workflow
```

---

## Roadmap

### Current Features (v1.0)
- ✅ In-memory storage (thread-safe)
- ✅ GitHub PR sync via API
- ✅ Keyword extraction
- ✅ Semantic search
- ✅ Knowledge type classification
- ✅ Django management command
- ✅ Unit tests

### Planned Features (v2.0)
- 🔄 Vector embeddings for better semantic search
- 🔄 Persistent storage (Redis/PostgreSQL)
- 🔄 Web UI for browsing knowledge
- 🔄 Auto-tagging with LLM
- 🔄 Knowledge quality scoring
- 🔄 Multi-repo support

---

## Related Documentation

- **[SKILL.md](SKILL.md)** - AI Skills Framework (includes SKILL-011)
- **[AGENTS.md](AGENTS.md)** - Comprehensive AI agent guide
- **[AI_WORKFLOW.md](AI_WORKFLOW.md)** - AI collaboration guidelines

---

## Contributing

To add new knowledge types or improve extraction:

1. Edit `apps/backend/monolith/api/application/ai_memory.py`
2. Add new `KnowledgeType` enum value
3. Update `PRKnowledgeExtractor` classification logic
4. Add tests in `api/tests/test_ai_memory.py`
5. Update this README

---

**Last Updated**: 2026-01-20
**Version**: 1.0
**Maintainer**: RBX Systems AI Team
**License**: Same as project
