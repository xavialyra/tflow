"""Run with python3 -m unittest discover -s tests -p 'test_clipboard.py'."""
import base64
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).parent / "fixtures/config/workflows/clipboard/scripts"
PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII="
)


def load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


items = load("items")
restore = load("restore")


class ClipboardTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        env = patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(self.root)})
        env.start()
        self.addCleanup(env.stop)

    def test_image_preview_preserves_bytes_and_reuses_private_cache(self):
        item = items.make_item("42", "[[ binary data 68 B png 1x1 ]]", PNG)
        path = Path(item["metadata"]["thumbnail"])
        self.assertEqual(path.read_bytes(), PNG)
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(path.parent.stat().st_mode & 0o777, 0o700)
        self.assertEqual(item["metadata"]["mime"], "image/png")
        self.assertEqual(len(item["display"]["rows"]), 2)
        self.assertIn("1x1", item["display"]["rows"][1]["cells"][1]["text"])
        self.assertNotIn("content", item["metadata"])
        self.assertEqual(items.cache_image(PNG, "png"), str(path))

    def test_text_and_unknown_binary(self):
        text = "第一行\n第二行\n"
        item = items.make_item("1", "第一行", text.encode())
        self.assertEqual(item["metadata"]["content"], text)
        self.assertIn("2 lines", item["metadata"]["summary"])
        self.assertEqual(len(item["display"]["rows"]), 2)
        self.assertIn(
            "2 lines",
            item["display"]["rows"][1]["cells"][1]["text"],
        )
        self.assertNotIn(
            "characters",
            item["display"]["rows"][1]["cells"][1]["text"],
        )
        self.assertNotIn("thumbnail", item["metadata"])
        binary = items.make_item("2", "binary", b"\x00\xff")
        self.assertIn("Preview unavailable", binary["metadata"]["content"])
        long = items.make_item("3", "long", b"a" * 20000)
        self.assertTrue(long["metadata"]["content"].endswith("(preview truncated)"))

    def test_query_matches_image_type_and_untruncated_list_label(self):
        def run(argv):
            data = (
                b"42\t[[ binary data 68 B png 1x1 ]]\n43\t" + b"a" * 130 + b" needle\n44\tdeleted\n"
                if argv[1] == "list" else PNG if argv[2] == "42" else b"text"
            )
            return subprocess.CompletedProcess(argv, 1 if argv[-1] == "44" else 0, data, b"")

        for query, expected in [("image png", ["42"]), ("needle", ["43"]), ("", ["42", "43"])]:
            output = io.StringIO()
            request = {"entrypoint": "picker-items", "context": {"engine": {"state": {"input": query}}}}
            with patch.object(items, "run", run), patch.object(items.shutil, "which", return_value="mock"), patch("sys.stdin", io.StringIO(json.dumps(request))), patch("sys.stdout", output):
                items.main()
            result = json.loads(output.getvalue())["items"]
            self.assertEqual([item["value"] for item in result], expected)
            if query == "image png":
                self.assertIn("1x1", result[0]["display"]["rows"][1]["cells"][1]["text"])

    def test_content_type_and_search_parameters(self):
        history = {"3": ("needle image", PNG), "2": ("text", b"needle image text"), "1": ("needle binary", b"\x00\xff")}

        def run(argv):
            data = (
                "".join(f"{key}\t{value[0]}\n" for key, value in history.items()).encode()
                if argv[1] == "list" else history[argv[2]][1]
            )
            return subprocess.CompletedProcess(argv, 0, data, b"")

        cases = [
            ({}, {}, ["3", "2", "1"]),
            ({"content_type": "all"}, {}, ["3", "2", "1"]),
            ({"content_type": "image"}, {}, ["3"]),
            ({"content_type": "text"}, {}, ["2"]),
            ({"content_type": "binary"}, {}, ["1"]),
            ({"content_type": "image", "search": "needle"}, {}, ["3"]),
            ({"content_type": "text", "search": "missing"}, {}, []),
            ({"content_type": "text", "search": "missing"}, {"input": "needle"}, ["2"]),
            ({"content_type": "text", "search": "missing"}, {"input": ""}, ["2"]),
        ]
        with patch.object(items, "run", side_effect=run) as mocked, patch.object(items.shutil, "which", return_value="mock"):
            for parameters, state, expected in cases:
                with self.subTest(parameters=parameters, state=state):
                    output = io.StringIO()
                    request = {"entrypoint": "picker-items", "context": {"parameters": parameters, "engine": {"state": state}}}
                    with patch("sys.stdin", io.StringIO(json.dumps(request))), patch("sys.stdout", output):
                        items.main()
                    self.assertEqual([item["value"] for item in json.loads(output.getvalue())["items"]], expected)
            self.assertEqual(sum(call.args[0][1] == "decode" for call in mocked.call_args_list), 3)

    def test_invalid_content_type_is_rejected_before_reading_history(self):
        for value in ("video", "", None, [], 1):
            with self.subTest(value=value), patch.object(items, "run") as mocked:
                with self.assertRaisesRegex(ValueError, "content_type must be one of"):
                    items.search_history([], value)
                mocked.assert_not_called()

    def test_warm_search_and_incremental_refresh(self):
        history = {"3": ("image", PNG), "2": ("short label", b"hidden body needle"), "1": ("old", b"old")}
        calls = []

        def run(argv):
            calls.append(argv)
            data = (
                "".join(f"{key}\t{value[0]}\n" for key, value in history.items()).encode()
                if argv[1] == "list" else history[argv[2]][1]
            )
            return subprocess.CompletedProcess(argv, 0, data, b"")

        with patch.object(items, "run", run):
            self.assertEqual([item["value"] for item in items.search_history([])], ["3", "2", "1"])
            self.assertEqual(sum(call[1] == "decode" for call in calls), 3)
            calls.clear()
            # A new module represents a new producer process: no Python memory
            # from the previous query is available to satisfy this search.
            fresh = load("items")
            with patch.object(fresh, "run", run):
                self.assertEqual([item["value"] for item in fresh.search_history(["needle"])], ["2"])
            self.assertEqual(calls, [["cliphist", "list"]])
            calls.clear()
            history.pop("1")
            history["2"] = ("changed", b"updated")
            history["4"] = ("new", b"new")
            result = items.search_history([])
            self.assertEqual([item["value"] for item in result], ["3", "2", "4"])
            self.assertEqual([call[2] for call in calls if call[1] == "decode"], ["2", "4"])
            self.assertEqual(items.search_history(["needle"]), [])
            self.assertEqual(items.search_history(["%"]), [])
            self.assertEqual(items.search_history(["_"]), [])
            index = next(self.root.glob("*/index-v3-*.sqlite3"))
            self.assertEqual(index.stat().st_mode & 0o777, 0o600)

    def test_missing_thumbnail_is_regenerated_only_when_matched(self):
        def run(argv):
            return subprocess.CompletedProcess(argv, 0, b"1\timage\n" if argv[1] == "list" else PNG, b"")

        with patch.object(items, "run", side_effect=run) as mocked:
            item = items.search_history([])[0]
            path = Path(item["metadata"]["thumbnail"])
            path.unlink()
            mocked.reset_mock()
            self.assertEqual(items.search_history(["no match"]), [])
            self.assertEqual(mocked.call_count, 1)
            self.assertFalse(path.exists())
            items.search_history(["image"])
            self.assertEqual(path.read_bytes(), PNG)
            self.assertEqual(mocked.call_count, 3)

    def test_replaced_database_does_not_reuse_cached_ids(self):
        database = self.root / "db"
        database.write_bytes(b"first")
        payload = b"first content"

        def run(argv):
            return subprocess.CompletedProcess(argv, 0, b"1\tsame label\n" if argv[1] == "list" else payload, b"")

        with patch.dict(os.environ, {"CLIPHIST_DB_PATH": str(database)}), patch.object(items, "run", run):
            self.assertEqual(items.search_history([])[0]["metadata"]["content"], "first content")
            replacement = self.root / "replacement"
            replacement.write_bytes(b"second")
            replacement.replace(database)
            payload = b"second content"
            self.assertEqual(items.search_history([])[0]["metadata"]["content"], "second content")

    def test_restore_binary_and_decode_failure(self):
        for name, body in {
            "cliphist": '#!/bin/sh\n[ "$2" != fail ] || exit 7\ncat "$PAYLOAD"\n',
            "setsid": '#!/bin/sh\nshift 2\nexec "$@"\n',
            "wl-copy": '#!/bin/sh\nprintf "%s\\n" "$@" > "$COPY_ARGS"\ncat > "$COPIED"\n',
        }.items():
            script = self.root / name
            script.write_text(body)
            script.chmod(0o700)
        payload, copied, args = [self.root / name for name in ("payload", "copied", "args")]
        payload.write_bytes(PNG)
        env = dict(os.environ, PATH=f"{self.root}:{os.environ['PATH']}", PAYLOAD=str(payload), COPIED=str(copied), COPY_ARGS=str(args))
        for entry_id in ("42", "fail"):
            request = {"context": {"engine": {"state": {"item": {"value": entry_id, "metadata": {"mime": "image/png"}}}}}}
            output = io.StringIO()
            with patch.dict(os.environ, env), patch("sys.stdin", io.StringIO(json.dumps(request))), patch("sys.stdout", output):
                restore.main()
            operation = json.loads(output.getvalue())["operation"]
            self.assertEqual(operation["success_message"], "Copied to clipboard")
            result = subprocess.run(operation["argv"], env=env)
            if entry_id == "42":
                self.assertEqual(result.returncode, 0)
                self.assertEqual(copied.read_bytes(), PNG)
                self.assertEqual(args.read_text().splitlines(), ["--type", "image/png"])
                copied.unlink()
            else:
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(copied.exists())


if __name__ == "__main__":
    unittest.main()
