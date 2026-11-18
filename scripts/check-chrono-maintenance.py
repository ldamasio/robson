#!/usr/bin/env python3
"""Operator-authorized 2026-11-17; temporary disposition, not CI acceptance."""
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
# subprocess runs only the hash-pinned scanner in main(), never a shell.
import subprocess  # nosec B404
import sys
import tomllib

ADVISORY = "RUSTSEC-2026-0543"
EXPIRES = dt.datetime(2026, 12, 17, tzinfo=dt.timezone.utc)
ADVISORY_SHA256 = "a405d95d63de33bfbdded87f899e75153b610832987e259d02606de7d95ffd69"
LOCKFILES = ("Cargo.lock", "frontend/pnpm-lock.yaml", "scripts/requirements-security.txt")
SCANNER_SHA256 = "15314940c10d26af9c6649f150b8a47c1262e8fc7e17b1d1029b0e479e8ed8a0"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def no_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON key")
        result[key] = value
    return result


def classify(raw, status, stderr, root, now):
    require(now.tzinfo is not None and now < EXPIRES, "temporary disposition expired")
    require(status == 1, "expected original scanner finding exit 1")
    require(not stderr, "scanner diagnostics at error verbosity; possible partial/failed scan")
    data = json.loads(raw, object_pairs_hook=no_duplicate_keys,
                      parse_constant=lambda value: require(False, "non-finite JSON"))
    require(isinstance(data, dict) and set(data) == {"results", "experimental_config"}, "top-level schema changed")
    require(data["experimental_config"] == {"licenses": {"summary": False, "allowlist": None}}, "scan configuration changed")
    require(isinstance(data["results"], list), "results schema")
    expected_sources = {str((root / name).resolve()) for name in LOCKFILES}
    sources, allowed = set(), 0
    for result in data["results"]:
        require(isinstance(result, dict) and set(result) == {"source", "packages"}, "source schema changed")
        source = result["source"]
        require(isinstance(source, dict) and set(source) == {"path", "type"}, "source identity schema")
        require(source["type"] == "lockfile" and isinstance(source["path"], str), "unexpected source kind")
        path = str(Path(source["path"]).resolve())
        require(path in expected_sources and path not in sources, "missing/extra/duplicate source")
        sources.add(path)
        require(isinstance(result["packages"], list) and result["packages"], "empty source package inventory")
        for package in result["packages"]:
            require(isinstance(package, dict) and set(package) <= {"package", "dependency_groups", "vulnerabilities", "groups"}, "package schema changed")
            info = package.get("package")
            require(isinstance(info, dict) and set(info) <= {"name", "version", "ecosystem", "commit", "os_package_name", "deprecated"}, "package identity schema")
            require(all(isinstance(info.get(k), str) and info[k] for k in ("name", "version", "ecosystem")), "incomplete package identity")
            require(not info.get("deprecated", False), "separate deprecated-package finding")
            vulns, groups = package.get("vulnerabilities", []), package.get("groups", [])
            require(isinstance(vulns, list) and isinstance(groups, list), "finding schema")
            if not vulns:
                require(not groups, "group without advisory")
                continue
            require(path == str((root / "Cargo.lock").resolve()) and info["name"] == "chrono" and info["ecosystem"] == "crates.io", "unrelated finding blocks")
            require(len(vulns) == 1, "additional advisory blocks")
            vuln = vulns[0]
            require(isinstance(vuln, dict) and vuln.get("id") == ADVISORY, "different advisory blocks")
            require(not vuln.get("aliases") and not vuln.get("severity") and not vuln.get("withdrawn"), "alias/CVE/severity/withdrawal blocks")
            affected = vuln.get("affected")
            require(isinstance(affected, list) and len(affected) == 1, "affected schema changed")
            a = affected[0]
            require(a.get("package") == {"name": "chrono", "ecosystem": "crates.io", "purl": "pkg:cargo/chrono"}, "affected package changed")
            require(a.get("database_specific", {}).get("informational") == "unmaintained", "reclassification blocks")
            require(not a.get("database_specific", {}).get("cvss"), "severity blocks")
            actual = hashlib.sha256(json.dumps(vuln, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
            require(actual == ADVISORY_SHA256, "advisory object changed from reviewed snapshot")
            require(groups == [{"ids": [ADVISORY], "aliases": [ADVISORY], "max_severity": ""}], "group aliases/severity/schema changed")
            allowed += 1
    require(sources == expected_sources and allowed == 1, "incomplete scan or missing exact maintenance advisory")
    return {"decision": "temporary_exact_maintenance_deferral", "advisory": ADVISORY,
            "raw_scanner_status": status, "expires": EXPIRES.isoformat(), "advisory_object_sha256": ADVISORY_SHA256}


def input_hashes(root):
    paths = [root / name for name in (*LOCKFILES, "osv-scanner.toml", "osv-scanner")]
    require(all(path.is_file() and not path.is_symlink() for path in paths), "missing/nonregular scanner input")
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}


def main():
    root = Path(__file__).resolve().parents[1]
    evidence = Path(os.environ["RUNNER_TEMP"]) / "robson-osv-evidence"
    evidence.mkdir(mode=0o700, exist_ok=False)
    decision = {"decision": "FAIL", "scanner_started": False}
    try:
        before = input_hashes(root)
        require(before["osv-scanner"] == SCANNER_SHA256, "scanner differs from pinned OSV 2.4.0")
        config = tomllib.loads((root / "osv-scanner.toml").read_text())
        require(all(entry.get("id") != ADVISORY for entry in config.get("IgnoredVulns", [])), "maintenance advisory must remain unignored")
        require(not config.get("PackageOverrides"), "package override is outside this disposition")
        argv = [str(root / "osv-scanner"), "scan", "source", "--format=json", "--all-packages", "--verbosity=error", "--config", "osv-scanner.toml"]
        for name in LOCKFILES:
            argv += ["--lockfile", name]
        decision.update(scanner_started=True, argv=argv, inputs_sha256=before)
        with (evidence / "scanner.stdout.json").open("xb") as out, (evidence / "scanner.stderr.log").open("xb") as err:
            # argv is fixed: the SHA-256-verified scanner and constant flags/lockfiles; no shell.
            completed = subprocess.run(argv, cwd=root, stdout=out, stderr=err, timeout=600, check=False)  # nosec B603
        decision["raw_scanner_status"] = completed.returncode
        (evidence / "scanner.exit.txt").write_text(str(completed.returncode) + "\n")
        require(before == input_hashes(root), "scanner/input changed during scan")
        raw = (evidence / "scanner.stdout.json").read_bytes()
        stderr = (evidence / "scanner.stderr.log").read_bytes()
        decision.update(classify(raw, completed.returncode, stderr, root, dt.datetime.now(dt.timezone.utc)))
        print("::warning::RUSTSEC-2026-0543 remains visible: temporary maintenance-risk disposition until 2026-12-17T00:00:00Z; original OSV exit 1 retained in artifact.")
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
        decision.update(decision="FAIL", error=str(error))
        print("Dependency Scan remains blocking; see preserved raw scanner evidence.", file=sys.stderr)
        return 1
    finally:
        decision["evidence_sha256"] = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in evidence.iterdir() if path.is_file()}
        (evidence / "decision.json").write_text(json.dumps(decision, indent=2) + "\n")


if __name__ == "__main__":
    sys.exit(main())
