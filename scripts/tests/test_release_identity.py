"""Small regressions for release identity; stdlib only, no Cargo invocation."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "release_identity", Path(__file__).resolve().parents[1] / "check-release-identity.py"
)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class ReleaseIdentityTests(unittest.TestCase):
    def fixture(self, root, version="2.6.0-alpha.1"):
        (root / "crate").mkdir()
        (root / "frontend").mkdir()
        (root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["crate"]\n[workspace.package]\n'
            f'version = "{version}"\n'
        )
        (root / "crate/Cargo.toml").write_text(
            '[package]\nname = "robson-example"\nversion.workspace = true\n'
        )
        (root / "Cargo.lock").write_text(
            f'[[package]]\nname = "robson-example"\nversion = "{version}"\n'
        )
        (root / "frontend/package.json").write_text(json.dumps({"version": version}))

    def test_semver_leading_zero_regression(self):
        self.assertEqual(CHECK.require_semver("2.5.14"), "2.5.14")
        for version in ["2.5.014", "02.5.14", "2.06.0", "2.6.0-alpha.01"]:
            with self.subTest(version=version), self.assertRaises(ValueError):
                CHECK.require_semver(version)

    def test_alpha_identity_and_tag(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            self.assertEqual(CHECK.check_release_identity(root, "v2.6.0-alpha.1"),
                             ("2.6.0-alpha.1", 1))
            for tag in ["v2.6.0-alpha.2", "2.6.0-alpha.1", "v2.5.014"]:
                with self.subTest(tag=tag), self.assertRaises(ValueError):
                    CHECK.check_release_identity(root, tag)

    def test_lock_mismatch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            path = root / "Cargo.lock"
            path.write_text(path.read_text().replace("2.6.0-alpha.1", "2.5.0"))
            with self.assertRaisesRegex(ValueError, "Cargo.lock"):
                CHECK.check_release_identity(root)

    def test_frontend_mismatch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            (root / "frontend/package.json").write_text('{"version":"2.5.0"}')
            with self.assertRaisesRegex(ValueError, "frontend"):
                CHECK.check_release_identity(root)


if __name__ == "__main__":
    unittest.main()
