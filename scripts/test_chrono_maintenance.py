"""Offline proposal model only: never calls main(), subprocess or OSV."""
import copy
import datetime as dt
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("proposal", HERE / "check-chrono-maintenance.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
ROOT = Path("/review-fixture")
NOW = dt.datetime(2026, 11, 17, 20, tzinfo=dt.timezone.utc)
BASE = json.loads('{\n  "id": "RUSTSEC-2026-0543",\n  "summary": "chrono is unmaintained",\n  "details": "The chrono crate is unmaintained; all versions are affected.\\n\\nRecommended alternatives:\\n\\n- [jiff](https://crates.io/crates/jiff)\\n- [time](https://crates.io/crates/time)",\n  "modified": "2026-11-17T10:15:02.344223779Z",\n  "published": "2026-09-14T12:00:00Z",\n  "database_specific": {\n    "license": "CC0-1.0"\n  },\n  "references": [\n    {\n      "type": "PACKAGE",\n      "url": "https://crates.io/crates/chrono"\n    },\n    {\n      "type": "ADVISORY",\n      "url": "https://rustsec.org/advisories/RUSTSEC-2026-0543.html"\n    },\n    {\n      "type": "REPORT",\n      "url": "https://github.com/chronotope/chrono/issues/1768"\n    }\n  ],\n  "affected": [\n    {\n      "package": {\n        "name": "chrono",\n        "ecosystem": "crates.io",\n        "purl": "pkg:cargo/chrono"\n      },\n      "ranges": [\n        {\n          "type": "SEMVER",\n          "events": [\n            {\n              "introduced": "0.0.0-0"\n            }\n          ]\n        }\n      ],\n      "ecosystem_specific": {\n        "affects": {\n          "os": [],\n          "functions": [],\n          "arch": []\n        },\n        "affected_functions": null\n      },\n      "database_specific": {\n        "categories": [],\n        "cvss": null,\n        "informational": "unmaintained",\n        "source": "https://github.com/rustsec/advisory-db/blob/osv/crates/RUSTSEC-2026-0543.json"\n      }\n    }\n  ],\n  "schema_version": "1.9.2"\n}\n')


def document():
    results = []
    for n in m.LOCKFILES:
        pkg = {"package": {"name": "fixture-clean", "version": "1.0.0", "ecosystem": "npm"}}
        if n == "Cargo.lock":
            pkg = {"package": {"name": "chrono", "version": "0.4.45", "ecosystem": "crates.io"},
                   "vulnerabilities": [copy.deepcopy(BASE)],
                   "groups": [{"ids": [m.ADVISORY], "aliases": [m.ADVISORY], "max_severity": ""}]}
        results.append({"source": {"path": str(ROOT / n), "type": "lockfile"}, "packages": [pkg]})
    return {"results": results, "experimental_config": {"licenses": {"summary": False, "allowlist": None}}}


class SameSnapshotModel(unittest.TestCase):
    def outcome(self, data=None, status=1, stderr=b"", now=NOW):
        return m.classify(json.dumps(document() if data is None else data).encode(), status, stderr, ROOT, now)

    def rejects_change(self, change, reason):
        data = document()
        change(data)
        with self.assertRaisesRegex(ValueError, reason):
            self.outcome(data)

    def test_positive_exact_reviewed_object(self):
        self.assertEqual(self.outcome()["decision"], "temporary_exact_maintenance_deferral")
        self.assertEqual(self.outcome()["raw_scanner_status"], 1)

    def test_cve_alias_on_same_object(self):
        self.rejects_change(lambda d: d["results"][0]["packages"][0]["vulnerabilities"][0].update(aliases=["CVE-2026-999999"]), "alias/CVE")

    def test_group_only_cve_alias(self):
        self.rejects_change(lambda d: d["results"][0]["packages"][0]["groups"][0]["aliases"].append("CVE-2026-999999"), "group aliases")

    def test_reclassification(self):
        self.rejects_change(lambda d: d["results"][0]["packages"][0]["vulnerabilities"][0]["affected"][0]["database_specific"].update(informational="unsound"), "reclassification")

    def test_missing_classification(self):
        self.rejects_change(lambda d: d["results"][0]["packages"][0]["vulnerabilities"][0]["affected"][0].pop("database_specific"), "reclassification")

    def test_unrelated_svelte_cves_both_block(self):
        for advisory in ["GHSA-3r8g-jcqq-3p5j", "GHSA-6cf5-gc4q-fm3x"]:
            with self.subTest(advisory=advisory):
                self.rejects_change(lambda d: d["results"][1]["packages"][0].update(vulnerabilities=[{"id": advisory, "aliases": ["CVE-2026-128120"]}]), "unrelated finding")

    def test_second_chrono_advisory(self):
        self.rejects_change(lambda d: d["results"][0]["packages"][0]["vulnerabilities"].append({"id": "CVE-2026-999999"}), "additional advisory")

    def test_wrong_identity(self):
        self.rejects_change(lambda d: d["results"][0]["packages"][0]["package"].update(ecosystem="npm"), "unrelated finding")

    def test_cvss_added(self):
        self.rejects_change(lambda d: d["results"][0]["packages"][0]["vulnerabilities"][0].update(severity=[{"type": "CVSS_V3", "score": "9.8"}]), "severity")

    def test_new_fix_or_any_advisory_change(self):
        self.rejects_change(lambda d: d["results"][0]["packages"][0]["vulnerabilities"][0]["affected"][0]["ranges"][0]["events"].append({"fixed": "0.5.0"}), "object changed")

    def test_at_expiry_and_after(self):
        for now in [m.EXPIRES, m.EXPIRES + dt.timedelta(microseconds=1)]:
            with self.subTest(now=now), self.assertRaisesRegex(ValueError, "expired"):
                self.outcome(now=now)
        self.assertEqual(self.outcome(now=m.EXPIRES - dt.timedelta(microseconds=1))["raw_scanner_status"], 1)

    def test_operational_exits_and_unexplained_clean_status(self):
        for status in [0, 2, 127, 128, 129, 130, -9]:
            with self.subTest(status=status), self.assertRaisesRegex(ValueError, "exit 1"):
                self.outcome(status=status)

    def test_partial_error_with_vulnerability_exit_one(self):
        with self.assertRaisesRegex(ValueError, "diagnostics"):
            self.outcome(stderr=b"ERROR error when retrieving vulns: partial response")

    def test_missing_scanned_lockfile_source(self):
        self.rejects_change(lambda d: d["results"].pop(), "incomplete scan")

    def test_duplicate_source(self):
        self.rejects_change(lambda d: d["results"].append(copy.deepcopy(d["results"][0])), "duplicate source")

    def test_schema_extension_and_omission(self):
        self.rejects_change(lambda d: d.update(unrecognized={"finding": "anything"}), "schema changed")
        self.rejects_change(lambda d: d.pop("experimental_config"), "schema changed")

    def test_malformed_truncated_and_duplicate_json(self):
        for raw in [b"", b"{", b'{"results":[],"results":[]}', b'{"results":NaN}']:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                m.classify(raw, 1, b"", ROOT, NOW)

    def test_missing_actual_input_file_and_symlink(self):
        with tempfile.TemporaryDirectory(dir=HERE) as directory:
            root = Path(directory)
            for name in (*m.LOCKFILES, "osv-scanner.toml", "osv-scanner"):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("inert offline file fixture; never executed")
            self.assertEqual(len(m.input_hashes(root)), 5)
            missing = root / m.LOCKFILES[1]
            missing.unlink()
            with self.assertRaisesRegex(ValueError, "missing/nonregular"):
                m.input_hashes(root)
            missing.symlink_to(root / "Cargo.lock")
            with self.assertRaisesRegex(ValueError, "missing/nonregular"):
                m.input_hashes(root)


if __name__ == "__main__":
    unittest.main(verbosity=2)
