import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).parent / "fixtures/config/workflows/calculator/scripts/copy.py"


class CalculatorCopyTests(unittest.TestCase):
    def test_copy_response_preserves_value_and_propagates_backend_failure(self):
        value = "42; $(must-not-run)\n世界"
        response = subprocess.run(
            ["python3", str(SCRIPT)],
            input=json.dumps({"context": {"engine": {"state": {"item": {"value": value}}}}}),
            text=True, capture_output=True, check=True,
        )
        operation = json.loads(response.stdout)["operation"]
        self.assertEqual(operation["success_message"], "Copied to clipboard")
        self.assertFalse(operation["exit"])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            backend = root / "wl-copy"
            backend.write_text('#!/bin/sh\ncat > "$COPIED"\nexit "$COPY_STATUS"\n')
            backend.chmod(0o700)
            copied = root / "copied"
            for status in (0, 7):
                env = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                           COPIED=str(copied), COPY_STATUS=str(status))
                result = subprocess.run(operation["argv"], env=env, timeout=5)
                self.assertEqual(result.returncode, status)
                self.assertEqual(copied.read_text(), value)

    def test_missing_selection_does_not_emit_success(self):
        result = subprocess.run(
            ["python3", str(SCRIPT)],
            input=json.dumps({"context": {"engine": {"state": {}}}}),
            text=True, capture_output=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
