# GitHub Ingestion Event Format

**Last Updated**: 2025-12-07
**Related ADR**: ADR-0010
**Status**: Proposed

---

## Overview

This document defines the JSON schema for ingestion events consumed by the RAG Indexer service.

---

## Event Type: github.pr (Pull Request)

```json
{
  "event_type": "github.pr",
  "repo": "ldamasio/robson",
  "pr_number": 128,
  "title": "feat: add RAG architecture",
  "author": {
    "username": "ldamasio",
    "avatar_url": "https://github.com/ldamasio.png"
  },
  "merged_at": "2025-12-07T10:30:00Z",
  "url": "https://github.com/ldamasio/robson/pull/128",
  "content_blocks": [
    {
      "kind": "description",
      "text": "This PR implements the RAG architecture..."
    }
  ],
  "metadata": {
    "labels": ["enhancement", "ai"],
    "reviewers": ["user1", "user2"]
  }
}
```

---

## Event Type: github.issue (Issue)

```json
{
  "event_type": "github.issue",
  "repo": "ldamasio/robson",
  "issue_number": 45,
  "title": "Bug: RAG query returns duplicate results",
  "author": {
    "username": "contributor123"
  },
  "closed_at": "2025-12-07T11:00:00Z",
  "url": "https://github.com/ldamasio/robson/issues/45",
  "content_blocks": [
    {
      "kind": "body",
      "text": "When querying the RAG endpoint..."
    }
  ],
  "metadata": {
    "labels": ["bug", "rag", "fixed"]
  }
}
```

---

## Event Type: github.doc (Documentation)

```json
{
  "event_type": "github.doc",
  "repo": "ldamasio/robson",
  "file_path": "docs/adr/ADR-0009-rag-architecture-paradedb.md",
  "url": "https://github.com/ldamasio/robson/blob/main/docs/adr/ADR-0009.md",
  "content_blocks": [
    {
      "kind": "markdown",
      "text": "# ADR-0009: RAG Architecture..."
    }
  ],
  "metadata": {
    "file_type": "markdown",
    "doc_category": "adr"
  }
}
```

---

**Maintainers**: Robson Bot Core Team
**License**: Same as project
