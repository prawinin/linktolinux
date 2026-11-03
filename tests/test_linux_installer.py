"""Offline installer behavior and hostile input regression tests."""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import shutil
import sys
import subprocess
import tempfile
import unittest
from unittest import mock
import zipfile

MODULE = Path(__file__).resolve().parents[1] / 'packaging/linux/installer.py'
spec = importlib.util.spec_from_file_location('linux_installer', MODULE)
assert spec is not None and spec.loader is not None
installer = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = installer
spec.loader.exec_module(installer)

FILES = ('linktolinux', 'LinkToLinux-Companion.apk', 'icon.svg', 'icons/32.png',
         'icons/128.png', 'icons/256.png', 'icons/512.png', 'linktolinux.desktop',
         'THIRD_PARTY_NOTICES.md', 'LICENSES/scrcpy-Apache-2.0.txt', 'LICENSES/dependencies.txt', 'LICENSE.txt', 'INSTALL.txt')


class InstallerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix='linux installer ')
        self.root = Path(self.temporary.name)
        self.paths = installer.Paths(self.root / 'data $ ` %', self.root / 'bin space')

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def bundle(self, *, corrupt: bool = False, architecture: str = 'x86_64',
               extra: str | None = None) -> Path:
        payload = {name: (b'executable' if name == 'linktolinux' else name.encode()) for name in FILES}
        metadata = {name: {'sha256': hashlib.sha256(data).hexdigest(), 'size': len(data),
                           'mode': 0o755 if name == 'linktolinux' else 0o644}
                    for name, data in payload.items()}
        manifest = {'format': 1, 'app_id': 'dev.linktolinux.app', 'name': 'LinkToLinux',
                    'version': '1.0.0', 'architecture': architecture, 'minimum_glibc': '2.34',
                    'files': metadata}
        target = self.root / 'relocated installer.run'
        with zipfile.ZipFile(target, 'w') as archive:
            archive.writestr('__main__.py', MODULE.read_bytes())
            archive.writestr('manifest.json', json.dumps(manifest))
            for name, data in payload.items():
                archive.writestr('payload/' + name, b'broken' if corrupt and name == 'icon.svg' else data)
            if extra:
                archive.writestr(extra, b'hostile')
        return target

    def test_integrity_failure_creates_no_installation(self) -> None:
        with self.assertRaises(installer.InstallerError):
            installer.Bundle.read(self.bundle(corrupt=True))
        self.assertFalse(self.paths.data.exists())
        self.assertFalse(self.paths.bin.exists())

    def test_unexpected_archive_paths_rejected(self) -> None:
        for extra in ('payload/../escape', '/absolute', 'payload/other'):
            with self.subTest(extra=extra), self.assertRaises(installer.InstallerError):
                installer.Bundle.read(self.bundle(extra=extra))
        self.assertFalse(self.paths.data.exists())

    def test_wrong_architecture_rejected_without_writes(self) -> None:
        bundle = installer.Bundle.read(self.bundle(architecture='aarch64'))
        with self.assertRaises(installer.InstallerError):
            installer.check_runtime(bundle)
        self.assertFalse(self.paths.data.exists())

    def test_install_upgrade_uninstall_preserves_user_files(self) -> None:
        bundle = installer.Bundle.read(self.bundle())
        installer.install(bundle, self.paths, MODULE.read_bytes())
        binary = self.paths.data / 'linktolinux/linktolinux'
        self.assertEqual(binary.read_bytes(), b'executable')
        self.assertEqual(stat.S_IMODE(binary.stat().st_mode), 0o755)
        self.assertEqual((self.paths.bin / 'linktolinux').resolve(), binary)
        user_file = binary.parent / 'pairings.json'
        user_file.write_text('private user state')
        desktop = (self.paths.data / 'applications/dev.linktolinux.app.desktop').read_text()
        self.assertIn('Actions=Uninstall;', desktop)
        self.assertIn('%%', desktop)
        installer.install(bundle, self.paths, MODULE.read_bytes())
        installer.uninstall(self.paths)
        self.assertFalse(binary.exists())
        self.assertFalse((self.paths.bin / 'linktolinux').is_symlink())
        self.assertEqual(user_file.read_text(), 'private user state')

    def test_unrelated_launcher_is_never_overwritten(self) -> None:
        self.paths.bin.mkdir(parents=True)
        launcher = self.paths.bin / 'linktolinux'
        launcher.write_text('unrelated')
        with self.assertRaises(installer.InstallerError):
            installer.install(installer.Bundle.read(self.bundle()), self.paths, MODULE.read_bytes())
        self.assertEqual(launcher.read_text(), 'unrelated')
        self.assertFalse(self.paths.data.exists())

    def test_modified_owned_file_survives_uninstall(self) -> None:
        installer.install(installer.Bundle.read(self.bundle()), self.paths, MODULE.read_bytes())
        icon = self.paths.data / 'icons/hicolor/scalable/apps/dev.linktolinux.app.svg'
        icon.write_text('replacement owned by user')
        installer.uninstall(self.paths)
        self.assertEqual(icon.read_text(), 'replacement owned by user')

    def test_tampered_ownership_manifest_cannot_delete_other_files(self) -> None:
        installer.install(installer.Bundle.read(self.bundle()), self.paths, MODULE.read_bytes())
        victim = self.root / 'unrelated'
        victim.write_text('keep')
        ownership = self.paths.data / 'linktolinux/installed-files.json'
        record = json.loads(ownership.read_text())
        record['files']['../../unrelated'] = {'sha256': hashlib.sha256(b'keep').hexdigest()}
        ownership.write_text(json.dumps(record))
        with self.assertRaises(installer.InstallerError):
            installer.uninstall(self.paths)
        self.assertEqual(victim.read_text(), 'keep')

    def test_installed_uninstaller_finds_custom_xdg_paths(self) -> None:
        installer.install(installer.Bundle.read(self.bundle()), self.paths, MODULE.read_bytes())
        script = self.paths.data / 'linktolinux/uninstall.py'
        environment = dict(os.environ)
        environment['XDG_DATA_HOME'] = str(self.root / 'other-data')
        environment['XDG_BIN_HOME'] = str(self.root / 'other-bin')
        result = subprocess.run([sys.executable, str(script), '--uninstall', '--yes', '--no-gui'],
                                env=environment, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.paths.bin / 'linktolinux').is_symlink())
        self.assertFalse((self.paths.data / 'linktolinux/linktolinux').exists())

    @unittest.skipUnless(shutil.which('desktop-file-validate'), 'desktop-file-validate unavailable')
    def test_desktop_entry_is_valid_with_shell_metacharacters_in_path(self) -> None:
        installer.install(installer.Bundle.read(self.bundle()), self.paths, MODULE.read_bytes())
        desktop = self.paths.data / 'applications/dev.linktolinux.app.desktop'
        result = subprocess.run(['desktop-file-validate', str(desktop)],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_desktop_exec_survives_keyfile_unescaping(self) -> None:
        try:
            from gi.repository import GLib
        except ImportError:
            self.skipTest('GLib Python bindings unavailable')
        value = '[Desktop Entry]\nExec=' + installer.desktop_quote('/tmp/bin $ ` % path') + '\n'
        keyfile = GLib.KeyFile()
        keyfile.load_from_data(value, len(value), GLib.KeyFileFlags.NONE)
        command = keyfile.get_string('Desktop Entry', 'Exec')
        parsed = GLib.shell_parse_argv(command)[1]
        self.assertEqual(parsed, ['/tmp/bin $ ` %% path'])

    def test_extraction_does_not_follow_directory_symlinks(self) -> None:
        target = self.root / 'extract'
        victim = self.root / 'victim'
        victim.mkdir()
        target.symlink_to(victim, target_is_directory=True)
        archive = self.bundle()
        result = subprocess.run([sys.executable, str(archive), '--extract', str(target), '--no-gui'],
                                capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(victim.iterdir()), [])

    def test_update_failure_rolls_back_previous_installation(self) -> None:
        bundle = installer.Bundle.read(self.bundle())
        installer.install(bundle, self.paths, MODULE.read_bytes())
        previous = (self.paths.data / 'linktolinux/linktolinux').read_bytes()
        updated = installer.Bundle(bundle.metadata, {**bundle.files, 'linktolinux': b'new version'})
        real_replace = os.replace
        replacements = 0

        def fail_once(source: str | Path, target: str | Path) -> None:
            nonlocal replacements
            replacements += 1
            if replacements == 3:
                raise OSError('simulated disk failure')
            real_replace(source, target)

        with mock.patch.object(installer.os, 'replace', side_effect=fail_once):
            with self.assertRaises(OSError):
                installer.install(updated, self.paths, MODULE.read_bytes())
        self.assertEqual((self.paths.data / 'linktolinux/linktolinux').read_bytes(), previous)
        installer.uninstall(self.paths)
        self.assertFalse((self.paths.data / 'linktolinux/linktolinux').exists())

    def test_existing_running_application_is_not_launched_again(self) -> None:
        installer.install(installer.Bundle.read(self.bundle()), self.paths, MODULE.read_bytes())
        binary = self.paths.data / 'linktolinux/linktolinux'
        binary.write_bytes(Path('/usr/bin/sleep').read_bytes())
        process = subprocess.Popen([str(binary), '10'])
        try:
            with mock.patch.object(installer.subprocess, 'Popen', side_effect=AssertionError('duplicate launch')):
                installer.launch(self.paths)
        finally:
            process.terminate()
            process.wait(timeout=5)

    def test_archive_symlinks_rejected_before_writes(self) -> None:
        archive = self.bundle()
        with zipfile.ZipFile(archive) as source:
            entries = [(entry, source.read(entry.filename)) for entry in source.infolist()]
        with zipfile.ZipFile(archive, 'w') as output:
            for entry, content in entries:
                if entry.filename == 'payload/icon.svg':
                    entry.create_system = 3
                    entry.external_attr = (stat.S_IFLNK | 0o777) << 16
                output.writestr(entry, content)
        with self.assertRaises(installer.InstallerError):
            installer.Bundle.read(archive)
        self.assertFalse(self.paths.data.exists())

    def test_extracted_archive_contains_only_verified_payload(self) -> None:
        target = self.root / 'inspect files'
        archive = self.bundle()
        result = subprocess.run([sys.executable, str(archive), '--extract', str(target), '--no-gui'],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((target / 'linktolinux').read_bytes(), b'executable')
        self.assertEqual(stat.S_IMODE((target / 'linktolinux').stat().st_mode), 0o755)
        self.assertFalse((target / '__main__.py').exists())
        self.assertFalse(self.paths.data.exists())

    def test_legacy_install_adopts_only_matching_shared_icons(self) -> None:
        bundle = installer.Bundle.read(self.bundle())
        app = self.paths.data / 'linktolinux'
        app.mkdir(parents=True)
        (app / 'linktolinux').write_bytes(b'previous binary')
        (app / 'LinkToLinux-Companion.apk').write_bytes(b'previous companion')
        (app / 'THIRD_PARTY_NOTICES.md').write_text('previous notices')
        (app / 'pairings.json').write_text('keep pairings')
        self.paths.bin.mkdir(parents=True)
        (self.paths.bin / 'linktolinux').symlink_to(app / 'linktolinux')
        desktop = self.paths.data / 'applications/dev.linktolinux.app.desktop'
        desktop.parent.mkdir(parents=True)
        desktop.write_text('[Desktop Entry]\nName=LinkToLinux\nIcon=dev.linktolinux.app\nExec=' + str(app / 'linktolinux') + '\n')
        for size in (32, 128, 256, 512):
            icon = self.paths.data / f'icons/hicolor/{size}x{size}/apps/dev.linktolinux.app.png'
            icon.parent.mkdir(parents=True)
            icon.write_bytes(bundle.files[f'icons/{size}.png'])
        vector = self.paths.data / 'icons/hicolor/scalable/apps/dev.linktolinux.app.svg'
        vector.parent.mkdir(parents=True)
        vector.write_bytes(bundle.files['icon.svg'])
        vector.write_bytes(b'unrelated icon')
        with self.assertRaises(installer.InstallerError):
            installer.install(bundle, self.paths, MODULE.read_bytes())
        self.assertEqual((app / 'linktolinux').read_bytes(), b'previous binary')
        self.assertEqual(vector.read_bytes(), b'unrelated icon')
        vector.write_bytes(bundle.files['icon.svg'])
        installer.install(bundle, self.paths, MODULE.read_bytes())
        self.assertEqual((app / 'linktolinux').read_bytes(), b'executable')
        installer.uninstall(self.paths)
        self.assertEqual((app / 'pairings.json').read_text(), 'keep pairings')
        self.assertFalse(vector.exists())

    def test_gui_completion_and_optional_desktop_cache_refresh(self) -> None:
        tools = self.root / 'tools'
        tools.mkdir()
        log = self.root / 'commands.jsonl'
        command_source = ('#!/usr/bin/env python3\nimport json, os, sys\n'
            'with open(os.environ["INSTALLER_TEST_LOG"], "a") as output:\n'
            '    output.write(json.dumps([os.path.basename(sys.argv[0])] + sys.argv[1:]) + "\\n")\n')
        for name in ('zenity', 'ldd', 'gtk-update-icon-cache', 'update-desktop-database'):
            command = tools / name
            command.write_text(command_source)
            command.chmod(0o755)
        archive = self.bundle()
        environment = {'PATH': str(tools) + os.pathsep + os.environ['PATH'],
                       'DISPLAY': ':installer-test', 'INSTALLER_TEST_LOG': str(log),
                       'XDG_DATA_HOME': str(self.paths.data), 'XDG_BIN_HOME': str(self.paths.bin)}
        result = subprocess.run([sys.executable, str(archive)], env={**os.environ, **environment},
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        questions = [call for call in calls if call[0] == 'zenity' and '--question' in call]
        completions = [call for call in calls if call[0] == 'zenity' and '--info' in call]
        self.assertEqual(len(questions), 1)
        self.assertEqual(len(completions), 1)
        self.assertIn('--no-markup', questions[0])
        self.assertIn('--no-markup', completions[0])
        self.assertTrue(any(call[0] == 'gtk-update-icon-cache' for call in calls))
        self.assertTrue(any(call[0] == 'update-desktop-database' for call in calls))

    def test_aliased_launcher_destination_rejected_before_mutations(self) -> None:
        paths = installer.Paths(self.root / 'data', self.root / 'data/linktolinux')
        with self.assertRaises(installer.InstallerError):
            installer.install(installer.Bundle.read(self.bundle()), paths, MODULE.read_bytes())
        self.assertFalse(paths.data.exists())
        self.assertFalse(paths.bin.exists())

    def test_xdg_roots_resolve_existing_system_symlink_once(self) -> None:
        physical = self.root / 'physical home'
        physical.mkdir()
        alias = self.root / 'home alias'
        alias.symlink_to(physical, target_is_directory=True)
        with mock.patch.dict(os.environ, {'XDG_DATA_HOME': str(alias / 'data'),
                                          'XDG_BIN_HOME': str(alias / 'bin')}):
            paths = installer.Paths.from_environment()
        self.assertEqual(paths.data, physical / 'data')
        self.assertEqual(paths.bin, physical / 'bin')
        installer.install(installer.Bundle.read(self.bundle()), paths, MODULE.read_bytes())
        self.assertTrue((physical / 'data/linktolinux/linktolinux').is_file())

    def test_update_rejects_implicit_launcher_directory_relocation(self) -> None:
        bundle = installer.Bundle.read(self.bundle())
        installer.install(bundle, self.paths, MODULE.read_bytes())
        manifest = self.paths.data / 'linktolinux/installed-files.json'
        previous = manifest.read_bytes()
        relocated = installer.Paths(self.paths.data, self.root / 'different bin')
        with self.assertRaises(installer.InstallerError):
            installer.install(bundle, relocated, MODULE.read_bytes())
        self.assertEqual(manifest.read_bytes(), previous)
        self.assertTrue((self.paths.bin / 'linktolinux').is_symlink())
        self.assertFalse(relocated.bin.exists())
        installer.uninstall(self.paths)
        self.assertFalse((self.paths.bin / 'linktolinux').is_symlink())
        self.assertFalse((self.paths.data / 'linktolinux/linktolinux').exists())

    def test_shared_icons_and_menu_are_installed_without_private_duplicates(self) -> None:
        installer.install(installer.Bundle.read(self.bundle()), self.paths, MODULE.read_bytes())
        private = self.paths.data / 'linktolinux'
        names = {str(path.relative_to(private)) for path in private.rglob('*') if path.is_file()}
        self.assertEqual(names, {'linktolinux', 'LinkToLinux-Companion.apk', 'THIRD_PARTY_NOTICES.md',
                                'LICENSES/scrcpy-Apache-2.0.txt', 'LICENSES/dependencies.txt', 'LICENSE.txt', 'INSTALL.txt',
                                'uninstall.py', 'installed-files.json'})
        self.assertTrue((self.paths.data / 'applications/dev.linktolinux.app.desktop').is_file())
        self.assertTrue((self.paths.data / 'icons/hicolor/scalable/apps/dev.linktolinux.app.svg').is_file())

    def test_old_script_quoted_launcher_and_matching_license_are_adopted(self) -> None:
        bundle = installer.Bundle.read(self.bundle())
        app = self.paths.data / 'linktolinux'
        (app / 'LICENSES').mkdir(parents=True)
        for name in ('linktolinux', 'LinkToLinux-Companion.apk', 'THIRD_PARTY_NOTICES.md',
                     'LICENSES/scrcpy-Apache-2.0.txt'):
            (app / name).write_bytes(bundle.files[name])
        self.paths.bin.mkdir(parents=True)
        launcher = self.paths.bin / 'linktolinux'
        launcher.symlink_to(app / 'linktolinux')
        desktop = self.paths.data / 'applications/dev.linktolinux.app.desktop'
        desktop.parent.mkdir(parents=True)
        old_desktop = ('[Desktop Entry]\nType=Application\nName=LinkToLinux\nExec="'
                       + str(launcher) + '"\nIcon=dev.linktolinux.app\nStartupWMClass=linktolinux\n')
        desktop.write_text(old_desktop)
        for size in (32, 128, 256, 512):
            icon = self.paths.data / f'icons/hicolor/{size}x{size}/apps/dev.linktolinux.app.png'
            icon.parent.mkdir(parents=True)
            icon.write_bytes(bundle.files[f'icons/{size}.png'])
        vector = self.paths.data / 'icons/hicolor/scalable/apps/dev.linktolinux.app.svg'
        vector.parent.mkdir(parents=True)
        vector.write_bytes(bundle.files['icon.svg'])
        desktop.write_text(old_desktop.replace('Exec="', 'Exec=unrelated-command "'))
        with self.assertRaises(installer.InstallerError):
            installer.install(bundle, self.paths, MODULE.read_bytes())
        desktop.write_text(old_desktop)
        license_file = app / 'LICENSES/scrcpy-Apache-2.0.txt'
        license_file.write_bytes(b'unrelated license')
        with self.assertRaises(installer.InstallerError):
            installer.install(bundle, self.paths, MODULE.read_bytes())
        self.assertEqual(license_file.read_bytes(), b'unrelated license')
        license_file.write_bytes(bundle.files['LICENSES/scrcpy-Apache-2.0.txt'])
        installer.install(bundle, self.paths, MODULE.read_bytes())
        installer.uninstall(self.paths)
        self.assertFalse(launcher.is_symlink())
        self.assertFalse(vector.exists())
        self.assertFalse((app / 'LICENSES/scrcpy-Apache-2.0.txt').exists())


if __name__ == '__main__':
    unittest.main()
