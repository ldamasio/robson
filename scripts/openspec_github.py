#!/usr/bin/env python3
"""Render OpenSpec progress; CI may update one explicitly mapped GitHub comment."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / 'openspec/github-tracking.json'
LIMIT = 1024 * 1024
HEADING = re.compile(r'^## (\d+)\. (AUTH-DUR-\d{2}): (.+)$')
TASK = re.compile(r'^- \[([ xX])\] (\d+\.\d+) (.+)$')


def read_text(path):
    with path.open('rb') as stream:
        data = stream.read(LIMIT + 1)
    if len(data) > LIMIT:
        raise ValueError('Input exceeds 1 MiB')
    return data.decode('utf-8')


def load_config(path=CONFIG):
    config = json.loads(read_text(path))
    if config['schema_version'] != 1 or config['repository'] != 'ldamasio/robson':
        raise ValueError('Unsupported tracking schema or repository')
    if config['change'] != 'durable-product-sessions':
        raise ValueError('This pilot only supports durable-product-sessions')
    numbers = [config['parent_issue'], *config['issues'].values()]
    if not 1 <= len(config['issues']) <= 64:
        raise ValueError('Expected 1..64 work packages')
    if any(type(n) is not int or n <= 0 for n in numbers + [config['progress_comment']]):
        raise ValueError('Expected positive GitHub identifiers')
    if len(numbers) != len(set(numbers)):
        raise ValueError('Issue mappings must be unique')
    if any(not re.fullmatch(r'AUTH-DUR-\d{2}', key) for key in config['issues']):
        raise ValueError('Invalid work package ID')
    return config


def parse_tasks(text):
    packages, seen_tasks = {}, set()
    current = None
    for line_number, line in enumerate(text.splitlines(), 1):
        heading = HEADING.fullmatch(line)
        if heading:
            section, key, title = heading.groups()
            if key in packages or int(key.rsplit('-', 1)[1]) != int(section):
                raise ValueError('Duplicate or mismatched work package ID')
            current = {'section': section, 'title': title, 'line': line_number, 'done': 0, 'total': 0}
            packages[key] = current
        elif line.startswith('## '):
            current = None
        elif re.match(r'^\s*(?:[-*+]|\d+[.)])\s*(?:\[|\d+\.\d+\b)', line):
            task = TASK.fullmatch(line)
            if not task or current is None:
                raise ValueError(f'Unrecognized checklist at line {line_number}')
            checked, task_id, _ = task.groups()
            if task_id in seen_tasks or task_id.split('.')[0] != current['section']:
                raise ValueError('Duplicate task or task in wrong work package')
            seen_tasks.add(task_id)
            current['total'] += 1
            current['done'] += int(checked.lower() == 'x')
    if not packages or len(packages) > 64 or len(seen_tasks) > 1000:
        raise ValueError('Unexpected work package/task count')
    if any(p['total'] == 0 for p in packages.values()):
        raise ValueError('Empty work package')
    return packages


def canonical_text(text):
    if not isinstance(text, str):
        raise ValueError('Expected a textual report')
    return text.replace('\r\n', '\n').rstrip() + '\n'


def seal_report(change, content):
    content = canonical_text(content)
    digest = hashlib.sha256(content.encode()).hexdigest()
    return f'<!-- robson-openspec-report:v1 {change} sha256={digest} -->\n{content}'


def verify_report(change, body):
    body = canonical_text(body)
    _, separator, content = body.partition('\n')
    if not separator or seal_report(change, content) != body:
        raise ValueError('Progress comment is missing its marker or was edited; refusing overwrite')


def render(config, tasks, source_sha):
    if not re.fullmatch(r'[0-9a-f]{40}', source_sha):
        raise ValueError('Source must be an immutable full Git commit SHA')
    packages = parse_tasks(tasks)
    if packages.keys() != config['issues'].keys():
        raise ValueError('Work package/issue mapping differs from tasks.md')
    total = sum(p['total'] for p in packages.values())
    done = sum(p['done'] for p in packages.values())
    base = f"https://github.com/{config['repository']}"
    source = f"{base}/blob/{source_sha}/openspec/changes/{config['change']}/tasks.md"
    lines = [
        '### OpenSpec implementation progress', '',
        f'**{done} of {total} technical tasks checked.** This counts the source checklist; it is not deployment approval.', '',
        f'[Source checklist at `{source_sha[:12]}`]({source})', '',
        '| Work package | Delivery issue | Checklist |',
        '| --- | --- | --- |',
    ]
    for key, package in packages.items():
        number = config['issues'][key]
        lines.append(f"| [{key}]({source}#L{package['line']}) | [#{number}]({base}/issues/{number}) | {package['done']}/{package['total']} |")
    lines += ['', 'Generated from repository files. Discuss changes in issue comments and update the checklist through reviewed PRs.',
              'Issue/Project status does not change this checklist. Planning, implementation, acceptance and deployment remain distinct.',
              'Automatic refresh runs from main after the tracking workflow is merged. Do not edit this report manually.', '']
    return seal_report(config['change'], '\n'.join(lines))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError('GitHub API redirect refused')


class GitHub:
    def __init__(self, token):
        if not token:
            raise ValueError('GH_TOKEN is required for publication')
        self.token = token
        self.requests = 0
        self.opener = urllib.request.build_opener(NoRedirect())

    def request(self, method, path, payload=None):
        if self.requests >= 3:
            raise ValueError('GitHub request budget exceeded')
        self.requests += 1
        if not path.startswith('repos/ldamasio/robson/'):
            raise ValueError('GitHub API path outside the pilot repository')
        request = urllib.request.Request(
            'https://api.github.com/' + path,
            data=json.dumps(payload).encode() if payload is not None else None,
            method=method,
            headers={'Authorization': 'Bearer ' + self.token, 'Accept': 'application/vnd.github+json',
                     'Content-Type': 'application/json', 'X-GitHub-Api-Version': '2022-11-28',
                     'User-Agent': 'robson-openspec-tracking'},
        )
        try:
            with self.opener.open(request, timeout=20) as response:
                data = response.read(LIMIT + 1)
        except urllib.error.HTTPError as error:
            raise ValueError(f'GitHub API returned HTTP {error.code}; no automatic retry') from None
        except (urllib.error.URLError, TimeoutError):
            raise ValueError('GitHub API unavailable; no automatic retry') from None
        if len(data) > LIMIT:
            raise ValueError('GitHub response exceeds 1 MiB')
        return json.loads(data)


def publish(config, report, source_sha, api, environment):
    # Publication is restricted to the serialized, trusted default-branch job.
    expected = {'GITHUB_ACTIONS': 'true', 'GITHUB_REPOSITORY': config['repository'],
                'GITHUB_REF': 'refs/heads/main', 'GITHUB_SHA': source_sha}
    if any(environment.get(key) != value for key, value in expected.items()):
        raise ValueError('Publication requires the trusted main-branch GitHub Actions job')
    if environment.get('GITHUB_EVENT_NAME') not in ('push', 'workflow_dispatch'):
        raise ValueError('Publication is forbidden for this event')
    verify_report(config['change'], report)
    base = f"repos/{config['repository']}"
    head = api.request('GET', base + '/git/ref/heads/main')
    if head['object']['sha'] != source_sha:
        raise ValueError('Stale workflow source: rerun at current main')
    path = base + '/issues/comments/' + str(config['progress_comment'])
    existing = api.request('GET', path)
    expected_issue = f"https://api.github.com/{base}/issues/{config['parent_issue']}"
    if existing.get('issue_url') != expected_issue:
        raise ValueError('Progress comment belongs to a different issue')
    verify_report(config['change'], existing.get('body', ''))
    if canonical_text(existing['body']) == canonical_text(report):
        return False
    result = api.request('PATCH', path, {'body': report})
    if canonical_text(result.get('body')) != canonical_text(report):
        raise ValueError('GitHub did not confirm the expected report; inspect before retry')
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--publish', action='store_true', help='CI only; default renders without network access')
    args = parser.parse_args()
    config = load_config()
    tasks = read_text(ROOT / 'openspec/changes' / config['change'] / 'tasks.md')
    report = render(config, tasks, args.source_sha)
    if args.publish:
        api = GitHub(os.environ.get('GH_TOKEN'))
        changed = publish(config, report, args.source_sha, api, os.environ)
        print(json.dumps({'changed': changed, 'requests': api.requests, 'source_sha': args.source_sha}))
    else:
        print(report, end='')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError) as error:
        print(f'OpenSpec tracking failed: {error}', file=sys.stderr)
        sys.exit(1)
