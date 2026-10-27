"""Name the gate's starved passes (agents' docs/design/landing-flow.md, 10).

Read the profiles' nextest JUnit with passing tests' output kept. A real-loop
test that waited for a CPU it wasn't given passes and says so in its output.
An unreadable report is named, but the gate's verdict remains its tests'.

    python3 -I -B scripts/gate_starved.py default fuzzy
"""

import json
from pathlib import Path
import subprocess
import sys
import xml.etree.ElementTree as ET


def starved_names(path, profile):
    report = ET.parse(path)
    names = set()
    for suite in report.iter("testsuite"):
        for test in suite.findall("testcase"):
            if any(test.find(tag) is not None for tag in ("failure", "error", "skipped")):
                continue
            if not any(line.startswith("skein-world: starved:")
                       for output in (*test.findall("system-out"), *test.findall("system-err"))
                       for line in (output.text or "").splitlines()):
                continue
            binary = test.get("classname") or suite.get("name")
            name = test.get("name")
            if not binary or not name or any(c.isspace() or not c.isprintable() for c in binary + name):
                raise ValueError("JUnit testcase lacks a printable binary/test name")
            names.add(f"{profile} {binary} {name}")
    return names


def main(profiles):
    try:
        metadata = json.loads(subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            check=True, capture_output=True, text=True).stdout)
        target = Path(metadata["target_directory"])
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError) as error:
        print(f"gate-starved: cannot read Cargo target directory: {error}", file=sys.stderr)
        return
    names = set()
    for profile in profiles:
        path = target / "nextest" / profile / "gate.xml"
        try:
            names.update(starved_names(path, profile))
        except (OSError, ET.ParseError, ValueError) as error:
            print(f"gate-starved: cannot read {path}: {error}", file=sys.stderr)
    for name in sorted(names):
        print(f"gate-starved {name}", file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1:])
