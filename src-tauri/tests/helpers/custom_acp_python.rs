//! Test-only Python 3 discovery; fixtures always launch literal interpreter argv.
use std::{path::PathBuf, process::Command, sync::OnceLock};

pub fn executable() -> String {
    static PYTHON: OnceLock<PathBuf> = OnceLock::new();
    PYTHON
        .get_or_init(|| {
            let candidates = if cfg!(windows) {
                ["python", "python3"]
            } else {
                ["python3", "python"]
            };
            for name in candidates {
                let Ok(found) = which::which(name) else { continue };
                let Ok(path) = found.canonicalize() else { continue };
                if !path.metadata().is_ok_and(|metadata| metadata.is_file()) {
                    continue;
                }
                let is_python3 = Command::new(&path)
                    .args(["-I", "-c", "import sys; sys.exit(0 if sys.version_info.major == 3 else 1)"])
                    .output()
                    .is_ok_and(|output| output.status.success());
                if is_python3 {
                    return path;
                }
            }
            panic!("ACP native tests require an installed Python 3 interpreter on PATH");
        })
        .to_str()
        .expect("the Python 3 interpreter path must be UTF-8")
        .to_owned()
}
