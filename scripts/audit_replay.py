#!/usr/bin/env python3
"""Audit replay/report tool for dragon-head structured audit logs.

Usage:
    python scripts/audit_replay.py <audit.ndjson> [--format markdown|json]
    python scripts/audit_replay.py <audit.ndjson> --out <report.md>

Reads sanitized audit event NDJSON, reconstructs the event sequence,
validates patch chains, and produces a report for CI artifacts or incident review.
"""

from __future__ import annotations

import argparse
import copy
import dataclasses
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional


# ---------------------------------------------------------------------------
# Data models
# ---------------------------------------------------------------------------

@dataclass
class StateChainEntry:
    state_hash: str
    timestamp: int
    kind: str  # "snapshot" | "patch"
    patch_applied: bool = True


@dataclass
class ToolCallEntry:
    tool_name: str
    timestamp: int
    has_redacted_args: bool


@dataclass
class PolicyDecisionEntry:
    rule_id: str
    action: str
    decision: str
    timestamp: int


@dataclass
class HitlEventEntry:
    event_type: str
    timestamp: int


@dataclass
class ReplayReport:
    total_events: int
    has_redacted_content: bool
    state_chain: list[StateChainEntry] = field(default_factory=list)
    tool_calls: list[ToolCallEntry] = field(default_factory=list)
    policy_decisions: list[PolicyDecisionEntry] = field(default_factory=list)
    hitl_events: list[HitlEventEntry] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Patch application (RFC 6902)
# ---------------------------------------------------------------------------

def _apply_json_patch(doc: Any, ops: list[dict]) -> Any:
    """Apply RFC 6902 JSON Patch operations. Raises ValueError on failure."""
    doc = copy.deepcopy(doc)
    for op in ops:
        operation = op.get("op")
        path = op.get("path", "")
        parts = _ptr_parts(path)

        if operation == "replace":
            _patch_set(doc, parts, op["value"])
        elif operation == "add":
            _patch_add(doc, parts, op["value"])
        elif operation == "remove":
            _patch_remove(doc, parts)
        elif operation == "test":
            actual = _patch_get(doc, parts)
            if actual != op.get("value"):
                raise ValueError(
                    f"test failed at {path!r}: expected {op.get('value')!r}, got {actual!r}"
                )
        elif operation == "copy":
            from_parts = _ptr_parts(op["from"])
            val = _patch_get(doc, from_parts)
            _patch_set(doc, parts, val)
        elif operation == "move":
            from_parts = _ptr_parts(op["from"])
            val = _patch_get(doc, from_parts)
            _patch_remove(doc, from_parts)
            _patch_set(doc, parts, val)
        else:
            raise ValueError(f"unsupported patch op: {operation!r}")
    return doc


def _ptr_parts(path: str) -> list[str]:
    """Split a JSON Pointer path into segments, decoding RFC 6902 escape sequences."""
    if not path:
        return []
    return [
        p.replace("~1", "/").replace("~0", "~")
        for p in path.split("/")
        if p != ""
    ]


def _resolve(doc: Any, parts: list[str]) -> tuple[Any, str]:
    """Walk to the parent of the target; return (parent, last_key)."""
    for part in parts[:-1]:
        if isinstance(doc, list):
            doc = doc[int(part)]
        else:
            doc = doc[part]
    return doc, parts[-1] if parts else ""


def _patch_get(doc: Any, parts: list[str]) -> Any:
    for part in parts:
        if isinstance(doc, list):
            doc = doc[int(part)]
        else:
            doc = doc[part]
    return doc


def _patch_set(doc: Any, parts: list[str], value: Any) -> None:
    if not parts:
        raise ValueError("cannot replace root")
    parent, key = _resolve(doc, parts)
    if isinstance(parent, list):
        parent[int(key)] = value
    else:
        parent[key] = value


def _patch_add(doc: Any, parts: list[str], value: Any) -> None:
    if not parts:
        raise ValueError("cannot add to root")
    parent, key = _resolve(doc, parts)
    if isinstance(parent, list):
        if key == "-":
            parent.append(value)
        else:
            parent.insert(int(key), value)
    else:
        parent[key] = value


def _patch_remove(doc: Any, parts: list[str]) -> None:
    if not parts:
        raise ValueError("cannot remove root")
    parent, key = _resolve(doc, parts)
    if isinstance(parent, list):
        del parent[int(key)]
    else:
        del parent[key]


# ---------------------------------------------------------------------------
# Redaction detection
# ---------------------------------------------------------------------------

def _contains_redaction(value: Any) -> bool:
    if isinstance(value, str):
        # "***" covers email/generic redaction; "****-****-****-" covers card numbers.
        return "***" in value or "****-****-****-" in value
    if isinstance(value, list):
        return any(_contains_redaction(v) for v in value)
    if isinstance(value, dict):
        return any(_contains_redaction(v) for v in value.values())
    return False


# ---------------------------------------------------------------------------
# Replay engine
# ---------------------------------------------------------------------------

def replay_events(events: list[dict]) -> ReplayReport:
    """Replay a list of parsed audit event dicts into a ReplayReport.

    Raises ValueError if a patch is encountered with no prior snapshot,
    or if patch application fails.
    """
    report = ReplayReport(total_events=len(events), has_redacted_content=False)
    current_snapshot: Optional[Any] = None

    for event in events:
        event_type = event.get("type")

        if event_type == "STATE_SNAPSHOT":
            current_snapshot = event.get("payload")
            report.state_chain.append(StateChainEntry(
                state_hash=event.get("state_hash", ""),
                timestamp=event.get("timestamp", 0),
                kind="snapshot",
            ))
            if _contains_redaction(event.get("payload")):
                report.has_redacted_content = True

        elif event_type == "STATE_PATCH":
            state_hash = event.get("state_hash", "")
            timestamp = event.get("timestamp", 0)
            if current_snapshot is None:
                raise ValueError(
                    f"STATE_PATCH (hash={state_hash}, t={timestamp}) has no preceding STATE_SNAPSHOT"
                )
            ops = event.get("patch", [])
            try:
                current_snapshot = _apply_json_patch(current_snapshot, ops)
            except (ValueError, KeyError, IndexError, TypeError) as exc:
                raise ValueError(
                    f"patch application failed for state_hash={state_hash} "
                    f"at timestamp={timestamp}: {exc}"
                ) from exc
            report.state_chain.append(StateChainEntry(
                state_hash=state_hash,
                timestamp=timestamp,
                kind="patch",
                patch_applied=True,
            ))

        elif event_type == "TOOL_CALL":
            args = event.get("args", {})
            has_redacted_args = _contains_redaction(args)
            if has_redacted_args:
                report.has_redacted_content = True
            report.tool_calls.append(ToolCallEntry(
                tool_name=event.get("tool_name", ""),
                timestamp=event.get("timestamp", 0),
                has_redacted_args=has_redacted_args,
            ))

        elif event_type == "POLICY_DECISION":
            report.policy_decisions.append(PolicyDecisionEntry(
                rule_id=event.get("rule_id", ""),
                action=event.get("action", ""),
                decision=event.get("decision", ""),
                timestamp=event.get("timestamp", 0),
            ))

        elif event_type == "HITL_EVENT":
            report.hitl_events.append(HitlEventEntry(
                event_type=event.get("event_type", ""),
                timestamp=event.get("timestamp", 0),
            ))

        # VISUAL_CAPTURE, PLUGIN_STATE_TRANSFORM, PLUGIN_POLICY_DECISION
        # are counted in total_events but not surfaced in the report sections.

    return report


# ---------------------------------------------------------------------------
# Report formatters
# ---------------------------------------------------------------------------

def _escape_md(s: str) -> str:
    """Escape characters that could break Markdown table structure."""
    return s.replace("`", "\\`").replace("|", "\\|").replace("\n", " ")


def report_to_markdown(report: ReplayReport) -> str:
    lines: list[str] = []
    lines.append("# Audit Replay Report\n")
    lines.append(f"**Total events:** {report.total_events}  ")
    lines.append(f"**Redacted content detected:** {str(report.has_redacted_content).lower()}\n")

    lines.append("## State Chain\n")
    if report.state_chain:
        lines.append("| # | Kind | Hash | Timestamp (ms) |")
        lines.append("|---|------|------|----------------|")
        for i, e in enumerate(report.state_chain, 1):
            kind = "SNAPSHOT" if e.kind == "snapshot" else "PATCH"
            lines.append(f"| {i} | {kind} | `{_escape_md(e.state_hash)}` | {e.timestamp} |")
    else:
        lines.append("_(no state events)_")
    lines.append("")

    lines.append("## Tool Calls\n")
    if report.tool_calls:
        lines.append("| Tool | Timestamp (ms) | Redacted Args |")
        lines.append("|------|----------------|---------------|")
        for tc in report.tool_calls:
            lines.append(f"| `{_escape_md(tc.tool_name)}` | {tc.timestamp} | {str(tc.has_redacted_args).lower()} |")
    else:
        lines.append("_(no tool calls)_")
    lines.append("")

    lines.append("## Policy Decisions\n")
    if report.policy_decisions:
        lines.append("| Rule | Action | Decision | Timestamp (ms) |")
        lines.append("|------|--------|----------|----------------|")
        for pd in report.policy_decisions:
            lines.append(
                f"| `{_escape_md(pd.rule_id)}` | {_escape_md(pd.action)} "
                f"| **{_escape_md(pd.decision)}** | {pd.timestamp} |"
            )
    else:
        lines.append("_(no policy decisions)_")
    lines.append("")

    lines.append("## HITL Events\n")
    if report.hitl_events:
        lines.append("| Event Type | Timestamp (ms) |")
        lines.append("|------------|----------------|")
        for he in report.hitl_events:
            lines.append(f"| `{_escape_md(he.event_type)}` | {he.timestamp} |")
    else:
        lines.append("_(no HITL events)_")

    return "\n".join(lines) + "\n"


def report_to_json(report: ReplayReport) -> str:
    return json.dumps(dataclasses.asdict(report), indent=2)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def _parse_ndjson(path: Path) -> list[dict]:
    events: list[dict] = []
    with path.open(encoding="utf-8") as fh:
        for lineno, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError as exc:
                print(f"[ERROR] line {lineno}: {exc}", file=sys.stderr)
                sys.exit(1)
    return events


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Replay sanitized audit event NDJSON and produce a report.",
    )
    parser.add_argument("input", type=Path, help="NDJSON audit log file")
    parser.add_argument(
        "--format",
        choices=["markdown", "json"],
        default="markdown",
        help="Output format (default: markdown)",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="Write report to this file instead of stdout",
    )
    args = parser.parse_args()

    if not args.input.exists():
        print(f"[ERROR] File not found: {args.input}", file=sys.stderr)
        sys.exit(1)

    events = _parse_ndjson(args.input)

    try:
        report = replay_events(events)
    except ValueError as exc:
        print(f"[ERROR] Replay failed: {exc}", file=sys.stderr)
        sys.exit(2)

    output = report_to_markdown(report) if args.format == "markdown" else report_to_json(report)

    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(output, encoding="utf-8")
        print(f"Report written to {args.out}")
    else:
        print(output, end="")


if __name__ == "__main__":
    main()
