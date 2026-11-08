import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("download_large_http", Path(__file__).with_name("download_large_http.py"))
download = importlib.util.module_from_spec(spec)
spec.loader.exec_module(download)


class DownloadRangeContract(unittest.TestCase):
    def test_exact_range_and_revision(self):
        download.verify_range("bytes 10-19/100", 10, 19, 100)
        self.assertTrue(all("/resolve/main/" not in value[0] for value in download.DATASETS.values()))

    def test_wrong_range_is_refused_before_append(self):
        for header in [None, "bytes 0-9/100", "bytes 10-18/100", "bytes 10-19/101", "bytes 10-19/*"]:
            with self.subTest(header=header), self.assertRaises(ValueError):
                download.verify_range(header, 10, 19, 100)


if __name__ == "__main__":
    unittest.main()
