"""Evaluation report boundary and CLI tests (ISSUE-382)."""

import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import evaluation_dashboard as dashboard


CRATES = (
    "core-runtime", "mcp-server", "skills-engine", "plugin-host",
    "marketplace", "hitl-bridge", "bench",
)


def report(crate="core-runtime"):
    # Same serialized fields as test_bench_support::EvaluationReport.
    return {
        "crate_name": crate,
        "suite_name": "comprehensive_evaluation",
        "mode": "smoke",
        "summary": {"passed": 1, "failed": 0, "total": 1},
        "scenarios": [{
            "scenario_id": "capture", "feature_area": "sre",
            "status": "passed", "duration_ms": 1.5, "details": {"nodes": 1},
        }],
    }


def complete_reports():
    return [report(crate) for crate in CRATES]


class TestReportValidation(unittest.TestCase):
    def test_complete_smoke_and_full_reports_pass(self):
        for mode in ("smoke", "full"):
            reports = complete_reports()
            for item in reports:
                item["mode"] = mode
            self.assertIn("Overall Status: PASS", dashboard.render_dashboard(reports))

    def test_real_rust_producer_fixture_is_accepted_as_partial(self):
        fixture = Path(__file__).parent / "fixtures/evaluation/core-runtime.json"
        reports = [json.loads(fixture.read_text(encoding="utf-8"))]
        rendered = dashboard.render_dashboard(reports, allow_partial=True)
        self.assertIn("Overall Status: PARTIAL", rendered)
        self.assertNotIn("Overall Status: PASS", rendered)

    def test_missing_browser_suites_and_duplicate_suites_are_rejected(self):
        for reports in (complete_reports()[2:], complete_reports() + [report()]):
            with self.subTest(reports=len(reports)), self.assertRaises(ValueError):
                dashboard.render_dashboard(reports)

    def test_invalid_report_shapes_are_rejected(self):
        bad = [None, [], {}, {**report(), "scenarios": []}]
        for field in report():
            item = report()
            del item[field]
            bad.append(item)
        for field, value in (("summary", []), ("scenarios", {}), ("mode", "short"),
                             ("crate_name", 1), ("suite_name", "")):
            bad.append({**report(), field: value})
        for item in bad:
            with self.subTest(item=item), self.assertRaises(ValueError):
                dashboard.render_dashboard([item], allow_partial=True)

    def test_counts_and_scenario_records_are_validated(self):
        bad = []
        for summary in ({"passed": 1, "failed": 0},
                        {"passed": True, "failed": 0, "total": 1},
                        {"passed": -1, "failed": 0, "total": 1},
                        {"passed": 0, "failed": 0, "total": 1}):
            bad.append({**report(), "summary": summary})
        for key, value in (("status", "skipped"), ("duration_ms", float("nan")),
                           ("duration_ms", float("inf")), ("duration_ms", True),
                           ("duration_ms", -1), ("scenario_id", ""),
                           ("feature_area", []), ("error", {}),
                           ("error", "actual failure")):
            item = report()
            item["scenarios"][0][key] = value
            bad.append(item)
        for key in report()["scenarios"][0]:
            item = report()
            del item["scenarios"][0][key]
            bad.append(item)
        for item in bad:
            with self.subTest(item=item), self.assertRaises(ValueError):
                dashboard.render_dashboard([item], allow_partial=True)

    def test_mixed_modes_and_duplicate_scenario_ids_are_rejected(self):
        mixed = complete_reports()
        mixed[-1]["mode"] = "full"
        duplicate = report()
        duplicate["scenarios"].append(copy.deepcopy(duplicate["scenarios"][0]))
        duplicate["summary"] = {"passed": 2, "failed": 0, "total": 2}
        for reports in (mixed, [duplicate]):
            with self.assertRaises(ValueError):
                dashboard.render_dashboard(reports, allow_partial=True)

    def test_failed_scenarios_cannot_be_hidden_by_summary(self):
        failed = report()
        failed["scenarios"][0]["status"] = "failed"
        failed["scenarios"][0]["error"] = "capture failed"
        with self.assertRaises(ValueError):
            dashboard.render_dashboard([failed], allow_partial=True)
        failed["summary"] = {"passed": 0, "failed": 1, "total": 1}
        rendered = dashboard.render_dashboard([failed], allow_partial=True)
        self.assertIn("Overall Status: FAIL", rendered)


class TestDashboardCli(unittest.TestCase):
    def run_cli(self, reports, *args):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            for index, item in enumerate(reports):
                (directory / f"{index}.json").write_text(json.dumps(item), encoding="utf-8")
            output = directory / "dashboard.md"
            result = subprocess.run(
                [sys.executable, str(Path(dashboard.__file__)),
                 "--input-dir", temp, "--output", str(output), *args],
                capture_output=True, text=True, timeout=10,
            )
            artifact = output.read_text(encoding="utf-8") if output.exists() else None
            return result, artifact

    def test_complete_report_cli_succeeds_and_writes_dashboard(self):
        result, artifact = self.run_cli(complete_reports())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Overall Status: PASS", artifact)

    def test_passed_scenario_with_error_is_rejected(self):
        reports = complete_reports()
        reports[0]["scenarios"][0]["error"] = "actual failure"
        for data, args in ((reports, ()), (reports[:1], ("--allow-partial",))):
            result, artifact = self.run_cli(data, *args)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Overall Status: FAIL", artifact)

    def test_cli_rejects_empty_missing_and_inconsistent_reports(self):
        inconsistent = report()
        inconsistent["scenarios"][0]["status"] = "failed"
        for reports in ([], [{}], [report()], complete_reports()[2:], [inconsistent]):
            with self.subTest(reports=len(reports)):
                result, artifact = self.run_cli(reports)
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue(result.stderr)
                self.assertIn("Overall Status: FAIL", artifact)

    def test_partial_only_permits_missing_suites(self):
        result, artifact = self.run_cli([report()], "--allow-partial")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Overall Status: PARTIAL", artifact)
        mixed = complete_reports()
        mixed[-1]["mode"] = "full"
        for reports in ([{}], [report(), report()], mixed):
            result, artifact = self.run_cli(reports, "--allow-partial")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Overall Status: FAIL", artifact)

    def test_failed_report_cli_exits_nonzero_with_failure_artifact(self):
        reports = complete_reports()
        reports[0]["scenarios"][0].update(status="failed", error="capture failed")
        reports[0]["summary"] = {"passed": 0, "failed": 1, "total": 1}
        for data, args in ((reports, ()), (reports[:1], ("--allow-partial",))):
            result, artifact = self.run_cli(data, *args)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Overall Status: FAIL", artifact)

    def test_invalid_input_replaces_stale_pass_artifact(self):
        for case in ("malformed", "empty", "missing-directory", "missing-suites"):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as temp:
                directory = Path(temp)
                reports = directory / "reports"
                reports.mkdir()
                if case == "malformed":
                    (reports / "bad.json").write_text("{", encoding="utf-8")
                elif case == "missing-directory":
                    reports = directory / "absent"
                elif case == "missing-suites":
                    (reports / "one.json").write_text(json.dumps(report()), encoding="utf-8")
                output = directory / "dashboard.md"
                output.write_text("- Overall Status: PASS\n", encoding="utf-8")
                result = subprocess.run(
                    [sys.executable, str(Path(dashboard.__file__)),
                     "--input-dir", str(reports), "--output", str(output)],
                    capture_output=True, text=True, timeout=10,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                artifact = output.read_text(encoding="utf-8")
                self.assertIn("Overall Status: FAIL", artifact)
                self.assertNotIn("Overall Status: PASS", artifact)


if __name__ == "__main__":
    unittest.main()
