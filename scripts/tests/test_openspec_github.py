"""Contract tests: no network or GitHub credentials required."""
import importlib.util
import io
from pathlib import Path
import random
import unittest
import urllib.error

SPEC = importlib.util.spec_from_file_location('tracking', Path(__file__).parents[1] / 'openspec_github.py')
tracking = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(tracking)
SHA = 'a' * 40
CONFIG = {'schema_version': 1, 'repository': 'ldamasio/robson', 'change': 'durable-product-sessions',
          'parent_issue': 10, 'progress_comment': 99, 'issues': {'AUTH-DUR-01': 11}}
TASKS = '## 1. AUTH-DUR-01: Contracts\n\n- [ ] 1.1 Model\n- [x] 1.2 Tests\n'
ENV = {'GITHUB_ACTIONS': 'true', 'GITHUB_REPOSITORY': 'ldamasio/robson',
       'GITHUB_REF': 'refs/heads/main', 'GITHUB_SHA': SHA, 'GITHUB_EVENT_NAME': 'push'}


class FakeAPI:
    def __init__(self, body, head=SHA, issue=10, fail_at=None):
        self.body, self.head, self.issue, self.fail_at = body, head, issue, fail_at
        self.calls = []

    def request(self, method, path, payload=None):
        self.calls.append((method, path, payload))
        if len(self.calls) == self.fail_at:
            raise ValueError('Simulated API failure')
        if path.endswith('/git/ref/heads/main'):
            return {'object': {'sha': self.head}}
        if method == 'GET':
            return {'body': self.body, 'issue_url': f'https://api.github.com/repos/ldamasio/robson/issues/{self.issue}'}
        self.body = payload['body']
        return {'body': self.body}


class TrackingTests(unittest.TestCase):
    def test_generated_progress_matches_checklist_across_seeded_cases(self):
        rng = random.Random(20261020)
        for _ in range(200):
            states = [rng.choice([False, True]) for _ in range(rng.randint(1, 100))]
            text = '## 1. AUTH-DUR-01: Contracts\n' + '\n'.join(
                f"- [{'x' if checked else ' '}] 1.{i} Task" for i, checked in enumerate(states, 1))
            report = tracking.render(CONFIG, text, SHA)
            self.assertIn(f'**{sum(states)} of {len(states)} technical tasks checked.**', report)
            tracking.verify_report(CONFIG['change'], report)

    def test_invalid_checklists_fail_instead_of_reporting_false_completion(self):
        for text in ['', TASKS + '- [ ] 1.1 Duplicate', TASKS.replace('1.2 Tests', '2.2 Wrong'),
                     TASKS + '\n## Notes\n- [ ] Unmapped', TASKS.replace('[x]', '[?]'),
                     TASKS.replace('- [x]', '* [x]'), TASKS.replace('- [x]', '-  [x]'),
                     TASKS.replace('- [x]', '-[x]'), TASKS.replace('- [x] ', '- '),
                     TASKS + '\n## 1. AUTH-DUR-01: Duplicate']:
            with self.subTest(text=text), self.assertRaises(ValueError):
                tracking.parse_tasks(text)

    def test_mapping_and_immutable_source_are_required(self):
        for config, sha in [(dict(CONFIG, issues={'AUTH-DUR-02': 12}), SHA), (CONFIG, 'main')]:
            with self.assertRaises(ValueError):
                tracking.render(config, TASKS, sha)

    def test_retry_after_lost_acknowledgment_is_noop(self):
        old = tracking.render(CONFIG, TASKS.replace('[x]', '[ ]'), SHA)
        report = tracking.render(CONFIG, TASKS, SHA)
        api = FakeAPI(old)
        self.assertTrue(tracking.publish(CONFIG, report, SHA, api, ENV))
        self.assertEqual([call[0] for call in api.calls], ['GET', 'GET', 'PATCH'])
        api.calls.clear()
        self.assertFalse(tracking.publish(CONFIG, report, SHA, api, ENV))
        self.assertEqual([call[0] for call in api.calls], ['GET', 'GET'])

    def test_refuses_other_issues_edited_comments_and_stale_jobs(self):
        report = tracking.render(CONFIG, TASKS, SHA)
        for api in [FakeAPI(report + 'Human edit'), FakeAPI(report, issue=20),
                    FakeAPI(report, head='b' * 40), FakeAPI('Unrelated comment')]:
            with self.subTest(api=api), self.assertRaises(ValueError):
                tracking.publish(CONFIG, report, SHA, api, ENV)
            self.assertFalse(any(call[0] == 'PATCH' for call in api.calls))

    def test_untrusted_events_and_branches_never_call_api(self):
        report = tracking.render(CONFIG, TASKS, SHA)
        for key, value in [('GITHUB_ACTIONS', 'false'), ('GITHUB_REPOSITORY', 'other/repo'),
                           ('GITHUB_REF', 'refs/pull/1/merge'), ('GITHUB_SHA', 'b' * 40),
                           ('GITHUB_EVENT_NAME', 'pull_request_target'), ('GITHUB_EVENT_NAME', 'pull_request')]:
            api = FakeAPI(report)
            with self.subTest(key=key), self.assertRaises(ValueError):
                tracking.publish(CONFIG, report, SHA, api, dict(ENV, **{key: value}))
            self.assertEqual(api.calls, [])

    def test_read_failures_never_mutate(self):
        report = tracking.render(CONFIG, TASKS, SHA)
        for failure in [1, 2]:
            api = FakeAPI(report, fail_at=failure)
            with self.assertRaises(ValueError):
                tracking.publish(CONFIG, report, SHA, api, ENV)
            self.assertFalse(any(call[0] == 'PATCH' for call in api.calls))

    def test_real_config_matches_every_work_package(self):
        plan = tracking.read_text(tracking.ROOT / 'openspec/changes/durable-product-sessions/tasks.md')
        packages = tracking.parse_tasks(plan)
        self.assertEqual(packages.keys(), tracking.load_config()['issues'].keys())

    def test_comment_line_endings_and_final_newline_are_normalized(self):
        report = tracking.render(CONFIG, TASKS, SHA)
        for variant in [report.replace('\n', '\r\n'), report.rstrip(), report + '\n  ']:
            tracking.verify_report(CONFIG['change'], variant)
            api = FakeAPI(variant)
            self.assertFalse(tracking.publish(CONFIG, report, SHA, api, ENV))

    def test_current_main_run_recovers_after_older_run_is_rejected(self):
        newer = 'b' * 40
        report = tracking.render(CONFIG, TASKS, SHA)
        api = FakeAPI(report, head=newer)
        with self.assertRaisesRegex(ValueError, 'Stale'):
            tracking.publish(CONFIG, report, SHA, api, ENV)
        current = tracking.render(CONFIG, TASKS, newer)
        self.assertTrue(tracking.publish(CONFIG, current, newer, api, dict(ENV, GITHUB_SHA=newer)))
        self.assertEqual(api.body, current)

    def test_http_adapter_bounds_requests_and_response_size(self):
        class Opener:
            def __init__(self, body):
                self.body = body
                self.calls = 0

            def open(self, request, timeout):
                self.calls += 1
                self.request, self.timeout = request, timeout
                return io.BytesIO(self.body)

        api = tracking.GitHub('test-only')
        api.opener = Opener(b'{}')
        for _ in range(3):
            self.assertEqual(api.request('GET', 'repos/ldamasio/robson/git/ref/heads/main'), {})
        with self.assertRaises(ValueError):
            api.request('GET', 'repos/ldamasio/robson/git/ref/heads/main')
        self.assertEqual(api.opener.calls, 3)
        self.assertEqual(api.opener.timeout, 20)
        api = tracking.GitHub('test-only')
        api.opener = Opener(b'x' * (tracking.LIMIT + 1))
        with self.assertRaisesRegex(ValueError, 'exceeds'):
            api.request('GET', 'repos/ldamasio/robson/git/ref/heads/main')

    def test_redirects_and_foreign_repository_are_rejected(self):
        with self.assertRaisesRegex(ValueError, 'redirect refused'):
            tracking.NoRedirect().redirect_request(None, None, 302, '', {}, 'https://example.invalid/')
        api = tracking.GitHub('test-only')
        with self.assertRaisesRegex(ValueError, 'outside'):
            api.request('GET', 'repos/other/repo/issues/1')

    def test_http_errors_do_not_echo_tokens_or_retry(self):
        class Opener:
            def open(self, request, timeout):
                raise urllib.error.HTTPError(request.full_url, 403, 'private server detail', {}, None)

        api = tracking.GitHub('test-only')
        api.opener = Opener()
        with self.assertRaisesRegex(ValueError, '^GitHub API returned HTTP 403; no automatic retry$'):
            api.request('GET', 'repos/ldamasio/robson/git/ref/heads/main')
        self.assertEqual(api.requests, 1)

    def test_read_timeout_is_bounded_failure_without_retry(self):
        class Opener:
            def open(self, request, timeout):
                raise TimeoutError('raw socket detail')

        api = tracking.GitHub('test-only')
        api.opener = Opener()
        with self.assertRaisesRegex(ValueError, '^GitHub API unavailable; no automatic retry$'):
            api.request('GET', 'repos/ldamasio/robson/git/ref/heads/main')
        self.assertEqual(api.requests, 1)


if __name__ == '__main__':
    unittest.main()
