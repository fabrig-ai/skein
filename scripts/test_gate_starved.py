"""The gate names starved passes from JUnit without changing its verdict."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET


SCRIPT = Path(__file__).with_name("gate_starved.py").resolve()
BINARY = "fixture::story"
TEST = "chosen_test"
STARVED = "skein-world: starved: waited 10 ms runnable, used 1 ms of CPU, patience 5 ms"


class GateStarved(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="test-gate-starved-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.target = self.root / "target"
        (self.root / "bin").mkdir()
        cargo = self.root / "bin/cargo"
        cargo.write_text(f"#!{sys.executable}\n" + """
import json
import os
import sys

assert sys.argv[1:] == ["metadata", "--no-deps", "--format-version", "1"]
print(json.dumps({"target_directory": os.environ["FIXTURE_TARGET"]}))
""")
        cargo.chmod(0o755)
        self.env = {"PATH": str(self.root / "bin"), "FIXTURE_TARGET": str(self.target)}

    def report(self, profile, cases):
        suite = ET.Element("testsuite", name=BINARY)
        for name, children in cases:
            case = ET.SubElement(suite, "testcase", name=name)
            for tag, text in children.items():
                ET.SubElement(case, tag).text = text
        path = self.target / "nextest" / profile / "gate.xml"
        path.parent.mkdir(parents=True, exist_ok=True)
        ET.ElementTree(suite).write(path)
        return path

    def run_script(self, *profiles):
        result = subprocess.run([sys.executable, "-I", "-B", str(SCRIPT), *profiles],
                                cwd=self.root, env=self.env, text=True,
                                capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        return result.stderr.splitlines()

    def test_starved_passes_are_named(self):
        self.report("default", [(TEST, {"system-out": "before\n" + STARVED})])
        self.report("fuzzy", [(TEST, {"system-err": STARVED + "\nafter"})])
        self.assertEqual(self.run_script("fuzzy", "default"),
                         [f"gate-starved {profile} {BINARY} {TEST}"
                          for profile in ("default", "fuzzy")])

    def test_no_starved_pass_prints_nothing(self):
        cases = [
            ("ordinary", {}),
            ("mentioned", {"system-out": "mentioned " + STARVED}),
            ("indented", {"system-err": " " + STARVED}),
            ("failed", {"system-out": STARVED, "failure": ""}),
            ("error", {"system-err": STARVED, "error": ""}),
            ("skipped", {"system-out": STARVED, "skipped": ""}),
        ]
        self.report("default", cases)
        self.report("fuzzy", [])
        self.assertEqual(self.run_script("default", "fuzzy"), [])

    def test_an_unreadable_file_is_named_and_passes(self):
        for reason in ("missing", "malformed"):
            with self.subTest(reason=reason):
                bad = self.target / "nextest/default/gate.xml"
                if reason == "malformed":
                    bad.parent.mkdir(parents=True, exist_ok=True)
                    bad.write_text("<testsuite>")
                self.report("fuzzy", [(TEST, {"system-err": STARVED})])
                lines = self.run_script("default", "fuzzy")
                self.assertEqual(len(lines), 2, lines)
                self.assertTrue(lines[0].startswith(f"gate-starved: cannot read {bad}: "), lines)
                self.assertEqual(lines[1], f"gate-starved fuzzy {BINARY} {TEST}")

    def test_an_invalid_name_is_named_and_passes(self):
        for name in ("", "two words", "control\tcharacter"):
            with self.subTest(name=name):
                path = self.report("default", [(name, {"system-out": STARVED})])
                self.assertEqual(self.run_script("default"), [
                    f"gate-starved: cannot read {path}: JUnit testcase lacks a printable binary/test name"])


if __name__ == "__main__":
    unittest.main()
