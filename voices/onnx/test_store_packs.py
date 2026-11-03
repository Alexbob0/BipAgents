"""store_packs.readable: an Apple pack keeps each file's mode, so every file must be readable by all (3/11)."""
import os
import stat
import tempfile
import unittest
from pathlib import Path

import store_packs


class Readable(unittest.TestCase):
    def test_owner_only_files_become_readable(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp) / "english-x"
            (folder / "voices").mkdir(parents=True)
            data = folder / "flowlm.onnx.data"
            data.write_bytes(b"w")
            os.chmod(data, 0o600)   # what onnx.save_model leaves
            (folder / "voices" / "mousse.kv.f16").write_bytes(b"v")
            store_packs.readable(folder)
            self.assertEqual(stat.S_IMODE(data.stat().st_mode), 0o644)
            self.assertEqual(stat.S_IMODE((folder / "voices").stat().st_mode), 0o755)


if __name__ == "__main__":
    unittest.main()
