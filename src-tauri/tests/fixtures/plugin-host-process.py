"""Process/fs SDK lifecycle fixture. Children write heartbeat files, never UI."""
import json
import os
import subprocess
import sys
import time
from pathlib import Path

mode = sys.argv[1]
if mode == 'echo':
    print(json.dumps({'args': sys.argv[2:], 'cwd': os.getcwd(), 'path': os.environ.get('PATH'), 'override': os.environ.get('PLUGIN_HOST_OVERRIDE'), 'removed': os.environ.get('PLUGIN_HOST_REMOVED'), 'home': os.environ.get('HOME')}), flush=True)
    print('stderr intact', file=sys.stderr, flush=True)
elif mode == 'output':
    os.write(1, b'a' * 65536)
    os.write(2, b'b' * 65536)
elif mode == 'invalid-utf8':
    os.write(1, b'\xff' * 10 + b'abc')
elif mode == 'mark':
    Path(sys.argv[2]).write_text('launched')
elif mode == 'heartbeat':
    if len(sys.argv) > 3:
        # First write follows the host's bounded drain window.
        time.sleep(.3)
    path = Path(sys.argv[2])
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if len(sys.argv) > 3:
            os.write(1, b'heartbeat\n')
        with path.open('a') as f:
            f.write('.')
        time.sleep(0.01)
elif mode == 'tree':
    # This child launches another child: both remain in the host-owned group.
    child = subprocess.Popen([sys.executable, __file__, 'parent', sys.argv[2]])
    print(child.pid, flush=True)
    time.sleep(60)
elif mode == 'parent':
    child = subprocess.Popen([sys.executable, __file__, 'heartbeat', sys.argv[2]])
    Path(sys.argv[2] + '.pid').write_text(str(child.pid))
    time.sleep(60)
elif mode == 'detached':
    # Keep stdout/stderr inherited to reproduce CLIs that launch a GUI and exit.
    child = subprocess.Popen([sys.executable, __file__, 'heartbeat', sys.argv[2], 'stdout'], start_new_session=True)
    Path(sys.argv[2] + '.pid').write_text(str(child.pid))
    print('launched detached child', flush=True)
elif mode == 'daemon':
    child = os.fork()
    if child:
        Path(sys.argv[2] + '.pid').write_text(str(child))
    else:
        time.sleep(.05)
        os.setsid()
        os.execv(sys.executable, [sys.executable, __file__, 'heartbeat', sys.argv[2]])
