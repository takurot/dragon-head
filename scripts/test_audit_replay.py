#!/usr/bin/env python3
"""Unit tests for audit_replay.py using stdlib unittest (ISSUE-271)."""

import sys
import unittest
from pathlib import Path

# Allow importing audit_replay from the same scripts/ directory.
sys.path.insert(0, str(Path(__file__).parent))

import audit_replay  # noqa: E402


class TestApplyJsonPatch(unittest.TestCase):
    def test_replace_at_root(self):
        # RFC 6902 §4.3: "replace" with path "" replaces the whole document.
        result = audit_replay._apply_json_patch({"a": 1}, [{"op": "replace", "path": "", "value": {"b": 2}}])
        self.assertEqual(result, {"b": 2})

    def test_add_at_root(self):
        # RFC 6902 §4.1: "add" with path "" also replaces the whole document.
        result = audit_replay._apply_json_patch({"a": 1}, [{"op": "add", "path": "", "value": [1, 2, 3]}])
        self.assertEqual(result, [1, 2, 3])

    def test_remove_at_root_is_an_error(self):
        with self.assertRaises(ValueError):
            audit_replay._apply_json_patch({"a": 1}, [{"op": "remove", "path": ""}])

    def test_replace_nested_field(self):
        result = audit_replay._apply_json_patch({"a": {"b": 1}}, [{"op": "replace", "path": "/a/b", "value": 2}])
        self.assertEqual(result, {"a": {"b": 2}})

    def test_add_array_append(self):
        result = audit_replay._apply_json_patch({"a": [1, 2]}, [{"op": "add", "path": "/a/-", "value": 3}])
        self.assertEqual(result, {"a": [1, 2, 3]})

    def test_add_array_insert(self):
        result = audit_replay._apply_json_patch({"a": [1, 3]}, [{"op": "add", "path": "/a/1", "value": 2}])
        self.assertEqual(result, {"a": [1, 2, 3]})

    def test_negative_array_index_rejected(self):
        # Python's int("-1") would silently wrap to the last element;
        # RFC 6901 forbids negative array indices entirely.
        with self.assertRaises(ValueError):
            audit_replay._apply_json_patch({"a": [1, 2, 3]}, [{"op": "replace", "path": "/a/-1", "value": 9}])

    def test_leading_zero_array_index_rejected(self):
        with self.assertRaises(ValueError):
            audit_replay._apply_json_patch({"a": [1, 2, 3]}, [{"op": "replace", "path": "/a/01", "value": 9}])

    def test_empty_string_key_segment_preserved(self):
        # "/a//b": key "a", then key "", then key "b" — the middle "" must
        # not be silently dropped.
        doc = {"a": {"": {"b": 1}}}
        result = audit_replay._apply_json_patch(doc, [{"op": "replace", "path": "/a//b", "value": 2}])
        self.assertEqual(result, {"a": {"": {"b": 2}}})

    def test_move_to_root(self):
        result = audit_replay._apply_json_patch(
            {"a": {"b": 1}, "c": 2}, [{"op": "move", "from": "/a", "path": ""}]
        )
        self.assertEqual(result, {"b": 1})

    def test_test_op_success_and_failure(self):
        ok = audit_replay._apply_json_patch({"a": 1}, [{"op": "test", "path": "/a", "value": 1}])
        self.assertEqual(ok, {"a": 1})
        with self.assertRaises(ValueError):
            audit_replay._apply_json_patch({"a": 1}, [{"op": "test", "path": "/a", "value": 2}])


class TestPtrParts(unittest.TestCase):
    def test_root_path_is_empty_parts(self):
        self.assertEqual(audit_replay._ptr_parts(""), [])

    def test_simple_path(self):
        self.assertEqual(audit_replay._ptr_parts("/a/b"), ["a", "b"])

    def test_middle_empty_segment_preserved(self):
        self.assertEqual(audit_replay._ptr_parts("/a//b"), ["a", "", "b"])

    def test_trailing_empty_segment_preserved(self):
        # "/a/" points through "a" to key "" — must not collapse to ["a"].
        self.assertEqual(audit_replay._ptr_parts("/a/"), ["a", ""])

    def test_escape_sequences_decoded(self):
        self.assertEqual(audit_replay._ptr_parts("/a~1b/c~0d"), ["a/b", "c~d"])

    def test_missing_leading_slash_rejected(self):
        with self.assertRaises(ValueError):
            audit_replay._ptr_parts("a/b")


class TestContainsRedaction(unittest.TestCase):
    def test_detects_in_string_value(self):
        self.assertTrue(audit_replay._contains_redaction("alice@***"))

    def test_detects_masked_card_number(self):
        self.assertTrue(audit_replay._contains_redaction("****-****-****-1234"))

    def test_detects_in_list(self):
        self.assertTrue(audit_replay._contains_redaction(["ok", "***"]))

    def test_detects_in_dict_value(self):
        self.assertTrue(audit_replay._contains_redaction({"email": "***"}))

    def test_detects_in_dict_key(self):
        # A redacted key (not just a redacted value) must also count.
        self.assertTrue(audit_replay._contains_redaction({"***": "unredacted-value"}))

    def test_false_for_clean_data(self):
        self.assertFalse(audit_replay._contains_redaction({"a": [1, "b", {"c": "d"}]}))


class TestParseNdjson(unittest.TestCase):
    def test_raises_value_error_not_sys_exit(self):
        import tempfile

        with tempfile.NamedTemporaryFile(mode="w", suffix=".ndjson", delete=False) as fh:
            fh.write('{"valid": true}\n')
            fh.write("not json\n")
            path = Path(fh.name)
        try:
            with self.assertRaises(ValueError) as ctx:
                audit_replay._parse_ndjson(path)
            self.assertIn("line 2", str(ctx.exception))
        finally:
            path.unlink()

    def test_parses_valid_lines_and_skips_blank(self):
        import tempfile

        with tempfile.NamedTemporaryFile(mode="w", suffix=".ndjson", delete=False) as fh:
            fh.write('{"a": 1}\n')
            fh.write("\n")
            fh.write('{"a": 2}\n')
            path = Path(fh.name)
        try:
            events = audit_replay._parse_ndjson(path)
            self.assertEqual(events, [{"a": 1}, {"a": 2}])
        finally:
            path.unlink()


class TestStateChainEntryHasNoDeadField(unittest.TestCase):
    def test_no_patch_applied_field(self):
        # patch_applied was always True (replay_events raises before ever
        # constructing an entry for a failed patch) — dead state, removed.
        entry = audit_replay.StateChainEntry(state_hash="h", timestamp=1, kind="patch")
        self.assertFalse(hasattr(entry, "patch_applied"))


if __name__ == "__main__":
    unittest.main()
