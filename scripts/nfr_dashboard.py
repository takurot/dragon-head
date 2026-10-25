#!/usr/bin/env python3

import argparse
import json
import math
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def _format_number(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return f"{value:.3f}"
    return str(value)


def _validate_numbers(values: Any, field: str) -> None:
    if not isinstance(values, dict) or not values:
        raise ValueError(f"{field} must be a nonempty object")
    for key, value in values.items():
        if not isinstance(key, str) or not key:
            raise ValueError(f"{field} keys must be nonempty strings")
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError(f"{field}.{key} must be a finite number")
        if isinstance(value, float) and not math.isfinite(value):
            raise ValueError(f"{field}.{key} must be a finite number")


def _evaluate_thresholds(values: dict[str, Any], thresholds: dict[str, Any]) -> tuple[bool, list[str]]:
    _validate_numbers(values, "values")
    _validate_numbers(thresholds, "thresholds")
    checks: list[str] = []
    all_passed = True

    for key, expected in sorted(thresholds.items()):
        metric_key, _, direction = key.rpartition("_")
        if not metric_key or direction not in ("min", "max"):
            raise ValueError(f"thresholds.{key} must end in _min or _max after a metric name")
        if metric_key not in values:
            raise ValueError(f"values.{metric_key} is missing for threshold {key}")
        actual = values[metric_key]
        passed = actual <= expected if direction == "max" else actual >= expected
        comparison = "<=" if direction == "max" else ">="
        checks.append(
            f"{metric_key}{comparison}{_format_number(expected)} ({_format_number(actual)}) {'PASS' if passed else 'FAIL'}"
        )
        all_passed = all_passed and passed

    return all_passed, checks


def _collect_metric_files(metrics_dir: Path) -> list[Path]:
    return sorted(metrics_dir.glob("*.json"))


def _load_metric(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as fp:
        metric = json.load(fp)
    if not isinstance(metric, dict):
        raise ValueError("metric must be an object")
    if not isinstance(metric.get("metric_id"), str) or not metric["metric_id"].strip():
        raise ValueError("metric_id must be a nonempty string")
    if metric.get("mode") not in ("short", "full"):
        raise ValueError("mode must be short or full")
    for field in ("values", "thresholds"):
        if field not in metric:
            raise ValueError(f"{field} is required")
        _validate_numbers(metric[field], field)
    display = metric.get("display", [])
    if not isinstance(display, list) or any(
        not isinstance(key, str) or key not in metric["values"] for key in display
    ):
        raise ValueError("display must be a list of existing value names")
    return metric


def _render_dashboard(rows: list[dict[str, Any]], overall_passed: bool) -> str:
    timestamp = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
    lines = [
        "# NFR Regression Dashboard",
        "",
        f"- Generated: {timestamp}",
        "",
        "| Metric | Mode | Values | Threshold Checks | Status |",
        "| --- | --- | --- | --- | --- |",
    ]

    for row in rows:
        lines.append(
            "| {metric} | {mode} | {values} | {checks} | {status} |".format(
                metric=row["metric"],
                mode=row["mode"],
                values=row["values"],
                checks=row["checks"],
                status="PASS" if row["passed"] else "FAIL",
            )
        )

    lines.extend(
        [
            "",
            f"Overall: {'PASS' if overall_passed else 'FAIL'}",
            "",
        ]
    )

    return "\n".join(lines)


def _fail_input(output_path: Path, message: str) -> int:
    print(message, file=sys.stderr)
    # A previous successful dashboard may still be present and uploaded by an
    # always() artifact step. Replace it so failed validation cannot publish PASS.
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(
        _render_dashboard([], False) + "\nInput validation failed; see stderr for details.\n",
        encoding="utf-8",
    )
    return 1


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate NFR dashboard from benchmark metrics")
    parser.add_argument(
        "--metrics-dir",
        default="target/nfr-metrics",
        help="Directory containing *.json metric files",
    )
    parser.add_argument(
        "--output",
        default="target/nfr-dashboard.md",
        help="Output markdown path",
    )
    args = parser.parse_args()

    metrics_dir = Path(args.metrics_dir)
    output_path = Path(args.output)

    metric_files = _collect_metric_files(metrics_dir)
    if not metric_files:
        return _fail_input(output_path, f"no metrics files found in {metrics_dir}")

    rows: list[dict[str, Any]] = []
    overall_passed = True

    for metric_file in metric_files:
        try:
            metric = _load_metric(metric_file)
            passed, checks = _evaluate_thresholds(metric["values"], metric["thresholds"])
        except (OSError, ValueError) as error:
            return _fail_input(output_path, f"Invalid NFR metric {metric_file.name}: {error}")
        metric_id = metric["metric_id"]
        mode = metric["mode"]
        values = metric["values"]
        display_order = metric.get("display", [])

        overall_passed = overall_passed and passed

        if display_order:
            value_pairs = [
                f"{key}={_format_number(values[key])}"
                for key in display_order
                if key in values
            ]
        else:
            value_pairs = [f"{k}={_format_number(v)}" for k, v in sorted(values.items())]

        rows.append(
            {
                "metric": metric_id,
                "mode": mode,
                "values": "<br>".join(value_pairs) if value_pairs else "-",
                "checks": "<br>".join(checks) if checks else "-",
                "passed": passed,
            }
        )

    dashboard = _render_dashboard(rows, overall_passed)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(dashboard, encoding="utf-8")
    print(dashboard)

    return 0 if overall_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
