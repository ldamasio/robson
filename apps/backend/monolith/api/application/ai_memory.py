"""
AI Memory Database - Runtime Knowledge Store for AI Agents

This module provides a thread-safe, in-memory knowledge database that:
- Extracts and stores knowledge from Pull Requests
- Provides fast lookup for AI agents during execution
- Syncs periodically with GitHub API
- Supports semantic search and contextual retrieval

Architecture:
- Memory Store: Thread-safe in-memory storage
- Knowledge Extractors: Extract patterns from PRs, code reviews, decisions
- Query Interface: Fast retrieval for AI agents
- Sync Manager: Background sync with GitHub

Usage:
    # Initialize (singleton)
    memory = AIMemoryDB.get_instance()

    # Query knowledge
    decisions = memory.query("position sizing strategy")
    patterns = memory.get_code_patterns("technical stop")

    # Sync from GitHub (scheduled job)
    memory.sync_from_github(repo="ldamasio/robson")
"""

from __future__ import annotations

import logging
import re
from dataclasses import dataclass, field
from datetime import datetime
from threading import Lock, RLock
from typing import Dict, List, Optional, Set, Tuple
from collections import defaultdict
from enum import Enum

logger = logging.getLogger(__name__)


class KnowledgeType(str, Enum):
    """Types of knowledge stored in memory."""

    DECISION = "DECISION"  # Architecture/design decisions from PRs
    CODE_PATTERN = "CODE_PATTERN"  # Code patterns and best practices
    BUG_FIX = "BUG_FIX"  # Bug fixes and solutions
    REFACTORING = "REFACTORING"  # Refactoring patterns
    CONFIGURATION = "CONFIGURATION"  # Config changes and rationale
    DISCUSSION = "DISCUSSION"  # Important discussions and consensus
    TEST_PATTERN = "TEST_PATTERN"  # Testing patterns


@dataclass
class KnowledgeEntry:
    """
    A single piece of knowledge extracted from a PR.

    Attributes:
        id: Unique identifier (e.g., "pr-123-comment-456")
        type: Type of knowledge
        content: The actual knowledge content
        source_pr: PR number
        source_url: URL to the source (PR, comment, review)
        keywords: Searchable keywords
        timestamp: When this knowledge was extracted
        confidence: Confidence score (0.0-1.0) for relevance
        metadata: Additional metadata (author, labels, etc.)
    """

    id: str
    type: KnowledgeType
    content: str
    source_pr: int
    source_url: str
    keywords: Set[str] = field(default_factory=set)
    timestamp: datetime = field(default_factory=datetime.utcnow)
    confidence: float = 1.0
    metadata: Dict[str, any] = field(default_factory=dict)

    def matches_query(self, query: str) -> float:
        """
        Calculate relevance score for a query.

        Returns:
            Float between 0.0-1.0 indicating relevance
        """
        query_lower = query.lower()
        score = 0.0

        # Keyword exact match
        if any(keyword in query_lower for keyword in self.keywords):
            score += 0.5

        # Content fuzzy match
        if query_lower in self.content.lower():
            score += 0.3

        # Type match (if query mentions the type)
        if self.type.value.lower() in query_lower:
            score += 0.2

        return min(score, 1.0) * self.confidence


@dataclass
class PRKnowledge:
    """
    Aggregated knowledge from a single PR.

    Groups all knowledge entries from one PR for efficient access.
    """

    pr_number: int
    title: str
    url: str
    author: str
    merged_at: Optional[datetime]
    labels: Set[str] = field(default_factory=set)
    entries: List[KnowledgeEntry] = field(default_factory=list)
    summary: str = ""

    def add_entry(self, entry: KnowledgeEntry) -> None:
        """Add a knowledge entry to this PR."""
        self.entries.append(entry)

    def get_entries_by_type(self, knowledge_type: KnowledgeType) -> List[KnowledgeEntry]:
        """Get all entries of a specific type."""
        return [e for e in self.entries if e.type == knowledge_type]


class AIMemoryDB:
    """
    Thread-safe, in-memory knowledge database for AI agents.

    Singleton pattern - only one instance per process.
    """

    _instance: Optional[AIMemoryDB] = None
    _lock = Lock()

    def __init__(self):
        """Private constructor - use get_instance() instead."""
        self._memory_lock = RLock()  # Reentrant lock for thread safety
        self._knowledge: Dict[str, KnowledgeEntry] = {}  # id -> entry
        self._pr_knowledge: Dict[int, PRKnowledge] = {}  # pr_number -> PR knowledge
        self._keyword_index: Dict[str, Set[str]] = defaultdict(set)  # keyword -> entry_ids
        self._type_index: Dict[KnowledgeType, Set[str]] = defaultdict(set)  # type -> entry_ids
        self._last_sync: Optional[datetime] = None
        self._sync_metadata: Dict[str, any] = {}

        logger.info("AIMemoryDB initialized")

    @classmethod
    def get_instance(cls) -> AIMemoryDB:
        """
        Get singleton instance of AIMemoryDB.

        Thread-safe singleton initialization.
        """
        if cls._instance is None:
            with cls._lock:
                if cls._instance is None:  # Double-check locking
                    cls._instance = cls()
        return cls._instance

    def store_entry(self, entry: KnowledgeEntry) -> None:
        """
        Store a knowledge entry in memory.

        Args:
            entry: Knowledge entry to store

        Note:
            Updates indexes for fast retrieval.
            Thread-safe.
        """
        with self._memory_lock:
            self._knowledge[entry.id] = entry

            # Update keyword index
            for keyword in entry.keywords:
                self._keyword_index[keyword.lower()].add(entry.id)

            # Update type index
            self._type_index[entry.type].add(entry.id)

            # Update PR knowledge
            if entry.source_pr not in self._pr_knowledge:
                # Create placeholder (will be updated with full PR data later)
                self._pr_knowledge[entry.source_pr] = PRKnowledge(
                    pr_number=entry.source_pr,
                    title=f"PR #{entry.source_pr}",
                    url=entry.source_url,
                    author="unknown",
                    merged_at=None,
                )

            self._pr_knowledge[entry.source_pr].add_entry(entry)

        logger.debug(f"Stored knowledge entry: {entry.id} (type: {entry.type})")

    def store_pr(self, pr_knowledge: PRKnowledge) -> None:
        """
        Store PR knowledge (overwrites existing).

        Args:
            pr_knowledge: PR knowledge to store
        """
        with self._memory_lock:
            self._pr_knowledge[pr_knowledge.pr_number] = pr_knowledge

        logger.debug(f"Stored PR knowledge: #{pr_knowledge.pr_number} ({len(pr_knowledge.entries)} entries)")

    def query(
        self,
        query: str,
        knowledge_type: Optional[KnowledgeType] = None,
        min_confidence: float = 0.3,
        limit: int = 10,
    ) -> List[Tuple[KnowledgeEntry, float]]:
        """
        Query knowledge database with semantic search.

        Args:
            query: Search query (natural language)
            knowledge_type: Filter by knowledge type (optional)
            min_confidence: Minimum relevance score (0.0-1.0)
            limit: Maximum results to return

        Returns:
            List of (entry, relevance_score) tuples, sorted by score descending
        """
        with self._memory_lock:
            results: List[Tuple[KnowledgeEntry, float]] = []

            # Get candidate entries
            if knowledge_type:
                candidate_ids = self._type_index.get(knowledge_type, set())
            else:
                candidate_ids = set(self._knowledge.keys())

            # Score all candidates
            for entry_id in candidate_ids:
                entry = self._knowledge[entry_id]
                score = entry.matches_query(query)

                if score >= min_confidence:
                    results.append((entry, score))

            # Sort by score descending
            results.sort(key=lambda x: x[1], reverse=True)

            return results[:limit]

    def get_by_keywords(self, keywords: List[str]) -> List[KnowledgeEntry]:
        """
        Get entries matching ANY of the keywords.

        Args:
            keywords: List of keywords to search

        Returns:
            List of matching knowledge entries
        """
        with self._memory_lock:
            entry_ids: Set[str] = set()

            for keyword in keywords:
                entry_ids.update(self._keyword_index.get(keyword.lower(), set()))

            return [self._knowledge[entry_id] for entry_id in entry_ids]

    def get_by_type(self, knowledge_type: KnowledgeType) -> List[KnowledgeEntry]:
        """
        Get all entries of a specific type.

        Args:
            knowledge_type: Type of knowledge to retrieve

        Returns:
            List of knowledge entries
        """
        with self._memory_lock:
            entry_ids = self._type_index.get(knowledge_type, set())
            return [self._knowledge[entry_id] for entry_id in entry_ids]

    def get_pr(self, pr_number: int) -> Optional[PRKnowledge]:
        """
        Get all knowledge from a specific PR.

        Args:
            pr_number: PR number

        Returns:
            PRKnowledge or None if not found
        """
        with self._memory_lock:
            return self._pr_knowledge.get(pr_number)

    def get_recent_decisions(self, limit: int = 10) -> List[KnowledgeEntry]:
        """
        Get most recent architectural/design decisions.

        Args:
            limit: Maximum number of decisions

        Returns:
            List of decision entries, sorted by timestamp descending
        """
        with self._memory_lock:
            decisions = self.get_by_type(KnowledgeType.DECISION)
            decisions.sort(key=lambda e: e.timestamp, reverse=True)
            return decisions[:limit]

    def get_code_patterns(self, context: str) -> List[KnowledgeEntry]:
        """
        Get code patterns relevant to a context.

        Args:
            context: Context description (e.g., "position sizing", "risk management")

        Returns:
            List of code pattern entries
        """
        results = self.query(context, knowledge_type=KnowledgeType.CODE_PATTERN, limit=5)
        return [entry for entry, score in results]

    def get_similar_bug_fixes(self, error_description: str) -> List[KnowledgeEntry]:
        """
        Find similar bug fixes based on error description.

        Args:
            error_description: Description of the bug/error

        Returns:
            List of bug fix entries
        """
        results = self.query(error_description, knowledge_type=KnowledgeType.BUG_FIX, limit=5)
        return [entry for entry, score in results]

    def clear(self) -> None:
        """
        Clear all knowledge from memory.

        Useful for testing or forced re-sync.
        """
        with self._memory_lock:
            self._knowledge.clear()
            self._pr_knowledge.clear()
            self._keyword_index.clear()
            self._type_index.clear()
            self._last_sync = None
            self._sync_metadata.clear()

        logger.warning("AIMemoryDB cleared")

    def get_stats(self) -> Dict[str, any]:
        """
        Get memory database statistics.

        Returns:
            Dict with statistics (entry count, PR count, last sync, etc.)
        """
        with self._memory_lock:
            type_counts = {k.value: len(v) for k, v in self._type_index.items()}

            return {
                "total_entries": len(self._knowledge),
                "total_prs": len(self._pr_knowledge),
                "entries_by_type": type_counts,
                "total_keywords": len(self._keyword_index),
                "last_sync": self._last_sync.isoformat() if self._last_sync else None,
                "sync_metadata": self._sync_metadata,
            }

    def mark_synced(self, metadata: Optional[Dict[str, any]] = None) -> None:
        """
        Mark database as synced with current timestamp.

        Args:
            metadata: Optional metadata about the sync (repo, commit, etc.)
        """
        with self._memory_lock:
            self._last_sync = datetime.utcnow()
            if metadata:
                self._sync_metadata.update(metadata)

        logger.info(f"AIMemoryDB marked as synced at {self._last_sync}")


class PRKnowledgeExtractor:
    """
    Extracts knowledge from Pull Request data.

    Uses heuristics and pattern matching to identify valuable knowledge.
    """

    # Keywords that indicate different types of knowledge
    DECISION_KEYWORDS = {
        "decided",
        "decision",
        "architecture",
        "approach",
        "strategy",
        "adr",
        "rationale",
    }

    CODE_PATTERN_KEYWORDS = {
        "pattern",
        "best practice",
        "should use",
        "always",
        "never",
        "prefer",
        "convention",
    }

    BUG_FIX_KEYWORDS = {
        "bug",
        "fix",
        "error",
        "issue",
        "problem",
        "broken",
        "crash",
    }

    REFACTORING_KEYWORDS = {
        "refactor",
        "cleanup",
        "improve",
        "restructure",
        "reorganize",
    }

    @staticmethod
    def extract_keywords(text: str) -> Set[str]:
        """
        Extract keywords from text using simple tokenization.

        Args:
            text: Text to extract keywords from

        Returns:
            Set of keywords (lowercase, 3+ chars)
        """
        # Remove special characters, split on whitespace
        words = re.findall(r"\b\w{3,}\b", text.lower())

        # Remove common stop words
        stop_words = {"the", "and", "for", "with", "this", "that", "from", "are", "was"}
        keywords = {w for w in words if w not in stop_words}

        return keywords

    @staticmethod
    def classify_knowledge_type(text: str, pr_labels: Set[str]) -> KnowledgeType:
        """
        Classify the type of knowledge based on text and PR labels.

        Args:
            text: Text content
            pr_labels: PR labels

        Returns:
            KnowledgeType
        """
        text_lower = text.lower()

        # Check labels first (more authoritative)
        if "bug" in pr_labels or "bugfix" in pr_labels:
            return KnowledgeType.BUG_FIX
        if "refactoring" in pr_labels or "refactor" in pr_labels:
            return KnowledgeType.REFACTORING
        if "test" in pr_labels or "tests" in pr_labels:
            return KnowledgeType.TEST_PATTERN

        # Check text content
        if any(kw in text_lower for kw in PRKnowledgeExtractor.DECISION_KEYWORDS):
            return KnowledgeType.DECISION
        if any(kw in text_lower for kw in PRKnowledgeExtractor.CODE_PATTERN_KEYWORDS):
            return KnowledgeType.CODE_PATTERN
        if any(kw in text_lower for kw in PRKnowledgeExtractor.BUG_FIX_KEYWORDS):
            return KnowledgeType.BUG_FIX
        if any(kw in text_lower for kw in PRKnowledgeExtractor.REFACTORING_KEYWORDS):
            return KnowledgeType.REFACTORING

        # Default to discussion
        return KnowledgeType.DISCUSSION

    @staticmethod
    def extract_from_pr_description(
        pr_number: int,
        title: str,
        body: str,
        pr_url: str,
        labels: Set[str],
    ) -> List[KnowledgeEntry]:
        """
        Extract knowledge from PR description.

        Args:
            pr_number: PR number
            title: PR title
            body: PR description body
            pr_url: PR URL
            labels: PR labels

        Returns:
            List of extracted knowledge entries
        """
        if not body or len(body.strip()) < 50:
            return []  # Too short to be valuable

        entries = []

        # Extract main content
        knowledge_type = PRKnowledgeExtractor.classify_knowledge_type(body, labels)
        keywords = PRKnowledgeExtractor.extract_keywords(f"{title} {body}")

        entry = KnowledgeEntry(
            id=f"pr-{pr_number}-description",
            type=knowledge_type,
            content=f"{title}\n\n{body}",
            source_pr=pr_number,
            source_url=pr_url,
            keywords=keywords,
            confidence=0.8,  # PR descriptions are generally high quality
            metadata={"labels": list(labels)},
        )

        entries.append(entry)

        return entries

    @staticmethod
    def extract_from_review_comment(
        pr_number: int,
        comment_id: int,
        author: str,
        body: str,
        comment_url: str,
        pr_labels: Set[str],
    ) -> Optional[KnowledgeEntry]:
        """
        Extract knowledge from a review comment.

        Args:
            pr_number: PR number
            comment_id: Comment ID
            author: Comment author
            body: Comment body
            comment_url: Comment URL
            pr_labels: PR labels

        Returns:
            KnowledgeEntry or None if not valuable
        """
        if not body or len(body.strip()) < 30:
            return None  # Too short

        # Skip simple approval comments
        if body.strip().lower() in ["lgtm", "approved", "looks good", "👍"]:
            return None

        knowledge_type = PRKnowledgeExtractor.classify_knowledge_type(body, pr_labels)
        keywords = PRKnowledgeExtractor.extract_keywords(body)

        return KnowledgeEntry(
            id=f"pr-{pr_number}-comment-{comment_id}",
            type=knowledge_type,
            content=body,
            source_pr=pr_number,
            source_url=comment_url,
            keywords=keywords,
            confidence=0.7,  # Review comments are valuable but less authoritative
            metadata={"author": author, "comment_id": comment_id},
        )


# Convenience function for easy access
def get_ai_memory() -> AIMemoryDB:
    """Get the singleton AIMemoryDB instance."""
    return AIMemoryDB.get_instance()
