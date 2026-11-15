#!/usr/bin/env python3
"""Validate Robson release identity without builds, downloads or publication."""
import argparse
import json
from pathlib import Path
import re
import sys
import tomllib


SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)


def require_semver(version):
    match = SEMVER.fullmatch(version) if isinstance(version, str) else None
    if not match or any(part.isdigit() and len(part) > 1 and part[0] == "0"
                        for part in (match.group(4) or "").split(".")):
        raise ValueError(f"invalid SemVer: {version!r}")
    return version


def check_release_identity(root, tag=None):
    workspace = tomllib.loads((root / "Cargo.toml").read_text())
    version = require_semver(workspace["workspace"]["package"]["version"])
    names = set()
    for member in workspace["workspace"]["members"]:
        package = tomllib.loads((root / member / "Cargo.toml").read_text())["package"]
        if package["version"] not in ({"workspace": True}, version):
            raise ValueError(f"workspace crate version mismatch: {package['name']}")
        names.add(package["name"])
    packages = tomllib.loads((root / "Cargo.lock").read_text())["package"]
    local = [package for package in packages if package["name"] in names]
    if len(local) != len(names) or {package["name"] for package in local} != names:
        raise ValueError("Cargo.lock must contain each workspace crate exactly once")
    if any(package.get("source") or package["version"] != version for package in local):
        raise ValueError("local Cargo.lock crate identity differs from workspace")
    frontend = json.loads((root / "frontend/package.json").read_text())
    if frontend["version"] != version:
        raise ValueError("frontend package version differs from workspace")
    if tag is not None:
        require_semver(tag[1:] if tag.startswith("v") else tag)
        if tag != "v" + version:
            raise ValueError(f"tag {tag!r} must equal v{version}")
    return version, len(names)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--tag", help="optional exact v-prefixed release tag")
    args = parser.parse_args()
    try:
        version, count = check_release_identity(args.root, args.tag)
    except (ValueError, OSError, KeyError) as error:
        print(f"release identity: {error}", file=sys.stderr)
        return 1
    print(f"release identity OK: {version}; {count} workspace crates; frontend aligned")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
