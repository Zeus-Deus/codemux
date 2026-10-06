#!/usr/bin/env python3
"""Compile the current native bridge with a temporary WebKit verification main.

Requires a Wayland desktop and built src-tauri/target/debug/deps. It opens one
non-focusing mock window, generates native input, and closes automatically.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--target-dir', type=Path, default=ROOT / 'src-tauri/target')
parser.add_argument('--output', type=Path)
mode = parser.add_mutually_exclusive_group()
mode.add_argument('--frontend-url', help='Vite server URL; also run the current production wheel animation')
mode.add_argument('--native', action='store_true', help='Observe passive wheel events and let WebKit perform native scrolling')
parser.add_argument('--clock-preload', type=Path, help='Optional production vblank fallback .so, applied only to the probe process')
args = parser.parse_args()
deps = args.target_dir.resolve() / 'debug/deps'
module = ROOT / 'src-tauri/src/precise_wheel.rs'
command = ['rustc', '--edition', '2021', '-L', f'dependency={deps}']
for name in ('gtk', 'webkit2gtk', 'libc', 'javascriptcore'):
    candidates = list(deps.glob(f'lib{name}-*.rlib'))
    if not candidates:
        parser.error(f'missing compiled {name} dependency in {deps}; build CodeMux first')
    artifact = max(candidates, key=lambda p: p.stat().st_mtime_ns)
    command.extend(('--extern', f'{name}={artifact}'))
with tempfile.TemporaryDirectory(prefix='codemux-native-precision-') as temporary:
    temporary = Path(temporary)
    source = temporary / 'probe.rs'
    executable = temporary / 'probe'
    # Read production code for every run; no copied implementation is committed.
    source.write_text(module.read_text() + '\n' + (HERE / 'native_precision_probe_main.rs').read_text())
    subprocess.run(command + [str(source), '-o', str(executable)], check=True)
    env = os.environ.copy()
    env.pop('CODEMUX_PRECISION_FRONTEND_URL', None)
    env.pop('CODEMUX_PRECISION_NATIVE', None)
    env.setdefault('WEBKIT_DMABUF_RENDERER_FORCE_SHM', '1')
    if args.frontend_url:
        env['CODEMUX_PRECISION_FRONTEND_URL'] = args.frontend_url
    if args.native:
        env['CODEMUX_PRECISION_NATIVE'] = '1'
    if args.clock_preload:
        preload = str(args.clock_preload.resolve(strict=True))
        env['LD_PRELOAD'] = preload + (':' + env['LD_PRELOAD'] if env.get('LD_PRELOAD') else '')
    completed = subprocess.run([str(executable)], env=env, capture_output=True, text=True, timeout=20, check=True)
    if 'available=true' not in completed.stdout:
        raise SystemExit('value120 bridge unavailable on this display; native probe requires a single version-8 Wayland seat')
    payloads = [line[4:] for line in completed.stdout.splitlines() if line.startswith('DOM ')]
    if len(payloads) != 1:
        raise SystemExit(f'probe did not complete: {completed.stdout}\n{completed.stderr}')
    result = json.loads(payloads[0])
    assert result['native'] == args.native
    assert result['frontend'] == bool(args.frontend_url)
    events = result['events']
    assert len(events) == 64, f'expected 64 fractional events, got {len(events)}'
    assert all(event['trusted'] and event['cancelable'] for event in events)
    distance = events[0]['deltaY']
    assert distance > 0 and all(event['deltaY'] == distance for event in events)
    assert result['total'] == 64 * distance
    expected_prevented = 0 if args.native else 64
    assert result['prevented'] == expected_prevented, f'expected {expected_prevented} prevented wheel events'
    assert abs(result['offset'] - result['total']) <= 1, 'scroll destination did not preserve fractional distance'
    result['validation'] = {
        'input_frames': 64,
        'input_value120': 15,
        'gtk_legacy_duplicate_frames': 8,
        'observed_dom_events': len(events),
        'all_trusted': True,
        'all_cancelable': True,
        'duplicates_suppressed': True,
        'distance_preserved': True,
        'production_module': 'src-tauri/src/precise_wheel.rs',
        'scroll_mode': 'native' if args.native else ('frontend' if args.frontend_url else 'manual-accumulator'),
        'no_javascript_scroll_writes': args.native,
        'fractional_paint_measured': False,
    }
    text = json.dumps(result, indent=2) + '\n'
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text)
    print(text, end='')
