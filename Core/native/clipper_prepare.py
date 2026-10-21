#!/usr/bin/env python3
"""Prepare the exact experimental Clipper source and generated resources."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
HERE = Path(__file__).resolve().parent
lock = json.loads((HERE / 'clipper.lock.json').read_text())
if len(sys.argv) != 2:
    sys.exit('usage: clipper_prepare.py IGNORED_SOURCE_DIRECTORY')
from target_config import checked_ignored_path
source = checked_ignored_path(Path(sys.argv[1]), HERE.parents[1])
if not source.exists():
    source.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(['git', 'clone', '--depth', '1', '--branch', lock['tag'], lock['source'], str(source)], check=True)
if subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip() != lock['commit']:
    sys.exit('Clipper source pin mismatch')
subprocess.run(['git', '-C', str(source), 'diff', '--exit-code', 'HEAD'], check=True)
spec = importlib.util.spec_from_file_location('clipper_build', source / 'tools/build.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.generate()
for name, expected in lock['generated_headers_sha256'].items():
    if hashlib.sha256((source / 'build/gen' / name).read_bytes()).hexdigest() != expected:
        sys.exit('Clipper generated resource mismatch: ' + name)
for name, expected in lock['notices_sha256'].items():
    if hashlib.sha256((source / name).read_bytes()).hexdigest() != expected:
        sys.exit('Clipper notice mismatch: ' + name)
print('Verified Clipper ' + lock['tag'])
