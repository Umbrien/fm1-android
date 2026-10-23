#!/usr/bin/env python3
"""Compile actual ChoralRoot/GHOULBOX firmware through their source-host HALs."""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
from additions_prepare import verify
from expansion_build import namespace_ir
from target_config import Toolchain, TRIPLES, checked_ignored_path
HERE = Path(__file__).resolve().parent

def run(*args):
    subprocess.run([str(a) for a in args], check=True)

def adapt(source, kind):
    if kind == 'jiant':
        return
    if kind == 'choralroot':
        # Run the source host's second-part job inline on the owning serial worker.
        p = source / 'firmware/src/cr_cpu1.h'
        text = p.read_text()
        anchor = '#if !defined(__EMSCRIPTEN__)'
        if text.count(anchor) != 1:
            raise ValueError('Serial CPU job adaptation anchor changed')
        p.write_text(text.replace(anchor, '#if !defined(FM1_SERIAL_HOST) && !defined(__EMSCRIPTEN__)'))
        return
    # The hardware stores 32-bit sample offsets, not truncated 64-bit host pointers.
    p = source / 'firmware/src/eng_sample.c'
    old = 'base = (uint32_t)(uintptr_t)(smp_user_xip(k) + SMP_USER_DATA) - (uint32_t)(uintptr_t)SMP_DATA;'
    text = p.read_text()
    if text.count(old) != 1:
        raise ValueError('Sample offset adaptation anchor changed')
    p.write_text(text.replace(old, 'base = 0x80000000u | (SMP_USER_BASE + k * SMP_USER_SIZE + SMP_USER_DATA);'))
    for name in ['eng_sample.c', 'eng_grain.c', 'eng_slice.c']:
        p = source / 'firmware/src' / name
        text, count = re.subn(r'SMP_DATA\[([^\]]+)\]', r'host_sample_byte(\1)', p.read_text())
        if count != 1:
            raise ValueError('Sample reader anchor changed: ' + name)
        p.write_text(text)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--target', required=True, choices=TRIPLES)
    parser.add_argument('--sdk', required=True)
    args = parser.parse_args()
    tc = Toolchain(args.target, args.sdk, llvm=True)
    out = args.out.resolve()
    checked_ignored_path(out, HERE.parents[1])
    out.mkdir(parents=True, exist_ok=True)
    objects = []
    for item in json.loads((HERE / 'additions.lock.json').read_text())['firmwares']:
        source = (args.source / item['id']).resolve()
        checked_ignored_path(source, HERE.parents[1])
        verify(source, item)
        work = out / item['id']
        upstream = work / 'upstream'
        for name in ['firmware', 'tools/emu', 'tests', 'web/emu']:
            if (source / name).exists():
                shutil.copytree(source / name, upstream / name, dirs_exist_ok=True)
        adapt(upstream, item['id'])
        wrapper = work / 'native'
        wrapper.mkdir(exist_ok=True)
        shutil.copy2(HERE / 'additions_core.c', wrapper / 'additions_core.c')
        marker = work / 'state-marker.h'
        marker.write_text('#include <stdint.h>\n#include <stddef.h>\n#include <stdlib.h>\n#include <stdio.h>\n#include <string.h>\n#include <math.h>\n#include <time.h>\n#include <pthread.h>\n'
            + f'#pragma clang section bss="{tc.bss}" data="{tc.data}"\n')
        flags = ['-target', TRIPLES[args.target], *tc.flags, '-O2', '-std=gnu11',
            '-fwrapv', '-fno-strict-aliasing', '-ffp-contract=off', '-w',
            '-include', marker, '-DFEL_PREFIX=' + item['prefix'], *tc.sections,
            '-DFM1_JIANT=' + str(int(item['id'] == 'jiant')),
            '-DFM1_CHORALROOT=' + str(int(item['id'] == 'choralroot')),
            '-DEMU_WEB', '-DFM1_SERIAL_HOST=1', '-DFELUCCA_SLICER=0' if item['id'] == 'choralroot' else '-DFELUCCA_SLICER=1',
            '-DFELUCCA_VERSION="' + item['package']['name'] + ' ' + item['package']['version'] + '"',
            '-I' + str(HERE), '-I' + str(source / 'build/gen'),
            '-I' + str(upstream / 'firmware/src'),
            '-I' + str(upstream / 'tools/emu/web/compat')]
        ir, namespaced, state, obj = [work / name for name in ['core.ll', 'namespaced.ll', 'state.ll', 'core.o']]
        run(tc.cc, *flags, '-S', '-emit-llvm', '-Xclang', '-disable-llvm-passes', wrapper / 'additions_core.c', '-o', ir)
        namespaced.write_text(namespace_ir(ir.read_text(), item['prefix']))
        run(sys.executable, HERE / 'fel_state.py', namespaced, state, item['prefix'])
        run(tc.cc, '-target', TRIPLES[args.target], *tc.flags, '-O2', '-ffp-contract=off', '-c', state, '-o', obj)
        objects.append(obj)
    run(*tc.ar, 'crs', out / 'libfm1_native_additions.a', *objects)

if __name__ == '__main__':
    main()
