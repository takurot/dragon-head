#!/usr/bin/env python3
"""Verify MVP operational documentation required by Issue #176."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

REQUIRED_DOCS = {
    "docs/operations.md": [
        "Starting and stopping",
        "Chrome path",
        "Log locations",
        "Session Vault",
        "Upgrading",
    ],
    "docs/known-constraints.md": [
        "First-call token overhead",
        "LoadProfile::Interactive",
        "Prompt injection",
        "Wasm plugin signatures",
        "Chrome/Chromium compatibility",
    ],
    "docs/incident-response.md": [
        "Chrome crash or disconnect",
        "Audit log write failure",
        "HITL escalation timeout",
        "Browser restart rate limit",
    ],
}


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def test_required_docs_exist_and_cover_required_topics() -> None:
    for path, topics in REQUIRED_DOCS.items():
        doc_path = ROOT / path
        assert doc_path.exists(), f"missing required document: {path}"
        body = doc_path.read_text(encoding="utf-8")
        for topic in topics:
            assert topic in body, f"{path} does not cover required topic: {topic}"


def test_readme_links_all_required_docs() -> None:
    readme = read("README.md")
    for path in REQUIRED_DOCS:
        assert f"]({path})" in readme, f"README.md does not link {path}"


def test_plan_marks_mvp_operational_docs_complete() -> None:
    plan = read("docs/PLAN.md")
    expected = (
        "- [x] 利用手順・既知制約・障害時手順が `docs/` に明示され、"
        "運用可能な状態になっている。"
    )
    assert expected in plan, "docs/PLAN.md does not mark the MVP docs condition complete"


if __name__ == "__main__":
    test_required_docs_exist_and_cover_required_topics()
    test_readme_links_all_required_docs()
    test_plan_marks_mvp_operational_docs_complete()
