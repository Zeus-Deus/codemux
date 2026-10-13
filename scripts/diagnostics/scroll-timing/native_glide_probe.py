#!/usr/bin/env python3
"""Compare native direct wheel dispatch with fractional frame-paced dispatch.

No production source is changed. A temporary copy of precise_wheel.rs replaces
only its final native dispatch with the diagnostic adapter in the companion
Rust main. Both modes retain the production bridge's normalization/suppression.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--target-dir', type=Path, default=ROOT / 'src-tauri/target')
p.add_argument('--clock-preload', type=Path)
p.add_argument('--output', type=Path, required=True)
args = p.parse_args()
deps = args.target_dir.resolve() / 'debug/deps'
command = ['rustc', '--edition', '2021', '-L', f'dependency={deps}']
for name in ('gtk', 'webkit2gtk', 'libc', 'javascriptcore'):
    candidates = list(deps.glob(f'lib{name}-*.rlib'))
    if not candidates:
        p.error(f'missing compiled {name} dependency in {deps}')
    artifact = max(candidates, key=lambda item: item.stat().st_mtime_ns)
    command.extend(('--extern', f'{name}={artifact}'))
production = (ROOT / 'src-tauri/src/precise_wheel.rs').read_text()
dispatch = '        view.event(&event);\n        break;'
assert production.count(dispatch) == 1, 'production dispatch changed; inspect probe adapter'
production = production.replace(dispatch, '        diagnostic_wheel(&view, &event);\n        break;')
results = {}
with tempfile.TemporaryDirectory(prefix='codemux-native-glide-') as temporary:
    temporary = Path(temporary)
    source = temporary / 'probe.rs'
    executable = temporary / 'probe'
    source.write_text(production + '\n' + (HERE / 'native_glide_probe_main.rs').read_text())
    subprocess.run(command + [str(source), '-o', str(executable)], check=True)
    for mode in ('direct', 'hermite200'):
        env = os.environ.copy()
        env['CODEMUX_NATIVE_GLIDE_MODE'] = mode
        env.setdefault('WEBKIT_DMABUF_RENDERER_FORCE_SHM', '1')
        if args.clock_preload:
            preload = str(args.clock_preload.resolve(strict=True))
            env['LD_PRELOAD'] = preload + (':' + env['LD_PRELOAD'] if env.get('LD_PRELOAD') else '')
        completed = subprocess.run([str(executable)], env=env, capture_output=True, text=True, timeout=15, check=True)
        if 'available=true' not in completed.stdout:
            raise SystemExit(f'native bridge unavailable: {completed.stdout}\n{completed.stderr}')
        payloads = [line[4:] for line in completed.stdout.splitlines() if line.startswith('DOM ')]
        if len(payloads) != 1:
            raise SystemExit(f'probe incomplete: {completed.stdout}\n{completed.stderr}')
        result = json.loads(payloads[0])
        start = result['inputs'][0][0]
        wall_offset = result['clock']['wall'] - result['clock']['monotonic']
        for event in result['events']:
            event['time'] -= wall_offset + start
        for frame in result['frames']:
            frame['time'] -= wall_offset + start
        result['inputs'] = [[time-start, value] for time, value in result['inputs']]
        result['gtkFrames'] = [time-start for time in result['gtkFrames']]
        result['time_basis'] = 'milliseconds relative to first scheduled native input; wall/monotonic clocks paired at capture'
        assert result['prevented'] == 0
        assert all(e['trusted'] and e['cancelable'] for e in result['events'])
        # 1 isolated +6 slow +4 forward -2 reverse +64 fast =73 partial notches.
        # The intervening +120/-120 pair has zero net distance.
        expected = 73 * 15.5
        assert abs(result['total'] - expected) < 0.01, (mode, result['total'], expected)
        assert abs(result['offset'] - expected) <= 1, (mode, result['offset'], expected)
        assert len(result['inputs']) == 79
        result['validation'] = dict(no_javascript_scroll_writes=True, all_trusted=True,
                                  all_cancelable=True, prevented_events=0,
                                  expected_distance_px=expected, distance_preserved=True,
                                  fractional_pixel_paint_measured=False)
        results[mode] = result
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(results, indent=2) + '\n')
print(json.dumps({mode: dict(events=len(r['events']), total=r['total'], offset=r['offset'],
                            gtk_frames=len(r['gtkFrames'])) for mode, r in results.items()}, indent=2))
