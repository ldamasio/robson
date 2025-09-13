#!/usr/bin/env python3
import sys
import re
from pathlib import Path


PATTERN = re.compile(r"[áàâãéèêíìîóòôõúùûçÁÀÂÃÉÈÊÍÌÎÓÒÔÕÚÙÛÇ]")

EXCLUDES = [
    ".venv/",
    "backends/monolith/.venv/",
    "backends/monolith/.old/",
    "backends/monolith/staticfiles/",
    "docs/vendor/",
    "frontends/web/node_modules/",
    "frontends/web/dist/",
    "frontends/web/build/",
]


def is_excluded(path: Path) -> bool:
    p = str(path)
    if any(p.endswith(ext) for ext in (".min.css", ".min.js")):
        return True
    return any(p.startswith(ex) for ex in EXCLUDES)


def main(argv: list[str]) -> int:
    violations = []
    for arg in argv:
        path = Path(arg)
        if not path.exists() or path.is_dir() or is_excluded(path):
            continue
        try:
            with path.open("rb") as f:
                for i, raw in enumerate(f, 1):
                    try:
                        line = raw.decode("utf-8", errors="ignore")
                    except Exception:
                        continue
                    if PATTERN.search(line):
                        violations.append(f"{path}:{i}: non-English character detected")
        except Exception:
            # ignore unreadable files
            continue
    if violations:
        sys.stderr.write("\n".join(violations) + "\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

