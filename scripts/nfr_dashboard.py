#!/usr/bin/env python3

import argparse
import json
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


def _evaluate_thresholds(values: dict[str, Any], thresholds: dict[str, Any]) -> tuple[bool, list[str]]:
    checks: list[str] = []
    all_passed = True

    for key, expected in sorted(thresholds.items()):
        if key.endswith("_max"):
            metric_key = key[: -len("_max")]
            actual = values.get(metric_key)
            if actual is None:
                checks.append(f"{metric_key} missing (expected <= {_format_number(expected)})")
                all_passed = False
                continue
            passed = actual <= expected
            checks.append(
                f"{metric_key}<={_format_number(expected)} ({_format_number(actual)}) {'PASS' if passed else 'FAIL'}"
            )
            all_passed = all_passed and passed
            continue

        if key.endswith("_min"):
            metric_key = key[: -len("_min")]
            actual = values.get(metric_key)
            if actual is None:
                checks.append(f"{metric_key} missing (expected >= {_format_number(expected)})")
                all_passed = False
                continue
            passed = actual >= expected
            checks.append(
                f"{metric_key}>={_format_number(expected)} ({_format_number(actual)}) {'PASS' if passed else 'FAIL'}"
            )
            all_passed = all_passed and passed
            continue

    return all_passed, checks


def _collect_metric_files(metrics_dir: Path) -> list[Path]:
    return sorted(metrics_dir.glob("*.json"))


def _load_metric(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as fp:
        return json.load(fp)


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
        raise SystemExit(f"no metrics files found in {metrics_dir}")

    rows: list[dict[str, Any]] = []
    overall_passed = True

    for metric_file in metric_files:
        metric = _load_metric(metric_file)
        metric_id = metric.get("metric_id", metric_file.stem)
        mode = metric.get("mode", "unknown")
        values = metric.get("values", {})
        thresholds = metric.get("thresholds", {})
        display_order = metric.get("display", [])

        passed, checks = _evaluate_thresholds(values, thresholds)
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
