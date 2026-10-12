#!/usr/bin/env python3
"""Verify the production executable export survives release LTO and stripping.

This isolated binary compiles the current clock module and uses the actual
build.rs export flag. It does not launch CodeMux or measure presentation timing.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--output', type=Path)
args = p.parse_args()
source = ROOT / 'src-tauri/src/vblank_fallback.rs'
build = (ROOT / 'src-tauri/build.rs').read_text()
flags = re.findall(r'cargo:rustc-link-arg-bin=codemux=([^"\n]+)', build)
assert len(flags) == 1, 'inspect the changed production export declaration'
with tempfile.TemporaryDirectory(prefix='codemux-release-clock-') as temporary:
    directory = Path(temporary)
    (directory / 'Cargo.toml').write_text('''[package]
name = "codemux-release-clock-probe"
version = "0.0.0"
edition = "2021"
[[bin]]
name = "codemux"
path = "main.rs"
[dependencies]
libc = "0.2"
[profile.release]
lto = "thin"
strip = "symbols"
opt-level = 3
''')
    (directory / 'build.rs').write_text('fn main() { println!("cargo:rustc-link-arg-bin=codemux=' + flags[0] + '"); }\n')
    (directory / 'main.rs').write_text('#[path = ' + json.dumps(str(source)) + ']\nmod vblank_fallback;\n' + '''fn main() {
    unsafe {
        // No direct reference to the Rust function: only the executable export
        // should keep it discoverable before the real libdrm is loaded.
        let before = libc::dlsym(libc::RTLD_DEFAULT, c"drmWaitVBlank".as_ptr());
        assert!(!before.is_null(), "production clock export was eliminated");
        let library = libc::dlopen(c"libdrm.so.2".as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
        assert!(!library.is_null(), "libdrm could not be loaded");
        let after = libc::dlsym(libc::RTLD_DEFAULT, c"drmWaitVBlank".as_ptr());
        assert_eq!(before, after, "dependent libraries do not resolve the executable export");
        println!("production export retained and interposes loaded libdrm");
    }
}
''')
    subprocess.run(['cargo', 'build', '--offline', '--release', '-j', '2', '--manifest-path', str(directory / 'Cargo.toml')], check=True)
    executable = directory / 'target/release/codemux'
    result = subprocess.run([str(executable)], check=True, capture_output=True, text=True)
    symbols = subprocess.run(['readelf', '--dyn-syms', '--wide', str(executable)], check=True, capture_output=True, text=True).stdout
    exported = [line.strip() for line in symbols.splitlines() if re.search(r'\bdrmWaitVBlank$', line)]
    assert len(exported) == 1 and 'GLOBAL' in exported[0] and ' UND ' not in exported[0]
    record = {
        'production_module': 'src-tauri/src/vblank_fallback.rs',
        'source_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
        'production_linker_flag': flags[0],
        'release_profile': {'opt_level': 3, 'lto': 'thin', 'strip': 'symbols'},
        'executable_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
        'dynamic_symbol': exported[0],
        'result': result.stdout.strip(),
        'limitations': 'Isolated production-module export test; not a full release CodeMux/AppImage launch, GUI callback or scanout measurement.',
    }
    text = json.dumps(record, indent=2) + '\n'
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text)
    print(text, end='')
