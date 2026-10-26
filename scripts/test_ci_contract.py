#!/usr/bin/env python3
"""Exercise local CI recipes without running toolchains or modifying sources."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
JUST = shutil.which("just")
CI_COMMANDS = [
    ["cargo", "fmt", "--all", "--", "--check"],
    ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
    ["cargo", "nextest", "run", "--workspace", "--profile", "ci"],
    ["cargo", "test", "--workspace", "--doc"],
    ["cargo", "deny", "check"],
    ["python3", "-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"],
    ["npm", "--prefix", "bench-playwright", "ci"],
    ["npm", "--prefix", "bench-playwright", "test"],
    ["npm", "--prefix", "bench-playwright", "audit", "--audit-level=moderate"],
    ["node", "--test", "npm/dragon-head-mcp/test/run.test.js"],
]


class TestCiRecipes(unittest.TestCase):
    def run_recipe(self, recipe, fail_command=None):
        self.assertIsNotNone(JUST, "install just to run the CI recipe contract tests")
        with tempfile.TemporaryDirectory(prefix="dragon-head-ci-contract-") as directory:
            root = Path(directory)
            calls = root / "calls.jsonl"
            marker = root / "source-marker"
            marker.write_text("original", encoding="utf-8")
            recorder = root / "record-command"
            recorder.write_text(
                f"#!{sys.executable}\n"
                "import json, os, pathlib, sys\n"
                "command = [pathlib.Path(sys.argv[0]).name, *sys.argv[1:]]\n"
                "with open(os.environ['DH_CI_COMMAND_LOG'], 'a') as stream:\n"
                "    stream.write(json.dumps(command) + '\\n')\n"
                "if command[:2] == ['cargo', 'fmt'] and '--check' not in command:\n"
                "    pathlib.Path(os.environ['DH_CI_SOURCE_MARKER']).write_text('rewritten')\n"
                "if command == json.loads(os.environ['DH_CI_FAIL_COMMAND']):\n"
                "    sys.exit(23)\n",
                encoding="utf-8",
            )
            recorder.chmod(0o755)
            for tool in ("cargo", "python3", "npm", "node"):
                (root / tool).symlink_to(recorder)
            env = os.environ.copy()
            env.update({
                "PATH": str(root) + os.pathsep + env.get("PATH", ""),
                "DH_CI_COMMAND_LOG": str(calls),
                "DH_CI_SOURCE_MARKER": str(marker),
                "DH_CI_FAIL_COMMAND": json.dumps(fail_command),
            })
            result = subprocess.run(
                [JUST, "--justfile", str(ROOT / "Justfile"), recipe],
                env=env, capture_output=True, text=True, timeout=20,
            )
            commands = [json.loads(line) for line in calls.read_text().splitlines()] if calls.exists() else []
            return result, commands, marker.read_text()

    def test_ci_runs_required_gates_without_rewriting_sources(self):
        result, commands, marker = self.run_recipe("ci")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(commands, CI_COMMANDS)
        self.assertEqual(marker, "original")

    def test_test_all_uses_the_same_nonmutating_gates(self):
        result, commands, marker = self.run_recipe("test-all")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(commands, CI_COMMANDS)
        self.assertEqual(marker, "original")

    def test_ci_stops_when_any_required_gate_fails(self):
        for index, command in enumerate(CI_COMMANDS):
            with self.subTest(command=command):
                result, commands, marker = self.run_recipe("ci", command)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(commands, CI_COMMANDS[:index + 1])
                self.assertEqual(marker, "original")

    def test_explicit_formatter_remains_available(self):
        result, commands, marker = self.run_recipe("fmt")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(commands, [["cargo", "fmt", "--all"]])
        self.assertEqual(marker, "rewritten")

    def test_lint_checks_test_targets(self):
        result, commands, _ = self.run_recipe("lint")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(commands, [CI_COMMANDS[1]])

    def test_ignored_mcp_binary_e2e_requires_an_explicit_recipe(self):
        result, commands, marker = self.run_recipe("test-mcp-e2e")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(commands, [[
            "cargo", "test", "-p", "mcp-server", "--test", "mcp_binary_e2e",
            "--", "--ignored", "--nocapture",
        ]])
        self.assertEqual(marker, "original")
        self.assertFalse(any("--ignored" in command for command in CI_COMMANDS))


if __name__ == "__main__":
    unittest.main()
