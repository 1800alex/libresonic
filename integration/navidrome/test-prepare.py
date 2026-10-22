"""Generator safety checks; generated media is covered by the server smoke test."""

from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import prepare


class GeneratorSafetyTests(unittest.TestCase):
    def setUp(self):
        cache = Path(__file__).resolve().parents[2] / ".cache"
        cache.mkdir(exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix="fixture-test-", dir=cache)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def test_existing_unmarked_music_is_untouched(self):
        target = self.root / "existing"
        target.mkdir()
        original = target / "recording.mp3"
        original.write_bytes(b"existing recording")
        with self.assertRaises(ValueError):
            prepare.generate(target)
        self.assertEqual(original.read_bytes(), b"existing recording")
        self.assertEqual(list(target.iterdir()), [original])

    def test_encoder_failure_leaves_no_partial_library(self):
        target = self.root / "new-library"
        with patch.object(prepare.shutil, "which", return_value="ffmpeg"), patch.object(
            prepare, "ffmpeg", side_effect=subprocess.CalledProcessError(1, "ffmpeg")
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                prepare.generate(target)
        self.assertFalse(target.exists())
        self.assertEqual(list(self.root.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
