#!/usr/bin/env python3
"""Unit tests for nfr_trend.py using stdlib unittest."""

import json
import sys
import tempfile
import unittest
from pathlib import Path

# Allow importing nfr_trend from the same scripts/ directory.
sys.path.insert(0, str(Path(__file__).parent))

import nfr_trend


def _write_json(directory: Path, filename: str, data: dict) -> Path:
    path = directory / filename
    path.write_text(json.dumps(data), encoding="utf-8")
    return path


_LATENCY_METRIC = {
    "metric_id": "nfr-latency",
    "mode": "short",
    "values": {
        "trials": 20,
        "avg_ms": 50.0,
        "p95_ms": 130.0,
        "p99_ms": 170.0,
        "max_ms": 220.0,
    },
    "thresholds": {
        "p95_ms_max": 260.0,
        "p99_ms_max": 340.0,
    },
    "display": ["trials", "avg_ms", "p95_ms", "p99_ms", "max_ms"],
}

_BANDWIDTH_METRIC = {
    "metric_id": "nfr-bandwidth",
    "mode": "short",
    "values": {
        "trials": 5,
        "reduction_avg_pct": 97.5,
        "reduction_min_pct": 96.0,
    },
    "thresholds": {
        "reduction_min_pct_min": 95.0,
    },
    "display": ["trials", "reduction_avg_pct", "reduction_min_pct"],
}


class TestNfrTrend(unittest.TestCase):

    def _run(
        self,
        metrics: dict,
        baseline: dict | None,
        warn_pct: float = 10.0,
        fail_pct: float = 25.0,
    ) -> tuple[int, str]:
        """Write metric/baseline JSON files, invoke _analyse_metric, and return
        (exit_code, rendered_report)."""
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            metrics_dir = tmp / "metrics"
            metrics_dir.mkdir()
            _write_json(metrics_dir, "nfr-latency.json", metrics)

            baseline_obj: dict | None = None
            if baseline is not None:
                baseline_dir = tmp / "baseline"
                baseline_dir.mkdir()
                _write_json(baseline_dir, "nfr-latency.json", baseline)
                baseline_obj = baseline

            metric_id = metrics.get("metric_id", "nfr-latency")
            rows, failed = nfr_trend._analyse_metric(
                metrics, baseline_obj, warn_pct, fail_pct, metric_id
            )
            has_baseline = baseline_obj is not None
            report = nfr_trend._render_trend(rows, has_baseline)
            return (1 if failed else 0, report)

    def test_no_degradation_exits_zero(self) -> None:
        """Current values identical to baseline: no regression."""
        code, _ = self._run(_LATENCY_METRIC, _LATENCY_METRIC)
        self.assertEqual(code, 0)

    def test_warn_degradation_flagged(self) -> None:
        """15% degradation on a _max key exceeds warn (10%) but not fail (25%)."""
        current = {
            **_LATENCY_METRIC,
            "values": {
                **_LATENCY_METRIC["values"],
                "p95_ms": 130.0 * 1.15,  # +15%
            },
        }
        code, report = self._run(current, _LATENCY_METRIC)
        self.assertEqual(code, 0, "warn-only regression should not exit 1")
        self.assertIn("⚠", report)

    def test_fail_degradation_exits_nonzero(self) -> None:
        """30% degradation on a _max key exceeds fail threshold (25%)."""
        current = {
            **_LATENCY_METRIC,
            "values": {
                **_LATENCY_METRIC["values"],
                "p95_ms": 130.0 * 1.30,  # +30%
            },
        }
        code, report = self._run(current, _LATENCY_METRIC)
        self.assertNotEqual(code, 0, "fail regression should exit non-zero")
        self.assertIn("✗", report)

    def test_min_threshold_degradation(self) -> None:
        """reduction_min_pct dropping from 97% to 70% is a _min regression."""
        baseline = {
            **_BANDWIDTH_METRIC,
            "values": {**_BANDWIDTH_METRIC["values"], "reduction_min_pct": 97.0},
        }
        current = {
            **_BANDWIDTH_METRIC,
            "values": {**_BANDWIDTH_METRIC["values"], "reduction_min_pct": 70.0},
        }
        # Degradation = (97 - 70) / 97 * 100 ≈ 27.8% → exceeds fail_pct=25
        code, report = self._run(current, baseline)
        self.assertNotEqual(code, 0)
        self.assertIn("✗", report)

    def test_missing_baseline_exits_zero(self) -> None:
        """When no baseline is provided, script shows current values and exits 0."""
        code, report = self._run(_LATENCY_METRIC, baseline=None)
        self.assertEqual(code, 0)
        self.assertIn("No baseline directory provided", report)

    def test_non_threshold_keys_skipped(self) -> None:
        """'trials' and 'sessions_target' must not appear in comparison rows."""
        baseline = {
            "metric_id": "nfr-capacity-minimal",
            "mode": "short",
            "values": {
                "trials": 2,
                "sessions_target": 25,
                "success_rate_min": 1.0,
            },
            "thresholds": {"success_rate_min_min": 1.0},
            "display": [],
        }
        current = {
            **baseline,
            "values": {
                "trials": 999,          # huge change — should be ignored
                "sessions_target": 999, # huge change — should be ignored
                "success_rate_min": 1.0,
            },
        }
        rows, failed = nfr_trend._analyse_metric(
            current, baseline, 10.0, 25.0, "nfr-capacity-minimal"
        )
        row_keys = [r["key"] for r in rows]
        self.assertNotIn("trials", row_keys, "'trials' must be skipped")
        self.assertNotIn("sessions_target", row_keys, "'sessions_target' must be skipped")
        self.assertEqual(failed, False)

    def test_baseline_soft_thresholds_detected(self) -> None:
        """Baseline-declared thresholds track metrics the Rust tests don't assert on.

        capture_p95_ms has no threshold in the current metric file (as produced
        by the Rust test), but the baseline file declares capture_p95_ms_max so
        the trend tool should still detect a 30% regression.
        """
        baseline = {
            "metric_id": "nfr-capacity-minimal",
            "mode": "short",
            "values": {
                "capture_p95_ms": 200.0,
                "success_rate_min": 1.0,
            },
            # Soft threshold declared only in the baseline file.
            "thresholds": {"success_rate_min_min": 1.0, "capture_p95_ms_max": 200.0},
            "display": [],
        }
        # current metric file as Rust test would write it — no capture_p95_ms threshold
        current = {
            "metric_id": "nfr-capacity-minimal",
            "mode": "short",
            "values": {
                "capture_p95_ms": 200.0 * 1.30,  # 30% worse
                "success_rate_min": 1.0,
            },
            "thresholds": {"success_rate_min_min": 1.0},  # no capture threshold here
            "display": [],
        }
        rows, failed = nfr_trend._analyse_metric(
            current, baseline, 10.0, 25.0, "nfr-capacity-minimal"
        )
        row_keys = [r["key"] for r in rows]
        self.assertIn("capture_p95_ms", row_keys, "soft threshold key must be compared")
        self.assertTrue(failed, "30% regression on soft threshold must set failed=True")


if __name__ == "__main__":
    unittest.main()
