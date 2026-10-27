"""The tier's phase selection and JUnit names, through its shell entry point."""

import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("after-landing.sh")
WHOLE = ["browser", "exhaustive"]
BINARY = "fixture::story"
TEST = "chosen_test"
STARVED = "skein-world: starved: waited 10 ms runnable, used 1 ms of CPU, patience 5 ms"

CARGO = """
import json
import os
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

arguments = sys.argv[1:]
story = json.loads(Path(os.environ["STORY"]).read_text())
with Path(os.environ["CARGO_LOG"]).open("a") as stream:
    stream.write(json.dumps(arguments) + "\\n")
if arguments == ["metadata", "--no-deps", "--format-version", "1"]:
    package = {"id": "fixture", "name": "fixture", "targets": [
        {"name": name, "kind": ["test"], "test": True}
        for name in story["targets"]]}
    print(json.dumps({"packages": [package], "workspace_members": ["fixture"]}))
elif arguments[:2] == ["nextest", "run"]:
    profile = arguments[arguments.index("--profile") + 1]
    assert "--no-fail-fast" in arguments
    config = arguments[arguments.index("--tool-config-file") + 1]
    assert config == "after-landing:" + str(Path.cwd() / ".config/nextest-after-landing.toml")
    with Path(os.environ["PHASE_LOG"]).open("a") as stream:
        stream.write(profile + "\\n")
    suite = ET.Element("testsuite")
    failed = False
    for binary, name, fails in story["cases"][profile]:
        if "-E" in arguments:
            expression = arguments[arguments.index("-E") + 1]
            matcher = "(binary_id(=" + binary + ") & test(=" + name + "))"
            if matcher not in expression:
                continue
        case = ET.SubElement(suite, "testcase", classname=binary, name=name)
        if fails:
            ET.SubElement(case, "failure")
            failed = True
        for tag, text in story.get("output", {}).get(name, {}).items():
            ET.SubElement(case, tag).text = text
    report = Path(os.environ["CARGO_TARGET_DIR"]) / "nextest" / profile / "after-landing.xml"
    report.parent.mkdir(parents=True, exist_ok=True)
    ET.ElementTree(suite).write(report)
    sys.exit(1 if failed else 0)
else:
    raise AssertionError(arguments)
"""


class AfterLanding(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="test-after-landing-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / "scripts").mkdir()
        shutil.copyfile(SCRIPT, self.root / "scripts/after-landing.sh")
        (self.root / "bin").mkdir()
        for tool in ("python3", "dirname"):
            (self.root / "bin" / tool).symlink_to(shutil.which(tool))
        self.executable("bin/cargo", CARGO)
        self.executable("scripts/browser.sh", """
import os
import sys

os.execvp("cargo", ["cargo", "nextest", "run", "--workspace", "--profile", "browser", *sys.argv[1:]])
""")
        self.executable("bin/chromium", "raise AssertionError('Chromium must not run')\n")
        self.env = {"PATH": str(self.root / "bin"),
                    "PHASE_LOG": str(self.root / "phases"),
                    "CARGO_LOG": str(self.root / "cargo-arguments"),
                    "STORY": str(self.root / "story.json"),
                    "SKEIN_TEST_CHROMIUM": str(self.root / "bin/chromium")}
        self.story = {"targets": ["story"],
                      "cases": {profile: [[BINARY, TEST, False]] for profile in WHOLE}}

    def executable(self, name, source):
        path = self.root / name
        path.write_text(f"#!{sys.executable}\n" + source)
        path.chmod(0o755)

    def run_script(self, *arguments):
        (self.root / "story.json").write_text(json.dumps(self.story))
        for name in ("phases", "cargo-arguments"):
            (self.root / name).unlink(missing_ok=True)
        result = subprocess.run(["/bin/sh", "scripts/after-landing.sh", *arguments],
                                cwd=self.root, env=self.env, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                timeout=10)
        log = self.root / "phases"
        phases = log.read_text().splitlines() if log.exists() else []
        return result.returncode, result.stdout, phases

    def test_whole_runs_every_phase(self):
        code, output, phases = self.run_script()
        self.assertEqual(code, 0, output)
        self.assertEqual(output, "")
        self.assertEqual(phases, WHOLE)

    def test_a_starved_pass_is_named(self):
        for tag in ("system-out", "system-err"):
            self.story["output"] = {TEST: {tag: "before\n" + STARVED + "\nafter"}}
            for arguments in ((), ("--only", f"exhaustive {BINARY} {TEST}")):
                with self.subTest(tag=tag, arguments=arguments):
                    code, output, _ = self.run_script(*arguments)
                    self.assertEqual(code, 0, output)
                    profiles = WHOLE if not arguments else ("exhaustive",)
                    self.assertEqual(output.splitlines(),
                                     [f"after-landing: starved {profile} {BINARY} {TEST}"
                                      for profile in profiles])

    def test_only_a_pass_is_starved(self):
        rows = [
            (True, {"system-err": STARVED}, [f"after-landing: failed exhaustive {BINARY} {TEST}"]),
            (False, {"system-out": "mentioned " + STARVED}, []),
            (False, {"system-err": " " + STARVED}, []),
            (False, {"system-out": STARVED, "skipped": ""}, []),
            (False, {"system-err": STARVED, "error": ""},
             [f"after-landing: failed exhaustive {BINARY} {TEST}"]),
        ]
        for fails, output, expected in rows:
            with self.subTest(fails=fails, output=output):
                self.story["cases"]["exhaustive"] = [[BINARY, TEST, fails]]
                self.story["output"] = {TEST: output}
                code, output, _ = self.run_script("--only", "exhaustive")
                self.assertEqual(code, 1 if expected else 0, output)
                self.assertEqual(output.splitlines(), expected)

    def test_a_failure_is_named(self):
        self.story["cases"]["exhaustive"] += [
            [BINARY, "z_failed", True], [BINARY, "a_failed", True]]
        self.story["output"] = {TEST: {"system-out": STARVED}}
        code, output, phases = self.run_script()
        self.assertEqual(code, 1, output)
        self.assertEqual(phases, WHOLE)
        self.assertEqual(output.splitlines(), [
            f"after-landing: failed exhaustive {BINARY} a_failed",
            f"after-landing: failed exhaustive {BINARY} z_failed",
            f"after-landing: starved browser {BINARY} {TEST}",
            f"after-landing: starved exhaustive {BINARY} {TEST}"])


if __name__ == "__main__":
    unittest.main()
