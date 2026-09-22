import os
import sys
import unittest
from unittest.mock import patch

from core.utils import get_external_process_env


class ExternalProcessEnvTests(unittest.TestCase):
    def test_frozen_linux_restores_original_library_path(self):
        with patch.object(sys, "platform", "linux"), patch.object(
            sys, "frozen", True, create=True
        ), patch.dict(
            os.environ,
            {
                "LD_LIBRARY_PATH": "/tmp/_MEI/lib",
                "LD_LIBRARY_PATH_ORIG": "/usr/lib:/opt/lib",
                "CUSTOM_VALUE": "preserved",
            },
            clear=True,
        ):
            parent = dict(os.environ)
            child = get_external_process_env()

            self.assertEqual(child["LD_LIBRARY_PATH"], "/usr/lib:/opt/lib")
            self.assertEqual(child["CUSTOM_VALUE"], "preserved")
            self.assertEqual(dict(os.environ), parent)

    def test_frozen_linux_preserves_empty_original_library_path(self):
        with patch.object(sys, "platform", "linux"), patch.object(
            sys, "frozen", True, create=True
        ), patch.dict(
            os.environ,
            {"LD_LIBRARY_PATH": "/tmp/_MEI/lib", "LD_LIBRARY_PATH_ORIG": ""},
            clear=True,
        ):
            child = get_external_process_env()

            self.assertIn("LD_LIBRARY_PATH", child)
            self.assertEqual(child["LD_LIBRARY_PATH"], "")

    def test_frozen_linux_removes_library_path_without_original(self):
        with patch.object(sys, "platform", "linux"), patch.object(
            sys, "frozen", True, create=True
        ), patch.dict(
            os.environ, {"LD_LIBRARY_PATH": "/tmp/_MEI/lib"}, clear=True
        ):
            child = get_external_process_env()

            self.assertNotIn("LD_LIBRARY_PATH", child)

    def test_unfrozen_linux_environment_is_unchanged(self):
        with patch.object(sys, "platform", "linux"), patch.object(
            sys, "frozen", False, create=True
        ), patch.dict(
            os.environ,
            {"LD_LIBRARY_PATH": "/source/lib", "CUSTOM_VALUE": "preserved"},
            clear=True,
        ):
            parent = dict(os.environ)
            child = get_external_process_env()

            self.assertEqual(child, parent)
            self.assertEqual(dict(os.environ), parent)

    def test_non_linux_environment_is_unchanged(self):
        with patch.object(sys, "platform", "win32"), patch.object(
            sys, "frozen", True, create=True
        ), patch.dict(
            os.environ,
            {"LD_LIBRARY_PATH": "bundled", "LD_LIBRARY_PATH_ORIG": "original"},
            clear=True,
        ):
            parent = dict(os.environ)
            child = get_external_process_env()

            self.assertEqual(child, parent)
            self.assertEqual(dict(os.environ), parent)


if __name__ == "__main__":
    unittest.main()
