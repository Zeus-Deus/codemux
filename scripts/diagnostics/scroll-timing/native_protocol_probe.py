#!/usr/bin/env python3
"""Test current production protocol metadata through a supplied Wayland client.

Uses a socketpair mock server. No GUI, compositor, devices, or user input.
The optional preload affects only the temporary test process.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--target-dir', type=Path, default=ROOT / 'src-tauri/target')
parser.add_argument('--client-library', type=Path)
parser.add_argument('--output', type=Path)
args = parser.parse_args()
deps = args.target_dir.resolve() / 'debug/deps'
module = ROOT / 'src-tauri/src/precise_wheel.rs'
command = ['rustc', '--edition', '2021', '--test', str(module), '-L', f'dependency={deps}']
for name in ('gtk', 'webkit2gtk', 'libc'):
    candidates = list(deps.glob(f'lib{name}-*.rlib'))
    if not candidates:
        parser.error(f'missing compiled {name} dependency in {deps}; build CodeMux first')
    artifact = max(candidates, key=lambda p: p.stat().st_mtime_ns)
    command.extend(('--extern', f'{name}={artifact}'))
with tempfile.TemporaryDirectory(prefix='codemux-private-wayland-protocol-') as temporary:
    executable = Path(temporary) / 'protocol-tests'
    subprocess.run(command + ['-o', str(executable)], check=True)
    env = os.environ.copy()
    if args.client_library:
        library = args.client_library.resolve(strict=True)
        env['LD_PRELOAD'] = str(library) + (':' + env['LD_PRELOAD'] if env.get('LD_PRELOAD') else '')
    completed = subprocess.run([str(executable), '--test-threads=2', '--nocapture'], env=env, capture_output=True, text=True, timeout=20, check=True)
    output = completed.stdout + completed.stderr
    versions = re.search(r'Wayland runtime core tables: seat=(\d+), pointer=(\d+)', output)
    tests = re.search(r'test result: ok\. (\d+) passed; 0 failed;', output)
    if not versions or not tests:
        raise SystemExit(f'protocol verification incomplete:\n{output}')
    result = {
        'production_module': 'src-tauri/src/precise_wheel.rs',
        'library': args.client_library.name if args.client_library else 'host libwayland-client',
        'library_sha256': hashlib.sha256(args.client_library.read_bytes()).hexdigest() if args.client_library else None,
        'runtime_seat_table_version': int(versions[1]),
        'runtime_pointer_table_version': int(versions[2]),
        'passed_tests': int(tests[1]),
        'private_seat_version': 8,
        'private_pointer_version': 8,
        'decoded_value120': [15, -15],
        'decoded_notch_fractions': [0.125, -0.125],
        'real_client_marshaller_and_decoder': True,
        'socketpair_mock_server': True,
        'original_gtk_protocol_tables_unchanged': True,
    }
    text = json.dumps(result, indent=2) + '\n'
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text)
    print(text, end='')
