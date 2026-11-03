"""Offline, per-user LinkToLinux installer using only Python's standard library."""
from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import zipfile
from collections.abc import Sequence

APP_ID = 'dev.linktolinux.app'
REQUIRED = frozenset(('linktolinux', 'LinkToLinux-Companion.apk', 'icon.svg',
    'icons/32.png', 'icons/128.png', 'icons/256.png', 'icons/512.png',
    'linktolinux.desktop', 'THIRD_PARTY_NOTICES.md',
    'LICENSES/scrcpy-Apache-2.0.txt', 'LICENSES/dependencies.txt', 'LICENSE.txt', 'INSTALL.txt'))
PRIVATE_FILES = frozenset(('linktolinux', 'LinkToLinux-Companion.apk',
    'THIRD_PARTY_NOTICES.md', 'LICENSES/scrcpy-Apache-2.0.txt', 'LICENSES/dependencies.txt', 'LICENSE.txt', 'INSTALL.txt'))
MAX_ARCHIVE = 256 * 1024 * 1024


class InstallerError(Exception):
    """A validated failure suitable for display to an end user."""


def object_value(value: object, description: str) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise InstallerError(f'Invalid {description}.')
    return value


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def valid_hash(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None


def relative_name(name: str) -> bool:
    path = PurePosixPath(name)
    return bool(name) and not path.is_absolute() and str(path) == name and all(
        part not in ('.', '..') for part in path.parts) and '\\' not in name and '\x00' not in name


@dataclass(frozen=True)
class Bundle:
    metadata: dict[str, object]
    files: dict[str, bytes]

    @classmethod
    def read(cls, path: Path) -> Bundle:
        try:
            with zipfile.ZipFile(path) as archive:
                entries = archive.infolist()
                names = [entry.filename for entry in entries]
                expected = {'__main__.py', 'manifest.json'} | {'payload/' + name for name in REQUIRED}
                if len(names) != len(set(names)) or set(names) != expected:
                    raise InstallerError('Installer archive contains unexpected or duplicate paths.')
                total = 0
                for entry in entries:
                    mode = entry.external_attr >> 16
                    total += entry.file_size
                    if (not relative_name(entry.filename) or stat.S_ISLNK(mode)
                            or entry.is_dir() or entry.flag_bits & 1 or total > MAX_ARCHIVE
                            or (stat.S_IFMT(mode) not in (0, stat.S_IFREG))):
                        raise InstallerError('Unsafe or oversized installer archive.')
                if archive.getinfo('manifest.json').file_size > 65536:
                    raise InstallerError('Oversized installer manifest.')
                metadata = object_value(json.loads(archive.read('manifest.json')), 'manifest')
                if (type(metadata.get('format')) is not int or metadata.get('format') != 1 or metadata.get('app_id') != APP_ID
                        or metadata.get('name') != 'LinkToLinux'
                        or not isinstance(metadata.get('version'), str)
                        or re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?', str(metadata['version'])) is None
                        or metadata.get('architecture') not in ('x86_64', 'aarch64')
                        or not isinstance(metadata.get('minimum_glibc'), str)
                        or re.fullmatch(r'[0-9]+\.[0-9]+', str(metadata['minimum_glibc'])) is None):
                    raise InstallerError('Unsupported installer metadata.')
                records = object_value(metadata.get('files'), 'payload records')
                if set(records) != REQUIRED:
                    raise InstallerError('Installer payload is incomplete.')
                files: dict[str, bytes] = {}
                for name, value in records.items():
                    record = object_value(value, 'payload record')
                    size = record.get('size')
                    mode = record.get('mode')
                    if (not valid_hash(record.get('sha256')) or type(size) is not int
                            or size < 0 or size > MAX_ARCHIVE or type(mode) is not int
                            or mode != (0o755 if name == 'linktolinux' else 0o644)):
                        raise InstallerError(f'Invalid payload metadata: {name}')
                    if archive.getinfo('payload/' + name).file_size != size:
                        raise InstallerError(f'Integrity check failed: {name}')
                    content = archive.read('payload/' + name)
                    if digest(content) != record['sha256']:
                        raise InstallerError(f'Integrity check failed: {name}')
                    files[name] = content
                return cls(metadata, files)
        except (OSError, ValueError, zipfile.BadZipFile, RuntimeError) as error:
            raise InstallerError(f'Cannot verify installer: {error}') from error


@dataclass(frozen=True)
class Paths:
    data: Path
    bin: Path

    @classmethod
    def from_environment(cls) -> Paths:
        home = Path.home()
        data = Path(os.environ.get('XDG_DATA_HOME', str(home / '.local/share')))
        binary = Path(os.environ.get('XDG_BIN_HOME', str(home / '.local/bin')))
        if not data.is_absolute() or not binary.is_absolute():
            raise InstallerError('XDG_DATA_HOME and XDG_BIN_HOME must be absolute paths.')
        return cls(data.resolve(), binary.resolve())


def check_runtime(bundle: Bundle) -> None:
    if platform.system() != 'Linux' or platform.machine() != bundle.metadata['architecture']:
        raise InstallerError(f"This installer requires Linux {bundle.metadata['architecture']}; detected {platform.system()} {platform.machine()}.")
    try:
        libc = os.confstr('CS_GNU_LIBC_VERSION') or ''
    except (OSError, ValueError):
        libc = ''
    match = re.fullmatch(r'glibc ([0-9]+)\.([0-9]+)', libc)
    minimum = tuple(int(part) for part in str(bundle.metadata['minimum_glibc']).split('.'))
    if match is None or tuple(map(int, match.groups())) < minimum:
        raise InstallerError(f"GNU glibc {bundle.metadata['minimum_glibc']} or newer is required. Use a supported glibc-based distribution.")
    ldd = shutil.which('ldd')
    if ldd is None:
        raise InstallerError('Install your distribution\'s glibc runtime tools (ldd), then run the installer again.')
    with tempfile.TemporaryDirectory(prefix='linktolinux-check-') as temporary:
        binary = Path(temporary) / 'linktolinux'
        binary.write_bytes(bundle.files['linktolinux'])
        binary.chmod(0o700)
        try:
            result = subprocess.run([ldd, str(binary)], capture_output=True, text=True, timeout=20, check=False)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise InstallerError(f'Unable to check desktop runtime libraries: {error}') from error
        output = result.stdout + result.stderr
        if result.returncode != 0 or 'not found' in output or re.search(r'version .* not found', output):
            details = '\n'.join(line.strip() for line in output.splitlines() if 'not found' in line)
            raise InstallerError('Missing or incompatible desktop runtime libraries. Install your distribution packages providing WebKitGTK 4.1, GTK 3, libsoup 3, and glibc.\n' + (details or output.strip()))


def desktop_quote(value: str) -> str:
    if any(ord(character) < 32 for character in value):
        raise InstallerError('Installation paths must not contain control characters.')
    escaped = value.replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
    # Exec quoting is decoded after the desktop keyfile unescapes backslashes.
    return '"' + escaped.replace('\\', '\\\\') + '"'


def destinations(paths: Paths) -> dict[str, Path]:
    result = {'data:linktolinux/' + name: paths.data / 'linktolinux' / name for name in PRIVATE_FILES}
    result['data:linktolinux/uninstall.py'] = paths.data / 'linktolinux/uninstall.py'
    result['data:applications/' + APP_ID + '.desktop'] = paths.data / 'applications' / (APP_ID + '.desktop')
    result['data:icons/hicolor/scalable/apps/' + APP_ID + '.svg'] = paths.data / 'icons/hicolor/scalable/apps' / (APP_ID + '.svg')
    for size in (32, 128, 256, 512):
        name = f'icons/hicolor/{size}x{size}/apps/{APP_ID}.png'
        result['data:' + name] = paths.data / name
    result['bin:linktolinux'] = paths.bin / 'linktolinux'
    checked = [*result.values(), paths.data / 'linktolinux/installed-files.json']
    # Resolve parents, preserving the final launcher symlink during normal upgrades.
    physical = [path.parent.resolve() / path.name for path in checked]
    if len(physical) != len(set(physical)) or any(
            first in second.parents for first in physical for second in physical if first != second):
        raise InstallerError('Installation destinations overlap. Set XDG_BIN_HOME to a separate executable directory.')
    return result


def safe_parents(path: Path) -> None:
    for parent in path.parents:
        if parent.is_symlink():
            raise InstallerError(f'Refusing a symbolic-link installation directory: {parent}')
        if parent.exists() and not parent.is_dir():
            raise InstallerError(f'Installation directory is not a directory: {parent}')


def ownership(paths: Paths) -> dict[str, object] | None:
    manifest = paths.data / 'linktolinux/installed-files.json'
    safe_parents(manifest)
    if manifest.is_symlink():
        raise InstallerError('Ownership manifest must not be a symbolic link.')
    if not manifest.exists():
        return None
    try:
        if manifest.stat().st_size > 65536:
            raise InstallerError('Oversized ownership manifest.')
        record = object_value(json.loads(manifest.read_text()), 'ownership manifest')
        files = object_value(record.get('files'), 'owned files')
        bin_home = record.get('bin_home')
        if not isinstance(bin_home, str) or not Path(bin_home).is_absolute():
            raise InstallerError('Invalid recorded launcher directory.')
        if Path(bin_home).resolve() != paths.bin.resolve():
            raise InstallerError(f'This installation uses launcher directory {bin_home}. Rerun with XDG_BIN_HOME set to that directory, or uninstall the old installation before moving it.')
        allowed = destinations(paths)
        if record.get('format') != 1 or record.get('app_id') != APP_ID or not set(files) <= set(allowed):
            raise InstallerError('Invalid ownership manifest paths.')
        for key, value in files.items():
            details = object_value(value, 'owned file')
            if key == 'bin:linktolinux':
                if details != {'target': str(paths.data / 'linktolinux/linktolinux')}:
                    raise InstallerError('Invalid owned launcher target.')
            elif not valid_hash(details.get('sha256')) or set(details) != {'sha256'}:
                raise InstallerError('Invalid owned file hash.')
        return record
    except (OSError, ValueError) as error:
        raise InstallerError(f'Cannot read ownership manifest: {error}') from error


def matches(path: Path, record: object) -> bool:
    details = object_value(record, 'owned file')
    if 'target' in details:
        return path.is_symlink() and os.readlink(path) == details['target']
    return not path.is_symlink() and path.is_file() and digest(path.read_bytes()) == details.get('sha256')


def atomic_write(path: Path, content: bytes, mode: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix='.linktolinux-', dir=path.parent)
    try:
        with os.fdopen(descriptor, 'wb') as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, mode)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def legacy_desktop_matches(path: Path, binary: Path, launcher: Path) -> bool:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 65536:
        return False
    try:
        text = path.read_text()
    except (OSError, UnicodeError):
        return False
    fields: dict[str, str] = {}
    section = ''
    main_sections = 0
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith('#'):
            continue
        if line.startswith('[') and line.endswith(']'):
            section = line[1:-1]
            if section == 'Desktop Entry':
                main_sections += 1
            continue
        if section != 'Desktop Entry':
            continue
        key, separator, value = line.partition('=')
        if not separator or key in fields:
            return False
        fields[key] = value
    expected_exec = {'linktolinux', str(binary), desktop_quote(str(binary)),
                     str(launcher), desktop_quote(str(launcher)),
                     '"' + str(binary) + '"', '"' + str(launcher) + '"'}
    return (main_sections == 1 and fields.get('Name') == 'LinkToLinux'
            and fields.get('Icon') == APP_ID and fields.get('Exec') in expected_exec)


def install(bundle: Bundle, paths: Paths, source: bytes) -> None:
    targets = destinations(paths)
    old = ownership(paths)
    old_files = object_value(old['files'], 'owned files') if old else {}
    binary = paths.data / 'linktolinux/linktolinux'
    contents = {'data:linktolinux/' + name: bundle.files[name] for name in PRIVATE_FILES}
    contents['data:linktolinux/uninstall.py'] = source
    uninstaller = paths.data / 'linktolinux/uninstall.py'
    desktop = ('[Desktop Entry]\nType=Application\nName=LinkToLinux\nComment=Connect your Android phone\n'
        f'Exec={desktop_quote(str(binary))}\nIcon={APP_ID}\nTerminal=false\nCategories=Utility;\n'
        'StartupNotify=true\nStartupWMClass=linktolinux\n'
        'Actions=Uninstall;\n\n[Desktop Action Uninstall]\nName=Uninstall LinkToLinux\n'
        f'Exec=python3 {desktop_quote(str(uninstaller))} --uninstall\n')
    contents['data:applications/' + APP_ID + '.desktop'] = desktop.encode()
    contents['data:icons/hicolor/scalable/apps/' + APP_ID + '.svg'] = bundle.files['icon.svg']
    for size in (32, 128, 256, 512):
        contents[f'data:icons/hicolor/{size}x{size}/apps/{APP_ID}.png'] = bundle.files[f'icons/{size}.png']
    legacy_launcher = paths.bin / 'linktolinux'
    legacy_desktop = paths.data / 'applications' / (APP_ID + '.desktop')
    safe_parents(legacy_launcher)
    safe_parents(legacy_desktop)
    legacy_layout = (old is None and legacy_launcher.is_symlink()
        and os.readlink(legacy_launcher) == str(binary)
        and legacy_desktop_matches(legacy_desktop, binary, legacy_launcher))
    for key, path in targets.items():
        safe_parents(path)
        if not (path.exists() or path.is_symlink()):
            continue
        if key in old_files and matches(path, old_files[key]):
            continue
        if key == 'bin:linktolinux' and path.is_symlink() and os.readlink(path) == str(binary):
            continue
        # The old installer owned only these files and its exact application menu entry.
        legacy = {'linktolinux', 'LinkToLinux-Companion.apk', 'THIRD_PARTY_NOTICES.md'}
        if legacy_layout and key in {'data:linktolinux/' + name for name in legacy} and not path.is_symlink() and path.is_file():
            continue
        if legacy_layout and key == 'data:applications/' + APP_ID + '.desktop':
            continue
        if (legacy_layout and key == 'data:linktolinux/LICENSES/scrcpy-Apache-2.0.txt'
                and not path.is_symlink() and path.is_file()
                and path.stat().st_size == len(contents[key])
                and digest(path.read_bytes()) == digest(contents[key])):
            continue
        if (legacy_layout and key.startswith('data:icons/hicolor/') and not path.is_symlink()
                and path.is_file() and path.stat().st_size == len(contents[key])
                and digest(path.read_bytes()) == digest(contents[key])):
            continue
        raise InstallerError(f'Refusing to overwrite an unrelated or modified file: {path}')
    owned: dict[str, object] = {key: {'sha256': digest(content)} for key, content in contents.items()}
    owned['bin:linktolinux'] = {'target': str(binary)}
    record = json.dumps({'format': 1, 'app_id': APP_ID, 'version': bundle.metadata['version'],
                         'bin_home': str(paths.bin), 'files': owned}, indent=2).encode()
    snapshots: dict[Path, tuple[bytes | str, int] | None] = {}
    manifest = paths.data / 'linktolinux/installed-files.json'
    try:
        for key, path in targets.items():
            snapshots[path] = ((os.readlink(path), 0) if path.is_symlink() else
                (path.read_bytes(), stat.S_IMODE(path.stat().st_mode)) if path.exists() else None)
            if key == 'bin:linktolinux':
                path.parent.mkdir(parents=True, exist_ok=True)
                with tempfile.TemporaryDirectory(prefix='.linktolinux-', dir=path.parent) as temporary:
                    link = Path(temporary) / 'launcher'
                    link.symlink_to(binary)
                    os.replace(link, path)
            else:
                atomic_write(path, contents[key], 0o755 if key == 'data:linktolinux/linktolinux' else 0o644)
        atomic_write(manifest, record, 0o600)
    except (OSError, KeyboardInterrupt):
        for path, snapshot in reversed(list(snapshots.items())):
            if snapshot is None:
                path.unlink(missing_ok=True)
            elif isinstance(snapshot[0], str):
                path.unlink(missing_ok=True)
                path.symlink_to(snapshot[0])
            else:
                atomic_write(path, snapshot[0], snapshot[1])
        raise


def uninstall(paths: Paths) -> None:
    record = ownership(paths)
    if record is None:
        raise InstallerError('No recorded LinkToLinux installation was found.')
    files = object_value(record['files'], 'owned files')
    targets = destinations(paths)
    for key in files:
        safe_parents(targets[key])
    for key, details in files.items():
        path = targets[key]
        if matches(path, details):
            path.unlink()
        elif path.exists() or path.is_symlink():
            print(f'Preserved modified file: {path}')
    manifest = paths.data / 'linktolinux/installed-files.json'
    manifest.unlink()
    folders = {parent for key in files for parent in targets[key].parents
               if parent != paths.data and parent != paths.bin and paths.data in parent.parents}
    for folder in sorted(folders, key=lambda path: len(path.parts), reverse=True):
        if folder.is_dir() and not folder.is_symlink() and not any(folder.iterdir()):
            folder.rmdir()


def gui_available(no_gui: bool) -> bool:
    return not no_gui and bool(os.environ.get('DISPLAY') or os.environ.get('WAYLAND_DISPLAY')) and shutil.which('zenity') is not None


def confirm(message: str, yes: bool, gui: bool) -> bool:
    if yes:
        return True
    if gui:
        try:
            return subprocess.run(['zenity', '--question', '--no-markup', '--icon=' + APP_ID,
                '--title=LinkToLinux', '--text=' + message], timeout=300, check=False).returncode == 0
        except (OSError, subprocess.TimeoutExpired) as error:
            raise InstallerError(f'Confirmation dialog unavailable: {error}') from error
    if not sys.stdin.isatty():
        raise InstallerError('Use --yes --no-gui for unattended installation.')
    return input(message + ' [y/N] ').strip().lower() in ('y', 'yes')


def launch(paths: Paths) -> None:
    binary = paths.data / 'linktolinux/linktolinux'
    deadline = time.monotonic() + 1
    with os.scandir('/proc') as entries:
        for index, entry in enumerate(entries):
            if index >= 16384 or time.monotonic() > deadline:
                break
            if not entry.name.isdecimal():
                continue
            try:
                if entry.stat(follow_symlinks=False).st_uid != os.geteuid():
                    continue
                executable = os.readlink(Path(entry.path) / 'exe')
            except (FileNotFoundError, PermissionError, ProcessLookupError):
                continue
            if executable.removesuffix(' (deleted)') == str(binary):
                print('LinkToLinux is already running; open its existing window.')
                return
    try:
        with open(os.devnull, 'wb') as output:
            process = subprocess.Popen([str(binary)], stdin=subprocess.DEVNULL, stdout=output,
                                       stderr=output, start_new_session=True)
        try:
            status = process.wait(timeout=2)
            if status != 0:
                print('The launch request exited; open LinkToLinux from your application menu.')
        except subprocess.TimeoutExpired:
            return
    except OSError as error:
        raise InstallerError(f'Cannot launch LinkToLinux: {error}') from error


def refresh_desktop(paths: Paths) -> None:
    commands = (
        ('gtk-update-icon-cache', '-f', '-t', str(paths.data / 'icons/hicolor')),
        ('update-desktop-database', str(paths.data / 'applications')),
    )
    for command in commands:
        executable = shutil.which(command[0])
        if executable is None or not Path(command[-1]).is_dir():
            continue
        try:
            result = subprocess.run([executable, *command[1:]], capture_output=True,
                                    text=True, timeout=10, check=False)
            if result.returncode != 0:
                print(f'Desktop cache refresh failed ({command[0]}); your desktop may refresh after signing in again.', file=sys.stderr)
        except (OSError, subprocess.TimeoutExpired) as error:
            print(f'Desktop cache refresh unavailable ({command[0]}): {error}', file=sys.stderr)


def completion(message: str, gui: bool) -> None:
    print(message)
    if gui:
        try:
            subprocess.run(['zenity', '--info', '--no-markup', '--icon=' + APP_ID,
                            '--title=LinkToLinux', '--text=' + message], timeout=120, check=False)
        except (OSError, subprocess.TimeoutExpired) as error:
            print(f'Cannot display completion dialog: {error}', file=sys.stderr)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description='Offline per-user LinkToLinux installer')
    parser.add_argument('--yes', action='store_true', help='Accept installation or removal')
    parser.add_argument('--no-gui', action='store_true', help='Use terminal output')
    parser.add_argument('--launch', action='store_true', help='Launch after installation')
    actions = parser.add_mutually_exclusive_group()
    actions.add_argument('--check', action='store_true', help='Verify payload and runtime without installing')
    actions.add_argument('--extract', metavar='DIR', type=Path, help='Extract verified payload for inspection')
    actions.add_argument('--uninstall', action='store_true', help='Remove recorded application files')
    args = parser.parse_args(argv)
    gui = gui_available(args.no_gui)
    try:
        paths = Paths.from_environment()
        if args.uninstall:
            script = Path(sys.argv[0]).absolute()
            if script.name == 'uninstall.py' and script.parent.name == 'linktolinux':
                data = script.parent.parent
                manifest = data / 'linktolinux/installed-files.json'
                safe_parents(manifest)
                if manifest.is_symlink() or not manifest.is_file() or manifest.stat().st_size > 65536:
                    raise InstallerError('Invalid installed ownership manifest.')
                saved = object_value(json.loads(manifest.read_text()), 'ownership manifest')
                bin_home = saved.get('bin_home')
                if not isinstance(bin_home, str) or not Path(bin_home).is_absolute():
                    raise InstallerError('Invalid installed launcher directory.')
                paths = Paths(data, Path(bin_home))
            if confirm('Remove LinkToLinux? Pairings, preferences and personal files are preserved.', args.yes, gui):
                uninstall(paths)
                refresh_desktop(paths)
                completion('LinkToLinux removed. Personal files were preserved.', gui and not args.yes)
            return 0
        bundle = Bundle.read(Path(sys.argv[0]).absolute())
        if args.extract is not None:
            target = args.extract.absolute()
            safe_parents(target / 'manifest.json')
            if target.exists() and (not target.is_dir() or any(target.iterdir())):
                raise InstallerError('Extraction directory must be empty or absent.')
            for name, content in bundle.files.items():
                atomic_write(target / name, content, 0o755 if name == 'linktolinux' else 0o644)
            atomic_write(target / 'manifest.json', json.dumps(bundle.metadata, indent=2).encode(), 0o644)
            print(f'Verified payload extracted to {target}')
            return 0
        check_runtime(bundle)
        if args.check:
            print('Payload integrity and required desktop runtime checks passed.')
            print('Optional features may require adb, ffmpeg or desktop integration tools; see INSTALL.txt.')
            return 0
        if os.geteuid() == 0:
            raise InstallerError('Run this installer as your desktop user, without sudo.')
        if not confirm(f"Install LinkToLinux {bundle.metadata['version']} for your user?", args.yes, gui):
            return 0
        with zipfile.ZipFile(Path(sys.argv[0])) as archive:
            source = archive.read('__main__.py')
        install(bundle, paths, source)
        refresh_desktop(paths)
        completion(f'Installed LinkToLinux to {paths.data / "linktolinux"}. Open LinkToLinux from your application menu.',
                   gui and not args.yes and not args.launch)
        if args.launch:
            launch(paths)
        return 0
    except (InstallerError, OSError, EOFError, ValueError) as error:
        message = str(error)
        print('LinkToLinux: ' + message, file=sys.stderr)
        if gui:
            try:
                subprocess.run(['zenity', '--error', '--no-markup', '--icon=' + APP_ID, '--title=LinkToLinux',
                                '--text=' + message], timeout=120, check=False)
            except (OSError, subprocess.TimeoutExpired) as dialog_error:
                print(f'Cannot display error dialog: {dialog_error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
