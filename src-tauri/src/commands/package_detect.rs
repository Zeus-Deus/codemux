use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct DetectedSetup {
    pub id: String,
    pub label: String,
    pub command: String,
    pub enabled: bool,
}

/// Scan a project directory for package managers, environment files, and other
/// tooling indicators. Returns suggested setup commands in priority order.
#[tauri::command]
pub fn detect_package_manager(project_path: String) -> Result<Vec<DetectedSetup>, String> {
    let root = Path::new(&project_path);
    if !root.is_dir() {
        return Err(format!("Not a directory: {project_path}"));
    }

    let mut results: Vec<DetectedSetup> = Vec::new();

    // ── JavaScript / Node ──
    // Check lockfiles first to pick the right package manager.
    let has_package_json = root.join("package.json").exists();
    let mut js_detected = false;

    if root.join("bun.lock").exists() || root.join("bun.lockb").exists() {
        results.push(DetectedSetup {
            id: "bun".into(),
            label: "Install dependencies (bun)".into(),
            command: "bun install".into(),
            enabled: true,
        });
        js_detected = true;
    } else if root.join("pnpm-lock.yaml").exists() {
        results.push(DetectedSetup {
            id: "pnpm".into(),
            label: "Install dependencies (pnpm)".into(),
            command: "pnpm install".into(),
            enabled: true,
        });
        js_detected = true;
    } else if root.join("yarn.lock").exists() {
        results.push(DetectedSetup {
            id: "yarn".into(),
            label: "Install dependencies (yarn)".into(),
            command: "yarn install".into(),
            enabled: true,
        });
        js_detected = true;
    } else if root.join("package-lock.json").exists() {
        results.push(DetectedSetup {
            id: "npm".into(),
            label: "Install dependencies (npm)".into(),
            command: "npm ci".into(),
            enabled: true,
        });
        js_detected = true;
    }

    // Fallback: package.json exists but no lockfile
    if has_package_json && !js_detected {
        results.push(DetectedSetup {
            id: "npm".into(),
            label: "Install dependencies (npm)".into(),
            command: "npm install".into(),
            enabled: true,
        });
    }

    // ── Rust ──
    if root.join("Cargo.toml").exists() {
        results.push(DetectedSetup {
            id: "cargo".into(),
            label: "Build project (cargo)".into(),
            command: "cargo build".into(),
            enabled: true,
        });
    }

    // ── Go ──
    if root.join("go.mod").exists() {
        results.push(DetectedSetup {
            id: "go".into(),
            label: "Download Go modules".into(),
            command: "go mod download".into(),
            enabled: true,
        });
    }

    // ── Python ──
    let has_pyproject = root.join("pyproject.toml").exists();
    if root.join("poetry.lock").exists() || (has_pyproject && !root.join("uv.lock").exists()) {
        // poetry.lock present, or pyproject.toml without uv.lock → poetry
        if root.join("poetry.lock").exists() {
            results.push(DetectedSetup {
                id: "poetry".into(),
                label: "Install dependencies (poetry)".into(),
                command: "poetry install".into(),
                enabled: true,
            });
        }
    }
    if root.join("uv.lock").exists() {
        results.push(DetectedSetup {
            id: "uv".into(),
            label: "Sync dependencies (uv)".into(),
            command: "uv sync".into(),
            enabled: true,
        });
    }
    if root.join("requirements.txt").exists()
        && !root.join("poetry.lock").exists()
        && !root.join("uv.lock").exists()
    {
        results.push(DetectedSetup {
            id: "pip".into(),
            label: "Install Python dependencies".into(),
            command: "pip install -r requirements.txt".into(),
            enabled: true,
        });
    }

    // ── Ruby ──
    if root.join("Gemfile").exists() {
        results.push(DetectedSetup {
            id: "ruby".into(),
            label: "Install Ruby dependencies".into(),
            command: "bundle install".into(),
            enabled: true,
        });
    }

    // ── PHP ──
    if root.join("composer.json").exists() {
        results.push(DetectedSetup {
            id: "php".into(),
            label: "Install PHP dependencies".into(),
            command: "composer install".into(),
            enabled: true,
        });
    }

    // ── Environment ──
    if root.join(".env.example").exists() {
        results.push(DetectedSetup {
            id: "env".into(),
            label: "Copy environment template".into(),
            command: "cp .env.example .env".into(),
            enabled: true,
        });
    } else if root.join(".env.sample").exists() {
        results.push(DetectedSetup {
            id: "env".into(),
            label: "Copy environment template".into(),
            command: "cp .env.sample .env".into(),
            enabled: true,
        });
    } else if root.join(".env.template").exists() {
        results.push(DetectedSetup {
            id: "env".into(),
            label: "Copy environment template".into(),
            command: "cp .env.template .env".into(),
            enabled: true,
        });
    }

    // ── Git submodules ──
    if root.join(".gitmodules").exists() {
        results.push(DetectedSetup {
            id: "submodules".into(),
            label: "Init git submodules".into(),
            command: "git submodule update --init --recursive".into(),
            enabled: true,
        });
    }

    // ── Docker Compose (disabled by default) ──
    let has_compose = root.join("docker-compose.yml").exists()
        || root.join("docker-compose.yaml").exists()
        || root.join("compose.yml").exists()
        || root.join("compose.yaml").exists();
    if has_compose {
        results.push(DetectedSetup {
            id: "docker".into(),
            label: "Start Docker services".into(),
            command: "docker compose up -d".into(),
            enabled: false,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_detect_npm_with_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("package-lock.json"), "{}").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "npm");
        assert_eq!(results[0].command, "npm ci");
    }

    #[test]
    fn test_detect_npm_without_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "npm");
        assert_eq!(results[0].command, "npm install");
    }

    #[test]
    fn test_detect_bun() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("bun.lock"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "bun");
    }

    #[test]
    fn test_detect_cargo() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "cargo");
    }

    #[test]
    fn test_detect_multiple() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        fs::write(dir.path().join(".env.example"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"pnpm"));
        assert!(ids.contains(&"cargo"));
        assert!(ids.contains(&"env"));
    }

    #[test]
    fn test_docker_compose_disabled_by_default() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("docker-compose.yml"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "docker");
        assert!(!results[0].enabled);
    }

    #[test]
    fn test_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_poetry_detection() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        fs::write(dir.path().join("poetry.lock"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"poetry"));
        assert!(!ids.contains(&"pip"));
    }

    #[test]
    fn test_uv_detection() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        fs::write(dir.path().join("uv.lock"), "").unwrap();

        let results = detect_package_manager(dir.path().to_string_lossy().to_string()).unwrap();
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"uv"));
        assert!(!ids.contains(&"pip"));
    }
}
