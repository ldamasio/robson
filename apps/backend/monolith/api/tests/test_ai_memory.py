"""
Unit Tests for AI Memory Database

Tests the in-memory knowledge store functionality.
"""

import pytest
from datetime import datetime

from api.application.ai_memory import (
    AIMemoryDB,
    KnowledgeEntry,
    KnowledgeType,
    PRKnowledge,
    PRKnowledgeExtractor,
    get_ai_memory,
)


@pytest.fixture
def clean_memory():
    """Provide a clean memory database for each test."""
    memory = get_ai_memory()
    memory.clear()
    yield memory
    memory.clear()  # Cleanup after test


class TestAIMemoryDB:
    """Test AI Memory Database core functionality."""

    def test_singleton_pattern(self):
        """Test that AIMemoryDB follows singleton pattern."""
        memory1 = get_ai_memory()
        memory2 = get_ai_memory()
        assert memory1 is memory2

    def test_store_and_retrieve_entry(self, clean_memory):
        """Test storing and retrieving a knowledge entry."""
        entry = KnowledgeEntry(
            id="test-001",
            type=KnowledgeType.CODE_PATTERN,
            content="Always use type hints in Python functions",
            source_pr=123,
            source_url="https://github.com/test/repo/pull/123",
            keywords={"python", "type", "hints"},
            confidence=0.9,
        )

        clean_memory.store_entry(entry)

        # Retrieve by keywords
        results = clean_memory.get_by_keywords(["python"])
        assert len(results) == 1
        assert results[0].id == "test-001"

    def test_query_semantic_search(self, clean_memory):
        """Test semantic search functionality."""
        # Store multiple entries
        entries = [
            KnowledgeEntry(
                id="test-001",
                type=KnowledgeType.CODE_PATTERN,
                content="Use hexagonal architecture for clean separation",
                source_pr=100,
                source_url="https://github.com/test/repo/pull/100",
                keywords={"hexagonal", "architecture", "clean"},
            ),
            KnowledgeEntry(
                id="test-002",
                type=KnowledgeType.CODE_PATTERN,
                content="Always validate user input at API boundaries",
                source_pr=101,
                source_url="https://github.com/test/repo/pull/101",
                keywords={"validation", "input", "api"},
            ),
            KnowledgeEntry(
                id="test-003",
                type=KnowledgeType.BUG_FIX,
                content="Fixed race condition by adding threading lock",
                source_pr=102,
                source_url="https://github.com/test/repo/pull/102",
                keywords={"race", "condition", "threading", "lock"},
            ),
        ]

        for entry in entries:
            clean_memory.store_entry(entry)

        # Query for architecture patterns
        results = clean_memory.query("architecture", knowledge_type=KnowledgeType.CODE_PATTERN)
        assert len(results) > 0
        assert results[0][0].id == "test-001"
        assert results[0][1] > 0.3  # Relevance score

        # Query for bug fixes
        results = clean_memory.query("race condition", knowledge_type=KnowledgeType.BUG_FIX)
        assert len(results) > 0
        assert results[0][0].id == "test-003"

    def test_get_by_type(self, clean_memory):
        """Test filtering by knowledge type."""
        patterns = [
            KnowledgeEntry(
                id=f"pattern-{i}",
                type=KnowledgeType.CODE_PATTERN,
                content=f"Pattern {i}",
                source_pr=i,
                source_url=f"https://github.com/test/repo/pull/{i}",
                keywords={f"pattern{i}"},
            )
            for i in range(3)
        ]

        decisions = [
            KnowledgeEntry(
                id=f"decision-{i}",
                type=KnowledgeType.DECISION,
                content=f"Decision {i}",
                source_pr=i + 10,
                source_url=f"https://github.com/test/repo/pull/{i+10}",
                keywords={f"decision{i}"},
            )
            for i in range(2)
        ]

        for entry in patterns + decisions:
            clean_memory.store_entry(entry)

        # Get only code patterns
        pattern_results = clean_memory.get_by_type(KnowledgeType.CODE_PATTERN)
        assert len(pattern_results) == 3

        # Get only decisions
        decision_results = clean_memory.get_by_type(KnowledgeType.DECISION)
        assert len(decision_results) == 2

    def test_pr_knowledge_aggregation(self, clean_memory):
        """Test PR knowledge aggregation."""
        pr = PRKnowledge(
            pr_number=123,
            title="Add stop-loss monitoring",
            url="https://github.com/test/repo/pull/123",
            author="testuser",
            merged_at=datetime(2026, 1, 20, 12, 0, 0),
            labels={"feature", "trading"},
        )

        # Add entries
        entry1 = KnowledgeEntry(
            id="pr-123-1",
            type=KnowledgeType.CODE_PATTERN,
            content="Pattern from PR 123",
            source_pr=123,
            source_url=pr.url,
            keywords={"stop", "loss"},
        )

        entry2 = KnowledgeEntry(
            id="pr-123-2",
            type=KnowledgeType.DISCUSSION,
            content="Discussion from PR 123",
            source_pr=123,
            source_url=pr.url,
            keywords={"monitoring"},
        )

        pr.add_entry(entry1)
        pr.add_entry(entry2)

        clean_memory.store_pr(pr)

        # Retrieve PR knowledge
        retrieved_pr = clean_memory.get_pr(123)
        assert retrieved_pr is not None
        assert retrieved_pr.pr_number == 123
        assert len(retrieved_pr.entries) == 2
        assert retrieved_pr.title == "Add stop-loss monitoring"

    def test_get_recent_decisions(self, clean_memory):
        """Test getting recent decisions."""
        # Create decisions with different timestamps
        for i in range(5):
            entry = KnowledgeEntry(
                id=f"decision-{i}",
                type=KnowledgeType.DECISION,
                content=f"Decision {i}",
                source_pr=i,
                source_url=f"https://github.com/test/repo/pull/{i}",
                keywords={"decision"},
                timestamp=datetime(2026, 1, i + 1, 12, 0, 0),
            )
            clean_memory.store_entry(entry)

        # Get recent decisions (should be sorted by timestamp desc)
        recent = clean_memory.get_recent_decisions(limit=3)
        assert len(recent) == 3
        assert recent[0].id == "decision-4"  # Most recent
        assert recent[1].id == "decision-3"
        assert recent[2].id == "decision-2"

    def test_get_code_patterns(self, clean_memory):
        """Test getting code patterns by context."""
        # Store various patterns
        patterns = [
            KnowledgeEntry(
                id="pattern-position-sizing",
                type=KnowledgeType.CODE_PATTERN,
                content="Position sizing must be calculated from technical stop",
                source_pr=100,
                source_url="https://github.com/test/repo/pull/100",
                keywords={"position", "sizing", "technical", "stop"},
            ),
            KnowledgeEntry(
                id="pattern-risk-management",
                type=KnowledgeType.CODE_PATTERN,
                content="Risk management uses 1% rule consistently",
                source_pr=101,
                source_url="https://github.com/test/repo/pull/101",
                keywords={"risk", "management", "rule"},
            ),
        ]

        for entry in patterns:
            clean_memory.store_entry(entry)

        # Query for position sizing patterns
        results = clean_memory.get_code_patterns("position sizing")
        assert len(results) > 0
        assert "position" in results[0].content.lower()

    def test_get_similar_bug_fixes(self, clean_memory):
        """Test finding similar bug fixes."""
        # Store bug fixes
        bugs = [
            KnowledgeEntry(
                id="bug-race-condition",
                type=KnowledgeType.BUG_FIX,
                content="Fixed race condition in stop monitor by adding lock",
                source_pr=200,
                source_url="https://github.com/test/repo/pull/200",
                keywords={"race", "condition", "lock", "threading"},
            ),
            KnowledgeEntry(
                id="bug-deadlock",
                type=KnowledgeType.BUG_FIX,
                content="Fixed deadlock in ThreadPoolExecutor by using timeout",
                source_pr=201,
                source_url="https://github.com/test/repo/pull/201",
                keywords={"deadlock", "threadpool", "timeout"},
            ),
        ]

        for entry in bugs:
            clean_memory.store_entry(entry)

        # Find similar bug fixes
        results = clean_memory.get_similar_bug_fixes("race condition in monitoring")
        assert len(results) > 0
        assert "race condition" in results[0].content.lower()

    def test_memory_stats(self, clean_memory):
        """Test memory statistics."""
        # Add various entries
        for i in range(3):
            clean_memory.store_entry(
                KnowledgeEntry(
                    id=f"pattern-{i}",
                    type=KnowledgeType.CODE_PATTERN,
                    content=f"Pattern {i}",
                    source_pr=i,
                    source_url=f"https://github.com/test/repo/pull/{i}",
                    keywords={f"kw{i}"},
                )
            )

        for i in range(2):
            clean_memory.store_entry(
                KnowledgeEntry(
                    id=f"decision-{i}",
                    type=KnowledgeType.DECISION,
                    content=f"Decision {i}",
                    source_pr=i + 10,
                    source_url=f"https://github.com/test/repo/pull/{i+10}",
                    keywords={f"kw{i+10}"},
                )
            )

        stats = clean_memory.get_stats()

        assert stats["total_entries"] == 5
        assert stats["entries_by_type"]["CODE_PATTERN"] == 3
        assert stats["entries_by_type"]["DECISION"] == 2
        assert stats["total_keywords"] == 5

    def test_thread_safety(self, clean_memory):
        """Test thread-safe operations."""
        import threading

        def store_entries(start_id, count):
            for i in range(count):
                entry = KnowledgeEntry(
                    id=f"thread-{start_id}-{i}",
                    type=KnowledgeType.CODE_PATTERN,
                    content=f"Content {i}",
                    source_pr=start_id * 100 + i,
                    source_url=f"https://github.com/test/repo/pull/{i}",
                    keywords={f"thread{start_id}"},
                )
                clean_memory.store_entry(entry)

        # Create multiple threads
        threads = []
        for thread_id in range(5):
            thread = threading.Thread(target=store_entries, args=(thread_id, 10))
            threads.append(thread)
            thread.start()

        # Wait for all threads
        for thread in threads:
            thread.join()

        # Verify all entries were stored
        stats = clean_memory.get_stats()
        assert stats["total_entries"] == 50  # 5 threads × 10 entries


class TestPRKnowledgeExtractor:
    """Test PR knowledge extraction logic."""

    def test_extract_keywords(self):
        """Test keyword extraction."""
        text = "This is a test with some keywords like Python, Django, and testing"
        keywords = PRKnowledgeExtractor.extract_keywords(text)

        assert "python" in keywords
        assert "django" in keywords
        assert "testing" in keywords
        assert "test" in keywords
        # Stop words should be excluded
        assert "this" not in keywords
        assert "with" not in keywords

    def test_classify_knowledge_type_from_labels(self):
        """Test knowledge type classification based on PR labels."""
        # Bug fix
        knowledge_type = PRKnowledgeExtractor.classify_knowledge_type(
            "Some content", {"bug", "urgent"}
        )
        assert knowledge_type == KnowledgeType.BUG_FIX

        # Refactoring
        knowledge_type = PRKnowledgeExtractor.classify_knowledge_type(
            "Some content", {"refactoring"}
        )
        assert knowledge_type == KnowledgeType.REFACTORING

        # Test pattern
        knowledge_type = PRKnowledgeExtractor.classify_knowledge_type(
            "Some content", {"test", "ci"}
        )
        assert knowledge_type == KnowledgeType.TEST_PATTERN

    def test_classify_knowledge_type_from_content(self):
        """Test knowledge type classification based on content."""
        # Decision
        knowledge_type = PRKnowledgeExtractor.classify_knowledge_type(
            "We decided to use hexagonal architecture because...", set()
        )
        assert knowledge_type == KnowledgeType.DECISION

        # Code pattern
        knowledge_type = PRKnowledgeExtractor.classify_knowledge_type(
            "Always use type hints for better code quality", set()
        )
        assert knowledge_type == KnowledgeType.CODE_PATTERN

        # Bug fix
        knowledge_type = PRKnowledgeExtractor.classify_knowledge_type(
            "This fixes the bug where the system would crash", set()
        )
        assert knowledge_type == KnowledgeType.BUG_FIX

    def test_extract_from_pr_description(self):
        """Test extracting knowledge from PR description."""
        entries = PRKnowledgeExtractor.extract_from_pr_description(
            pr_number=123,
            title="feat(trading): add stop-loss monitoring",
            body="This PR implements continuous stop-loss monitoring using a background thread. "
            "The monitor checks positions every 30 seconds and executes market orders when "
            "stop levels are triggered.",
            pr_url="https://github.com/test/repo/pull/123",
            labels={"feature", "trading"},
        )

        assert len(entries) == 1
        entry = entries[0]
        assert entry.id == "pr-123-description"
        assert entry.source_pr == 123
        assert "monitoring" in entry.keywords
        assert "stop" in entry.keywords or "stop-loss" in entry.keywords

    def test_extract_from_pr_description_too_short(self):
        """Test that short PR descriptions are not extracted."""
        entries = PRKnowledgeExtractor.extract_from_pr_description(
            pr_number=123,
            title="Fix typo",
            body="Fixed typo in README",
            pr_url="https://github.com/test/repo/pull/123",
            labels=set(),
        )

        assert len(entries) == 0  # Too short to be valuable

    def test_extract_from_review_comment(self):
        """Test extracting knowledge from review comments."""
        entry = PRKnowledgeExtractor.extract_from_review_comment(
            pr_number=123,
            comment_id=456,
            author="reviewer",
            body="Good point about thread safety. We should use locks to prevent race conditions.",
            comment_url="https://github.com/test/repo/pull/123#discussion_r456",
            pr_labels={"refactoring"},
        )

        assert entry is not None
        assert entry.id == "pr-123-comment-456"
        assert entry.source_pr == 123
        assert entry.metadata["author"] == "reviewer"
        assert "thread" in entry.keywords or "safety" in entry.keywords

    def test_extract_from_review_comment_too_short(self):
        """Test that short comments are not extracted."""
        entry = PRKnowledgeExtractor.extract_from_review_comment(
            pr_number=123,
            comment_id=456,
            author="reviewer",
            body="LGTM",
            comment_url="https://github.com/test/repo/pull/123#discussion_r456",
            pr_labels=set(),
        )

        assert entry is None  # Too short


class TestAIMemoryIntegration:
    """Integration tests for AI Memory usage in agent workflows."""

    def test_agent_workflow_with_memory(self, clean_memory):
        """Test a complete agent workflow using memory."""
        # Step 1: Populate memory with knowledge
        clean_memory.store_entry(
            KnowledgeEntry(
                id="pattern-position-sizing",
                type=KnowledgeType.CODE_PATTERN,
                content="Position sizing formula: (Capital × 1%) / |Entry - Technical Stop|",
                source_pr=100,
                source_url="https://github.com/ldamasio/robson/pull/100",
                keywords={"position", "sizing", "formula", "risk"},
                confidence=0.95,
            )
        )

        clean_memory.store_entry(
            KnowledgeEntry(
                id="decision-risk-rule",
                type=KnowledgeType.DECISION,
                content="We decided to use 1% risk rule for all trades to ensure consistent risk management",
                source_pr=50,
                source_url="https://github.com/ldamasio/robson/pull/50",
                keywords={"risk", "rule", "management", "consistency"},
                confidence=0.9,
            )
        )

        # Step 2: Agent receives task
        task = "Calculate position size for new trade"

        # Step 3: Agent queries memory for relevant knowledge
        patterns = clean_memory.get_code_patterns(task)
        decisions = clean_memory.query("risk rule", knowledge_type=KnowledgeType.DECISION)

        # Step 4: Verify agent found relevant knowledge
        assert len(patterns) > 0
        assert "position sizing" in patterns[0].content.lower()

        assert len(decisions) > 0
        assert "1%" in decisions[0][0].content

        # Step 5: Agent would inject this knowledge into LLM context
        # (This is the key integration point)
        context = f"""
        Task: {task}

        Relevant patterns from past PRs:
        - {patterns[0].content} (PR #{patterns[0].source_pr})

        Relevant decisions:
        - {decisions[0][0].content} (PR #{decisions[0][0].source_pr})

        Implement the task following these established patterns.
        """

        assert "Position sizing formula" in context
        assert "1% risk rule" in context
