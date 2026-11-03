#!/usr/bin/env python3
"""Collect the license texts shipped by locked desktop dependencies."""
from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent.parent
FALLBACKS = {
    ('alloc-stdlib', '0.3.0'): 'alloc-stdlib-BSD-3-Clause.txt',
    ('dlopen2', '0.8.2'): 'dlopen2-MIT.txt',
    ('dlopen2_derive', '0.4.3'): 'dlopen2-MIT.txt',
    ('libappindicator-sys', '0.9.0'): 'libappindicator-sys-MIT.txt',
    ('selectors', '0.38.0'): 'selectors-MPL-2.0.txt',
}

@dataclass(frozen=True)
class Dependency:
    name: str
    version: str
    license: str
    source: str
    files: tuple[Path, ...]


def document(path: Path) -> dict[str, object]:
    value: object = json.loads(path.read_text(encoding='utf-8'))
    if not isinstance(value, dict):
        raise ValueError(f'Invalid package metadata: {path.name}')
    return value


def string(value: object) -> str:
    if not isinstance(value, str) or not value.strip() or '\n' in value:
        raise ValueError('Invalid dependency identity')
    return value


def license_files(folder: Path) -> tuple[Path, ...]:
    return tuple(sorted(path for path in folder.iterdir() if path.is_file()
                        and path.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'COPYRIGHT', 'NOTICE'))))


def rust_dependencies(metadata: Path) -> list[Dependency]:
    packages = document(metadata).get('packages')
    if not isinstance(packages, list):
        raise ValueError('Cargo package list is missing')
    result: list[Dependency] = []
    for package in packages:
        if not isinstance(package, dict):
            raise ValueError('Invalid Cargo package')
        name = string(package.get('name'))
        if name == 'linktolinux':
            continue
        version = string(package.get('version'))
        files = license_files(Path(string(package.get('manifest_path'))).parent)
        if not files:
            fallback = FALLBACKS.get((name, version))
            if fallback is None:
                raise ValueError(f'No license text for {name} {version}')
            files = (ROOT / 'LICENSES/upstream' / fallback,)
        result.append(Dependency(name, version, string(package.get('license')),
                                 f'https://crates.io/api/v1/crates/{name}/{version}/download', files))
    return result


def node_dependencies() -> list[Dependency]:
    manifest = document(ROOT / 'package.json').get('dependencies')
    if not isinstance(manifest, dict):
        raise ValueError('Application dependencies are missing')
    pending = [string(name) for name in manifest]
    found: dict[str, Dependency] = {}
    while pending:
        name = pending.pop()
        if name in found:
            continue
        folder = ROOT / 'node_modules' / name
        package = document(folder / 'package.json')
        version = string(package.get('version'))
        files = license_files(folder)
        if not files:
            raise ValueError(f'No license text for {name} {version}')
        found[name] = Dependency(name, version, string(package.get('license')),
                                  f'https://www.npmjs.com/package/{name}/v/{version}', files)
        children = package.get('dependencies', {})
        if not isinstance(children, dict):
            raise ValueError(f'Invalid dependencies for {name}')
        pending.extend(string(child) for child in children)
    return list(found.values())


def collect(metadata: Path) -> None:
    dependencies = sorted(rust_dependencies(metadata) + node_dependencies(), key=lambda item: (item.name, item.version))
    texts: dict[str, str] = {}
    lines = ['LinkToLinux dependency licenses', '',
             'These licenses apply to the named third-party packages, independently of the LinkToLinux Personal Use License.',
             'Source URLs identify the unmodified dependency versions, including MPL-covered source files.', '']
    for package in dependencies:
        references: list[str] = []
        for path in package.files:
            data = path.read_bytes()
            if len(data) > 1048576:
                raise ValueError(f'License file exceeds size limit: {path.name}')
            text = data.decode('utf-8-sig').strip()
            identity = hashlib.sha256(text.encode()).hexdigest()[:20]
            texts[identity] = text
            references.append(identity)
        lines.extend((f'{package.name} {package.version}', f'License: {package.license}',
                      f'Source: {package.source}', 'License texts: ' + ', '.join(references), ''))
    for identity, text in sorted(texts.items()):
        lines.extend(('=' * 72, f'License text {identity}', '', text, ''))
    (ROOT / 'LICENSES/dependencies.txt').write_text('\n'.join(lines), encoding='utf-8')
    print(f'Collected licenses for {len(dependencies)} dependencies')


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cargo-metadata', required=True, type=Path)
    args = parser.parse_args()
    try:
        collect(args.cargo_metadata)
        return 0
    except (OSError, ValueError) as error:
        print(f'Cannot collect licenses: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
