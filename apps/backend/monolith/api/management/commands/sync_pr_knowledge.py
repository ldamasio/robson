"""
Django Management Command: Sync PR Knowledge

Syncs knowledge from GitHub Pull Requests into the AI Memory Database.

Usage:
    # Sync all merged PRs from last 30 days
    python manage.py sync_pr_knowledge

    # Sync specific PR
    python manage.py sync_pr_knowledge --pr 123

    # Sync PRs with specific labels
    python manage.py sync_pr_knowledge --labels "architecture,bug"

    # Dry run (show what would be synced without storing)
    python manage.py sync_pr_knowledge --dry-run

    # Clear memory before sync (force re-sync)
    python manage.py sync_pr_knowledge --clear

Environment Variables:
    GITHUB_TOKEN: GitHub API token for authentication (required)
    GITHUB_REPO: Repository in format "owner/repo" (default: ldamasio/robson)
"""

import os
from datetime import datetime, timedelta
from typing import List, Optional, Set

from django.core.management.base import BaseCommand, CommandError

from api.application.ai_memory import (
    AIMemoryDB,
    PRKnowledge,
    PRKnowledgeExtractor,
    get_ai_memory,
)


class Command(BaseCommand):
    """Sync Pull Request knowledge into AI Memory Database."""

    help = "Sync knowledge from GitHub Pull Requests into AI Memory Database"

    def add_arguments(self, parser):
        parser.add_argument(
            "--pr",
            type=int,
            help="Sync a specific PR number",
        )

        parser.add_argument(
            "--labels",
            type=str,
            help="Filter by labels (comma-separated)",
        )

        parser.add_argument(
            "--days",
            type=int,
            default=30,
            help="Sync PRs from last N days (default: 30)",
        )

        parser.add_argument(
            "--dry-run",
            action="store_true",
            help="Show what would be synced without storing",
        )

        parser.add_argument(
            "--clear",
            action="store_true",
            help="Clear memory before syncing",
        )

        parser.add_argument(
            "--repo",
            type=str,
            default=os.getenv("GITHUB_REPO", "ldamasio/robson"),
            help="GitHub repository (default: from GITHUB_REPO env var or ldamasio/robson)",
        )

        parser.add_argument(
            "--stats",
            action="store_true",
            help="Show current memory stats and exit",
        )

    def handle(self, *args, **options):
        """Execute the command."""
        memory = get_ai_memory()

        # Show stats and exit
        if options["stats"]:
            self._show_stats(memory)
            return

        # Clear memory if requested
        if options["clear"]:
            self.stdout.write(self.style.WARNING("Clearing AI Memory Database..."))
            memory.clear()
            self.stdout.write(self.style.SUCCESS("Memory cleared"))

        # Validate GitHub token
        github_token = os.getenv("GITHUB_TOKEN")
        if not github_token:
            raise CommandError(
                "GITHUB_TOKEN environment variable not set. "
                "Get a token from https://github.com/settings/tokens"
            )

        repo = options["repo"]
        dry_run = options["dry_run"]

        self.stdout.write(f"Syncing knowledge from GitHub repo: {repo}")

        if dry_run:
            self.stdout.write(self.style.WARNING("DRY RUN MODE - No data will be stored"))

        try:
            # Import GitHub library (lazy import to avoid dependency if not used)
            from github import Github, GithubException

            gh = Github(github_token)
            repository = gh.get_repo(repo)

            # Sync specific PR or multiple
            if options["pr"]:
                self._sync_single_pr(repository, options["pr"], memory, dry_run)
            else:
                self._sync_multiple_prs(
                    repository,
                    options["days"],
                    options.get("labels"),
                    memory,
                    dry_run,
                )

            # Mark as synced (if not dry run)
            if not dry_run:
                memory.mark_synced(
                    metadata={
                        "repo": repo,
                        "synced_at": datetime.utcnow().isoformat(),
                        "command": "sync_pr_knowledge",
                    }
                )

            # Show stats
            self._show_stats(memory)

            self.stdout.write(self.style.SUCCESS("Sync completed successfully"))

        except ImportError:
            raise CommandError(
                "PyGithub library not installed. Install with: pip install PyGithub"
            )
        except GithubException as e:
            raise CommandError(f"GitHub API error: {e}")
        except Exception as e:
            raise CommandError(f"Sync failed: {e}")

    def _sync_single_pr(self, repository, pr_number: int, memory: AIMemoryDB, dry_run: bool):
        """Sync a single PR."""
        self.stdout.write(f"Fetching PR #{pr_number}...")

        try:
            pr = repository.get_pull(pr_number)
        except Exception as e:
            raise CommandError(f"Failed to fetch PR #{pr_number}: {e}")

        entries = self._extract_pr_knowledge(pr)

        if dry_run:
            self.stdout.write(f"Would store {len(entries)} knowledge entries from PR #{pr_number}")
            for entry in entries:
                self.stdout.write(f"  - [{entry.type.value}] {entry.id}: {entry.content[:80]}...")
        else:
            # Store PR knowledge
            pr_knowledge = PRKnowledge(
                pr_number=pr.number,
                title=pr.title,
                url=pr.html_url,
                author=pr.user.login,
                merged_at=pr.merged_at,
                labels={label.name for label in pr.labels},
                entries=entries,
                summary=pr.title,
            )
            memory.store_pr(pr_knowledge)

            # Store individual entries
            for entry in entries:
                memory.store_entry(entry)

            self.stdout.write(
                self.style.SUCCESS(f"Stored {len(entries)} knowledge entries from PR #{pr_number}")
            )

    def _sync_multiple_prs(
        self,
        repository,
        days: int,
        labels_filter: Optional[str],
        memory: AIMemoryDB,
        dry_run: bool,
    ):
        """Sync multiple PRs based on filters."""
        since_date = datetime.utcnow() - timedelta(days=days)

        self.stdout.write(f"Fetching merged PRs since {since_date.date()}...")

        # Parse labels filter
        label_set: Optional[Set[str]] = None
        if labels_filter:
            label_set = {label.strip() for label in labels_filter.split(",")}
            self.stdout.write(f"Filtering by labels: {label_set}")

        # Fetch PRs
        pulls = repository.get_pulls(state="closed", sort="updated", direction="desc")

        total_entries = 0
        total_prs = 0

        for pr in pulls:
            # Skip if not merged
            if not pr.merged:
                continue

            # Skip if too old
            if pr.merged_at < since_date:
                break  # PRs are sorted by updated desc, so we can break

            # Filter by labels
            if label_set:
                pr_labels = {label.name for label in pr.labels}
                if not label_set.intersection(pr_labels):
                    continue

            # Extract knowledge
            entries = self._extract_pr_knowledge(pr)

            if not entries:
                continue

            total_prs += 1
            total_entries += len(entries)

            self.stdout.write(f"PR #{pr.number}: {pr.title} ({len(entries)} entries)")

            if not dry_run:
                # Store PR knowledge
                pr_knowledge = PRKnowledge(
                    pr_number=pr.number,
                    title=pr.title,
                    url=pr.html_url,
                    author=pr.user.login,
                    merged_at=pr.merged_at,
                    labels={label.name for label in pr.labels},
                    entries=entries,
                    summary=pr.title,
                )
                memory.store_pr(pr_knowledge)

                # Store individual entries
                for entry in entries:
                    memory.store_entry(entry)

        if dry_run:
            self.stdout.write(
                self.style.WARNING(
                    f"Would store {total_entries} knowledge entries from {total_prs} PRs"
                )
            )
        else:
            self.stdout.write(
                self.style.SUCCESS(
                    f"Stored {total_entries} knowledge entries from {total_prs} PRs"
                )
            )

    def _extract_pr_knowledge(self, pr) -> List:
        """Extract knowledge from a PR."""
        from api.application.ai_memory import KnowledgeEntry

        entries = []

        # Extract from PR description
        pr_labels = {label.name for label in pr.labels}

        description_entries = PRKnowledgeExtractor.extract_from_pr_description(
            pr_number=pr.number,
            title=pr.title,
            body=pr.body or "",
            pr_url=pr.html_url,
            labels=pr_labels,
        )
        entries.extend(description_entries)

        # Extract from review comments
        try:
            reviews = pr.get_reviews()
            for review in reviews:
                if review.body:
                    entry = PRKnowledgeExtractor.extract_from_review_comment(
                        pr_number=pr.number,
                        comment_id=review.id,
                        author=review.user.login,
                        body=review.body,
                        comment_url=review.html_url,
                        pr_labels=pr_labels,
                    )
                    if entry:
                        entries.append(entry)
        except Exception as e:
            self.stdout.write(
                self.style.WARNING(f"Failed to fetch reviews for PR #{pr.number}: {e}")
            )

        # Extract from issue comments (discussions)
        try:
            comments = pr.get_issue_comments()
            for comment in comments:
                if comment.body:
                    entry = PRKnowledgeExtractor.extract_from_review_comment(
                        pr_number=pr.number,
                        comment_id=comment.id,
                        author=comment.user.login,
                        body=comment.body,
                        comment_url=comment.html_url,
                        pr_labels=pr_labels,
                    )
                    if entry:
                        entries.append(entry)
        except Exception as e:
            self.stdout.write(
                self.style.WARNING(f"Failed to fetch comments for PR #{pr.number}: {e}")
            )

        return entries

    def _show_stats(self, memory: AIMemoryDB):
        """Show memory database statistics."""
        stats = memory.get_stats()

        self.stdout.write("\n" + "=" * 60)
        self.stdout.write(self.style.SUCCESS("AI Memory Database Statistics"))
        self.stdout.write("=" * 60)

        self.stdout.write(f"Total Knowledge Entries: {stats['total_entries']}")
        self.stdout.write(f"Total PRs Indexed: {stats['total_prs']}")
        self.stdout.write(f"Total Keywords: {stats['total_keywords']}")

        self.stdout.write("\nEntries by Type:")
        for ktype, count in stats["entries_by_type"].items():
            self.stdout.write(f"  - {ktype}: {count}")

        if stats["last_sync"]:
            self.stdout.write(f"\nLast Sync: {stats['last_sync']}")

        if stats["sync_metadata"]:
            self.stdout.write("\nSync Metadata:")
            for key, value in stats["sync_metadata"].items():
                self.stdout.write(f"  - {key}: {value}")

        self.stdout.write("=" * 60 + "\n")
