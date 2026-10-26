"""Production workspace graph and architecture documentation contract (ISSUE-399)."""

import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parent))
import check_architecture_dependencies as checker


ROOT = Path(__file__).resolve().parents[1]


def package(name):
    return {
        "id": f"path+file:///workspace/{name}#0.1.0",
        "name": name,
        "manifest_path": f"/workspace/{name}/Cargo.toml",
        "dependencies": [],
    }


def metadata():
    packages = [package("app"), package("leaf")]
    packages[0]["dependencies"] = [{"name": "leaf", "kind": None, "path": "/workspace/leaf"}]
    return {"packages": packages, "workspace_members": [p["id"] for p in packages]}


def document(*edges):
    return "# Dependency direction\n\n```workspace-dependencies\n" + "\n".join(edges) + "\n```\n"


class TestMetadataGraph(unittest.TestCase):
    def test_normal_renamed_optional_and_target_dependencies_use_actual_package_name(self):
        for flags in ({"rename": "renamed_leaf"}, {"optional": True}, {"target": "cfg(unix)"}):
            data = metadata()
            data["packages"][0]["dependencies"][0].update(flags)
            names, edges = checker.workspace_graph(data)
            self.assertEqual(names, {"app", "leaf"})
            self.assertEqual(edges, {("app", "leaf")})

    def test_dev_build_external_and_registry_dependencies_are_excluded(self):
        data = metadata()
        data["packages"][0]["dependencies"] += [
            {"name": "app", "kind": "dev", "path": "/workspace/app"},
            {"name": "leaf", "kind": "build", "path": "/workspace/leaf"},
            {"name": "outside", "kind": None, "path": "/workspace/outside"},
            {"name": "serde", "kind": None},
        ]
        # Metadata may contain a package outside workspace_members.
        data["packages"].append(package("outside"))
        self.assertEqual(checker.workspace_graph(data)[1], {("app", "leaf")})

    def test_malformed_metadata_is_rejected(self):
        cases = [None, {}, {"packages": [], "workspace_members": []}]
        for key, value in (("packages", {}), ("workspace_members", "app"),
                           ("workspace_members", ["missing-id"])):
            item = metadata()
            item[key] = value
            cases.append(item)
        duplicate = metadata()
        duplicate["packages"].append(copy.deepcopy(duplicate["packages"][0]))
        cases.append(duplicate)
        for field in ("name", "manifest_path"):
            item = metadata()
            item["packages"][1][field] = item["packages"][0][field]
            cases.append(item)
        for field, value in (("dependencies", {}), ("name", ""),
                             ("manifest_path", None), ("id", None)):
            item = metadata()
            item["packages"][0][field] = value
            cases.append(item)
        for key, value in (("kind", "unknown"), ("path", ""), ("path", 1)):
            item = metadata()
            item["packages"][0]["dependencies"][0][key] = value
            cases.append(item)
        for item in cases:
            with self.subTest(data=item), self.assertRaises(ValueError):
                checker.workspace_graph(item)


class TestDocumentGraph(unittest.TestCase):
    def test_fenced_edges_ignore_surrounding_prose(self):
        text = "prose -> ignored\n" + document("app -> leaf") + "\nmore -> prose\n"
        self.assertEqual(checker.documented_edges(text), {("app", "leaf")})

    def test_invalid_empty_duplicate_and_multiple_fences_are_rejected(self):
        for text in ("no graph", document(), document("app => leaf"),
                     document("app -> app"), document("app -> leaf", "app -> leaf"),
                     document("app -> leaf") * 2,
                     "```workspace-dependencies\napp -> leaf\n"):
            with self.subTest(document=text), self.assertRaises(ValueError):
                checker.documented_edges(text)

    def test_added_removed_and_unknown_edges_fail(self):
        checker.check_dependencies(metadata(), document("app -> leaf"))
        for text in (document("leaf -> app"),
                     document("app -> leaf", "leaf -> app"),
                     document("app -> leaf", "unknown -> app")):
            with self.subTest(document=text), self.assertRaises(ValueError):
                checker.check_dependencies(metadata(), text)

    def test_added_and_removed_manifest_dependencies_fail(self):
        removed = metadata()
        removed["packages"][0]["dependencies"] = []
        added = metadata()
        added["packages"][1]["dependencies"] = [
            {"name": "app", "kind": None, "path": "/workspace/app"},
        ]
        for data, reason in ((removed, "extra"), (added, "missing")):
            with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, reason):
                checker.check_dependencies(data, document("app -> leaf"))

    def test_real_workspace_document_matches_cargo_metadata(self):
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("PASS", result.stdout)

    def run_cli(self, *args):
        return subprocess.run(
            [sys.executable, str(Path(checker.__file__)), *args],
            capture_output=True, text=True, timeout=40,
        )

    def test_real_cli_rejects_both_removed_and_added_document_edges(self):
        original = (ROOT / "docs/ARCHITECTURE.md").read_text(encoding="utf-8")
        removed = original.replace("bench -> core-runtime\n", "", 1)
        added = original.replace("```workspace-dependencies\n",
                                 "```workspace-dependencies\nskills-engine -> core-runtime\n", 1)
        for name, text, reason in (("removed", removed, "missing"), ("added", added, "extra")):
            with self.subTest(case=name), tempfile.TemporaryDirectory() as temp:
                path = Path(temp) / "ARCHITECTURE.md"
                path.write_text(text, encoding="utf-8")
                result = self.run_cli("--doc", str(path))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(reason, result.stderr)
                self.assertNotIn("PASS", result.stdout)

    def test_cargo_failure_and_invalid_metadata_fail_closed(self):
        for result in (
            subprocess.CompletedProcess([], 1, "", "cargo failed"),
            subprocess.CompletedProcess([], 0, "{}", ""),
            subprocess.CompletedProcess([], 0, "not json", ""),
        ):
            with self.subTest(result=result), patch.object(checker.subprocess, "run", return_value=result):
                self.assertEqual(checker.main([]), 1)


if __name__ == "__main__":
    unittest.main()
