#!/usr/bin/env python3
"""Process-level regression tests for the audit replay CLI."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("audit_replay.py")


class TestAuditReplayCli(unittest.TestCase):
    def run_replay(self, body: str) -> subprocess.CompletedProcess:
        with tempfile.TemporaryDirectory() as directory:
            audit_path = Path(directory) / "audit.ndjson"
            audit_path.write_text(body, encoding="utf-8")
            return subprocess.run(
                [sys.executable, str(SCRIPT), str(audit_path), "--format", "json"],
                capture_output=True,
                text=True,
                timeout=10,
            )

    def test_valid_ndjson_emits_json_report(self):
        event = {
            "type": "TOOL_CALL",
            "tool_name": "get_state",
            "args": {},
            "timestamp": 123,
        }
        result = self.run_replay(json.dumps(event) + "\n")

        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["total_events"], 1)
        self.assertEqual(report["tool_calls"][0]["tool_name"], "get_state")
        self.assertEqual(report["tool_calls"][0]["timestamp"], 123)

    def test_invalid_ndjson_exits_nonzero_without_report(self):
        result = self.run_replay("{invalid JSON}\n")

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("line 1", result.stderr)


if __name__ == "__main__":
    unittest.main()
