#!/usr/bin/env python3
"""Fetch pinned native additions and generate locked resources into ignored Research/."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from target_config import checked_ignored_path
HERE = Path(__file__).resolve().parent

def verify(source, descriptor):
    head = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    if head != descriptor['commit']:
        raise ValueError('Source pin mismatch: ' + descriptor['id'])
    subprocess.run(['git', '-C', str(source), 'diff', '--exit-code', 'HEAD'], check=True)
    for name, digest in descriptor['inputs'].items():
        if hashlib.sha256((source / name).read_bytes()).hexdigest() != digest:
            raise ValueError('Changed pinned source: ' + name)
    headers = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
               for p in (source / 'build/gen').glob('*.h')}
    if headers != descriptor['generated_headers_sha256']:
        raise ValueError('Exact generated headers differ: ' + descriptor['id'])

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    args = parser.parse_args()
    root = args.source.resolve()
    checked_ignored_path(root, HERE.parents[1])
    for item in json.loads((HERE / 'additions.lock.json').read_text())['firmwares']:
        source = root / item['id']
        if not source.exists():
            root.mkdir(parents=True, exist_ok=True)
            subprocess.run(['git', 'clone', item['source'], str(source)], check=True)
            subprocess.run(['git', '-C', str(source), 'checkout', '--detach', item['commit']], check=True)
        head = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
        if head != item['commit']:
            raise ValueError('Source pin mismatch: ' + item['id'])
        subprocess.run(['git', '-C', str(source), 'diff', '--exit-code', 'HEAD'], check=True)
        subprocess.run([sys.executable, '-c',
            'import sys; sys.path.insert(0, sys.argv[1]+"/tools"); import build; build.generate()', str(source)], check=True)
        verify(source, item)
        print('Verified pinned native source/resources: ' + item['id'])

if __name__ == '__main__':
    main()
