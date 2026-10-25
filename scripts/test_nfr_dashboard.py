#!/usr/bin/env python3
"""NFR dashboard contract and CLI regression tests (ISSUE-383)."""

import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import nfr_dashboard


FIXTURES = json.loads(
    (Path(__file__).parent / "fixtures/nfr_dashboard_metrics.json").read_text()
)
SCRIPT = Path(__file__).with_name("nfr_dashboard.py")


class TestThresholds(unittest.TestCase):
    def test_all_current_producer_shapes_pass(self):
        for metric in FIXTURES:
            with self.subTest(metric=metric["metric_id"]):
                passed, checks = nfr_dashboard._evaluate_thresholds(
                    metric["values"], metric["thresholds"]
                )
                self.assertTrue(passed)
                self.assertEqual(len(checks), len(metric["thresholds"]))

    def test_comparison_removes_only_the_final_suffix(self):
        passed, checks = nfr_dashboard._evaluate_thresholds(
            {"success_rate_min": 0.5}, {"success_rate_min_min": 1.0}
        )
        self.assertFalse(passed)
        self.assertIn("success_rate_min>=", checks[0])

    def test_empty_unknown_and_missing_threshold_inputs_are_errors(self):
        cases = [({}, {}), ({"p95_ms": 99999}, {"p95_ms_limit": 100}),
                 ({"p95_ms": 1}, {"_max": 2}), ({}, {"p95_ms_max": 100})]
        for values, thresholds in cases:
            with self.subTest(values=values, thresholds=thresholds):
                with self.assertRaises(ValueError):
                    nfr_dashboard._evaluate_thresholds(values, thresholds)

    def test_both_comparison_operands_must_be_finite_nonbool_numbers(self):
        for bad in [True, False, "10", None, float("nan"), float("inf"), -float("inf")]:
            for values, thresholds in [({"p95_ms": bad}, {"p95_ms_max": 100}),
                                       ({"p95_ms": 1}, {"p95_ms_max": bad})]:
                with self.subTest(bad=bad, values=values):
                    with self.assertRaises(ValueError):
                        nfr_dashboard._evaluate_thresholds(values, thresholds)


class TestDashboardCli(unittest.TestCase):
    def run_cli(self, metrics):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, metric in enumerate(metrics):
                (root / f"metric-{index}.json").write_text(json.dumps(metric))
            output = root / "dashboard.md"
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--metrics-dir", str(root), "--output", str(output)],
                capture_output=True, text=True, timeout=10,
            )
            return result, output.read_text() if output.exists() else None

    def test_real_producer_shapes_and_optional_display(self):
        metrics = copy.deepcopy(FIXTURES)
        del metrics[0]["display"]
        result, dashboard = self.run_cli(metrics)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Overall: PASS", dashboard)
        for metric in metrics:
            self.assertIn(metric["metric_id"], dashboard)

    def test_regression_still_writes_failure_dashboard(self):
        metric = copy.deepcopy(FIXTURES[1])
        metric["values"]["p95_ms"] = 99999
        result, dashboard = self.run_cli([metric])
        self.assertEqual(result.returncode, 1)
        self.assertIn("Overall: FAIL", dashboard)

    def test_invalid_metrics_exit_nonzero_without_pass_artifact(self):
        cases = [{}, [], None]
        for field in ["metric_id", "mode", "values", "thresholds"]:
            metric = copy.deepcopy(FIXTURES[1])
            del metric[field]
            cases.append(metric)
        for field, bad in [("metric_id", ""), ("metric_id", 1), ("mode", "smoke"),
                           ("mode", []), ("values", []), ("values", {}),
                           ("thresholds", []), ("thresholds", {}), ("display", "p95_ms"),
                           ("display", [None]), ("display", ["missing"])]:
            metric = copy.deepcopy(FIXTURES[1])
            metric[field] = bad
            cases.append(metric)
        for values, thresholds in [({"p95_ms": 99999}, {"p95_ms_limit": 100}),
                                   ({}, {"p95_ms_max": 100}),
                                   ({"p95_ms": True}, {"p95_ms_max": 100}),
                                   ({"p95_ms": 1}, {"p95_ms_max": float("nan")}),
                                   ({"p95_ms": 1, "trials": "twenty"}, {"p95_ms_max": 100}),
                                   ({"p95_ms": float("inf")}, {"p95_ms_max": 100})]:
            cases.append({"metric_id": "test", "mode": "short", "values": values, "thresholds": thresholds})
        for metric in cases:
            with self.subTest(metric=metric):
                result, dashboard = self.run_cli([metric])
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Overall: FAIL", dashboard)
                self.assertNotIn("PASS", dashboard)
                self.assertTrue(result.stderr.strip())
                self.assertNotIn("Traceback", result.stderr)

    def test_no_metrics_fails(self):
        result, dashboard = self.run_cli([])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Overall: FAIL", dashboard)

    def test_stale_pass_is_replaced_when_input_is_invalid_or_absent(self):
        for scenario in ("invalid_schema", "invalid_json", "empty", "missing_directory"):
            with self.subTest(scenario=scenario), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                metrics = root / "metrics"
                if scenario != "missing_directory":
                    metrics.mkdir()
                if scenario in ("invalid_schema", "invalid_json"):
                    (metrics / "metric.json").write_text(
                        "{}" if scenario == "invalid_schema" else "{"
                    )
                output = root / "dashboard.md"
                output.write_text("# NFR Regression Dashboard\n\nOverall: PASS\n")
                result = subprocess.run(
                    [sys.executable, str(SCRIPT), "--metrics-dir", str(metrics),
                     "--output", str(output)],
                    capture_output=True, text=True, timeout=10,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Overall: FAIL", output.read_text())
                self.assertNotIn("PASS", output.read_text())


if __name__ == "__main__":
    unittest.main()
