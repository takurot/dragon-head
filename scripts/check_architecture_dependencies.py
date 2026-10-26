#!/usr/bin/env python3
"""Compare documented production edges with Cargo workspace path dependencies."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]


def workspace_graph(metadata: dict) -> tuple[set[str], set[tuple[str, str]]]:
    if not isinstance(metadata, dict):
        raise ValueError("cargo metadata must be an object")
    packages = metadata.get("packages")
    members = metadata.get("workspace_members")
    if not isinstance(packages, list) or not isinstance(members, list) or not members:
        raise ValueError("cargo metadata requires packages and nonempty workspace_members arrays")
    by_id = {}
    for package in packages:
        if not isinstance(package, dict) or not isinstance(package.get("id"), str):
            raise ValueError("package requires a string id")
        if package["id"] in by_id:
            raise ValueError("duplicate package id")
        by_id[package["id"]] = package
    if any(not isinstance(member, str) or member not in by_id for member in members):
        raise ValueError("workspace_members must reference known package ids")
    if len(set(members)) != len(members):
        raise ValueError("duplicate workspace member")
    workspace = [by_id[member] for member in members]
    by_path = {}
    names = set()
    for package in workspace:
        name = package.get("name")
        manifest = package.get("manifest_path")
        if not isinstance(name, str) or not name or not isinstance(manifest, str) or not manifest:
            raise ValueError("workspace package requires name and manifest_path strings")
        path = Path(manifest).resolve().parent
        if name in names or path in by_path:
            raise ValueError("duplicate workspace package name or manifest directory")
        names.add(name)
        by_path[path] = name
    edges = set()
    for package in workspace:
        dependencies = package.get("dependencies")
        if not isinstance(dependencies, list):
            raise ValueError("package dependencies must be an array")
        for dependency in dependencies:
            if not isinstance(dependency, dict) or "kind" not in dependency:
                raise ValueError("dependency requires a kind")
            if dependency["kind"] not in (None, "dev", "build"):
                raise ValueError("unknown dependency kind")
            if dependency["kind"] is not None:
                continue
            path = dependency.get("path")
            if path is None:
                continue
            if not isinstance(path, str) or not path:
                raise ValueError("dependency path must be a nonempty string")
            target = by_path.get(Path(path).resolve())
            if target is not None:
                # Resolve by path, not the local rename or package-id string syntax.
                edges.add((package["name"], target))
    return names, edges


def documented_edges(document: str) -> set[tuple[str, str]]:
    opening = r"^```workspace-dependencies[ \t]*$"
    blocks = re.findall(r"^```workspace-dependencies[ \t]*\n(.*?)^```[ \t]*$",
                        document, re.MULTILINE | re.DOTALL)
    if len(re.findall(opening, document, re.MULTILINE)) != 1 or len(blocks) != 1:
        raise ValueError("expected exactly one closed workspace-dependencies fence")
    edges = set()
    for line in blocks[0].splitlines():
        if not line.strip():
            continue
        match = re.fullmatch(r"([a-zA-Z0-9_-]+) -> ([a-zA-Z0-9_-]+)", line)
        if match is None:
            raise ValueError(f"invalid dependency edge: {line!r}")
        edge = match.groups()
        if edge[0] == edge[1] or edge in edges:
            raise ValueError(f"self or duplicate dependency edge: {line}")
        edges.add(edge)
    if not edges:
        raise ValueError("workspace-dependencies fence must contain edges")
    return edges


def check_dependencies(metadata: dict, document: str) -> None:
    names, actual = workspace_graph(metadata)
    documented = documented_edges(document)
    if any(source not in names or target not in names for source, target in documented):
        raise ValueError("documented edge references an unknown workspace package")
    differences = []
    for label, edges in (("missing", actual - documented), ("extra", documented - actual)):
        if edges:
            differences.append(f"{label} documented edges: " + ", ".join(
                f"{source} -> {target}" for source, target in sorted(edges)))
    if differences:
        raise ValueError("; ".join(differences))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--doc", type=Path, default=ROOT / "docs/ARCHITECTURE.md")
    parser.add_argument("--manifest-path", type=Path, default=ROOT / "Cargo.toml")
    args = parser.parse_args(argv)
    try:
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1",
             "--manifest-path", str(args.manifest_path)],
            capture_output=True, text=True, timeout=30,
        )
        if result.returncode != 0:
            raise ValueError(f"cargo metadata failed (exit {result.returncode}): {result.stderr.strip()}")
        metadata = json.loads(result.stdout)
        check_dependencies(metadata, args.doc.read_text(encoding="utf-8"))
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"Architecture dependency check failed: {error}", file=sys.stderr)
        return 1
    print("PASS: architecture dependency edges match Cargo workspace normal path dependencies")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
