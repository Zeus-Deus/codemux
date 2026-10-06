"""Build the production clock module as an isolated diagnostic shared library."""
from pathlib import Path
import json
import subprocess
import tempfile

source = Path(__file__).resolve().parents[3] / "src-tauri/src/vblank_fallback.rs"
probe = Path(tempfile.mkdtemp(prefix="codemux-vblank-probe-"))
manifest = probe / "Cargo.toml"
manifest.write_text(
    '[package]\nname = "codemux-vblank-probe"\nversion = "0.0.0"\nedition = "2021"\n'
    '[lib]\ncrate-type = ["cdylib"]\npath = ' + json.dumps(str(source)) + '\n'
    '[dependencies]\nlibc = "0.2"\n'
)
subprocess.run(
    ["cargo", "build", "--offline", "-j", "2", "--manifest-path", str(manifest)],
    check=True,
)
print(probe / "target/debug/libcodemux_vblank_probe.so")
