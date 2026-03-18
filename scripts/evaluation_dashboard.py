#!/usr/bin/env python3

import argparse
import json
import sys
from pathlib import Path


def load_reports(input_dir: Path) -> list[dict]:
    reports = []
    for path in sorted(input_dir.glob("*.json")):
        with path.open("r", encoding="utf-8") as handle:
            reports.append(json.load(handle))
    return reports


def render_dashboard(reports: list[dict]) -> str:
    overall_failed = sum(report.get("summary", {}).get("failed", 0) for report in reports)
    overall_passed = overall_failed == 0 and bool(reports)

    lines = [
        "# Dragon Head Evaluation Bench Dashboard",
        "",
        f"- Reports: {len(reports)}",
        f"- Overall Status: {'PASS' if overall_passed else 'FAIL'}",
        "",
        "| Crate | Suite | Mode | Passed | Failed | Total |",
        "| :--- | :--- | :--- | ---: | ---: | ---: |",
    ]

    for report in reports:
        summary = report.get("summary", {})
        lines.append(
            "| {crate} | {suite} | {mode} | {passed} | {failed} | {total} |".format(
                crate=report.get("crate_name", "unknown"),
                suite=report.get("suite_name", "unknown"),
                mode=report.get("mode", "unknown"),
                passed=summary.get("passed", 0),
                failed=summary.get("failed", 0),
                total=summary.get("total", 0),
            )
        )

    for report in reports:
        lines.extend(
            [
                "",
                f"## {report.get('crate_name', 'unknown')} / {report.get('suite_name', 'unknown')}",
                "",
                "| Scenario | Feature | Status | Duration (ms) | Details |",
                "| :--- | :--- | :--- | ---: | :--- |",
            ]
        )
        for scenario in report.get("scenarios", []):
            details = scenario.get("error") or json.dumps(
                scenario.get("details", {}), ensure_ascii=False, sort_keys=True
            )
            details = details.replace("|", "\\|")
            if len(details) > 120:
                details = details[:117] + "..."
            lines.append(
                "| {scenario_id} | {feature_area} | {status} | {duration_ms:.2f} | {details} |".format(
                    scenario_id=scenario.get("scenario_id", "unknown"),
                    feature_area=scenario.get("feature_area", "unknown"),
                    status=scenario.get("status", "unknown"),
                    duration_ms=float(scenario.get("duration_ms", 0.0)),
                    details=details,
                )
            )

    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Generate evaluation bench dashboard from crate reports"
    )
    parser.add_argument("--input-dir", default="target/evaluation-bench")
    parser.add_argument("--output", default="target/evaluation-dashboard.md")
    args = parser.parse_args()

    input_dir = Path(args.input_dir)
    output_path = Path(args.output)
    reports = load_reports(input_dir)
    if not reports:
        print(f"No evaluation reports found in {input_dir}", file=sys.stderr)
        return 1

    dashboard = render_dashboard(reports)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(dashboard, encoding="utf-8")
    print(dashboard)

    overall_failed = sum(report.get("summary", {}).get("failed", 0) for report in reports)
    return 0 if overall_failed == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
