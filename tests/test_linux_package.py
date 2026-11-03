"""Checks for the offline Linux release artifact."""
from __future__ import annotations

import hashlib
import json
import os
import shutil
from pathlib import Path
import subprocess
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parent.parent


class LinuxPackageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.temporary = tempfile.TemporaryDirectory(prefix="linktolinux-package-")
        cls.directory = Path(cls.temporary.name)
        cls.artifact = cls.directory / "Installer with spaces.run"
        subprocess.run(["python3", str(ROOT / "scripts/package-linux.py"), "--output", str(cls.artifact)], cwd=ROOT, check=True, timeout=60)

    @classmethod
    def tearDownClass(cls) -> None:
        cls.temporary.cleanup()

    def test_self_contained_payload_keeps_the_existing_app_and_icon(self) -> None:
        self.assertTrue(os.access(self.artifact, os.X_OK))
        with zipfile.ZipFile(self.artifact) as archive:
            manifest = json.loads(archive.read("manifest.json"))
            self.assertEqual(manifest["architecture"], "x86_64")
            self.assertEqual(manifest["minimum_glibc"], "2.34")
            self.assertEqual(archive.read("payload/linktolinux"), (ROOT / "release/linktolinux").read_bytes())
            self.assertEqual(archive.read("payload/LinkToLinux-Companion.apk"), (ROOT / "release/LinkToLinux-Companion.apk").read_bytes())
            self.assertEqual(archive.read("payload/icon.svg"), (ROOT / "assets/icon.svg").read_bytes())
            self.assertFalse(any("signing" in name or "keystore" in name or "node_modules" in name for name in archive.namelist()))
            self.assertEqual(set(archive.namelist()), {"__main__.py", "manifest.json"} | {f"payload/{name}" for name in manifest["files"]})
            for name, entry in manifest["files"].items():
                payload = archive.read(f"payload/{name}")
                self.assertEqual(hashlib.sha256(payload).hexdigest(), entry["sha256"])
                self.assertEqual(len(payload), entry["size"])

    def test_release_includes_personal_use_and_dependency_licenses(self) -> None:
        with zipfile.ZipFile(self.artifact) as archive:
            self.assertEqual(archive.read("payload/LICENSE.txt"), (ROOT / "LICENSE").read_bytes())
            self.assertEqual(archive.read("payload/LICENSES/dependencies.txt"), (ROOT / "LICENSES/dependencies.txt").read_bytes())

    def test_release_contains_no_build_machine_home_path(self) -> None:
        self.assertFalse(str(Path.home()).encode() in (ROOT / "release/linktolinux").read_bytes(), "Release contains a build-machine home path")

    def test_same_inputs_produce_the_same_installer(self) -> None:
        second = self.directory / "second.run"
        subprocess.run(["python3", str(ROOT / "scripts/package-linux.py"), "--output", str(second)], cwd=ROOT, check=True, timeout=60)
        self.assertEqual(self.artifact.read_bytes(), second.read_bytes())

    def test_help_runs_without_a_checkout_or_installation(self) -> None:
        working = self.directory / "Empty folder"
        working.mkdir(exist_ok=True)
        environment = os.environ.copy()
        environment.update(XDG_DATA_HOME=str(working / "share"), XDG_BIN_HOME=str(working / "bin"))
        result = subprocess.run([str(self.artifact), "--help"], cwd=working, env=environment, check=True, capture_output=True, text=True, timeout=15)
        self.assertIn("--uninstall", result.stdout)
        self.assertFalse((working / "share").exists())
        self.assertFalse((working / "bin").exists())

    def test_extract_preserves_the_bundled_executable_and_apk(self) -> None:
        destination = self.directory / "Inspected payload"
        subprocess.run([str(self.artifact), "--extract", str(destination), "--no-gui"], cwd=self.directory, check=True, capture_output=True, text=True, timeout=20)
        self.assertEqual((destination / "linktolinux").read_bytes(), (ROOT / "release/linktolinux").read_bytes())
        self.assertEqual((destination / "LinkToLinux-Companion.apk").read_bytes(), (ROOT / "release/LinkToLinux-Companion.apk").read_bytes())

    @unittest.skipIf(os.geteuid() == 0, "The user installer deliberately refuses root installation")
    def test_offline_install_and_remove_from_an_empty_directory(self) -> None:
        working = self.directory / "Offline download folder"
        working.mkdir(exist_ok=True)
        installer = working / "LinkToLinux.run"
        shutil.copy2(self.artifact, installer)
        data = working / "User data"
        binary_home = working / "User commands"
        pairing = data / "dev.linktolinux.app/pairing-code"
        pairing.parent.mkdir(parents=True)
        pairing.write_bytes(b"retained pairing")
        environment = os.environ.copy()
        environment.update(XDG_DATA_HOME=str(data), XDG_BIN_HOME=str(binary_home))
        subprocess.run([str(installer), "--yes", "--no-gui"], cwd=working, env=environment, check=True, capture_output=True, text=True, timeout=40)
        application = data / "linktolinux"
        self.assertEqual((application / "linktolinux").read_bytes(), (ROOT / "release/linktolinux").read_bytes())
        self.assertTrue((binary_home / "linktolinux").is_symlink())
        desktop = (data / "applications/dev.linktolinux.app.desktop").read_text()
        self.assertIn("StartupWMClass=linktolinux\n", desktop)
        self.assertIn("StartupNotify=true\n", desktop)
        self.assertEqual((data / "icons/hicolor/scalable/apps/dev.linktolinux.app.svg").read_bytes(), (ROOT / "assets/icon.svg").read_bytes())
        subprocess.run(["python3", str(application / "uninstall.py"), "--uninstall", "--yes", "--no-gui"], cwd=working, env=environment, check=True, capture_output=True, text=True, timeout=30)
        self.assertFalse((binary_home / "linktolinux").exists())
        self.assertFalse((application / "linktolinux").exists())
        self.assertEqual(pairing.read_bytes(), b"retained pairing")


if __name__ == "__main__":
    unittest.main()
