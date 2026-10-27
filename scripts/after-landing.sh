#!/bin/sh
# The tiers outside the gate, with named failures and narrow reruns for the
# after-landing runner (agents' docs/design/landing-flow.md, 4, step 8).
set -eu
umask 077
cd -P "$(dirname "$0")/.."
exec python3 -I -B - "$@" <<'PY'
"""Run the selected slow phases and print the runner's failure contract."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import xml.etree.ElementTree as ET


ROOT = Path.cwd()
TARGET = ROOT / "target"
TOOL_CONFIG = ["--tool-config-file", f"after-landing:{ROOT / '.config/nextest-after-landing.toml'}"]


def nextest(profile, *arguments):
    return ["cargo", "nextest", "run", *arguments, "--profile", profile, *TOOL_CONFIG]


def executable(path):
    return bool(path) and Path(path).is_file() and os.access(path, os.X_OK)


def binary_ids():
    # nextest rejects an unknown binary_id before it can return no-tests (4).
    # Cargo's target metadata lets a name from an older commit disappear cleanly.
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"], text=True))
    names = set()
    for package in metadata["packages"]:
        if package["id"] not in metadata["workspace_members"]:
            continue
        for target in package["targets"]:
            if not target["test"]:
                continue
            for kind in target["kind"]:
                if kind in ("lib", "rlib", "dylib", "staticlib", "cdylib", "proc-macro"):
                    names.add(package["name"])
                elif kind == "test":
                    names.add(f"{package['name']}::{target['name']}")
                elif kind in ("bin", "example", "bench"):
                    names.add(f"{package['name']}::{kind}/{target['name']}")
    return names


def expression(tests):
    # Escape filter delimiters while keeping exact binary/test matchers.
    def escaped(value):
        for character in ("\\", ")", ",", "/"):
            value = value.replace(character, "\\" + character)
        return value.replace("(", "\\u{28}")

    return " | ".join(f"(binary_id(={escaped(binary)}) & test(={escaped(test)}))"
                      for binary, test in sorted(tests)) or "none()"


def junit(profile):
    report = ET.parse(TARGET / "nextest" / profile / "after-landing.xml")
    failures = set()
    starved = set()
    for suite in report.iter("testsuite"):
        for test in suite.findall("testcase"):
            failed = test.find("failure") is not None or test.find("error") is not None
            passed_starved = not failed and test.find("skipped") is None and any(
                line.startswith("skein-world: starved:")
                for output in (*test.findall("system-out"), *test.findall("system-err"))
                for line in (output.text or "").splitlines())
            if failed or passed_starved:
                binary = test.get("classname") or suite.get("name")
                name = test.get("name")
                if not binary or not name or any(c.isspace() or not c.isprintable() for c in binary + name):
                    raise ValueError("JUnit testcase lacks a printable binary/test name")
                (failures if failed else starved).add(f"{profile} {binary} {name}")
    return failures, starved


def run(phases):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", action="append", default=[], metavar="NAME")
    args = parser.parse_args()
    by_name = {name: (profile, prerequisites, command)
               for name, profile, prerequisites, command in phases}
    profiles = {profile: name for name, profile, _, _ in phases if profile}
    selected = set()
    filters = {}
    unfiltered = set()

    def select(name):
        if name not in selected:
            selected.add(name)
            for prerequisite in by_name[name][1]:
                select(prerequisite)

    for name in args.only:
        if name in by_name:
            select(name)
            unfiltered.add(name)
            continue
        words = name.split(" ")
        if (len(words) != 3 or words[0] not in profiles or
                not all(words) or any(c.isspace() or not c.isprintable() for c in "".join(words))):
            parser.error(f"unknown name: {name}")
        profile, binary, test = words
        phase = profiles[profile]
        select(phase)
        filters.setdefault(phase, set()).add((binary, test))
    if not args.only:
        selected.update(by_name)

    missing = requirements(selected)
    if missing:
        for name in sorted(missing):
            print(f"after-landing: lacks {name}", flush=True)
        return 3
    # Ignore inherited external targets; all build and JUnit outputs stay here.
    if TARGET.is_symlink() or (TARGET.exists() and not TARGET.is_dir()):
        for name in sorted(selected):
            print(f"after-landing: failed {name}", flush=True)
        return 1
    os.environ.update(CARGO_TARGET_DIR=str(TARGET), NEXTEST_DOUBLE_SPAWN="0",
                      NEXTEST_USER_CONFIG_FILE="none")
    statuses = {}
    failures = set()
    starved = set()
    for name, profile, prerequisites, command in phases:
        if name not in selected:
            continue
        if any(not statuses[prerequisite] for prerequisite in prerequisites):
            statuses[name] = False
            failures.add(name)
            continue
        filtered = name in filters and name not in unfiltered
        command = list(command)
        if profile:
            command += ["--no-fail-fast"]
            if filtered:
                try:
                    known = binary_ids()
                except (OSError, subprocess.SubprocessError, ValueError) as error:
                    print(f"{name}: cannot read Cargo targets: {error}", flush=True)
                    statuses[name] = False
                    failures.add(name)
                    continue
                command += ["-E", expression({pair for pair in filters[name] if pair[0] in known})]
            try:
                (TARGET / "nextest" / profile / "after-landing.xml").unlink(missing_ok=True)
            except OSError as error:
                print(f"{name}: cannot clear stale JUnit: {error}", flush=True)
                statuses[name] = False
                failures.add(name)
                continue
        try:
            code = subprocess.call(command)
        except OSError as error:
            print(f"{name}: {error}", flush=True)
            code = 1
        statuses[name] = code == 0 or (bool(profile) and filtered and code == 4)
        names = set()
        if profile and not (filtered and code == 4):
            try:
                names, passed_starved = junit(profile)
                starved.update(passed_starved)
            except (OSError, ET.ParseError, ValueError) as error:
                if code:
                    print(f"{name}: JUnit unavailable: {error}", flush=True)
        if names:
            statuses[name] = False
            failures.update(names)
        elif not statuses[name]:
            failures.add(name)
    for name in sorted(failures):
        print(f"after-landing: failed {name}", flush=True)
    for name in sorted(starved):
        print(f"after-landing: starved {name}", flush=True)
    return 1 if failures else 0


def requirements(selected):
    missing = set()
    if selected and not shutil.which("cargo"):
        missing.add("cargo")
    if "browser" in selected:
        chromium = os.environ.get("SKEIN_TEST_CHROMIUM")
        if not executable(chromium):
            chromium = shutil.which("chromium")
        if executable(chromium):
            os.environ["SKEIN_TEST_CHROMIUM"] = str(Path(chromium).resolve())
        else:
            missing.add("Chromium")
    return missing


PHASES = [
    ("browser", "browser", (), ["scripts/browser.sh", *TOOL_CONFIG]),
    ("exhaustive", "exhaustive", (), nextest("exhaustive", "--workspace")),
]

raise SystemExit(run(PHASES))
PY
