#!/usr/bin/env python3

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


# Keys that represent counts or targets rather than performance measurements.
# These are intentionally excluded from regression comparison.
_SKIP_KEYS = {"trials", "sessions_target"}


def _format_number(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return f"{value:.3f}"
    return str(value)


def _collect_metric_files(metrics_dir: Path) -> list[Path]:
    return sorted(metrics_dir.glob("*.json"))


def _load_json(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as fp:
        return json.load(fp)


def _degradation_pct(
    key: str,
    current: float,
    baseline: float,
    thresholds: dict[str, Any],
) -> float | None:
    """Return degradation percentage for a value, or None if not threshold-tracked.

    Positive return value means performance got worse.
    Negative return value means performance improved.
    None means the key is not referenced in thresholds.
    """
    if baseline == 0.0:
        return None

    max_key = key + "_max"
    min_key = key + "_min"

    if max_key in thresholds:
        # Lower is better: degradation = how much worse current is vs baseline
        return (current - baseline) / abs(baseline) * 100.0

    if min_key in thresholds:
        # Higher is better: degradation = how much lower current is vs baseline
        return (baseline - current) / abs(baseline) * 100.0

    return None


def _status_symbol(degradation: float, warn_pct: float, fail_pct: float) -> str:
    if degradation >= fail_pct:
        return "✗"  # ✗
    if degradation >= warn_pct:
        return "⚠"  # ⚠
    return "✓"  # ✓


def _render_trend(
    rows: list[dict[str, Any]],
    has_baseline: bool,
) -> str:
    timestamp = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
    lines = [
        "# NFR Trend Analysis",
        "",
        "- Generated: " + timestamp,
        "",
    ]

    if not has_baseline:
        lines += [
            "_No baseline directory provided — showing current values only._",
            "",
            "| Metric | Mode | Key | Current |",
            "| --- | --- | --- | --- |",
        ]
        for row in rows:
            lines.append(
                "| {metric} | {mode} | {key} | {current} |".format(
                    metric=row["metric"],
                    mode=row["mode"],
                    key=row["key"],
                    current=row["current"],
                )
            )
    else:
        lines += [
            "| Metric | Mode | Key | Baseline | Current | Delta | Status |",
            "| --- | --- | --- | --- | --- | --- | --- |",
        ]
        for row in rows:
            lines.append(
                "| {metric} | {mode} | {key} | {baseline} | {current} | {delta} | {status} |".format(
                    metric=row["metric"],
                    mode=row["mode"],
                    key=row["key"],
                    baseline=row["baseline"],
                    current=row["current"],
                    delta=row["delta"],
                    status=row["status"],
                )
            )

    lines.append("")
    return "\n".join(lines)


def _analyse_metric(
    metric: dict[str, Any],
    baseline: dict[str, Any] | None,
    warn_pct: float,
    fail_pct: float,
    metric_id: str,
) -> tuple[list[dict[str, Any]], bool]:
    """Return (rows, any_failed) for a single metric file."""
    mode = metric.get("mode", "unknown")
    values = metric.get("values", {})
    thresholds = metric.get("thresholds", {})

    rows: list[dict[str, Any]] = []
    any_failed = False

    if baseline is None:
        # No baseline: emit current values for threshold-tracked keys only
        for key, val in sorted(values.items()):
            if key in _SKIP_KEYS:
                continue
            if not isinstance(val, (int, float)) or isinstance(val, bool):
                continue
            max_key = key + "_max"
            min_key = key + "_min"
            if max_key not in thresholds and min_key not in thresholds:
                continue
            rows.append(
                {
                    "metric": metric_id,
                    "mode": mode,
                    "key": key,
                    "current": _format_number(val),
                    "baseline": "—",
                    "delta": "—",
                    "status": "—",
                }
            )
        return rows, False

    baseline_values = baseline.get("values", {})
    # Union of hard (test-enforced) and soft (baseline-declared) thresholds so
    # that baseline files can declare trend-only thresholds for metrics that the
    # Rust tests do not assert on (e.g. capacity duration percentiles).
    merged_thresholds = {**metric.get("thresholds", {}), **baseline.get("thresholds", {})}

    for key in sorted(values.keys()):
        if key in _SKIP_KEYS:
            continue
        val = values[key]
        if not isinstance(val, (int, float)) or isinstance(val, bool):
            continue

        base_val = baseline_values.get(key)
        if base_val is None or not isinstance(base_val, (int, float)) or isinstance(base_val, bool):
            continue

        deg = _degradation_pct(key, float(val), float(base_val), merged_thresholds)
        if deg is None:
            continue

        symbol = _status_symbol(deg, warn_pct, fail_pct)
        if deg >= fail_pct:
            any_failed = True

        delta_str = ("+" if deg > 0 else "") + f"{deg:.1f}%"

        rows.append(
            {
                "metric": metric_id,
                "mode": mode,
                "key": key,
                "baseline": _format_number(float(base_val)),
                "current": _format_number(float(val)),
                "delta": delta_str,
                "status": symbol,
            }
        )

    return rows, any_failed


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Compare NFR benchmark metrics against a baseline and detect regressions"
    )
    parser.add_argument(
        "--metrics-dir",
        default="target/nfr-metrics",
        help="Directory containing current *.json metric files",
    )
    parser.add_argument(
        "--baseline-dir",
        default=None,
        help="Directory containing baseline *.json metric files (optional)",
    )
    parser.add_argument(
        "--output",
        default="target/nfr-trend.md",
        help="Output markdown path",
    )
    parser.add_argument(
        "--warn-pct",
        type=float,
        default=10.0,
        help="Degradation percentage that triggers a warning (default: 10)",
    )
    parser.add_argument(
        "--fail-pct",
        type=float,
        default=25.0,
        help="Degradation percentage that triggers a failure (default: 25)",
    )
    args = parser.parse_args()

    metrics_dir = Path(args.metrics_dir)
    baseline_dir = Path(args.baseline_dir) if args.baseline_dir else None
    output_path = Path(args.output)

    metric_files = _collect_metric_files(metrics_dir)
    if not metric_files:
        raise SystemExit(f"no metric files found in {metrics_dir}")

    has_baseline = baseline_dir is not None and baseline_dir.is_dir()

    all_rows: list[dict[str, Any]] = []
    overall_failed = False

    for metric_file in metric_files:
        metric = _load_json(metric_file)
        metric_id = metric.get("metric_id", metric_file.stem)

        baseline: dict[str, Any] | None = None
        if has_baseline:
            baseline_file = baseline_dir / metric_file.name  # type: ignore[operator]
            if baseline_file.exists():
                baseline = _load_json(baseline_file)

        rows, any_failed = _analyse_metric(
            metric, baseline, args.warn_pct, args.fail_pct, metric_id
        )
        all_rows.extend(rows)
        overall_failed = overall_failed or any_failed

    report = _render_trend(all_rows, has_baseline)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(report, encoding="utf-8")
    print(report)

    return 1 if overall_failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
