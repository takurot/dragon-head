#!/usr/bin/env python3

import argparse
import json
import math
import sys
from pathlib import Path


REQUIRED_SUITES = {
    (crate, "comprehensive_evaluation")
    for crate in (
        "core-runtime", "mcp-server", "skills-engine", "plugin-host",
        "marketplace", "hitl-bridge", "bench",
    )
}


def _require_text(value: object, field: str) -> None:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{field} must be a nonempty string")


def validate_reports(reports: list[dict], allow_partial: bool = False) -> set[tuple[str, str]]:
    if not reports:
        raise ValueError("No evaluation reports found")
    identities = set()
    modes = set()
    for report in reports:
        if not isinstance(report, dict):
            raise ValueError("evaluation report must be an object")
        for field in ("crate_name", "suite_name", "mode"):
            _require_text(report.get(field), field)
        identity = (report["crate_name"], report["suite_name"])
        if identity in identities:
            raise ValueError(f"duplicate evaluation suite: {identity}")
        identities.add(identity)
        if report["mode"] not in ("smoke", "full"):
            raise ValueError("evaluation mode must be smoke or full")
        modes.add(report["mode"])
        scenarios = report.get("scenarios")
        if not isinstance(scenarios, list) or not scenarios:
            raise ValueError("scenarios must be a nonempty array")
        counts = {"total": len(scenarios), "passed": 0, "failed": 0}
        scenario_ids = set()
        for scenario in scenarios:
            if not isinstance(scenario, dict):
                raise ValueError("scenario must be an object")
            for field in ("scenario_id", "feature_area", "status"):
                _require_text(scenario.get(field), field)
            if scenario["scenario_id"] in scenario_ids:
                raise ValueError("duplicate scenario_id in evaluation suite")
            scenario_ids.add(scenario["scenario_id"])
            if scenario["status"] not in ("passed", "failed"):
                raise ValueError("scenario status must be passed or failed")
            counts[scenario["status"]] += 1
            duration = scenario.get("duration_ms")
            try:
                valid_duration = (type(duration) in (int, float)
                                  and math.isfinite(duration) and duration >= 0)
            except OverflowError:
                valid_duration = False
            if not valid_duration:
                raise ValueError("duration_ms must be a finite nonnegative number")
            if "details" not in scenario:
                raise ValueError("scenario is missing details")
            if scenario.get("error") is not None and not isinstance(scenario["error"], str):
                raise ValueError("scenario error must be a string or null")
        summary = report.get("summary")
        if not isinstance(summary, dict):
            raise ValueError("summary must be an object")
        for field, expected in counts.items():
            if type(summary.get(field)) is not int or summary[field] != expected:
                raise ValueError(f"summary.{field} does not match scenario records")
    if len(modes) != 1:
        raise ValueError("evaluation reports must use the same mode")
    missing = REQUIRED_SUITES - identities
    if missing and not allow_partial:
        names = ", ".join(f"{crate}/{suite}" for crate, suite in sorted(missing))
        raise ValueError(f"missing required evaluation suites: {names}")
    return missing


def load_reports(input_dir: Path) -> list[dict]:
    reports = []
    for path in sorted(input_dir.glob("*.json")):
        with path.open("r", encoding="utf-8") as handle:
            reports.append(json.load(handle))
    return reports


def render_dashboard(reports: list[dict], allow_partial: bool = False) -> str:
    missing = validate_reports(reports, allow_partial)
    overall_failed = sum(report["summary"]["failed"] for report in reports)
    status = "FAIL" if overall_failed else ("PARTIAL" if missing else "PASS")

    lines = [
        "# Dragon Head Evaluation Bench Dashboard",
        "",
        f"- Reports: {len(reports)}",
        f"- Overall Status: {status}",
        f"- Coverage: {'partial' if missing else 'complete'} ({len(REQUIRED_SUITES - missing)}/{len(REQUIRED_SUITES)} required suites)",
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
    parser.add_argument(
        "--allow-partial", action="store_true",
        help="Allow missing required suites, reported as PARTIAL rather than PASS",
    )
    args = parser.parse_args()

    input_dir = Path(args.input_dir)
    output_path = Path(args.output)
    try:
        reports = load_reports(input_dir)
        dashboard = render_dashboard(reports, args.allow_partial)
    except (OSError, ValueError) as error:
        print(f"Invalid evaluation reports in {input_dir}: {error}", file=sys.stderr)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(
            "# Dragon Head Evaluation Bench Dashboard\n\n"
            "- Overall Status: FAIL\n"
            "- Input validation failed; see stderr for details.\n",
            encoding="utf-8",
        )
        return 1

    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(dashboard, encoding="utf-8")
    print(dashboard)

    overall_failed = sum(report.get("summary", {}).get("failed", 0) for report in reports)
    return 0 if overall_failed == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
